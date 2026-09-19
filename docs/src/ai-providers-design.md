# Proveedor de IA — Diseño Final

> **Estado**: Diseño definitivo para la feature de proveedores de IA remotos (embeddings + LLM completions).
> Implementación a desarrollar en el worktree `feat/ai-providers`.

---

## Estado actual (lo que existe hoy)

### Traits de dominio (ya existentes)

- **`EmbeddingPort`** (`domain/embedding_port.rs`) — trait abierto, no sellado, para generación de embedding vectors:
  - `embed(text) -> Vec<f32>` — un embedding por texto
  - `embed_batch(texts) -> Vec<Vec<f32>>` — batched (overrideable)
  - `embedding_dim() -> usize` — dimensión declarada
  - `model_tag() -> &str` — tag del modelo (para namespacing en vault)
  - Implementaciones existentes: `EmbeddingAdapter` (local ONNX, en `webfang_ai`), `RemoteEmbeddingAdapter` (HTTP remoto, en `webfang_core` infra)

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
    LocalOnnx,          // Proveedor local ONNX (para embeddings, reutiliza InferencePool)
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

**Resolución por capabilidad:** en vez de `AnyProviderHandle` (eliminado en la ronda 3 como redundante contra el diseño por slot), el registry expone métodos que resuelven directamente por capabilidad:

```rust
impl ProviderRegistry {
    pub fn resolve_embedding(&self, provider_id: &str) -> Result<Arc<dyn EmbeddingPort>, RegistryError>
    pub fn resolve_completion(&self, provider_id: &str) -> Result<Arc<dyn LlmPort>, RegistryError>
    pub fn resolve_capabilities(&self, provider_id: &str) -> Result<Vec<Capability>, RegistryError>
}
```

Esto permite que un caller existente (ej. `VaultSearchService`) resuelva embeddings directamente sin pasar por un handle genérico que debe ser desechado en el uso correcto.

### 6. `TenantAiContext` — contexto multi-tenant

```rust
pub struct TenantAiContext {
    registry: ProviderRegistry,
    // Mapeo de (tenant_id, provider_id) → puerto resuelto
    // Implementación deferred hasta que haya un segundo tenant real
}
```

**Interfaz ahora, implementación después:** `TenantAiContext` se define como trait o struct con la interfaz pública, pero la implementación completa (con lookup por tenant_id y mapeo de providers por tenant) se retrasa hasta que haya un segundo tenant real. El primer tenant (el del proveedor por defecto) se resuelve directamente del registry.

**Nota sobre escalabilidad de `wreq::Client`:** el diseño actual de `OpenAiLlmClient` crea su propio `wreq::Client` internamente. Para compartir un pool entre múltiples tenants con el mismo `base_url`, se usa `with_http(http, base_url, api_key)` con un `wreq::Client` construido por `build_default_http_client()`. Esto permite que el registry comparta clientes entre providers del mismo tipo sin requerir cambios en la firma de `new`.

### 7. `LocalSecretStore` — setup helper, no runtime resolver

`LocalSecretStore::detect()` es un helper de setup que sugiere al usuario qué fuentes de credenciales están disponibles en su sistema durante la configuración inicial:

```rust
pub struct DetectedStore {
    pub keyring_available: bool,
    pub encrypted_file_path: Option<PathBuf>,
    pub env_vars_detected: Vec<String>,
}
```

**No es un runtime resolver.** Durante la ejecución normal, `AuthSource::resolve()` es quien obtiene las credenciales. `LocalSecretStore::detect()` solo se usa en el wizard de setup inicial para guiar al usuario.

---

## Flujo de implementación propuesto

### Phase 0: Infraestructura de soporte (sin features visibles)

1. **`AuthSource` + `AuthError` + `resolve()`** con tests unitarios por variante (keyring, encrypted_file, env), incluyendo casos de error.
2. **`LocalSecretStore::detect()`** → `DetectedStore` informativo.
3. **`build_default_http_client()`** extraída y reutilizable.
4. **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — aditivo, `new()` intacto.
5. **`ProvidersConfig`** parsing + `ProviderRegistry` con resolución por ID y por capability.
6. **`TenantAiContext`** como struct vacío con interfaz pública futura (sin lógica de tenant aún).

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

12. **`TenantAiContext::resolve_for_tenant(tenant_id, provider_id)`** — implementado cuando haya un segundo tenant real.
13. Tests de multi-tenant integrados.

---

## Criterios de aceptación

### Criterios de seguridad

- `[ ]` `AuthSource::resolve()` no hace fallback implícito entre fuentes.
- `[ ]` `AuthSource::Keyring` falla con `AuthError::KeyringUnavailable` si keyring no disponible, no intenta otra fuente.
- `[ ]` `AuthSource::Env` es soportado pero documentado como "legacy/CI-only", no como default.
- `[ ]` `Inline` no está disponible en v1 (ausente del enum).
- `[ ]` La API key nunca aparece en logs, traces, ni mensajes de error (`Debug` redactado en `ApiKey`, `SecretString`).
- `[ ]` `LocalSecretStore::detect()` no es usado como runtime resolver de credenciales.
- `[ ]` Credenciales persistidas en disco (si se implementa `EncryptedFile` en v1) usan cifrado con age, no texto plano.

### Criterios de correctitud

