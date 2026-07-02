# AGENTS.md — Rust Scraper

Production-ready web scraper. Clean Architecture, TUI selector, AI semantic cleaning, and sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · ratatui · tract-onnx (feature-gated) · SQLite
**Hardware:** Ryzen 7 5700X (8C/16T), 32GB DDR4, NVMe — local dev for most tasks

---

## Project Overview

Rust Scraper is a **headless web scraper** optimized for RAG pipelines and AI document ingestion. Crawls websites, extracts clean content (HTML → Markdown), detects WAF blocks, renders JavaScript, and exports to multiple formats (JSONL, Markdown, Obsidian, vector embeddings). Features a full TUI for interactive use, an MCP server for AI-tool integration, and AI-powered semantic cleaning via ONNX.

### Key capabilities

- **WAF Evasion** — TLS fingerprint impersonation via `wreq`, User-Agent rotation, retry with backoff
- **Wide crawling** — Batch processing, sitemap parsing, URL deduplication, rate limiting
- **Content extraction** — Readability, HTML-to-Markdown, syntax highlighting, Obsidian wikilinks
- **TUI** — Real-time progress, URL selector, error log viewer, config forms
- **AI cleaning** — ONNX model (all-MiniLM-L6-v2) for semantic relevance scoring and chunking (feature `ai`)
- **MCP server** — Expose scraper capabilities to any MCP-compatible AI agent

---

## Architecture & Project Structure

Clean Architecture (hexagonal) con capas bien definidas. Las dependencias apuntan hacia adentro: `infrastructure` → `adapters` → `application` → `domain`.

```
src/
├── main.rs              # Entry point: CLI dispatch, signal handling
├── lib.rs               # Public API: ScraperBuilder, scrape(), crawl()
├── config.rs            # Global configuration struct
├── di.rs                # Dependency injection wiring (trait → impl)
├── error.rs             # Top-level error enum (ScraperError)
│
├── cli/                 # CLI layer (clap + wizard + flow orchestration)
│   ├── args.rs          #   CLI argument definitions
│   ├── commands.rs      #   Command dispatch
│   ├── scrape_flow.rs   #   Single-page scrape orchestration
│   ├── orchestrator.rs  #   Crawl orchestration
│   ├── wizard.rs        #   Interactive config wizard
│   └── preflight.rs     #   Pre-flight validation
│
├── domain/              # Core business logic (ZERO external dependencies)
│   ├── entities.rs      #   Domain entities (Page, CrawlJob, ScrapedContent)
│   ├── value_objects.rs #   Value objects (Url, ContentType, Timestamp)
│   ├── repositories.rs  #   Repository traits (puertos de salida)
│   ├── exporter.rs      #   Export traits
│   ├── semantic_cleaner.rs  # AI cleaning trait
│   ├── crawler_entities.rs  # Crawl-specific entities
│   ├── url_validation.rs    # URL validation logic
│   └── error/           #   Domain-specific error types
│
├── application/         # Application services (orquesta casos de uso)
│   ├── crawler_service.rs    # Crawl orchestration (the big one)
│   ├── scraper_service.rs    # Single-page scrape
│   ├── http_client/          # HTTP client abstraction
│   ├── url_filter.rs         # URL filtering logic
│   ├── rate_limiter.rs       # Rate limiting
│   ├── deduplicator.rs       # URL deduplication
│   └── container.rs          # DI container
│
├── adapters/            # Adapter implementations (implementan traits del domain)
│   ├── downloader/      #   HTTP download implementations
│   ├── extractor/       #   Content extraction
│   ├── detector/        #   MIME type detection
│   ├── url_path.rs      #   URL path handling
│   └── tui/             #   TUI (ratatui): app, components, widgets
│
├── infrastructure/      # External systems, frameworks, drivers
│   ├── http/            #   wreq HTTP client + WAF engine
│   ├── crawler/         #   Batch processor, URL queue, sitemap parser
│   ├── scraper/         #   Readability, asset download, fallback
│   ├── ai/              #   ONNX inference, embeddings, chunking, tokenizer
│   ├── converter/       #   HTML→Markdown, Obsidian, syntax highlight
│   ├── export/          #   JSONL, state store, vector export
│   ├── persistence/     #   SQLite repository implementation
│   ├── observability/   #   Tracing, metrics, logging
│   ├── obsidian/        #   Obsidian vault detection + URI handling
│   ├── mcp_server/      #   MCP protocol server
│   ├── bridge.rs        #   Thread-safe bridge (sync ↔ async)
│   └── config.rs        #   Config loading from env/files
│
├── extractor/           # Link extraction engine (HTML parsing)
│
tests/
├── common/              # Shared test helpers
├── *_integration.rs     # Integration tests per module
├── *_test.rs            # Binary/CLI tests
├── mcp_proptest.rs      # Property-based MCP tests
├── property_tests.rs    # Property-based domain tests
└── stress_test.rs       # Stress/load tests
```

