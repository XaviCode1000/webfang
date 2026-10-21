# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · SQLite

> **Cómo usar este archivo.** Es un **router**, no un manual. Cada bloque dice o bien la
> regla (la que no podés deducir del código ni del compilador), o bien **dónde está el
> resto**. El detalle largo vive en Engram y en el harness.
>
> **Nada se documenta dos veces.** Si un hecho tiene fuente de verdad —un script, un
> `support.json`, un test, un lockfile— este archivo **enlaza** esa fuente, no la copia.
> Una copia en prosa es una copia que se pudre.

## 🔎 Consultar en Engram

Todo lo que no está acá se busca en Engram: `mem_search` (MCP) o
`engram search "<query>" --project webfang [--scope personal]`.

| Necesito | Query | Scope | Obs |
|---|---|---|---|
| Worktrees: bootstrap, seeds, cleanup, target dirs | `worktree target dir seed bootstrap envrc` | project | #2307 |
| Perfil de build de la máquina: mbx, mold, OOM, timings | `build profile mbx mold cargo target OOM` | personal | #2306 |
| Líneas de soporte, hotfix routing, versionado, gobernanza | `support line routing hotfix branch taxonomy versioning` | project | #2308 |
| Gates de PR, pre-commit, merge, batch, CHANGELOG | `merge-when-green batch merge pre-commit changelog` | project | #2309 |
| Delivery gates: hooks, push a main, verificación en la nube | `delivery gates hooksPath main-push verify cloud timeout` | project | #2316 |
| Contingencia SSH :22 → :443 | `ssh port 22 443 identity` | personal | #2311 |
| Contaminación, historia inmutable, hard-deny | `contamination history immutable hard-deny governance` | project | #2312 |
| Revisión nativa RDD: ciclo completo | `gentle-ai review assess start consent capture acknowledge` | project | #2213 + skill `rdd-defect-workflow` |
| Índice de skills con los paths exactos | `skill-registry skills paths` | project | #43 + `.atl/skill-registry.md` (generado por árbol) |
| Persistencia de estado, `--resume`, StateStore | `StateStore ExportState load_or_default version bak` | project | #216 + `docs/persistence-resume.md` |
| Dónde vive el conocimiento (decisión del owner) | `knowledge architecture AGENTS.md decision` | project | #370 |
| Best practices de AGENTS.md | `agents md best practices` | project | #368 |
| Índice completo bloque → obs | `knowledge index bloque destino query verificada` | project | #2313 |

**Fuentes de verdad en el repo — enlazá, no copies:**

| Hecho | Fuente de verdad |
|---|---|
| Estado de las líneas de soporte | `.github/support.json` (`jq`), renderizado a `SUPPORT.md` |
| Grafo de dependencias entre crates | `cargo metadata` + `scripts/check_dependency_direction.sh` |
| Conteo de tools MCP | `crates/webfang_mcp/tests/stdio_handshake.rs` |
| Topología de branches admitida | `scripts/check-topology.sh` |
| Reglas de PR que CI exige | `.github/workflows/pr-validation.yml` |
| Pines de versión en docs | `scripts/check_docs_version_pins.sh` |

## 🔧 Comandos

```bash
# Verificación (el gate de CI completo; el `--all-features` es obligatorio)
cargo check
cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines
cargo fmt --all -- --check          # `--check` verifica; un `cargo fmt` pelado es un fixer
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps

# Tests
cargo nextest run --test <name>
cargo insta review                  # revisar cada diff de snapshot

# Gate local (lane-aware: docs / CI / code / full)
bash scripts/ci_fast_gate.sh

# Pruebas manuales
scripts/merge-when-green.sh <PR>            # esperar checks verdes y mergear
scripts/ci_status.sh <PR>                   # resumen read-only
scripts/ci_pr_overlap.sh <N1> <N2>          # ¿archivos disjuntos? (batch merge)
```

## 🚧 Reglas del proyecto

Lo que el compilador **no** puede cazar. Todo lo demás es del harness o de la tabla de
arriba.

- **Nunca `.unwrap()` en producción** — usá `?`, `match` o `.context()`.
- **Errores user-facing en español; tracing fields y logs en inglés.**
- **HTTP: siempre `wreq`, nunca `reqwest`** — impersonación de fingerprint TLS para evasión
  de WAF. Un agente que sugiera `reqwest` está equivocado.
- **No unificar dependencias en conflicto** (dashmap 5/6, selectors 0.35/0.37/0.38): son
  deliberadas, tres crates distintos necesitan versiones distintas.
- **No cites números de línea.** Citá el símbolo (`WreqDownloader::fetch_inner`) o el path.
  Una cita por línea se invalida con cada edit anterior del archivo.
- **No copies estado mutable en prosa.** Si cambia solo (versiones, conteos, líneas de
  soporte, labels), enlazá la fuente.
- **No toques `CHANGELOG.md`** en un work PR — un solo escritor (el Release PR).
- **No toques `docs/`** salvo pedido explícito.

## 🏗️ Arquitectura