- `[ ]` `ProvidersConfig` parsing falla con error claro si `base_url` no es válido (no un URL válido).
- `[ ]` `ProviderRegistry::resolve_embedding(provider_id)` falla con `RegistryError::ProviderNotFound` si el ID no existe.
- `[ ]` `ProviderRegistry::resolve_embedding(provider_id)` falla con `RegistryError::CapabilityMismatch` si el provider no tiene capacidad `Embedding`.
- `[ ]` `ProviderRegistry::resolve_completion(provider_id)` falla con `RegistryError::CapabilityMismatch` si el provider no tiene capacidad `Completion`.
- `[ ]` `OpenAiCompatibleProvider` envuelve `OpenAiLlmClient` correctamente y pasa las credenciales resueltas.
- `[ ]` `build_default_http_client()` produce un cliente con la misma config canónica que `OpenAiLlmClient::new()` (Chrome145, 60s timeout, 10s connect, SSRF guard armado).

### Criterios de infraestructura

- `[ ]` `codedb` reindexado en este worktree y `codedb status` muestra `root: .` con los archivos correctos.
- `[ ]` `codegraph` inicializado en este worktree.
- `[ ]` `cargo check` verde en este worktree.
- `[ ]` `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` verde.
- `[ ]` `cargo fmt` verde.
- `[ ]` `cargo nextest run` verde en las áreas afectadas.

---

## Notas de arquitectura

### Sobre `EmbeddingPort` vs `EmbeddingCapable`

La auditoría de rondas previas concluyó correctamente que crear `EmbeddingCapable`/`CompletionCapable` como nuevos traits era redundante: `EmbeddingPort` ya cubre el caso de embeddings remotos (tiene `embed_batch()`, `embedding_dim()`, `model_tag()`, y maneja errores con `SemanticError` que distingue Transport/Auth/Status). Reutilizar `EmbeddingPort` para providers remotos es la decisión correcta.

La única razón por la que se consideró crear nuevos traits era dar "capacidades" a un provider (embeddings, completions, ambos). Pero eso se resuelve mejor con `ProviderConfig.capabilities: Vec<Capability>` en la config y con métodos de resolución en `ProviderRegistry` que verifican la capacidad antes de retornar el port.

### Sobre `AnyProviderHandle`

El documento 5 propuso `AnyProviderHandle` como enum con variantes `EmbeddingOnly`, `CompletionOnly`, `Both`. La auditoría de documento 6 señaló que esto era redundante contra el diseño por slot que el mismo documento 5 había propuesto dos rondas antes (`resolve_embedding(provider_id) -> Arc<dyn EmbeddingPort>`). El diseño final adopta la resolución por slot: no hay handle intermedio, el registry resuelve directamente por ID y por capability.

### Sobre `wreq::Client` compartido

La objeción de escalabilidad (muchos tenants → muchos pools de conexiones) es válida a largo plazo, pero la solución no requiere cambiar la firma de `OpenAiLlmClient::new()`. Se añade `with_http(http, base_url, api_key)` que acepta un `wreq::Client` externo. El registry puede compartir clientes entre providers del mismo `base_url` creando un `wreq::Client` con `build_default_http_client()` y pasándolo a `with_http()`.

Esto es una extensión aditiva, no un refactor del cliente existente. `OpenAiLlmClient::new(base_url, api_key)` sigue funcionando igual, creando su propio cliente internamente. Solo el nuevo código de registry/provider tiene la opción de compartir.

### Sobre la inversión de dependencia `AuthSource` → `ProvidersConfig`

La crítica de documento 8 fue correcta: `AuthSource` es upstream de `ProvidersConfig`. No se puede escribir el parser de `ProvidersConfig` sin saber qué fuentes de auth soporta, porque eso define el schema de `auth` en cada provider config. Por eso el orden de implementación es: primero `AuthSource` + `AuthError` + `resolve()`, luego `ProvidersConfig` + `ProviderRegistry`.

---

## Estado del trabajo en este worktree

- ✓ New worktree `feat/ai-providers` creado desde `main`.
- ✓ `codedb` reindexado (790 archivos). `codedb status` reporta `root: .`, `head: a8b29748`.
- ✓ `codegraph` inicializado (`.codegraph/` con 12.6K nodos, 41.5K edges).
- ✓ `.envrc` y `.env` copiados desde `main`, con `CARGO_TARGET_DIR` configurado.
- ✓ `direnv allow` ejecutado (implícito por el ambiente actual).
- ✓ `cargo check` aprobado (el diseño es zero-code hasta ahora).

### Pendiente de implementación

1. **`AuthSource` + `AuthError` + `resolve()`** — archivo `crates/webfang_core/src/domain/auth_source.rs` (o se añade a `credentials.rs`).
2. **`LocalSecretStore::detect()`** — archivo `crates/webfang_core/src/infrastructure/secrets/local_secret_store.rs`.
3. **`build_default_http_client()`** extraída de `OpenAiLlmClient` — archivo `crates/webfang_core/src/infrastructure/http/build_default_client.rs` (o se añade a un módulo existente).
4. **`OpenAiLlmClient::with_http()`** — modificación de `crates/webfang_core/src/infrastructure/llm/client.rs`.
5. **`ProvidersConfig` + `ProviderRegistry`** — archivos `crates/webfang_core/src/application/providers/config.rs` y `registry.rs`.
6. **`TenantAiContext`** vacío con interfaz pública — `crates/webfang_core/src/application/tenants/tenant_ai_context.rs`.
7. **`OpenAiCompatibleProvider`** — `crates/webfang_core/src/infrastructure/providers/openai_compatible.rs`.
8. **Fixtures de contrato** — `crates/webfang_core/tests/fixtures/{openai,openrouter,freellm,groq}.json`.
9. **Integración con container** — modificación de `crates/webfang_core/src/application/container.rs`.
10. **Tests de integración** — en archivos de tests existentes o nuevos.

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