---

## Setup & Build

### Build dependencies (required)

`cmake` is mandatory — `wreq` → `boring2` → `boring-sys2` needs it to compile BoringSSL. Without it, nothing compiles:

```bash
# Fedora
sudo dnf install cmake
```

### Toolchain (Mise)

This project uses **Mise** (`mise.toml`) for deterministic toolchain management. Run once:

```bash
mise install       # Installs Rust 1.88, sccache, mold, just, cargo tools
mise trust         # Trust the mise.toml config (first time)
```

Mise manages all dev tools — Rust, sccache (compiler cache), mold (linker), just (task runner), cargo-nextest, cargo-deny, cargo-audit, and more. No need to install them manually. The `RUSTC_WRAPPER=sccache` env var is set automatically by `mise.toml`.

**Version policy:** Project pins exact tool versions in `mise.toml` for reproducible builds. Global `~/.config/mise/config.toml` can use `latest` for personal defaults, but the project file always locks specific versions — never `latest` at project level.

### Quick start

```bash
git clone https://github.com/XaviCode1000/rust-scraper
cd rust_scraper
mise install                           # Install toolchain (skip if already done)
cargo build --release                  # Release build (~3-5 min first time)
cargo build --release --features ai    # With AI semantic cleaning
rust_scraper --help                    # Verify installation
```

### Mise tasks

Mise also provides project-level task recipes (run with `mise run <task>`):

| Task | Description | Equivalent to |
|---|---|---|
| `mise run setup` | Install tools + verify | `mise install && just setup` |
| `mise run check` | Fast compile check | `cargo check` |
| `mise run analyze` | Refresh GitNexus index | `gitnexus analyze` |

For the full task list, check `mise.toml` at the project root.

### Commands

**Local (safe, <5s total):**

```bash
cargo check                    # Verify compilation
cargo check --features ai      # With AI feature
cargo clippy -- -D warnings    # Lint — fix ALL warnings
cargo fmt --check              # Format check
cargo fmt                      # Format
```

**Local (moderate, <5 min):**

```bash
cargo nextest run              # Full suite, ~1-2 min
cargo nextest run --all-features  # With AI, ~2-3 min
just test-ci                   # Full gate (fmt+clippy+tests), ~3-5 min
cargo build --release          # ~3-5 min (LTO fat)
```

> **Note:** `cargo build --release` uses LTO fat + codegen-units=1. First clean build compiles BoringSSL from C++ — much longer. Incremental builds with sccache are significantly faster.

**Prefer CI (slow, >5 min):**

```bash
cargo llvm-cov                 # Coverage instrumentation (~5-8 min)
```

**Miri (unsafe/concurrent code development only):**

```bash
cargo +nightly miri test infrastructure::bridge::
cargo +nightly miri test infrastructure::network::
# Miri requires nightly. Run from project root.
# MIRIFLAGS defined in .github/workflows/ci.yml line 181.
```

### Feature flags

| Feature        | Dependencies                         | Purpose                     |
| -------------- | ------------------------------------ | --------------------------- |
| `default`      | —                                    | Core scraper (no AI)        |
| `images`       | mimetype-detector                    | Image MIME detection        |
| `documents`    | mimetype-detector                    | Document MIME detection     |
| `full`         | images + documents                   | All non-AI features         |
| `ai`           | tract-onnx, tokenizers, hf-hub       | ONNX semantic cleaning      |
| `console`      | console-subscriber                   | Tokio runtime observability |
| `otel`         | opentelemetry, tracing-opentelemetry | Distributed tracing         |
| `otel-metrics` | otel + metrics                       | OTLP metrics export         |