### Mapa del workspace (5 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure + cli
│   ├── webfang_mcp/              # MCP server (36 tools, 9 categorías)
│   ├── webfang_cli/              # binario `webfang` (main.rs)
│   ├── webfang_test_utils/       # harness de tests compartido (no se publica)
│   └── webfang_benchmark/        # harness de benchmarks (leaf, tooling)
```

- **El código de la CLI vive en `crates/webfang_core/src/cli/`** (`error.rs`, `scrape_flow.rs`,
  `args/`, `preflight.rs`, `orchestrator.rs`). `crates/webfang_cli/src/main.rs` es el binario
  delgado. Buscá el módulo, no el crate.

### Dirección de dependencias (POLÍTICA, verificada por gate)

| Crate | Puede depender de |
|---|---|
| `webfang_core` | — |
| `webfang_mcp` | `webfang_core` |
| `webfang_cli` | `webfang_core` |
| `webfang_test_utils` | `webfang_core` |
| `webfang_benchmark` | `webfang_core`, `webfang_test_utils` |

- La **dirección es `cli → core`, `mcp → core`; `cli` NO depende de `mcp`**. Si necesitás
  algo del server MCP en la CLI, es una decisión de arquitectura, no un import.
- **Dev tier:** cualquier crate puede apuntar a `webfang_test_utils` desde
  `[dev-dependencies]` (es el harness compartido y nunca se publica). Una arista desde
  `[dependencies]` está prohibida.
- Gate: `scripts/check_dependency_direction.sh` (corre en el job `repo-guards` de `ci.yml`),
  con su harness de semántica `scripts/test_dependency_direction.sh`.

### Capas intra-crate (Clean Architecture)

```text
infrastructure → adapters → application → domain   (sólo hacia adentro)
```

Domain define puertos (traits) → Infrastructure los implementa → Application orquesta.
Al escribir código nuevo, copiá el patrón del módulo equivalente más cercano: trait en
`application/crawler/engine.rs`, entidad de dominio en `domain/entities.rs` con
constructor + `TryFrom` + `Display`/`Debug`/`PartialEq`, adapter en
`infrastructure/`, error nuevo en el archivo de error de la capa.

**Avoid:** componentes gigantes. Ningún archivo nuevo debería pasar de ~500 líneas, y si
tocás uno que ya la supera, partilo. Para ver los archivos más grandes del repo:
`find crates -name '*.rs' -not -path '*/tests/*' | xargs wc -l | sort -rn | head`.

### Estratificación de errores

El error de aplicación es `ScraperError` (`crates/webfang_core/src/error.rs`) y **todo
converge hacia él**: no es una cadena de envoltorios, es un embudo.

```text
DomainError    (domain/error/domain_error.rs,  7 variantes) ─┐
InfraError     (infrastructure/error.rs,         15 variantes) ─┤
CrawlError     (domain/error/crawl_error.rs)                   ─┼─ From ─→ ScraperError
DownloadError  (domain/downloader_port.rs)                     ─┘
```

- `DomainError` e `InfraError` implementan `From<…> for ScraperError`: son **entradas** del
  error de aplicación, no una cadena de envoltorios. `domain/error/domain_error.rs` no define
  ningún `impl From`.
- `CliError` (`crates/webfang_core/src/cli/error.rs`) es **independiente**: no tiene relación
  con `ScraperError`; traduce a exit codes (`EXIT_CONFIG = 78` para error de configuración).
- `HttpError` es un enum de **dominio** (`domain/http_error.rs`), no una variante de
  `InfraError`. `WafError` **no es un tipo de error**: es una variante de `FetchBehavior`
  (`application/crawler/crawl_task.rs`). `ParseError` no existe en este repo.
- Matriz completa y reglas de exit code: `docs/error-classification-matrix.md`.
- Una variante nueva de error entra por `From` hacia `ScraperError`, y su mensaje user-facing
  va en español.

### Cadena de guardas del fetch (ORDEN OBLIGATORIO)

Todo camino de fetch — CLI, engine, tool MCP, benchmark, adapter futuro— tiene que cablear
las protecciones **en este orden**. Cada etapa asume que la anterior ya filtró: desviarse
del orden es un **bug**, no una elección de estilo (pacer *después* de una request condenada
gasta presupuesto de rate sobre algo que el SSRF iba a rechazar; clasificar reintentos
*después* de leer el cuerpo gasta una lectura completa de una respuesta que nunca se va a
aceptar).

```text
ValidUrl (entrada) → rate limit (pacing) → por intento:
    timeout → redirect policy → SSRF en el socket dial (cada hop)
