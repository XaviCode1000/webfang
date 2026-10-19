# Proveedor de IA — Diseño Final

> **Estado**: Diseño definitivo para la feature de proveedores de IA remotos (embeddings + LLM completions).
> Implementación en `crates/webfang_core/src/infrastructure/llm/` (`client.rs`, `provider.rs`, `remote_embedding.rs`, `mod.rs`).
>
> **Actualización ADR-0004 paso 6:** el ONNX local se eliminó (crate `webfang_ai` borrado, feature
> `ai` retirada). El `EmbeddingAdapter` local ONNX ya no existe; el único adaptador de embeddings
> vivo es el remoto (`RemoteEmbeddingAdapter`/`EmbeddingPort` en `webfang_core`), y el slot
> `ProviderKind::LocalOnnx` propuesto abajo perdió su base (`InferencePool` ya no existe) — si el
> registro de proveedores aterriza, llega sin la variante local o con un ADR nuevo.

---

## Estado actual (lo que existe hoy)

### Traits de dominio (ya existentes)

- **`EmbeddingPort`** (`domain/embedding_port.rs`) — trait abierto, no sellado, para generación de embedding vectors:
  - `embed(text) -> Vec<f32>` — un embedding por texto
  - `embed_batch(texts) -> Vec<Vec<f32>>` — batched (overrideable)
  - `embedding_dim() -> usize` — dimensión declarada
  - `model_tag() -> &str` — tag del modelo (para namespacing en vault)
  - Implementaciones existentes: `RemoteEmbeddingAdapter` (HTTP remoto, en `webfang_core` infra). El `EmbeddingAdapter` local ONNX fue removido en ADR-0004 paso 6

- **`LlmPort`** (`domain/llm_port.rs`) — trait abierto, no sellado, para completions de LLM:
  - `send_completion(request: LlmRequest) -> LlmResponse` — una single completion
  - Implementación existente: `OpenAiLlmClient` (OpenAI-compatible, con `base_url` configurable como parámetro de construcción)

### Infraestructura existente

- **`OpenAiLlmClient`** — cliente LLM OpenAI-compatible; **no es un provider genérico**, es un cliente concreto que puede apuntar a cualquier endpoint OpenAI-compatible (OpenRouter, FreeLLM, etc.) configurando `base_url`. No soporta multi-tenant (un solo client = un solo tenant/key).
- **`RemoteEmbeddingAdapter`** — adapter de embeddings HTTP para proveedores remotos (el que se conocía como "esta feature").
- **`build_default_http_client()`** — función que construye el `wreq::Client` con config canónica (Chrome145, 60s timeout, 10s connect, SSRF guard armado). **Extraída del constructor de `OpenAiLlmClient`** para ser reutilizable.
- **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — constructor alternativo que acepta un `wreq::Client` ya construido (para compartir pool).
- **`CredentialStore`** — store de credenciales en memoria con `ApiKey`/`AccessToken` (wrappers de `SecretString` con zeroize y Debug redactado). **No persiste a disco**.

### Limitaciones del diseño actual

1. **Sin gestión de credenciales por proveedor:** las API keys de proveedores remotos se inyectan vía env vars o construcción directa; no hay forma de decir "esta key es para OpenRouter, esta otra para FreeLLM".
2. **Sin soporte multi-tenant:** `OpenAiLlmClient` (y `RemoteEmbeddingAdapter`) son sigletons por instancia; si necesitas dos tenants con diferentes keys/base_urls, necesitas dos instancias separadas y no hay registro que las maneje.
3. **Sin persistencia segura de secretos:** `CredentialStore` es solo memoria; no hay mecanismo de almacenamiento persistente cifrado.
4. **Sin concepto de "proveedor con capacidades":** no hay forma de declarar "este provider hace embeddings, este otro hace completions, este otro hace ambos".

---

## Conceptos nuevos

### 1. `AuthSource` — fuente de una credencial

