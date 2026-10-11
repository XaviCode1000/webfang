# Feature: #1947 — B2 slice 2 — threading de `NetworkPolicy` al cliente de embeddings

**Issue:** #1947 — `feat(domain): NetworkPolicy{allow_cidrs, allow_hosts} per provider (B2)`
**Slice:** 2 de 2 (secuencial, nunca stacked — ver `b2-network-policy.md`)
**Base:** `origin/main` @ `f932cbc5` (v2.9.0)
**Branch:** `feat/b2-slice2-embedding-lan`
**Worktree:** `~/Projects/Rust/webfang-worktrees/b2-slice2-embedding-lan`
**ADR:** `docs/adr/0004-onnx-local-removal.md` — B2, criterio M2

## Slice 1, ya mergeado (`ac8c212a`, PR #1957)

Construyó el **lado de la decisión**: `NetworkPolicy{mode, allow_cidrs, allow_hosts}` +
validación fail-loud (`domain/network_policy.rs`, 17 tests), campo serde en `ProviderConfig`
(4 tests), y el veredicto dial-time `is_forbidden_ip_with_policy` + `is_always_denied_ip`
(`domain/ssrf_guard.rs:303`, 9 tests).

**Rutas reales (verificadas en el worktree del slice 2):**
`crates/webfang_core/src/infrastructure/llm/remote_embedding.rs` — **no** `ai/`.
`entry_gate` en `:191`, cliente en `:162` (`secure_client_with_loopback`),
`new(config)` en `:375`.

**El gap:** ese veredicto tiene cero callers fuera de los tests. La cadena real de
enforcement sigue consultando únicamente `allow_loopback`, así que declarar
`allow_cidrs: ["10.0.0.0/8"]` hoy no produce ningún efecto observable — un vLLM en LAN
sigue tan inalcanzable como antes. Este slice es el threading que conecta la decisión
con el socket.