---

## Session Start (Index Freshness)

```bash
gitnexus analyze --index-only --skip-agents-md    # Refresh index on clean tree
gitnexus analyze --skills --index-only --skip-agents-md  # Regenerate skills if communities changed
codedb /home/xavi/Projects/rust_scraper status   # Verify CodeDB index
```

If you see "Index is stale" from gitnexus → stop and run `gitnexus analyze` first.
If `codedb status` shows stale → run `codedb /home/xavi/Projects/rust_scraper index` to rebuild.

Before reindexing, make sure worktree is clean. If you still need `gitnexus_detect_changes()` later, do not rerun `gitnexus analyze` after editing files.

If `gitnexus analyze` crashes with `Napi::Error` → clean first:

```bash
gitnexus clean -f && gitnexus analyze --index-only --skip-agents-md
```

**Use** `--skip-agents-md` when refreshing index without modifying AGENTS.md.
**Do not** rerun `gitnexus analyze` in a dirty worktree if you still need `detect_changes()`.

---

## Before Editing Code — MANDATORY INTELLIGENCE GATE

**No code is read, written, or modified without first using CodeDB + GitNexus.** Skip only for trivial documentation or config changes.

### Step 1 — Orient with CodeDB (always first)

```bash
codedb_context /home/xavi/Projects/rust_scraper task="describe the change you're about to make"
```

`codedb_context` replaces 3-5 sequential tool calls. Use this FIRST.

### Step 2 — Deep dive with CodeDB (choose by situation)

| Situación                | Herramienta                                | Qué devuelve                   |
| ------------------------ | ------------------------------------------ | ------------------------------ |
| Definición de símbolo    | `codedb_symbol name="NombreExacto"`        | Archivo, línea, tipo           |
| Quién llama a X          | `codedb_callers name="miFuncion"`          | Call sites con snippet         |
| Estructura de archivo    | `codedb_outline path="src/main.rs"`        | Funciones, structs, imports    |
| Búsqueda de texto/patrón | `codedb_search query="algo"`               | Coincidencias con contexto     |
| Identificador exacto     | `codedb_word word="identificador"`         | Ocurrencias (O(1), más rápida) |
| Árbol de dependencias    | `codedb_deps path="..." [transitive=true]` | Importadores o dependencias    |
| Archivos recientes       | `codedb_hot`                               | Archivos tocados ordenados     |
| Navegación directorio    | `codedb_ls path="src/"`                    | Hijos del directorio           |

**Regla de dedo:** nombre exacto → `codedb_symbol`/`codedb_callers`. Patrón o desconocido → `codedb_search`. Tarea nueva → `codedb_context`.

### Step 3 — Impact analysis with GitNexus (before modifying)

```
gitnexus_impact({target: "symbolName", direction: "upstream"})
gitnexus_context({name: "symbolName"})  # 360° view if needed
```

**Conducta obligatoria:**

1. Reportar resultados al usuario ANTES de editar
2. Interpretar riesgo según tabla:

| Riesgo       | Señal                                 | Acción                              |
| ------------ | ------------------------------------- | ----------------------------------- |
| **LOW**      | d=1: 0-4 items, sin procesos críticos | Proceder, actualizar callers        |
| **MEDIUM**   | d=1: 5-14 items o 2-5 procesos        | Planificar secuencia, test suite    |
| **HIGH**     | d=1: 15+ items o muchos procesos      | Parar, advertir, obtener aprobación |
| **CRITICAL** | d=1 en auth/integridad de datos       | Parar, requerir sign-off            |

### Step 4 — Traza de flujos (opcional, cambios complejos)

```
gitnexus_query({query: "concepto del cambio"})
gitnexus_read_resource(resource="gitnexus://repo/rust_scraper/process/FlowName")
```

### Checklist pre-edit

```
- [ ] codedb_context primera llamada de orientación
- [ ] codedb_symbol / codedb_callers / codedb_outline según necesidad
- [ ] gitnexus_impact({target, direction: "upstream"})
- [ ] Revisar d=1: estos se ROMPEN seguro
- [ ] Reportar riesgo al usuario
- [ ] Solo proceder tras confirmación si HIGH/CRITICAL
```

### Anti-patrones: NUNCA hacer