→ clasificación de reintentos → lectura del cuerpo
```

| # | Etapa | Implementación de referencia | Por qué |
|---|---|---|---|
| 1 | Validación de entrada | `ValidUrl` / `try_from_url` (`domain/value_objects.rs`), construido en el boundary del argv | El texto de URL no confiable se vuelve un tipo de dominio validado antes de entrar a cualquier flujo. Nunca aceptes un `Url`/`String` crudo como target de fetch. |
| 2 | Rate limit (pacing) | Scrape: token bucket en `scrape_urls` (`cli/scrape_flow.rs`, `build_scrape_rate_limiter`), con pacing cancel-aware vía `until_ready_or_cancel_observed` (`application/rate_limiter.rs`). Crawl: `RateLimiterConfig` (`application/rate_limiter.rs`), construido en `application/crawler/engine.rs` desde `delay_ms` + burst del budget | La espera ocurre **antes** de tocar la red: una espera cancelada cuenta como skipped, nunca como fetch bloqueado. |
| 3a | Timeout | `Client::builder()` en `infrastructure/downloader/wreq_downloader.rs`: request timeout + connect timeout (el connect se clampa en `infrastructure/downloader/fetch_router.rs`) | Techo por request. |
| 3b | Redirect policy | `redirect_policy()` (`domain/ssrf_guard.rs`): límite de 10 hops + parada síncrona cuando el target del redirect es una IP literal prohibida | Cada hop de redirect re-entra a la cadena: es un dial nuevo, no un pase libre. |
| 3c | SSRF en el dial | `ssrf_guard()` envolviendo el `Client` (`infrastructure/downloader/wreq_downloader.rs`, impl en `domain/ssrf_guard.rs`): chequeo a nivel de resolver en **cada** conexión; rangos privados / link-local / loopback rechazados antes de que exista el socket | Nada puede conectar a una dirección prohibida, ni por nombre DNS ni por hop de redirect. E2E en `crates/webfang_core/tests/ssrf_rfc1918_e2e_test.rs`: rechazo pre-socket de RFC1918/CGNAT/NAT64/mapeados, exit 69, cero packets salientes. |
| 4 | Clasificación de reintentos | `fetch_inner` (`infrastructure/downloader/wreq_downloader.rs`): transitorios de mid-body (`ConnectionReset`/`UnexpectedEof`) reintentan; 429 → `max(Retry-After, backoff exponencial)`; 5xx → exponencial; 403 → un reintento con UA rotada **sólo** si la UA no está pineada, capturando el status rotado; **los timeouts también reintentan**; 4xx terminal y errores de builder se reportan tal cual; agotados los reintentos se reporta el **último status observado**, nunca uno hardcodeado; WARN sólo cuando hubo un reintento | La clasificación decide el gasto: dormir, rotar o fallar. Corre **antes** de leer el cuerpo. |
| 5 | Lectura del cuerpo | `read_body_capped` (el método del downloader en `infrastructure/downloader/wreq_downloader.rs`; el helper genérico de tope, en `domain/body_cap.rs`), tope `DEFAULT_MAX_PAGE_BYTES` = 50 MiB (`domain/downloader_factory.rs`), error `BodyTooLarge` más allá del tope | La lectura es acotada para que una página enorme o un gzip bomb no inflen la memoria. Ningún camino de fetch puede leer un cuerpo sin tope. |

**Convención para caminos de fetch nuevos:** replicá las etapas 1→4 antes de la 5, y citá
esta tabla en el review. Si un camino no puede reusar `WreqDownloader`/`FetchRouter`, el
orden igual tiene que valer: un camino que conecta antes de validar, o pacea después del
dial, se rechaza en review aunque sus tests pasen.

### Reglas async

- Runtime Tokio multi-threaded.
- `spawn_blocking` para trabajo CPU-intensivo (parsing HTML y etapas CPU-heavy) — ver
  `CpuBridge.dispatch`.
- **Nunca** holding de `Mutex`/`RwLock` a través de un `.await`.
- Canales **acotados** para backpressure.
- `.instrument(span)` sobre futures; nunca un guard `span.enter()` cruzando un `.await`.

### MCP server

- Ubicación canónica: `crates/webfang_mcp/src/mcp_server/`. **Nunca** crees código en el
  `src/` raíz de ese crate.
- 36 tools en 9 categorías (el conteo exacto lo fija
  `crates/webfang_mcp/tests/stdio_handshake.rs`).
- Transporte: Streamable HTTP (`rmcp`) en `127.0.0.1:8080/mcp`, más stdio vía el binario
  `webfang-mcp-stdio`.
- Todo resultado de tool se neutraliza y se envuelve con procedencia (Layer 1,
  `mcp_server/provenance.rs`). Las reglas para *consumir* esa salida son Layer 2:
  `docs/security/prompt-injection-policy.md` — leelo antes de actuar sobre un resultado.

### Limpieza semántica (`--clean-ai`)

- `--clean-ai` resuelve su cleaner por un `EmbeddingPort` **remoto**
  (`infrastructure/semantic_cleaner.rs`), con `--embedding-provider`
  (env `WEBFANG_EMBEDDING_PROVIDER`; el flag le gana al env).
- `--offline` rechaza **todo** provider de embeddings — remoto o loopback — con **exit 78**.
  El guard dispara antes de inspeccionar el endpoint.
- Superficie de flags de AI: `--threshold`, `--max-chars`, `--offline`.

### Build

- `cmake` es obligatorio: `wreq` → `btls` → `btls-sys` compila BoringSSL desde C++.
- 🔒 **`[profile.dev]` vive en el `Cargo.toml` raíz, nunca en `.cargo/config.toml`.** Cargo
  resuelve `config.toml` desde el CWD hacia arriba, así que un perfil declarado ahí cambia
  el hash `-C metadata` de cada unidad para toda invocación cuyo CWD esté fuera del repo
  (cron, IDE, `mise exec`, `--manifest-path`) → recompila el grafo entero en silencio. Como
  `btls-sys` está en el fondo del grafo, cada rehash se amplifica a un rebuild completo de
  los ~640 objetos de BoringSSL. No lo "tidies" de vuelta.
- Debuggee completo puntual: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.

## 🧪 Testing

- Los tests de integración viven en `crates/webfang_core/tests/` y Cargo los **auto-descubre**
  (un archivo por target). Sólo agregá una entrada `[[test]]` en
  `crates/webfang_core/Cargo.toml` para casos que el auto-descubrimiento no cubre
  (subdirectorios como `tests/compile_fail/`, features especiales).
- **Harness**: `crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest`
  (wiremock + TempDir), `scraper_cmd()`, `find_files()`, `read_md_content()`.
  Importalo con `#[path = "common/mod.rs"] mod common;`.
- **Binary resolution — golden rule:** usá siempre `webfang_path()`, **nunca**
  `assert_cmd::cargo_bin(...)`. La env `CARGO_BIN_EXE_*` sólo se setea para el crate dueño, y
  en este workspace virtual el binario `webfang` lo construye `webfang_cli`, un crate hermano:
  los tests que corren bajo `webfang_core` no lo resuelven con `cargo_bin`.

```bash
Command::new(webfang_path())   // ✅
Command::cargo_bin("webfang")   // ❌
```

- **Snapshots (`insta`)**: todo test que produzca Markdown/JSON/stderr debe usar snapshots, no
  `assert!(output.contains("..."))`. Ciclo: `cargo nextest run` (falla con `.snap.new`) →
  `cargo insta review` (revisá cada diff) → `cargo nextest run` (verde). `.snap.new` está
  gitignored: nunca commitees snapshots pendientes.
- **Sanitización obligatoria**: `redact_nondeterministic()` normaliza path de TempDir →
  `[TEMP_PATH]`, timestamps ISO-8601 → `[TIMESTAMP]`, puertos de wiremock → `[PORT]`, ANSI →
  `[ANSI]`. Para más campos no deterministas, `insta::with_settings!({ add_filter(...) })`.