Desbloqueado: la suite wiremock (#1948) que motivó el corte ya está en `main`.

## Decisión de diseño a fijar por test: semántica de `allow_hosts`

El doc comment del campo dice *"hostnames allowed to bypass the private-resolution denial
**once the resolver yields an allowlisted network**"*. Dos lecturas:

- **AND (adoptada):** un dial por hostname a red privada exige **ambos** — la IP resuelta
  ∈ `allow_cidrs` **y** el hostname ∈ `allow_hosts`.
- **OR:** `allow_cidrs` solo basta para cualquier dial, `allow_hosts` queda decorativo.

Se adopta **AND** porque es la única lectura que da función de seguridad al campo: bajo
OR, permitir `10.0.0.0/8` abre el rango entero a cualquier dominio que resuelva ahí — el
ataque clásico de DNS rebinding (dominio atacante con registro A apuntando a tu LAN)
atraviesa el allowlist sin fricción. El comment del campo describe exactamente eso: la
red allowlisteada es la *precondición*, el hostname declarado es el *permiso*.

**Implicancia UX a diseñar:** bajo AND, declarar solo `allow_cidrs` y usar
`base_url: http://vllm.lan:8000` falla en el primer dial. El error user-facing (español)
**debe nombrar `allow_hosts`** o el usuario no entiende qué falta.

**Asimetría intencional:** los literales (`http://10.0.0.5:8000`) no pasan por hostname,
así que `allow_cidrs` solos les bastan. Fijarla por test.

## El gap, punto por punto

`network_policy` viaja intacto desde el config file → `build_embedding_provider`
(`webfang_cli/src/llm_wire.rs:166`) → `RemoteEmbeddingAdapter::new(config)`, y muere ahí:

```rust
// remote_embedding.rs:375
pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
    let http = embedding_http_client(config.allow_loopback)?;  // ← policy ignorada
```

Cuatro capas, con el threading de `allow_loopback` (#1462) como molde exacto — parámetro
por-cliente, nunca estado de registro:

1. **Entry gate** (`remote_embedding.rs:191`): `entry_gate` usa
   `is_forbidden_literal_host_with(host, allow_loopback)`. Hace falta
   `is_forbidden_literal_host_with_policy(host, allow_loopback, policy)` en
   `domain/ssrf_guard.rs`. **Imprescindible:** el conector de wreq parsea los IP-literal
   directo y nunca invoca el resolver custom (documentado en
   `infrastructure/ssrf.rs:38-42`), así que sin entry gate policy-aware el caso literal es
   inalcanzable *por construcción* — ningún layer posterior lo ve.
2. **Trait `SsrfGuard`** (sellado, `domain/ssrf_guard.rs:696`): sumar
   `secure_client_with_policy(builder, allow_loopback, policy)` como método **aditivo**.
   **NO** cambiar la firma de `secure_client_with_loopback`: `wreq_downloader.rs:180`
   depende de ella.
   **Son 3 impls, no 1:** el real (`infrastructure/ssrf.rs:243`) + dos stubs de test
   (`domain/ssrf_guard.rs:1277` y `application/container.rs:1181`). El trait es sellado;
   olvidar un stub rompe el `cargo check` de `webfang_core`, no el de `webfang_core`
   aislado — correr `--all-targets --all-features`.
3. **`ValidatingResolver`** (`infrastructure/ssrf.rs:116`): capturar `allow_loopback` +
   `policy`; `with_policy(...)` (-> `with_allow_loopback` + policy). Firmas actuales:
   `gai_lookup(host, validate, allow_loopback)` (`:157`, estática, `allow_loopback` por
   valor, NO lee `self`) y `fail_closed_scan(host: &str, addrs, allow_loopback)`
   (`:182`, pura y ya recibe `host`). **El enforcement de `allow_hosts` solo puede vivir
   en `fail_closed_scan`** — es el único lugar del sistema que conoce a la vez el hostname
   (`Name::as_str()`) y las IPs resueltas. `NetworkPolicy` es `Clone`; pasarlo por
   referencia evita clonar por conexión.
4. **Redirect policy**: `redirect_policy_with_loopback(bool)` → variante con policy, para que
   un hop de redirect a un literal LAN allowlisteado se siga.

El wiring superior (`llm_wire.rs`) **no cambia**: CLI (`webfang_cli/src/main.rs:732`) y el
camino lazy de MCP (`webfang_mcp/src/mcp_server/ai_wiring.rs:148`) pasan el `ProviderConfig`
completo.

## Estrategia E2E: probar "permitido" sin LAN real

El criterio M2 del ADR exige lo difícil: "vLLM en LAN con `allow_cidrs`: **funciona**". No
se puede bindear un listener LAN en CI. Patrón ya existente en el repo:
`assert_fails_at_connect_without_ssrf_rejection` (`infrastructure/ssrf.rs:316`) — dial a
`10.x.x.x:9` (discard): si el policy **permite**, la request falla en fase **CONNECT**; si
**niega**, falla por SSRF (`ForbiddenResolutionError`). La discriminación es la *clase* del
error, no la red. Los casos con hostname van a nivel unitario sobre `fail_closed_scan` con
answers sintéticos (camino determinista ya identificado en
`ssrf_rfc1918_e2e_test.rs:26-28`).

| Caso E2E | Expectativa |
| :--- | :--- |
| Literal LAN declarado (`allow_cidrs` sí) | falla en CONNECT, nunca en el guard |
| Literal LAN **sin** declarar | rechazo pre-socket, cero paquetes |
| CGNAT `100.64.0.0/10` sin opt-in | rechazo (M2 literal) |
| Hostname + CIDR + `allow_hosts` | permitido |
| Hostname + CIDR **sin** `allow_hosts` | rechazo con error que nombra `allow_hosts` |
| `169.254.169.254` aunque allowlisteado | negado (invariante 1, re-pin E2E) |

El registro reproducible contra vLLM real en LAN queda como evidencia manual archivada
(así hizo slice 1 con Ollama/vLLM en loopback), no como gate de CI.

## Invariantes no negociables (FIN-017, heredadas de slice 1)

1. Link-local / unspecified / multicast / broadcast / reserved **siempre** bloqueadas
   aunque estén allowlisteadas.
2. `100.64.0.0/10` bloqueada por defecto; opt-in explícito por provider.
3. CIDR inválido en config → fail loud.
4. Decisión en `domain/`; la allowlist es parámetro, nunca estado global del registro.

**No-regresión:** `NetworkPolicy::default()` (Restricted, vacío) debe comportarse
*exactamente* como hoy. La suite `ssrf_rfc1918_e2e_test.rs` no debe cambiar ni una línea —
si cambia, es un bug del slice, no un ajuste del test.

## Riesgos

1. **Redirect hops por hostname:** cada hop que resuelve a red privada exige que *ese*
   hostname esté en `allow_hosts` (el resolver ve cada conexión). Correcto, pero
   documentarlo.
2. **Trailing dot FQDN** (`vllm.lan.`) en la normalización runtime: la validación ya
   guarda lowercase; falta fijar el comportamiento.
3. **Observabilidad** (regla de hierro del repo): cada rechazo policy-aware con tracing
   estructurado, fields en inglés (`host`, `ip`, `cidr`); errores user-facing en español
   señalando el campo de config responsable.

## Fuera de scope

El camino de **completions** (`build_default_http_client` → `secure_client`,
`provider.rs:86`) ni siquiera parameteriza `allow_loopback` hoy — un LLM LAN para
extracción es inalcanzable con o sin policy. No lo cubre #1947 (que habla del cliente de
*embeddings*); merece issue aparte para no inflar este slice.

## Tareas

- [x] **Task 1 — Decisión AND + normalización runtime de `allow_hosts`** en el resolver,
  con RED→GREEN sobre `fail_closed_scan` con answers sintéticos.
- [x] **Task 2 — Entry gate policy-aware** (`is_forbidden_literal_host_with_policy`) +
  wiring en `remote_embedding.rs` + literal LAN allowlisteado llega a CONNECT.
- [x] **Task 3 — `secure_client_with_policy` + `ValidatingResolver::with_policy` +
  redirect policy con policy**, método aditivo, sin romper `secure_client_with_loopback`.
- [x] **Task 4 — E2E LAN** (`ssrf_policy_e2e_test.rs`) con la matriz de 6 filas +
  no-regresión de `ssrf_rfc1918_e2e_test.rs` byte-idéntica.
- [x] **Task 5 — Observabilidad** (tracing en cada rechazo policy-aware) + gate completo +
  work-unit commit.

**Work-unit commits (ver tabla en "Desenlace")**

## Desviaciones del brief (aceptadas, con razón)

1. **`Arc<NetworkPolicy>` en vez de `&NetworkPolicy`.** El brief pedía por referencia, pero
   el boxed future del resolver tiene que ser `'static`. Refcount por resolución en vez de
   deep clone por conexión; el redirect policy igual necesitaba un clonedeck propio.
2. **`render_error_chain` en `remote_embedding.rs`.** El diagnóstico del resolver vive al
   FINAL de la cadena `source` del error de transporte, no en su `Display` top-level
   (medido: `error sending request for uri (…)` → `client error (Connect)` → `dns error` →
   `<mensaje>`). Sin recorrerla, el operador ve "Connect" y nunca se entera de que le falta
   `allow_hosts`.
3. **Hallazgo del writer sobre wreq, confirmado:** un cliente wreq pelado NO puede rechazar
   IP-literals — su conector los parsea directo y nunca enruta literales al resolver. Es la
   medición que confirma por qué el entry gate *tenía* que volverse policy-aware.

## Corrección post-verificación (padre)

El primer E2E de la fila 1 era **vacuo**: `dial_class` hacía `.to_string()` sobre el error,
que devuelve solo el `Display` top-level — `error sending request for uri (…)`, sin
"forbidden" ni "SSRF" **ni siquiera cuando el guard deniega**. El `assert!` negativo no
podía fallar nunca. Corregido componiendo la cadena en el helper del test
(`rendered_chain`), y se agregó **row7** como guard de vacuidad: un dial realmente denegado
(`localhost` con policy default → loopback) DEBE exponer el wording del guard en esa misma
cadena. Medido:

```text
error sending request for uri (http://localhost:9/v1): client error (Connect): client error (Connect): dns error: DNS resolution of 'localhost' returned forbidden address 127.0.0.1 (SSRF)
```

## Revisión nativa (RDD switch global ON)

- START sobre `dc12f6be`, lineage `review-df4230f23ad90e13`, tier `medium`,
  lente `review-reliability`, presupuesto de corrección 200.
- **Veredicto**: `correction_required` — hallazgo `R3-mode-bypass` (CRITICAL,
  `evidence_class: deterministic`, `causal_disposition: introduced`).
- **Corrección aplicada** en `7502f412` (56 líneas de diff):
  `allows_ip` y `allows_host` devuelven `false` salvo que el modo sea
  `Allowlist`. RED observado (3/3 reintentos fallan sin el check) → GREEN.
  Gates: fmt ✓, clippy ✓, doc ✓, `ci_fast_gate` GREEN, nextest **3753 passed /
  21 skipped / 0 failed**.

### Corrección al framing del hallazgo

El reviewer afirmó que el gap *"hace discrepar hostname e IP-literal"*. **No es
así**: `is_forbidden_ip_with_policy` de slice 1 tiene la misma forma y tampoco
consultaba `mode`. Los dos caminos coincidían y los dos heredaban el gap. El fix
en el helper compartido es lo que los mantiene coincidiendo.

### Bloqueo de lifecycle (no resuelto)

El plan de corrección quedó **inusable**: tras comitear la corrección, el
`target_identity` del candidato se movió de `sha256:b55c30…` a `sha256:b7d538…`
(nuevo `current_candidate_tree` `644da898`), pero el `collectBinding` que el
provider emitió para `capture-correction-plan` sigue llevando el target viejo.
El validador lo rechaza (`capture-binding-rejected: collectBinding does not
carry one non-empty matching provider lineage and target token`). STATUS
re-ofrece el mismo slot con el mismo token obsoleto.

No se puede componer ni parchear un token provider-issued, y no se reintenta
en loop.

**Resuelto por decisión del maintainer (2026-10-11):** RDD apagado a scope
clone y luego re-encendido; ver abajo el desenlace real.

### Desenlace

SeCsvpraron tres lineages sucesivos por el mismo motivo estructural —cada
commit de corrección mueve el `target_identity` y vuelve obsoleto el
`collectBinding` emitido antes—: `review-df4230f23ad90e13` (colgado),
`review-b9c8a85c23ee4589` (admisión rechazada dos veces) y
**`review-2277a2cdaaf986db` (aprobada y reconocida)**.

El bloqueo de la admisión no era el del relay: era **el contenido**. El reviewer
citaba el dial target `10.255.255.254:9` como *evidence location* y el
validador lo parseaba como ruta de repositorio fuera del árbol congelado
(`evidence_path_out_of_scope`). Neutralizado en `d543001b` componiendo host y
puerto por separado.

**Corolario operativo: commitear TODAS las correcciones antes de abrir el
lineage.** Con una sola corrección pendiente, el commit que la aplica deja
obsoleto el binding emitido y hay que abrir un lineage fresco.

**Hallazgo final (informativo, no bloqueante):** `R3-TEST-DIAL-ASSUMPTION`
(WARNING) — la asunción de dial de la fila 1, ya documentada en su docstring.

### Work-unit commits

| SHA | Contenido |
| :--- | :--- |
| `dc12f6be` | threading del `NetworkPolicy` al cliente de embeddings — 7 archivos, +1378/−39 |
| `7502f412` | corrección de `R3-mode-bypass`: `allows_ip`/`allows_host` exigen `mode: Allowlist` |
| `d543001b` | composing de dial targets en runtime (elimina literales `IP:port` adyacentes) |

## Entrega

**PR #1966** — https://github.com/XaviCode1000/webfang/pull/1966
Base `main`, label `type:feature` (una sola), `Closes #1947` (la issue quedó
OPEN tras slice 1, así que el cierre corresponde a este PR).
`mergeStateStatus: BLOCKED` esperando CI. Merge automático queda como decisión
del maintainer vía `scripts/merge-when-green.sh 1966`.

### Defectos de tooling encontrados (candidatos a issue propia)

1. **`scripts/githooks/review-gate.sh:69`** compara `delivery == "unmanaged"`
   con igualdad exacta. El valor real con RDD apagado a scope clone es
   `disabled/unmanaged`, que no matchea y cae en BLOCK — aunque el propio gate
   responda `action: repository-policy`. **Apagar RDD para destrabar una review
   empeora el bloqueo en vez de aflojarlo.**
2. **La admisión de evidencia del review parsea `IP:puerto` como `path:line`.**
   Cualquier test de allowlist SSRF contiene IP-literales con puerto, así que
   esto bloquea a *cualquier* cambio de SSRF que pase por review nativo.
   Debería reconocer un IP literal en vez de tratarlo como ruta.
3. **`acknowledge-approved` rechaza `input` e `idempotencyKey`** como
   controller-only; solo `lineageId` + `workspaceRoot` funcionan.

## Verificación

| Gate | Resultado |
| :--- | :--- |
| `scripts/ci_fast_gate.sh` | ✓ GREEN (26 PASS / 0 FAIL / 1 SKIP), lane CODE |
| `cargo nextest run -p webfang_core` | ✓ **3752 passed / 21 skipped / 0 failed** |
| `cargo check --all-targets --all-features` | ✓ |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | ✓ |
| `cargo fmt --all -- --check` | ✓ |
| `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` | ✓ |
| `git diff -- ssrf_rfc1918_e2e_test.rs` | ✓ **0 líneas** (byte-idéntica) |
| Determinismo E2E | ✓ 5 corridas, `6 passed / 0 skipped` idéntico, 2.016 s en las cinco |

**Nit abierto (no bloqueante, aceptado):** en `entry_gate` el hint de "nunca habilitable"
usa `host.parse().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))`. El fallback es un asco —
`parse_ip_literal(host)` es el helper canónico y ya cubre las formas WHATWG. Es cosmético
(solo cambia el texto de un mensaje), no toca la decisión de seguridad.

## Notas de la misión slice 1 (contexto operativo)

El writer `gentle-ai-worker` falló 6 veces en slice 1 (cuota `/tmp`, errores de modelo,
muertes sin reporte). **En slice 2 el mismo writer completó las 5 tareas sin incidentes**;
la degradación a implementación inline no hizo falta.