Enum que describe de dónde se obtiene la API key de un provider:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AuthSource {
    /// Credencial en keyring del sistema (libsecret / macOS keychain / Windows Credential Manager)
    Keyring { service: String, account: String },
    /// Credencial en archivo cifrado con age en disco
    EncryptedFile { path: PathBuf },
    /// Credencial en variable de entorno (legacy, solo CI o desarrollo)
    Env { var: String },
}
```

**Decisiones de diseño:**

- **`Default::Keyring`** — el default es keyring, no env. Esto evita que los usuarios nuevos inherited el hábito de "poner la key en una variable de entorno" como práctica canónica.
- **Sin `Inline` en v1** — escribir la key literalmente en un archivo de config (aunque sea TOML cifrado con age) es un riesgo de seguridad que no tenemos mitigación real para (un warning de log es absolución, no mitigación; alguien apurado lo ignorará). Si en el futuro se justifica, entrará con `SecretString` y discusión propia.
- **Sin fallback implícito** — un `AuthSource::Keyring` fallando NO intenta `EncryptedFile` ni `Env`. El usuario tiene que configurar explícitamente qué fuente usar, y si esa fuente falla, falla explícitamente con el error de esa fuente. Esto es intencional: un fallback silencioso `Keyring → EncryptedFile → Env` significaría que un usuario cree estar usando keyring y termina en env var sin saberlo, heredando una vulnerabilidad.

### 2. `AuthError` — errores de resolución de credencial

```rust
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("keyring no disponible en este sistema")]
    KeyringUnavailable,
    #[error("no se encontró credencial para {service}/{account}")]
    CredentialNotFound { service: String, account: String },
    #[error("archivo de credenciales no encontrado: {0}")]
    FileNotFound(PathBuf),
    #[error("variable de entorno {0} no configurada")]
    EnvNotSet(String),
    #[error("credencial inválida: {0}")]
    Invalid(String),
}
```

### 3. `AuthSource::resolve()` — obtener la credencial

```rust
impl AuthSource {
    pub fn resolve(&self) -> Result<ApiKey, AuthError> {
        match self {
            Self::Keyring { service, account } => resolve_keyring(*service, *account),
            Self::EncryptedFile { path } => resolve_encrypted_file(path),
            Self::Env { var } => resolve_env(var),
        }
    }
}
```

Cada variante maneja su propia lógica de resolución y retorna `AuthError` específico si falla. No hay orchestrator que intente la siguiente fuente.

### 4. `ProvidersConfig` — configuración de proveedores

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ProvidersConfig {
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    pub base_url: Url,
    pub auth: AuthSource,
    pub capabilities: Vec<Capability>,
    pub model: Option<String>,       // modelo por defecto para este provider
    pub embedding_dim: Option<usize>, // dimensión explícita (si difiere del modelo por defecto)
}
```