- **Diagnóstico de 6 nodos** para cada test que escribas o modifiques:
  1. **Comportamiento observable** — probá puertos públicos, nunca estado interno.
  2. **Adapters efímeros** — wiremock para HTTP, TempDir para filesystem. Sin red real.
  3. **Aserciones semánticas** — invariantes de negocio vía snapshots, no dumps de datos.
  4. **Distribución del esfuerzo** — máxima inversión en la lógica de dominio estable.
  5. **Simplicidad del Arrange** — un Arrange de más de 5 líneas es un defecto de diseño de
     producción, no del test.
  6. **Determinismo absoluto** — tiempo y azar inyectados, cero flakiness.
  Si el Arrange se complicó, arreglá el diseño de producción, no el test.
- Para crear un test de integración: archivo en `crates/webfang_core/tests/` (sin
  `[[test]]`), import del harness, `webfang_path()` para el binario, snapshots para la
  salida, y `cargo nextest run --test <name>`.

## 🌭 Observabilidad

**Iron rule:** toda feature, hot path o cambio de comportamiento se entrega con
observabilidad. Código que no se puede tracear en producción no está terminado. El stack es
el crate `tracing` + el **FileTraceLayer** siempre disponible (`--trace-file out.jsonl`) +
correlation IDs nativos. Sin backend de métricas externo: no se introduce OpenTelemetry ni
ningún otro collector.

| Situación | Requisito |
|---|---|
| Hot path / operación nueva | `#[instrument(skip(...), fields(url = %url, ...))]` con los campos que identifican la operación |
| Camino de error | `log_scrape_error(&err, url, stage, correlation_id, "context")` — nunca un `warn!`/`eprintln!` pelado |
| Flujo crawl/batch nuevo | Generá un `CorrelationId` en la entrada y propagalo; cada unidad de trabajo recibe un `.child()` |
| Operación larga | Log de progreso periódico + resumen estructurado final (`total`, `succeeded`, `errors`, `duration`, `trace_id`) |
| Spans async | `.instrument(span)` sobre futures; nunca un guard `span.enter()` cruzando un `.await` |

- **Campos estructurados, no sopa de strings:** `tracing::info!(pages = n, url = %url, "msg")`.
- **Correlación:** cada evento lleva un `trace_id` de 16 hex **efímero, identidad dentro de la
  corrida** (no lo persistas ni lo unas entre corridas; la identidad durable entre corridas es
  el UUID del `CorrelationId`). Para reconstruir una corrida: `ROOT=<16-hex root>; jq -c
  'select(.trace_id == "$ROOT")' <file>` — el `trace_id` es de **nivel superior**, no
  `.fields.trace_id` (ese inner sólo se puebla en el camino de error). Los registros
  `span_close` comparten el nombre `.span`, así que los conteos por etapa tienen que excluir
  `.record == "span_close"`.
- **Errores user-facing en español; tracing fields y logs en inglés.**
- **Sin backends de métricas nuevos:** emití un evento `tracing` estructurado y consultalo del
  JSONL. `docs/src/debugging.md` y `scripts/analyze-trace.sh` tienen el recetario de queries.
- **Snapshots deterministas:** `correlation_id`/`trace_id` son internos y van con `#[serde(skip)]`.

## 🔬 Intelligence Stack

Dos herramientas complementarias. Elegí por misión, no por costumbre. **No son skills: son
CLIs** (`codedb 0.2.5849`, `codegraph 1.6.2`) — invocalas por comando, no cargando un skill.

| Momento | Herramienta | Por qué |
|---|---|---|
| "¿De qué se trata esta tarea?" — orientación inicial | **CodeDB** `context` | 1 llamada: keywords + defs de símbolos + archivos rankeados + snippets. Reemplaza 3-5 llamadas secuenciales. |
| "Mostrame el código de X" — explorar y entender | **CodeGraph** `explore` | Devuelve el fuente verbatim + call paths + blast radius en UNA llamada. Elimina el loop grep→Read. |
| "¿Dónde está definido X?" — lookup | **CodeDB** `word` / `symbol` | Índice invertido, O(1). |
| "¿Quién llama a X?" — chequeo táctico | **CodeDB** `callers` | 1 round-trip: fusiona índice de palabras + scope del outline. |
| "¿Cómo fluye X por el código?" — más profundo | **CodeGraph** `explore` / `impact` | Call paths + resumen de blast radius. |
| Diagnósticos post-edición | `cargo check` / `cargo clippy` | codedb no tiene herramienta de diagnósticos: el compilador es el gate. |
| Consultar un repo público (sin clone) | **DeepWiki** MCP (`read_wiki_structure`, `ask_question`) | MCP remoto aparte, registrado por el instalador de codedb. codedb es local-only. |

**Regla de bolsillo:** CodeDB para *encontrar y leer* (táctico, O(1)); CodeGraph para
*explorar y entender* (devuelve fuente, call paths y blast radius).

### Gates no negociables

- Antes de editar un símbolo → `CodeGraph explore` (callers + impacto). **Nunca** edites a ciegas.
- Antes de renombrar → chequeá **todas** las usages (`codedb callers` / `CodeGraph explore`).
- Antes de commitear → `cargo check` + `cargo clippy` + `cargo fmt --all -- --check`, y
  releé el diff.
- **Excepciones legítimas a `grep`/`rg`:** logs, output de CI, texto de `.env`/config, y
  archivos fuera del índice — nunca código fuente.

### Rutas absolutas en worktrees (CRÍTICO)

En un worktree, **ambas** herramientas necesitan el **path absoluto** del worktree. Con el
nombre pelado resuelven al checkout principal, en silencio:

| Herramienta | Parámetro |
|---|---|
| CodeDB MCP | `project="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |
| CodeGraph MCP | `projectPath="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |

Nunca uses el nombre del repo pelado en un worktree: es ambiguo entre main y todos los
worktrees.

**CodeDB CLI — la raíz es POSITIONAL-FIRST:** `codedb <abs-root> <cmd>` (`usage: codedb
[root] <command> [args...]`). Sólo `mcp` acepta el orden invertido (`codedb mcp <path>`).
`codedb status <path>` y `codedb reindex <path>` **ignoran el path final** y operan sobre el
proyecto del cwd con exit 0 — verificá siempre con `codedb <abs-root> status`: `root` debe
ser el path del worktree y `head` debe coincidir con `git -C <abs-root> rev-parse --short HEAD`.