| ❌ Mal                                           | ✅ Bien                                   |
| ------------------------------------------------ | ----------------------------------------- |
| `grep` o `rg` para buscar código                 | `codedb_symbol` o `codedb_search`         |
| Editar sin `impact` primero                      | Siempre `impact()` antes de tocar         |
| Leer archivos enteros para encontrar una función | `codedb_outline` → líneas → `codedb_read` |
| `codedb_search` para identificador exacto        | `codedb_word` (O(1) vs trigrama)          |
| Asumir qué función falla sin trazar              | `gitnexus_query` + `context` primero      |
| Renombrar con find-and-replace                   | `gitnexus_rename` (entiende el grafo)     |

---

## Before Writing Rust — rust-skills OBLIGATORIO

Cargar `rust-skills` y aplicar categorías según el tipo de trabajo:

| Tipo de trabajo            | Categorías primarias                                                                          |
| -------------------------- | --------------------------------------------------------------------------------------------- |
| Nueva función              | `own-` (ownership), `err-` (errores), `name-` (nombrado), `pat-` (pattern matching)           |
| Nuevo struct / API pública | `api-` (diseño de API), `type-` (type safety), `conv-` (conversiones), `doc-` (documentación) |
| Código async               | `async-` (Tokio, cancelación), `own-` (locks through await)                                   |
| Concurrencia / paralelismo | `conc-` (rayon, atomics), `async-`, `own-`                                                    |
| Código unsafe              | `unsafe-` (SAFETY comments, Miri), `type-`, `test-`                                           |
| Manejo de errores          | `err-` (thiserror, anyhow, context), `api-`, `pat-`                                           |
| Serialización / serde      | `serde-` (rename, flatten, try_from), `type-`, `api-`                                         |
| Observabilidad / logging   | `obs-` (tracing, structured fields), `err-`                                                   |
| Optimización memoria       | `mem-` (capacity, SmallVec, Cow), `own-`, `perf-`                                             |
| Optimización rendimiento   | `opt-` (inline, LTO, SIMD), `mem-`, `perf-`                                                   |
| Tests                      | `test-` (proptest, mockall, doctest), `unsafe-` (Miri)                                        |
| Code review                | `anti-` (anti-patrones), `lint-` (clippy)                                                     |

**Regla:** no se escribe Rust sin cargar `rust-skills` y pasar la categoría correcta.

---

## Pre-Commit Protocol (every commit)

```bash
cargo check                    # Must pass
cargo clippy -- -D warnings    # Must pass — fix ALL warnings
cargo fmt                      # Must run
gitnexus_detect_changes()      # Verify only expected symbols changed
```

If `gitnexus_detect_changes()` shows unexpected affected symbols → review before committing.

---

## Cloud Verification (after commit)

```bash
gh workflow run ci.yml --ref $(git branch --show-current)
gh run watch
```

Only push after CI shows ✅. If CI fails → fix locally, re-commit, re-trigger CI.

---

## Key Patterns & Conventions

### Error messages

- **User-facing error messages in Spanish** (`Archivo no encontrado`, `Error de conexión`)
- Internal debug logs in English
- Use `thiserror` for library errors, `anyhow` for application-level
- Never `.unwrap()` in production — use `?`, `match`, or `.context()`

### HTTP client

- **Always `wreq`**, never `reqwest` — wreq provides TLS fingerprint impersonation for WAF evasion
- WAF detection runs on EVERY HTTP 200 response (Cloudflare, reCAPTCHA, DataDome, etc.)
- On WAF detection → UA rotation + retry. Still blocked → `ScraperError::WafBlocked`

### Async

- Tokio multi-threaded runtime
- Use `spawn_blocking` for CPU-intensive work (ONNX inference, HTML parsing)
- Never hold `Mutex`/`RwLock` across `.await`
- Bounded channels for backpressure

### Error handling chain

```
[CLI] → ScraperError :: [domain] CrawlError :: [infra] HttpError/WafError/ParseError
```

Top-level error enum (`ScraperError` in `src/error.rs`) unifies all domain + infra errors.
Add context with `.context()` / `.with_context()` on every error conversion.

### Testing