Donde:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub enum ProviderKind {
    OpenAiCompatible,   // OpenAI-compatible API (OpenAI, OpenRouter, FreeLLM, Groq, Together, Ollama)
    // NOTA ADR-0004 paso 6: la variante LocalOnnx propuesta aquí quedó sin base
    // (no queda `InferencePool` que reutilizar); si el registro aterriza, llega
    // sin la variante local o con un ADR nuevo.
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub enum Capability {
    Embedding,
    Completion,
}
```

### 5. `ProviderRegistry` — registro de proveedores por ID

```rust
pub struct ProviderRegistry {
    providers: Vec<ProviderConfig>,
}
```

No es un trait, es un struct concreto con lógica de lookup por ID.

**Resolución por capabilidad:** en vez de `AnyProviderHandle` (eliminado en la ronda 3 como redundante contra el diseño por slot), el registry resuelve directamente por capabilidad, con un único método de lookup:

```rust
impl ProviderRegistry {
    pub fn resolve(
        &self,
        provider_id: &str,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError>

    pub fn resolve_default_completion(&self) -> Result<&ProviderConfig, RegistryError>

    pub fn resolve_default_embedding(&self) -> Result<&ProviderConfig, RegistryError>
}
```

Verificado en `crates/webfang_core/src/domain/providers.rs:251` (companions en `:305` y `:322`), con `RegistryError::ProviderNotFound` en `:58` y `RegistryError::CapabilityMismatch` en `:64`.

**El registry devuelve una referencia a la config, no un port.** La construcción del adapter —y por tanto el `Arc<dyn EmbeddingPort>` / `Arc<dyn LlmPort>`— ocurre en la capa de binario, no aquí: `build_llm_provider` / `build_embedding_provider` en `crates/webfang_core/src/cli/llm_wire.rs`. Esa separación es deliberada: el dominio no sabe qué adapter concreto implementa cada `ProviderKind`.

Las capacidades no necesitan un método propio: se leen de `ProviderConfig.capabilities` (`Vec<Capability>`, `providers.rs:87`), y `resolve()` valida contra esa lista antes de devolver la config. El diseño no incluye un accessor de capacidades separado.

Esto permite que un caller existente (ej. `VaultSearchService`) resuelva embeddings directamente sin pasar por un handle genérico que debe ser desechado en el uso correcto.

### 6. `TenantAiContext` — contexto multi-tenant

> **No implementado.** `TenantAiContext` **no existe en el código**: cero ocurrencias en `crates/`, y no hay ningún tipo ni método con ese nombre que revisar. El contrato de abajo sigue vigente por decisión explícita del owner y está trackeado en la issue **#2018**. No hay struct, ni trait, ni `resolve_for_tenant(tenant_id, provider_id)` que citar.

El contrato que este diseño propone, y que #2018 tendrá que implementar cuando exista un segundo tenant real, es: un contexto por tenant que envuelve un `ProviderRegistry` y mapea `(tenant_id, provider_id)` al puerto ya resuelto.

**Nota sobre escalabilidad de `wreq::Client`:** el diseño actual de `OpenAiLlmClient` crea su propio `wreq::Client` internamente. Para compartir un pool entre múltiples tenants con el mismo `base_url`, se usa `with_http(http, base_url, api_key)` con un `wreq::Client` construido por `build_default_http_client()`. Esto permite que el registry comparta clientes entre providers del mismo tipo sin requerir cambios en la firma de `new`.

### 7. `DetectedStore` — setup helper, no runtime resolver

> **Renombrado en la implementación.** Este diseño llamó `LocalSecretStore` al tipo; lo implementado es `DetectedStore`, en `crates/webfang_core/src/domain/local_secret_store.rs:16` (`detect()` en `:48`). `LocalSecretStore` quedó solo como nombre de módulo.

`DetectedStore::detect()` es un helper de setup que sugiere al usuario qué fuentes de credenciales están disponibles en su sistema durante la configuración inicial:

```rust
pub struct DetectedStore {
    pub keyring_available: bool,
    pub encrypted_file_path: Option<PathBuf>,
    pub env_vars_detected: Vec<String>,
}
```

**No es un runtime resolver.** Durante la ejecución normal, `AuthSource::resolve()` es quien obtiene las credenciales. `DetectedStore::detect()` solo se usa en el wizard de setup inicial para guiar al usuario.

### 8. Wizard — contrato UX (implementación pendiente, decisiones cerradas)

Las decisiones de producto del wizard, fijadas en revisión antes de escribirlo:

- **(a) Primer run en Linux: genera la identity automáticamente** (modelo SSH:
  archivo con 0600, no passphrase interactiva). Pero el wizard **lo dice en
  voz alta**: "voy a crear una identity en `~/.config/webfang/identity.key`;
  si la perdés, vas a tener que rotar tus providers". Sin ese mensaje,
  `EncryptedFile` parece "seguro por magia" y el usuario nunca hace backup.
- **(b) Container permisivo, binario valida en startup.** El Container NO falla
  en construcción sin provider; pero todo binario de servicio que arranque con
  un feature LLM habilitado DEBE validar `llm_port().is_some()` en startup y
  salir con error claro si falta (falla en startup si el feature está
  habilitado; no falla si no lo está). Sin esa validación el fallo llega tarde
  en la primera llamada — daemon que dice OK al arrancar y falla cuando un
  usuario/cron invoca el feature, con reintentos amplificando antes de que
  nadie lo note. `absent_llm_port_is_config_error` testea el comportamiento
  del Container, no lo justifica como contrato del servicio.
  **Caducidad del DEBE**: el lint/test que lo enforce nace con el primer
  binario que exponga un flag tipo `--extract-with-llm` — ese PR no mergea sin
  él. Un DEBE sin fecha es el octavo binario sin validación que nadie notó.
- **(c) Dos flujos, sin abstracción unificadora.** Linux (`EncryptedFile`:
  identity + permisos + rotación) y macOS/Windows (`Keyring` si el backend
  responde) son flujos distintos. El wizard los implementa por separado; una
  abstracción común que "simplifique" el caso Linux es un bug de diseño.
- **Rotate, no "re-configurar".** Si `identity.key` se pierde, TODOS los
  ciphertexts cifrados con ella quedan indescifrables a la vez (tantos
  providers como usen esa identity). El wizard ofrece un comando explícito de
  **rotate**: genera nueva identity, re-cifra lo que el usuario provea de
  nuevo, borra los `.age` huérfanos. "Re-configurar" deja archivos muertos
  tirados que parecen estar en uso.

---

## Flujo de implementación propuesto

### Phase 0: Infraestructura de soporte (sin features visibles)

1. **`AuthSource` + `AuthError` + `resolve()`** con tests unitarios por variante (keyring, encrypted_file, env), incluyendo casos de error.
2. **`DetectedStore::detect()`** → diagnóstico informativo (`domain/local_secret_store.rs:48`).
3. **`build_default_http_client()`** extraída y reutilizable.
4. **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — aditivo, `new()` intacto.
5. **`ProvidersConfig`** parsing + `ProviderRegistry` con resolución por ID y por capability.
6. **`TenantAiContext`** como struct vacío con interfaz pública futura (sin lógica de tenant aún). — **No implementado**, ver §6 e issue #2018.

### Phase 1: Proveedor OpenAI-compatible concreto

7. **`OpenAiCompatibleProvider`** — struct que implementa `LlmPort` (y potencialmente `EmbeddingPort` futuro) usando `OpenAiLlmClient` internamente:
   - Construye el client con `with_http(build_default_http_client()?, base_url, api_key_resolved)`.
   - `api_key_resolved` viene de `AuthSource::resolve()` configurado en `ProviderConfig`.
8. **Fixtures de contrato** (`tests/fixtures/{openai,openrouter,freellm,groq}.json`) contra el parser de responses del provider.

### Phase 2: Integración con container

9.  **`Container::with_providers_config(config: ProvidersConfig)`** — inyecta el registry.
10. **`Container::resolve_provider(provider_id)`** — resolución por ID para callers existentes.
11. Tests de integración: container + registry + provider concreto.

### Phase 3: Soporte multi-tenant (deferred)

12. **`TenantAiContext::resolve_for_tenant(tenant_id, provider_id)`** — implementado cuando haya un segundo tenant real. **No implementado**, ver §6 e issue #2018.
13. Tests de multi-tenant integrados.

---

## Criterios de aceptación

### Criterios de seguridad

- `[x]` `AuthSource::resolve()` no hace fallback implícito entre fuentes — un `match` de una sola fuente por variante (`domain/auth_source.rs:118-126`), sin orchestrator.
- `[x]` `AuthSource::Keyring` falla con `AuthError::KeyringUnavailable` si keyring no disponible, no intenta otra fuente (`resolve_keyring`, `auth_source.rs:151`).
- `[x]` `AuthSource::Env` es soportado pero documentado como "legacy/CI-only", no como default (`auth_source.rs:72`: "Credencial en variable de entorno (legacy, solo CI o desarrollo)").
- `[x]` `Inline` no está disponible en v1 (ausente del enum) — declarado explícitamente en `auth_source.rs:6`.
- `[ ]` La API key nunca aparece en logs, traces, ni mensajes de error (`Debug` redactado en `ApiKey`, `SecretString`).
- `[x]` `DetectedStore::detect()` no es usado como runtime resolver de credenciales (`domain/local_secret_store.rs:48`; el que resuelve es `AuthSource::resolve()`).
- `[ ]` Credenciales persistidas en disco (si se implementa `EncryptedFile` en v1) usan cifrado con age, no texto plano.

### Criterios de correctitud

- `[x]` `ProvidersConfig` parsing falla con error claro si `base_url` no es válido (no un URL válido) — `base_url` está tipado como `Url`, no como `String` (`domain/providers.rs:83`), así que el parseo falla antes de que exista un `ProviderConfig`.
- `[x]` `ProviderRegistry::resolve(provider_id, Capability)` falla con `RegistryError::ProviderNotFound` si el ID no existe (`domain/providers.rs:251`, test en `:360`).
- `[x]` `ProviderRegistry::resolve(provider_id, Capability::Embedding)` falla con `RegistryError::CapabilityMismatch` si el provider no declara `Embedding` (`providers.rs:263`, test en `:369`).
- `[x]` `ProviderRegistry::resolve(provider_id, Capability::Completion)` falla con `RegistryError::CapabilityMismatch` si el provider no declara `Completion` (mismo camino de capacidad).
- `[x]` `OpenAiCompatibleProvider` envuelve `OpenAiLlmClient` y resuelve la credencial una vez en startup (`infrastructure/llm/provider.rs:113`, `new()` en `:129`).
- `[x]` `build_default_http_client()` produce la config canónica (Chrome145, 60s timeout, 10s connect, SSRF guard armado) — `infrastructure/llm/provider.rs:86-95`.

> `resolve()` devuelve `&ProviderConfig`, no un port: la construcción del adapter es de la capa de binario (`crates/webfang_core/src/cli/llm_wire.rs`). Ver §5.

---

## Notas de arquitectura

### Sobre `EmbeddingPort` vs `EmbeddingCapable`

La auditoría de rondas previas concluyó correctamente que crear `EmbeddingCapable`/`CompletionCapable` como nuevos traits era redundante: `EmbeddingPort` ya cubre el caso de embeddings remotos (tiene `embed_batch()`, `embedding_dim()`, `model_tag()`, y maneja errores con `SemanticError` que distingue Transport/Auth/Status). Reutilizar `EmbeddingPort` para providers remotos es la decisión correcta.

La única razón por la que se consideró crear nuevos traits era dar "capacidades" a un provider (embeddings, completions, ambos). Pero eso se resuelve mejor con `ProviderConfig.capabilities: Vec<Capability>` en la config y con métodos de resolución en `ProviderRegistry` que verifican la capacidad antes de retornar el port.

### Sobre `AnyProviderHandle`

El documento 5 propuso `AnyProviderHandle` como enum con variantes `EmbeddingOnly`, `CompletionOnly`, `Both`. La auditoría de documento 6 señaló que esto era redundante contra el diseño por slot que el mismo documento 5 había propuesto dos rondas antes (`resolve(provider_id, Capability) -> Result<&ProviderConfig, RegistryError>`). El diseño final adopta la resolución por slot: no hay handle intermedio, el registry resuelve directamente por ID y por capabilidad, y devuelve la config; el port lo construye la capa de binario.

### Sobre `wreq::Client` compartido

La objeción de escalabilidad (muchos tenants → muchos pools de conexiones) es válida a largo plazo, pero la solución no requiere cambiar la firma de `OpenAiLlmClient::new()`. Se añade `with_http(http, base_url, api_key)` que acepta un `wreq::Client` externo. El registry puede compartir clientes entre providers del mismo `base_url` creando un `wreq::Client` con `build_default_http_client()` y pasándolo a `with_http()`.

Esto es una extensión aditiva, no un refactor del cliente existente. `OpenAiLlmClient::new(base_url, api_key)` sigue funcionando igual, creando su propio cliente internamente. Solo el nuevo código de registry/provider tiene la opción de compartir.

### Sobre la inversión de dependencia `AuthSource` → `ProvidersConfig`

La crítica de documento 8 fue correcta: `AuthSource` es upstream de `ProvidersConfig`. No se puede escribir el parser de `ProvidersConfig` sin saber qué fuentes de auth soporta, porque eso define el schema de `auth` en cada provider config. Por eso el orden de implementación es: primero `AuthSource` + `AuthError` + `resolve()`, luego `ProvidersConfig` + `ProviderRegistry`.

---

## Pendiente de implementación

Estado real de cada punto del diseño, verificado contra el árbol.

1. [x] **`AuthSource` + `AuthError` + `resolve()`** — `crates/webfang_core/src/domain/auth_source.rs` (`AuthError` `:20`, `AuthSource` `:58`, `resolve()` `:118`).
2. [x] **`DetectedStore::detect()`** — `crates/webfang_core/src/domain/local_secret_store.rs` (`DetectedStore` `:16`, `detect()` `:48`). El tipo que este diseño llamaba `LocalSecretStore` se implementó como `DetectedStore`; el nombre antiguo quedó solo como nombre de módulo (ver §7).
3. [x] **`build_default_http_client()`** extraída de `OpenAiLlmClient` — `crates/webfang_core/src/infrastructure/llm/provider.rs:86` (no en `infrastructure/http/`).
4. [x] **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — `crates/webfang_core/src/infrastructure/llm/client.rs:119`.
5. [x] **`ProvidersConfig` + `ProviderRegistry`** — `crates/webfang_core/src/domain/providers.rs` (`ProvidersConfig` `:197`, `ProviderRegistry` `:228`); viven en `domain/`, no en `application/`.
6. [x] **`OpenAiCompatibleProvider`** — `crates/webfang_core/src/infrastructure/llm/provider.rs:113`.
7. [x] **Integración con container** — el wiring real está en `crates/webfang_core/src/cli/llm_wire.rs:55` (`build_llm_provider` / `build_embedding_provider`), no en `container.rs`.
8. [x] **Tests de integración** — `crates/webfang_core/tests/auth_source_none_test.rs` (`:33`, `:452`).
9. [ ] **`TenantAiContext`** — **no implementado**, ver §6 e issue **#2018**.
10. [ ] **Fixtures de contrato** (`crates/webfang_core/tests/fixtures/{openai,openrouter,freellm,groq}.json`) — se mantienen por decisión del owner: la cobertura cross-proveedor sigue siendo necesaria.

---

## Referencias

- Traits existentes: `EmbeddingPort` (`domain/embedding_port.rs`), `LlmPort` (`domain/llm_port.rs`).
- Implementaciones existentes: `EmbeddingAdapter` (`webfang_ai/.../embedding_adapter.rs`), `RemoteEmbeddingAdapter` (`webfang_core/.../remote.rs`), `OpenAiLlmClient` (`webfang_core/.../client.rs`).
- Gestión de credenciales existente: `CredentialStore` + `ApiKey` (`domain/credentials.rs`).
- Diseño de namespacing de embeddings: `model_tag` y `embedding_dim` en `EmbeddingPort`, con validación en `vault_search.rs`.
- Error classification: `ScraperError::classify()` → `ErrorClass` (`error.rs`), con `RateLimited(u64)` para 429 y `Http { status }` para otros códigos.


---

## Implementación eliminada conocida (del worktree `feat/remote-ai-providers`)

> **Nota:** Este worktree fue eliminado y sus commits no son físicamente recuperables (la rama fue borrada con `git branch -D` y `git worktree remove`). Sin embargo, el diseño completo está documentado aquí basándose en el análisis de sus 8 commits (~2000 líneas).

### SemanticError::{Transport, Auth, Status} (commit b1104ef1)


### RemoteEmbeddingAdapter con guard-chain completa (commit 5fcf8e0f, ~651 líneas)

- Implementa `EmbeddingPort` con:
  - `embed(text)` → un request HTTP POST al endpoint con el texto codificado, recibe `EmbedResponse` con los vectores.
  - `embed_batch(texts)` → un solo request con todos los textos (no uno por texto), decodifica `Vec<Vec<f32>>` del response.
  - **Guard-chain completa (ESTO ES HEREDABLE y debe replicarse en `OpenAiCompatibleProvider`):**
    1. **Timeout** — 60s request / 10s connect (configurado en el cliente `wreq`)

### SSRF hardening por salto de redirección (commits 594c53ea + 71564877 + a0198312, ~159 líneas)

- **`resolve_and_validate(url) -> Result<(), ForbiddenResolution>`** (domain/ssrf_guard.rs, commit 594c53ea, +210 líneas):
  - Función pura (resolver inyectado para testing determinista).
  - Si el host es un **literal IP** (IPv4 o IPv6), lo parsea y valida contra `is_forbidden_ip()` **sin hacer DNS** — rechaza síncronamente si es privado/loopback/link-local.
  - Si el host es un **hostname**, resuelve A/AAAA records y valida **cada registro** contra `is_forbidden_ip()`.
  - **Empty answers** → falla cerrado (rechaza).
  - **DNS errors** → falla cerrado (rechaza).
  - Solo hostnames que resuelven a IPs públicas pasan.
  - Esta función es **inyectable** (el resolver DNS real se pasa desde infraestructura) pero la lógica de validación es pura y determinista.
- **Per-hop redirect re-validation** (infrastructure/ssrf.rs, commit 71564877, +279 líneas):
  - `redirect_policy()` se mueve de `domain/ssrf_guard.rs` a `infrastructure/ssrf.rs`.
  - **Cada destino de redirección** (cada salto 301/302/307/308) pasa por `resolve_and_validate()` antes de seguir.
  - Si el redirect target es un literal IP prohibido → se detiene en tiempo de redirección.
  - Si el target es un hostname que resuelve a IP privada → se detiene.
  - El `ValidatingResolver` de connect-time **sigue armado** como backstop TOCTOU.
  - La composición `secure_client()` no cambia.
- **Test E2E redirect-to-loopback-hostname** (ssrf_rfc1918_e2e_test.rs, commit a0198312, +62 líneas):
  - Row 10 del E2E test — un 302 con host `localhost` (o hostname que resuelve a loopback) se detiene en tiempo de redirect, produce error terminal (exit 69), y el journal del seed es el único registro.
  - Se refactoriza `resolve_and_validate()` en helpers `validate_literal()` (IPs literales, sin DNS) y `validate_answer_set()` (hostnames resueltos, valida cada registro).

> **Este bloque es EL más importante de heredar:** sin `resolve_and_validate()` y la revalidación por-hop en redirecciones, `OpenAiCompatibleProvider` (Fase 3) **retrocede en seguridad** respecto al estado que ya existió — reintroduce la ventana TOCTOU en redirects que ya estaba cerrada. El SSRF hardening ya está implementado en código que se puede reimplementar idénticamente desde este documento.

    2. **SSRF entry-check antes de cada intento** — `resolve_and_validate()` (ver abajo) para validar que el endpoint no sea IP privada/loopback/link-local
    3. **Retry classification** — backoff exponencial 200ms → 2s, máx 3 intentos, con cancelación por timeout
    4. **Body cap 50 MiB** — streaming read con `read_body_capped`, error `BodyTooLarge` si excede (protección real que el diseño nuevo debe incluir desde el día 1)
  - Decodifica `Vec<Vec<f32>>` del response como little-endian `f32` BLOB.
  - Valida que la cantidad de vectores devuelta coincida con la cantidad de textos enviados.
- **`embedding_dim()`** devuelve la dimensión declarada del modelo.
- **`model_tag()`** devuelve `"remote"` (o el override configurado).

### VaultAiPorts con inyección at-most-once (commit beef4388, ~84 líneas)

- `Container::vault_ai_ports: Option<VaultAiPorts>`
- `VaultAiPorts::embedding_port: Option<Arc<dyn EmbeddingPort>>` — `None` por defecto
- `vault_ai_ports.with_remote_embedding(port)` — inyección at-most-once (semantics de `OnceCell`).
- **ACOPADO AL MODELO ANTIGUO.** Debe ser reimplementado sobre `ProviderRegistry` en el nuevo diseño; no se recupera tal cual.

### Namespacing de vectores por model_tag (commit 34ecc730, ~317 líneas)

- `NoteChunkVector` tiene campo `model_tag: String` con `#[serde(default = "default_model_tag")]` que devuelve `LEGACY_MODEL_TAG = "granite-97m-local"` para filas legacy sin tag.
- **`ADD COLUMN model_tag` en SQLite** con backfill perezoso (las filas existentes se leen como legacy, no se re-escriben).
- Al indexar un note: `index_note_stamps_embedder_model_tag` — cada chunk vector escribe con el `model_tag` del embedder que lo generó (extraído de `EmbeddingPort::model_tag()`).
- **`search_skips_dimension_mismatched_vectors`** — en `vault_search.rs`, si el query vector tiene una dimensión que no coincide con los vectores almacenados de ese `model_tag`, se saltan (no se puntuán 0.0 silenciosamente).
- **ESTO ES HEREDABLE Y YA IMPLEMENTADO.** La columna `model_tag`, el backfill perezoso, y el rechazo cross-dim ya existen en el vault store.

- `Transport(String)` — clasificación `TransientRetriable`. Fallos de red temporales: TCP reset, timeout de request/connect, DNS transitorio. Se reintenta con backoff.
- `Auth(String)` — clasificación `PermanentFatal`. Respuestas 401/403 del endpoint remoto (clave desconocida, missing, o revocada). No se reintenta — la credencial no es válida.
- `Status { status: u16, detail: String }` — depende del código: 429 → `TransientBackoff` (backoff con `Retry-After`), 5xx → `TransientRetriable` (exponential), resto → terminal.
- Estas variantes alimentan la clasificación de errores del adapter remoto para decidir si reintentar o fallar de forma terminal.

### RemoteEmbeddingConfig::from_env (commit 739b0d67)

- Configuración para el adapter: `endpoint: Url`, `api_key: ApiKey`, Bearer en cada request.
- `Debug` redactado: la API key **nunca aparece en logs, traces ni mensajes de error** (criterio explícito de la issue #1462).
- `model_tag` configurable, con default `"remote"` si no se override.
- **ACOPADO AL MODELO ANTIGUO DE CREDENCIALES.** Debe ser reimplementado sobre `AuthSource` en el nuevo diseño; no se recupera tal cual.