## 🌳 Política de worktrees

Cada rama activa vive en su propio directorio **hermano** del repo (nunca adentro: los
worktrees internos rompen file watchers, ripgrep y las herramientas de código). Repositorio
compartido de objetos `.git`; working tree, índice y HEAD aislados.

### Reglas de hierro

- **El CWD es la frontera absoluta.** Nunca accedas a rutas fuera de tu worktree con
  `../<otro-worktree>/`.
- **Un worktree por sesión.** Nunca cambies de branch a mitad de tarea: creá otro worktree.
- **`.git/worktrees/` es estado interno de Git.** Nunca lo edites a mano: usá
  `git worktree add` / `remove` / `prune` / `repair`.
- **Comandos prohibidos:**
  - `git checkout`, `git switch` — cambian la branch dentro del worktree actual. Usá
    `git worktree add`. Desde git 2.44 upstream lo exige: `git checkout -B <branch>` se niega
    (exit 128) si la branch está en uso en otro worktree. El hatch es
    `git checkout --ignore-other-worktrees -B <branch>` (el flag va **antes** de `-B`; después
    se parsea como refspec y el comando igual falla). Es el mismo clase de escape que
    `--force`: sólo con autorización humana explícita.
  - `git stash` / `stash pop` / `stash apply` / `stash drop` — el storage (`refs/stash`) es
    **compartido por TODOS los worktrees**: un `pop` en un worktree puede aplicar el stash de
    otra sesión. Si necesitás apartar algo, commitealo en una branch descartable.
  - `git worktree move`, `git worktree lock` — usá `remove` + `add`.
  - `git worktree add --force` — saltea el guard nativo de Git que rechaza una branch ya
    checkoutada en otro worktree. Dos agentes en la misma branch son exactamente la falla que
    ese guard previene.

### Placement, naming y el proxy de identidad

```text
~/Projects/Rust/
├── webfang/                     # main repo (siempre en main)
└── webfang-worktrees/           # hermanos del repo (gitignored global)
    ├── feat-auth/               # branch: feat/auth
    └── fix-crawler-timeout/     # branch: fix/crawler-timeout
```

La convención es `/ → -`: `feat/auth` → `feat-auth`. **El invariante que importa es que el
commit aterrice en la branch prevista para la tarea o el PR.** El proxy de nombre es la
verificación por defecto — corré esto antes de cada commit en un worktree:

```bash
[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"
```

### Protocolo de excepción de identidad

Un proxy fallido **no** prueba que estés en el worktree equivocado — pero el nombre local de
la branch tampoco prueba que estés en el correcto. Commitear bajo un mismatch de
directorio/nombre sólo se permite si se cumplen **las tres**:

1. **Expectativa anclada externamente.** La branch prevista se conoce por una fuente fija
   fuera de este worktree *antes* de commitear: metadata de PR
   (`gh pr view --json headRefOid,headRefName`), metadata de tarea, ref upstream, o
   instrucción humana explícita. Una branch que sólo existe adentro del worktree no prueba
   nada.
2. **Binding verificable.** El worktree está observablemente atado a esa branch:
   `git symbolic-ref --short HEAD` y `git worktree list` coinciden (path → branch). Con
   upstream, `git rev-parse @{u}` es señal extra.
3. **Ascendencia, no igualdad.** El HEAD pre-commit tiene que estar en o descendiente del
   tip/base esperado — un check estricto `HEAD == tip` se rompe después del primer commit de
   una secuencia, así que la ascendencia es el test correcto.

Cada commit de excepción registra la evidencia en el body (trailer compacto):

```text
Worktree-Identity-Exception: directory-name mismatch
Intended-Branch: <branch>
Expected-Head: <sha>
Evidence: <external source> + git symbolic-ref + git worktree list
```

Si alguna condición falla, MISMATCH es un STOP duro.

### Bootstrap por worktree

El `.envrc` es **por worktree**, nunca compartido, y se escribe **desde el nombre del propio
árbol** (nunca derivándolo del de main: un `sed` sobre el de main dejó un
`CARGO_TARGET_DIR` perfectamente válido apuntando al target de MAIN, en silencio). Son
**exactamente 5 líneas**:

```bash
TREE="$(basename "$PWD")"
cat > .envrc <<EOF
export CARGO_TARGET_DIR=\$HOME/.cache/cargo-target/\$TREE
export CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER
export CARGO_LLVM_COV_TARGET_DIR=\$HOME/.cache/cargo-target/\$TREE-llvm-cov
PATH_add \$HOME/.local/share/mbx/bin
EOF
direnv allow
cp ~/Projects/Rust/webfang/.env .        # .env es gitignored
```

Después: indexá el worktree (`codegraph init`; `codedb "$PWD" reindex && codedb "$PWD"
status`) y, opcionalmente, `bash scripts/seed_target.sh` (siembra un target ya compilado;
`cold` es un resultado normal, `refused` es el único que no es un build). El detalle
operativo, los errores que ya Pagaron y el contrato de seeds: **obs #2307**.

**Sin los dos índices, el agente está ciego en el worktree**: las herramientas resuelven al
checkout principal o devuelven vacío. Reiniciá la conexión MCP después de indexar (el server
cachea el registry al arrancar).

### Commits frecuentes (obligatorio en worktrees)

Commiteá **después de cada paso completado**: un `git mv`, un replace masivo, un
`cargo check` que pasa, los tests en verde. El trabajo sin commitear en un worktree se puede
perder en silencio si perdés contexto o ocurre un checkout. El patrón completo (qué va en un
commit y qué no) está en el skill `work-unit-commits`.

### Limpieza post-merge (el merge no está terminado hasta que el repo queda listo)

Desde el repo **main**:

1. **Verificá que el merge entró** — `gh pr view <N> --json state,mergedAt,mergeCommit`;
   `state` debe ser `MERGED`.