- Unit tests in `#[cfg(test)] mod tests` within each source module
- Integration tests in `tests/` directory (separate binary)
- Property-based tests with `proptest` for URL validation, URL filtering, etc.
- MCP protocol tests with proptest strategies

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed
- `quick-xml` 0.37 (direct) + 0.38 (via syntect→plist) — both needed
- `scraper` 0.27 → selectors 0.35, `legible` → dom_query → selectors 0.38 — both needed

### AI feature (`--features ai`)

- Loads ~90MB ONNX model (all-MiniLM-L6-v2) — async init, reused across pages
- Model cached in `~/.cache/rust_scraper/models/`
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings

---

## Safety & Permissions (Agent)

### Allowed without asking

- Read any file in the repo
- Run `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`
- Run CodeDB and GitNexus tools
- Edit files within `src/`, `tests/`, `benches/`, `examples/`

### Ask first

- Adding or removing dependencies (`Cargo.toml`)
- Changing feature flags
- Modifying `Cargo.toml` profiles (release, dev, bench)
- Deleting files or directories
- Running `cargo build --release` (slow, ~3-5 min)
- Running `cargo llvm-cov` (coverage, slow)
- Modifying CI/CD files (`.github/`)
- Creating new files outside `src/`, `tests/`, `benches/`, `examples/`

### Never

- Commit secrets, `.env`, or credentials
- Use `.unwrap()` in production code — use `?` or `match`
- Force push to main
- Modify `target/`, `dist/`, `build/` directories
- Run `gitnexus analyze` in dirty worktree (breaks `detect_changes()`)

---

## Good & Bad Examples

### New service/trait — copy `crawler_service.rs`

- Location: `src/application/crawler_service.rs`
- Pattern: trait definition → impl with DI → error type per method
- Uses `async_trait`, `#[instrument]` for tracing, typed errors

### New domain entity — copy `domain/entities.rs`

- Location: `src/domain/entities.rs`
- Pattern: struct + constructor + `TryFrom` validation
- Implements `Display`, `Debug`, `PartialEq` for all public types

### New adapter — copy `infrastructure/crawler/`

- Location: `src/infrastructure/crawler/`
- Pattern: domain trait → impl in `infrastructure`
- Module with `mod.rs`, files split by concern

### New error type — copy `cli/error.rs`

- Location: `src/cli/error.rs`
- Pattern: `thiserror::Error` + `From` impls for upper layers
- Error messages in Spanish for user-facing, English for debug

### Avoid — legacy patterns

- `adapters/tui/` has very complex components (`progress_widget.rs`: 551 lines). Keep new TUI components focused.
- `infrastructure/mcp_server/mod.rs` (1404 lines) → prefer splitting into `handlers/` per domain.

---

## PR & Commit Guidelines

### Commit format

```
type(scope): description

- type: feat | fix | refactor | test | docs | perf | chore | revert
- scope: cli | tui | crawler | ai | mcp | exporter | http | domain | infra
- description: imperative, lowercase, no period
```

Examples: `feat(crawler): add sitemap priority parsing`, `fix(http): handle connection reset gracefully`

### PR checklist

- [ ] `cargo check` + `cargo clippy -- -D warnings` + `cargo fmt`
- [ ] `cargo nextest run` passes (at least the affected module)
- [ ] `gitnexus_detect_changes()` shows only expected symbols
- [ ] Diff is focused (no unrelated changes)
- [ ] Error messages in Spanish if user-facing
- [ ] New public items have doc comments

---

## Delegation Rules

Sub-agents get a fresh context with no memory. The orchestrator controls context access.

### Árbol de Decisión — Cuándo delegar y qué herramientas pasar

```
Tarea recibida?
├── "Investigar / entender código"
│   └── → DELEGAR a exploración
│       Skills: gitnexus, codedb
│       Instrucción: codedb_context primero, luego query/context de gitnexus
│
├── "Escribir código nuevo" (2+ archivos)
│   └── → DELEGAR a writer sub-agent
│       Skills: gitnexus, codedb, rust-skills
│       Instrucción: impact() antes, codedb para entender, rust-skills por categoría
│
├── "Modificar código existente"
│   └── ¿Cambio en 1 archivo mecánico?
│   │   → INLINE (con impact + codedb antes)
│   └── ¿Cambio en 2+ archivos o lógica nueva?
│       → DELEGAR writer sub-agent (gitnexus, codedb, rust-skills)
│
├── "Refactorizar / renombrar"
│   └── → DELEGAR writer (gitnexus_rename, NUNCA find-and-replace)
│
├── "Corregir bug"
│   └── → DELEGAR escritura (gitnexus_query con error, context en sospechosos)
│
├── "Ejecutar tests / CI"
│   └── → DELEGAR sub-agent (sin code tools)
│
└── "Revisar PR / verify"
    └── → DELEGAR verify/review (detect_changes + impact por símbolo)
```

### Mandatory: Sub-agents MUST usar CodeDB + GitNexus + rust-skills

Cada sub-agente que lee, escribe o revisa código DEBE:

1. Usar `codedb_context` como PRIMERA llamada de orientación
2. Usar `codedb_symbol`/`codedb_callers` para entender código antes de escribirlo
3. Ejecutar `gitnexus_impact` ANTES de editar cualquier símbolo
4. Aplicar `rust-skills` con la categoría correcta (ver tabla Before Writing Rust)
5. Ejecutar `gitnexus_detect_changes` antes de devolver el resultado
6. NUNCA usar `grep`/`rg` para búsqueda de código (usar CodeDB)
7. NUNCA renombrar con find-and-replace (usar `gitnexus_rename`)

### Template: delegar escritura de código

```
## Contexto del cambio
<descripción, archivos a tocar>

## MANDATORY: Herramientas de code intelligence
Carga los skills: gitnexus, codedb, rust-skills.

### Antes de escribir:
1. ORIENTACIÓN — codedb_context primero:
   codedb_context project="/home/xavi/Projects/rust_scraper" task="<descripción>"

2. ENTIENDE DEPENDENCIAS — según necesidad:
   - codedb_symbol name="NombreExacto" — definición de símbolo
   - codedb_callers name="miFuncion" — quién llama a esto
   - codedb_outline path="src/algo.rs" — estructura del archivo
   - codedb_search query="patron" — búsqueda de texto
   - codedb_deps path="src/algo.rs" transitive=true — árbol de dependencias
   - codedb_word word="identificador" — ocurrencias exactas (O(1))

3. IMPACTO — gitnexus_impact ANTES de editar:
   gitnexus_impact({target: "simbolo", direction: "upstream"})
   → HIGH/CRITICAL: parar, reportar, no editar sin aprobación

4. RUST-SKILLS — según tipo de código:
   - Nuevo: own-, err-, name-, api-, type-
   - Async: async-, own-
   - Unsafe: unsafe- (SAFETY comments + Miri)
   - Tests: test-
   Lee las reglas específicas de cada categoría antes de escribir.

### Durante escritura:
5. CODEDB para verificar:
   - codedb_callers — confirmar que no te saltas callers
   - codedb_symbol — confirmar nombres exactos
   - NUNCA grep/búsqueda textual

### Antes de devolver:
6. VERIFICACIÓN FINAL:
   - gitnexus_detect_changes() — solo los símbolos esperados cambiaron
   - Inesperados → revisar antes de devolver

Ruta fallback si CodeDB MCP no disponible:
codedb /home/xavi/Projects/rust_scraper <comando>
```

### Template: delegar exploración/investigación

```
## Contexto de la investigación
<qué necesito entender>

Carga los skills: gitnexus, codedb.

1. ORIENTACIÓN:
   codedb_context project="/home/xavi/Projects/rust_scraper" task="<lo que busco>"

2. GITNEXUS — flujos de ejecución:
   gitnexus_query({query: "<concepto>"})
   → gitnexus_read_resource("gitnexus://repo/rust_scraper/process/<nombre>")

3. CODEDB — deep dive según necesidad:
   codedb_outline path="src/main.rs"
   codedb_symbol name="StructClave"
   codedb_callers name="funcionImportante"
   codedb_deps path="src/algo.rs"

4. GITNEXUS — contexto completo:
   gitnexus_context({name: "simboloClave"})
   → 360°: quién lo llama, a quién llama, procesos

### Devuelve:
- Resumen de lo entendido
- Archivos clave con línea y propósito
- Flujos de ejecución afectados
- Símbolos y relaciones principales
- Riesgos o decisiones de diseño
```

### Template: delegar revisión de PR / verify