2. **Sync de main (ff-only)** — `git fetch origin && git merge --ff-only origin/main`. Si
   `--ff-only` falla, main divergió: **STOP** e investigá, no lo papelees.
3. **Sacá el worktree de la misión** — `git worktree remove ~/Projects/Rust/webfang-worktrees/<dir>`.
4. **Borrá la branch local** — `git branch -D <type>/<desc>` (el squash reescribe historia, así
   que el `-d` seguro se niega; el check del paso 1 es tu red). Nunca toques `main`,
   `gh-pages`, `backup/*` ni la branch actual.
5. **Purgá la metadata huérfana** — `git worktree prune`.
6. **Borrá el target dir de la misión** — por **ownership**: el directorio que vos nombraste
   (su basename exacto, o el nombre custom que exportaste). **Nunca** enumeres el padre
   cazando candidatos extra: agents nombran sus targets libremente y git nunca ve esos
   nombres. Re-chequeá con **ventana móvil** (conteo de archivos dos veces a 60 s con delta 0,
   más cero `cargo`/`rustc`/`cargo-nextest` vivos: `pgrep -x`, un nombre por llamada).
7. **Verificá el handoff** — `git worktree list` y `git branch -vv` sin restos **de la misión
   que mergeaste**, y `git status --short` vacío. Otras worktrees y branches son de otras
   misiones: su presencia no es un handoff fallido ni es tu negocio.

### Recursos compartidos vs por-worktree

| Recurso | ¿Compartido? | Acción |
|---|---|---|
| Store de objetos `.git/` | ✅ | automático |
| Git config, hooks | ✅ | automático |
| `Cargo.lock` | ✅ | vía Git |
| `target/` | ❌ | `.envrc` + `direnv allow` por worktree; lo exige `scripts/ci_fast_gate.sh` |
| `.envrc` | ❌ | se escribe por worktree; gitignored |
| `.env` | ❌ | `cp` desde main |
| Índices `.codegraph/` y `codedb.snapshot` | ❌ | indexar por worktree |
| Seeds (`~/.cache/cargo-target/seeds/`) | ✅ read-only | los consume `seed_target.sh`; nunca los escribe un worktree |
| Quarantine (`~/.cache/cargo-target/quarantine/`) | ⚠️ | objetos retirados explícitamente: **jamás** un build target, nunca se consume ni se limpia en un bootstrap normal |
| `refs/stash` | ⚠️ **DANGER** | **NUNCA** `git stash` |

### Namespace reservado `~/.cache/cargo-target/`

```text
~/.cache/cargo-target/
├── <worktree-target>/   ← build state de un owner
├── seeds/               ← RESERVADO: store de seeds publicados
└── quarantine/          ← RESERVADO: objetos retirados, nunca un build target
```

`seeds/` y `quarantine/` son namespaces reservados y jamás pueden ser el
`CARGO_TARGET_DIR` de un worktree. No inventes categorías nuevas (`scratch/`, `shared/`,
`cache/`, `tmp/`): el guard rechaza tres identidades conocidas y acepta cualquier otra, así
que un nombre que nadie pensó entra por la puerta de atrás. El denylist no frena la
invención; el namespace explícito sí.

### Contaminación

Si detectás que operaste fuera de tu worktree, o que un `git stash pop` aplicó cambios
inesperados:

1. **STOP** de inmediato en todas las operaciones.
2. **NO** intentes limpiar: ni `git reset`, ni force-push, ni parcheo manual.
3. Reportá exactamente: *"Contamination detected. Worktree: `<path>`. Intruder commit:
   `<hash>` or unexpected stash applied. Awaiting human instructions."*
4. Esperá autorización humana explícita antes de cualquier acción correctiva.

### Historia pusheada es inmutable

Una vez que un commit existe en `origin/<branch>`, su SHA es público y cualquier otro
worktree o corrida de CI puede tenerlo. Reescribirlo (`git reset HEAD~N` + recommit)
desincroniza local de remoto, y la única vuelta es un force-push — que el harness
**hard-deny** sin ruta de aprobación. La corrección va **encima**, como commit nuevo.

| Situación | ❌ | ✅ |
|---|---|---|
| Commit pusheado defectuoso | `git reset HEAD~N` + recommit | commit nuevo que describe el fix |
| Commit pusheado con mensaje mejorable | `git commit --amend` sobre historia pusheada | commit nuevo (o `--amend` sólo si no está pusheado) |
| Commit **no** pusheado | — | `git commit --amend` |

⚠️ La puerta es asimétrica: `git reset --hard` está hard-deny, pero un `git reset HEAD~N`
pelado no está gateado. Podés recorrer una reescritura con libertad y descubrir recién al
final que el push está bloqueado.

## 🏷️ Branching, releases y líneas de soporte

- **Taxonomía — sólo estas existen:** `main` (desarrollo; bases `feat/*`, `fix/*`,
  `refactor/*`, `perf/*`, `docs/*`, `test/*`, `chore/*`, + `ci/*`, `build/*`, `style/*`,
  `revert/*`) · `release/X.Y` (estabilización temporal de un MINOR/MAJOR, cortada por el
  maintainer desde un SHA explícito de main; acepta sólo `fix/*`; se borra al shippear
  `vX.Y.0`; un PATCH nunca la crea) · `support/X.Y` (línea de mantenimiento, creada on
  demand desde el último tag de la línea cuando necesita un patch mientras main está
  adelante; acepta sólo `hotfix/*`; se borra al EOL — pero `support.json` es la fuente de
  verdad, no la branch) · `hotfix/*` (DEBE basarse en `support/X.Y`; base de PR =
  `support/X.Y`, nunca main).
- **El estado de las líneas no está acá.** Leé `.github/support.json` (jq). El modelo
  completo, el routing de hotfixes y los scripts de gobernanza: **obs #2308**.