```
## Cambios a revisar
<commit range o descripción>

Carga los skills: gitnexus, codedb.

1. DETECTAR CAMBIOS:
   gitnexus_detect_changes({scope: "compare", base_ref: "main"})
   → Mapea diff a símbolos y flujos

2. IMPACTO POR SÍMBOLO:
   Por cada símbolo no trivial:
   gitnexus_impact({target: "<simbolo>", direction: "upstream"})
   → d=1 fuera del PR = flag de posible ruptura

3. CODEDB — validación estructural:
   codedb_callers name="simboloCambiado"
   codedb_deps path="src/cambiado.rs"

4. COBERTURA DE TESTS:
   gitnexus_impact({target: "<simbolo>", direction: "upstream", includeTests: true})

### Devuelve:
- Nivel de riesgo
- Símbolos modificados y flujos afectados
- Posibles rupturas (d=1 fuera del PR)
- Cobertura de tests faltante
- Recomendación: approve / request changes
```

---

## Skills Reference — Uso Obligatorio

| Propósito                    | Skill         | Herramienta específica       | Cuándo usarla                        |
| ---------------------------- | ------------- | ---------------------------- | ------------------------------------ |
| Orientación tarea nueva      | `codedb`      | `codedb_context`             | **SIEMPRE primero**                  |
| Definición de símbolo        | `codedb`      | `codedb_symbol`              | Saber dónde se define algo           |
| Quién llama a X              | `codedb`      | `codedb_callers`             | Antes de refactorizar/modificar      |
| Ocurrencias exactas (rápida) | `codedb`      | `codedb_word`                | Nombre exacto conocido               |
| Búsqueda texto/patrón        | `codedb`      | `codedb_search`              | Nombre exacto desconocido            |
| Estructura de archivo        | `codedb`      | `codedb_outline`             | Antes de leer un archivo             |
| Árbol de dependencias        | `codedb`      | `codedb_deps`                | Impacto de cambiar módulo            |
| Archivos recientes           | `codedb`      | `codedb_hot`                 | Ver qué se está tocando              |
| Análisis de impacto          | `gitnexus`    | `gitnexus_impact`            | **ANTES** de editar símbolo          |
| Traza de flujos              | `gitnexus`    | `gitnexus_query` + process   | Entender cómo funciona algo          |
| Contexto 360° de símbolo     | `gitnexus`    | `gitnexus_context`           | Ver todo: callers, callees, procesos |
| Renombrar seguro             | `gitnexus`    | `gitnexus_rename`            | NUNCA find-and-replace               |
| Detectar cambios             | `gitnexus`    | `gitnexus_detect_changes`    | Pre-commit y pre-PR                  |
| Depuración de errores        | `gitnexus`    | `gitnexus_query` + `context` | Tracing de bugs                      |
| Revisión de PR               | `gitnexus`    | `detect_changes` + `impact`  | Review de PRs                        |
| Calidad de Rust              | `rust-skills` | Reglas por categoría         | **SIEMPRE** al escribir Rust         |
| Planificación SDD            | `sdd-*`       | Skills de fase               | Planning/verificación                |

### Categorías rust-skills por tipo de trabajo

| Tipo de código      | Prefijos de reglas                         |
| ------------------- | ------------------------------------------ |
| Funciones nuevas    | `own-`, `err-`, `name-`, `pat-`            |
| Structs/API pública | `api-`, `type-`, `serde-`, `doc-`, `name-` |
| Async               | `async-`, `own-`, `err-`                   |
| Concurrencia        | `conc-`, `async-`                          |
| Unsafe              | `unsafe-`, `test-` (Miri)                  |
| Errores             | `err-`, `api-`                             |
| Tests               | `test-`, `unsafe-`                         |
| Performance         | `opt-`, `mem-`, `perf-`                    |
| Serde               | `serde-`, `type-`                          |
| Memoria             | `mem-`, `own-`                             |
| Numeric             | `num-`, `type-`                            |

Skills are auto-discovered by OpenCode from `~/.config/opencode/skills/`. Reference by name only.

<!-- gitnexus:start -->

# GitNexus — Code Intelligence