- **Operaciones prohibidas (review-blocking, enforced en CI):** mergear `main` INTO
  `support/*` · cherry-pick de `main` → `support/*` sin issue aprobada · taggear `vX.Y.Z+1`
  desde `main` mientras está adelante · hotfix PR con base `main` · crear `support/*` "por
  las dudas" · editar `SUPPORT.md` a mano · correr `release-plz` sobre una branch de soporte
  · batchear PRs con distinto `baseRefName` (archivos disjuntos no alcanza).
- **Versionado:** los bumps de versión son actos de release, nunca parte de un commit de
  feature/fix. Los pins inter-crate quedan en `^old`; sólo se mueve `[workspace.package]`.
  `release-plz` gobierna `main`; en `support/*` el PATCH es manual, dentro del hotfix PR.

## 🔒 Permisos y límites

### Allowed sin preguntar

- Leer cualquier archivo del repo.
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`.
- Las dos herramientas de intelligence (CodeDB, CodeGraph).
- Editar dentro de `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Gestión de worktrees: `git worktree add`, `remove`, `list`, `prune`.
- Inspección cross-branch de sólo lectura: `git show <branch>:<file>`, `git log <branch>`.

### Preguntar primero

- Agregar o quitar dependencias (`Cargo.toml`).
- Cambiar feature flags o perfiles.
- Borrar archivos.
- `cargo build --release` o `cargo llvm-cov`.
- Modificar CI/CD (`.github/`).
- Archivos nuevos fuera de `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Mergear un PR (el merge lo decide el maintainer).

### Nunca

- Commitear secretos, `.env` o credenciales.
- `.unwrap()` en producción.
- Force-push a main.
- Modificar `target/`, `dist/`, `build/`.
- `git checkout` / `git switch` para cambiar de branch (usá `git worktree add`).
- `git stash` en cualquier forma.
- Acceder a worktrees hermanos por paths relativos (`../feat-auth/...`).
- Commitear en un worktree con mismatch de branch/directorio sin el protocolo de excepción.
- Usar el nombre pelado del repo para las herramientas de intelligence en un worktree.
- Reescribir historia pusheada (`git reset` / `rebase` sobre commits que ya están en
  `origin/<branch>`).

### Hard-deny: no hay ruta de aprobación

Algunas clases de comando abren un diálogo que el humano aprueba. Otras son **hard-deny** y
no muestran diálogo nunca. La autorización conversacional **no puede** cambiar una decisión
hard-deny, porque no hay diálogo que apretar.

| Clase | Ejemplos |
|---|---|
| **Confirm** (diálogo → aprueba → corre) | `git push`, `git rebase`, `git branch -D` forzado, `npm publish`, `pi remove` |
| **Hard-deny** (nada es aprobable) | force-push (`--force`, `--force-with-lease`, `-f`), `git reset --hard`, `git clean -f -d`, `rm -rf /` o `~`, `chmod -R 777`, `chown -R` |

Cuando un hard-deny bloquea: **no re-preguntes el permiso en conversación** (es estructuralmente
inútil y quema un turno); **no rutees alrededor del patrón** partiendo el comando,
reordenando flags o envolviéndolo en un script (la lista es deliberadamente no-configurable);
**reportá el bloque una vez**, con el comando exacto para el humano y con un equivalente no
destructivo que vos sí puedas ejecutar.

## 📝 Commits, PRs y CI

**Formato:** `type(scope): description` — type: `feat` | `fix` | `refactor` | `test` | `docs` |
`perf` | `chore` | `revert` — scope: `cli` | `crawler` | `ai` | `mcp` | `exporter` | `http` |
`domain` | `infra`.

**Los 5 gates de PR** (job `Validate PR metadata`, required, sin `continue-on-error`):

1. **Issue vinculada** — el body debe traer `Closes #N` / `Fixes #N` / `Resolves #N`. Para un
   slice parcial de un umbrella: `Closes part of #N` (pasa validación y **no** cierra el
   umbrella). Un `Closes #N` pelado contra un umbrella con scope restante lo auto-cierra a
   mitad de plan.
2. **La issue vinculada lleva `status:approved`** antes de abrir el PR. El label es
   protegido: **nunca** te auto-apruebes — pedí la aprobación del maintainer. Si el PR se
   abrió antes del label, la validación no se re-evalúa sola: `gh run rerun <id> --failed`.
3. **Exactamente un label `type:*`.** El vocabulario de labels **no** es el de commit types:
   `feat`→`type:feature`, `fix`→`type:bug`, `refactor`→`type:refactor`, `docs`→`type:docs`,
   `chore`→`type:chore`, breaking→`type:breaking-change`, tests→`type:testing`.
4. **Branch conventional** — la regex vive en `pr-validation.yml`; no la reescribas acá (una
   cita de regex se pudre: la fuente de verdad es el workflow).
5. **Sin atribución de IA** en los commits del PR.

**Pre-commit (tiene que replicar CI exactamente):**

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines \
  && cargo fmt --all -- --check \
  && env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
```

- El `--all-features` es una **bandera de seguridad**: hay código gated por `chromium` cuyos
  únicos consumidores están tras `#[cfg(feature = "chromium")]`, y correr clippy sin él hace
  que esos imports parezcan muertos — `clippy --fix` los **borraría**, y `cargo check` con
  features default igual pasa. Nunca corras `clippy --fix` contra un set más angosto.
- `cargo fmt` pelado es un **fixer** (reescribe el árbol y sale 0 haya cambiado o no): un
  verde de eso no es evidencia. Verificá con `--check` y commiteá el resultado.
- El cuarto comando replica el job `doc-quality` de CI (niega lints de rustdoc). `check`,
  `clippy` y `fmt` nunca documentan un item.

**Checklist antes de abrir el PR:**