This project is indexed by GitNexus as **rust_scraper** (4465 symbols, 9237 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> Index stale? Run `gitnexus analyze --index-only --skip-agents-md` from the project root. Use `gitnexus analyze --skills --index-only --skip-agents-md` only when regenerating skill files.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows. For regression review, compare against the default branch: `detect_changes({scope: "compare", base_ref: "main"})`.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `rename` which understands the call graph.
- NEVER commit changes without running `detect_changes()` to check affected scope.

## Resources

| Resource                                      | Use for                                  |
| --------------------------------------------- | ---------------------------------------- |
| `gitnexus://repo/rust_scraper/context`        | Codebase overview, check index freshness |
| `gitnexus://repo/rust_scraper/clusters`       | All functional areas                     |
| `gitnexus://repo/rust_scraper/processes`      | All execution flows                      |
| `gitnexus://repo/rust_scraper/process/{name}` | Step-by-step execution trace             |

## Skills

| Task                                         | Skill      |
| -------------------------------------------- | ---------- |
| Understand architecture / "How does X work?" | `gitnexus` |
| Blast radius / "What breaks if I change X?"  | `gitnexus` |
| Trace bugs / "Why is X failing?"             | `gitnexus` |
| Rename / extract / split / refactor          | `gitnexus` |
| Review pull requests                         | `gitnexus` |
| Tools, resources, schema reference           | `gitnexus` |
| Index, status, clean, wiki CLI commands      | `gitnexus` |

<!-- gitnexus:end -->

<!-- codedb:start -->

# CodeDB — Structural Code Search

CodeDB is a fast structural search engine. Prefer CodeDB MCP tools for indexed structural search. Use the CLI with the explicit project path only as a fallback. GitNexus handles deep graph analysis and execution flows.

> **MCP status:** CodeDB MCP is available again. Use MCP first. If it fails or cannot load the project, fall back to the CLI with explicit path: `codedb /home/xavi/Projects/rust_scraper <command>`.
>
> Index stale? Run `codedb /home/xavi/Projects/rust_scraper index` from the project root.

## When to Use CodeDB

- **Quick file tree** — `codedb_tree` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper tree`
- **Find symbol definitions** — `codedb_symbol` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper symbol <name>`
- **Full-text search** — `codedb_search` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper search <query>`
- **Find all callers** — `codedb_callers` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper callers <name>`
- **File outline** — `codedb_outline` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper outline <path>`
- **Dependency graph** — `codedb_deps` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper deps <path>`
- **Index status** — `codedb_status` MCP, or CLI fallback: `codedb /home/xavi/Projects/rust_scraper status`

## CodeDB vs GitNexus

| Use CodeDB for                      | Use GitNexus for               |
| ----------------------------------- | ------------------------------ |
| Fast structural search (sub-ms)     | Deep execution flow analysis   |
| File trees, outlines, symbol lookup | Impact analysis (blast radius) |
| Full-text search (trigram)          | Process tracing, call chains   |
| Dependency graph (import analysis)  | Community detection, clusters  |

**Use both:** CodeDB for quick lookups, GitNexus for deep analysis.

## CLI Command Reference

| Command                        | Example                                            |
| ------------------------------ | -------------------------------------------------- |
| `codedb <root> tree`           | Project orientation — file tree with symbol counts |
| `codedb <root> symbol <name>`  | Find where a symbol is defined                     |
| `codedb <root> search <query>` | Full-text search (supports regex with `--regex`)   |
| `codedb <root> callers <name>` | Every call site of a symbol                        |
| `codedb <root> outline <path>` | Functions/structs/imports in a file                |
| `codedb <root> deps <path>`    | Dependency graph (`--depends-on`, `--transitive`)  |
| `codedb <root> status`         | Index freshness and size                           |
| `codedb <root> hot`            | Recently modified files                            |
| `codedb <root> find <name>`    | Fuzzy file-name search                             |
| `codedb <root> context <task>` | Task-shaped context bundle                         |

`<root>` = `/home/xavi/Projects/rust_scraper` for this project.

## Never Do

- NEVER use `codedb_edit` when native edit tools work — it's a fallback only
- NEVER use CodeDB for impact analysis — use GitNexus `impact` instead
- NEVER use CodeDB for execution flow tracing — use GitNexus `query`/`context` instead
- NEVER invoke `codedb mcp` manually during normal agent work — use the configured MCP tools. Use CLI only as fallback with explicit `/home/xavi/Projects/rust_scraper` path.

<!-- codedb:end -->