- [ ] `bash scripts/ci_fast_gate.sh` GREEN
- [ ] `cargo check` + `cargo clippy --all-targets --all-features` (comando completo arriba) + `cargo fmt --all -- --check`
- [ ] `cargo doc` con `RUSTDOCFLAGS=-D warnings` si cambió código de biblioteca
- [ ] `cargo nextest run` (al menos el módulo afectado)
- [ ] `git diff --stat main...HEAD` revisado: sólo los símbolos/archivos esperados
- [ ] Errores user-facing en español; los items públicos nuevos con doc comments
- [ ] Exactamente un label `type:*` + issue vinculada **con** `status:approved` + branch conventional
- [ ] `CHANGELOG.md` **no** tocado por este PR
- [ ] Worktree verificado: `git branch --show-current` coincide con el directorio
- [ ] Ningún `git checkout`/`switch`/`stash` en la sesión

**Merge:** `scripts/merge-when-green.sh <PR>` (squash por defecto; `--merge` para batches y
**obligatorio** cuando el PR declara breaking change). **Nunca** uses `--auto` (no está
habilitado en este repo). El detalle de los guards, el batching y las excepciones:
**obs #2309** y `docs/merge-queue-manual.md`.

**Hooks y verificación:** el `pre-commit` y el `pre-push` consultan el gate de review; en un
clone nuevo hay que activar el wiring con `git config core.hooksPath scripts/githooks`. Para
esperar CI preferí el patrón no bloqueante (`gh run list --workflow=ci.yml …`) y nunca
`gh run watch` bajo un timeout corto; un `git push` que dio timeout no implicó que falló
(verificá con `git ls-remote`). Detalle: **obs #2316**.

**No hay PRs apiladas en este repo.** `scripts/check-topology.sh` sólo admite los pares
head-prefix → base de su tabla, y **ningún head prefix de feature puede apuntar a otra branch
de trabajo** (sí están admitidos `fix/*` → `release/X.Y` y `hotfix/*` → `support/X.Y`, que son
líneas versionadas, no branches de trabajo). La vía que funciona es **entrega secuencial**: slice 1 como PR normal sobre `main` →
merge → slice 2 desde el `main` nuevo → PR normal → repetir. Nunca retargetees un PR abierto
a una branch de trabajo para "achicar el diff": ese movimiento volvió inmergeable un PR sano
una vez, y rehacer sobre `main` siempre está disponible y siempre es legal.

## 🧾 Revisión nativa (RDD)

La revisión nativa es una **vista externa**, nunca una autorización de entrega: después del
acknowledgement, commit / push / PR / release siguen bajo política ordinaria del repo.

- El **candidato** es un slice revisable — un commit work-unit o un slice de PR. Nunca un TODO,
  nunca la branch acumulada.
- El ciclo completo (assess → status → start → consent → collect → capture → acknowledge), las
  4 lentes, los envelopes de fallo y las vías de recuperación: **obs #2213** y el skill
  `rdd-defect-workflow`.
- El **consentimiento es por candidato**, no es el kill switch del switch global. Nunca
  fabricar una aprobación ni re-habilitar el switch por el usuario.

## 🗺️ Skills y CLIs

- **Índice de skills:** `.atl/skill-registry.md` — autogenerado **por árbol** y gitignored:
  no existe en un clone nuevo hasta que corras `gentle-ai skill-registry refresh` en ese árbol.
  Es un **índice** para delegadores: el `SKILL.md` es la fuente de verdad y hay que **inyectar
  paths exactos**, no resúmenes. El registry saltea `sdd-*`, `_shared` y `skill-registry` por
  contrato.
- **CodeDB y CodeGraph son CLIs, no skills** — ver § Intelligence Stack.
- Skills relevantes (el scope lo define el registry, no este archivo): `rust-skills` (265
  reglas / 26 categorías; prefix por categoría: `own-`, `err-`, `async-`, `api-`, `test-`, …),
  `branch-pr`, `work-unit-commits`, `chained-pr`, `rdd-defect-workflow`,
  `cognitive-doc-design` (scope user) y `codebase-refactoring`, `dry-refactoring`, `jscpd`,
  `webfang-architecture` (scope repo).
- **Cada delegación a un subagente debe incluir:** los paths de skills a leer, el mandato de
  intelligence (explorar impacto antes de editar; `cargo check` antes de devolver), los
  comandos de verificación exactos, y el path absoluto del worktree.

## 💾 Engram (uso correcto)

El server de Engram resuelve la sesión activa desde el working directory. Con varias
sesiones de agente concurrentes en este repo, `mem_save` / `mem_judge` pueden fallar con
`multiple active runtime sessions match the current project and directory`.

- **Nunca inventes un `session_id`.** Registrá uno con `mem_session_start` (ID explícito y
  único) y pasá ese mismo `session_id` a cada `mem_save` / `mem_judge` de la sesión.
- **Cerrá la sesión al terminar** (`mem_session_end(id, summary)`). Desde engram v2.1.0 cada
  registro escribe un lease de runtime de 30 minutos, y un lease expirado queda excluido de
  la resolución: una sesión cuyo runtime nunca renueva deja de ser candidata **por sí sola**,
  30 minutos después de su última renovación. Cerrar antes sólo importa mientras el runtime
  sigue vivo.
- Si `mem_save` devuelve `judgment_required: true`, juzgá **cada** candidato con **su**
  `judgment_id` (el de `candidates[]`), nunca con el de nivel superior.
- Ante **cualquier** error de engram, corré `engram --help` primero (después
  `engram <cmd> --help` para el comando que falló) antes de reintentar o inventar flags. El
  CLI es el fallback cuando el MCP falla: `engram save`, `engram search`, `engram context`.
- Diagnóstico de la colisión de sesiones:
  `engram doctor --check ambiguous_active_runtime_sessions --project webfang` (es
  diagnostic-only; `doctor repair` no cubre ese check). Si la colisión es entre dos sesiones
  **genuinamente vivas**, es por diseño: terminá la que terminó, o pasá `session_id`.
- El **conocimiento del proyecto vive acá**, no en el repo: decisiones, dirección, hallazgos de
  auditoría y razonamiento. El repo guarda las **reglas** y sus fuentes de verdad.
  (Decisión del owner: obs #370.)
