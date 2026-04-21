# Agent Instructions — Rust Scraper

Production-ready web scraper with Clean Architecture, TUI selector, and AI semantic cleaning.

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · ratatui · tract-onnx (feature-gated)  
**Hardware:** Intel i5-4590 (4C), 8GB DDR3, HDD — all commands are HDD-optimized

---

## Key Commands

### Just Recipes (preferred — orchestrate tasks)

```bash
just check          # fmt + clippy strict
just check-fast     # cargo check (fastest)
just test           # nextest --test-threads 2
just test-ai        # nextest with AI features
just audit          # audit + deny + machete
just cov            # coverage HTML report
just fmt            # format code
just build-release  # optimized build
```

### Raw Commands (when Just isn't available)

```bash
# Verify compilation (FAST — use this)
cargo check

# Verify with AI feature
cargo check --features ai

# Lint (quick pass)
cargo clippy -- -D warnings

# Full lint with all features
cargo clippy --all-targets --all-features -- -D warnings

# Run tests (ALWAYS use nextest, never cargo test)
cargo nextest run --test-threads 2
cargo nextest run --test-threads 2 --features ai

# Coverage
cargo llvm-cov --html --output-dir coverage-llvm

# Format check
cargo fmt --check

# Background checker (auto-runs clippy)
bacon
```

**⚠️ HDD timeout rules:** First `cargo check` takes ~4 min (cold compile, 300 crates). After that, `sccache` makes everything fast. **ALWAYS set explicit timeouts** for heavy commands. Prefer `cargo check` over `cargo build` during development. Never run `cargo build --release` unless explicitly asked.

### 🚀 Estrategia GitNexus + Just (2026 - Anti-Timeout)

**Para agentes de código: Usa esta estrategia OBLIGATORIA para evitar timeouts:**

#### ✅ Secuencia recomendada (iteraciones de 10-30 segundos)
```bash
# 1. ANTES de cualquier cambio → refresca GitNexus
just analyze

# 2. DESPUÉS de cada edición → tests solo de lo afectado
just test-dev

# 3. Si GitNexus te da un filtro específico → usa filtro preciso
just test-filter 'package(rust_scraper) + test(contains("scraper"))'

# 4. Solo al FINAL (cuando yo diga "tarea terminada") → suite completa
just test
```

#### 🎯 Comandos que el agente debe usar (PIPELINE COMPLETO 2026)
| Comando | Uso | Tiempo | Cuándo usarlo |
|---------|-----|--------|--------------|
| `just analyze` | Refrescar GitNexus | < 8s | Al inicio de sesión |
| `just watch-dev` | **Modo automático principal** | automático | **Durante TODO el desarrollo** |
| `just test-dev-with-impact` | Tests inteligentes manuales | 8-25s | Si watch no está corriendo |
| `just test-filter 'filtro'` | Filtro personalizado | 10-30s | Cuando GitNexus da filtro específico |
| `just test` | Tests completos | 2-6 min | Alternativa ligera al final |
| `just test-ci` | **GATE FINAL** (fmt + clippy + tests) | 2-7 min | **Solo cuando yo diga "tarea terminada"** |

### 🚨 Prompts para Agentes de Código (2026 Best Practices)

**Principio clave**: Los agentes empiezan cada sesión sin estado. Un prompt claro mejora dramáticamente la calidad del output (+4 puntos porcentuales según estudio ETH Zurich 2025).

#### 🎯 Prompt Definitivo para Agentes (Copia y Pega)

> **Estrategia anti-timeout OBLIGATORIA 2026 (GitNexus + Just):**
> 1. Al inicio de la sesión: `just analyze`
> 2. Inmediatamente después: `just watch-dev` (modo automático)
> 3. El agente **nunca** ejecuta tests manualmente. Solo edita código y deja que watch-dev haga el resto.
> 4. Solo cuando yo diga explícitamente **"tarea terminada"** o **"finalizar"**, entonces ejecuta `just test-ci`
> 5. Si quieres verificar algo específico usa `just test-filter 'filtro'`

#### 📋 Prompts Especializados por Tipo de Tarea

**Para debugging:**
> Usa GitNexus para impact analysis antes de cualquier cambio. Ejecuta `gitnexus_impact({target: "function_name"})` y reporta riesgos HIGH/CRITICAL.

**Para refactoring:**
> Ejecuta `gitnexus_detect_changes()` antes de commits. Nunca renombres sin `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})`.

**Para nuevas features:**
> Sigue Clean Architecture: lógica de negocio en `domain/`, casos de uso en `application/`, adaptadores externos en `infrastructure/`.

#### ⚙️ Configuración de Prompts por Herramienta

```bash
# Claude Code - symlink para compatibilidad
ln -sf AGENTS.md CLAUDE.md

# Cursor - reglas específicas
echo "AGENTS.md contiene instrucciones completas" > .cursorrules

# GitHub Copilot - workspace rules
mkdir -p .github
cp AGENTS.md .github/copilot-instructions.md
```

#### 📊 Efectividad de Prompts (Datos 2026)

- **Archivos humanos**: +4 puntos de mejora vs sin contexto
- **Archivos auto-generados**: -0.5% a -2% performance
- **Tamaño óptimo**: ≤150 líneas (60K+ repos adoptaron estándar AAIF)
- **Jerarquía**: AGENTS.md anidados por directorio tienen precedencia

#### 🎨 Estructura de Prompt Efectiva

1. **Contexto primero**: Stack, herramientas, convenciones
2. **Comandos críticos**: just analyze, just watch-dev, just test-ci
3. **Reglas claras**: Qué hacer automáticamente vs pedir permiso
4. **Ejemplos concretos**: Referencias a archivos reales del repo
5. **Límites definidos**: Zonas prohibidas, patrones no usar

#### ⚠️ NUNCA uses estos comandos (causan timeouts)
```bash
cargo nextest run                    # ❌ Suite completa innecesaria
cargo nextest run 2>&1 \| tail -5    # ❌ Pipes bloquean output
just test-ci                         # ❌ Solo para CI
just test-dev                        # ❌ Usa watch-dev en su lugar
```

---

## Code Style

Error messages are in **Spanish** (not English). HTTP client is **`wreq`**, not `reqwest`.

```rust
// src/error.rs — Error messages in Spanish
#[derive(Error, Debug)]
pub enum ScraperError {
    #[error("URL inválida: {0}")]
    InvalidUrl(String),
    #[error("error de red: {0}")]
    Network(String),
    #[error("WAF/CAPTCHA detectado en {url}: {provider}")]
    WafBlocked { url: String, provider: String },
}

// src/application/http_client.rs — wreq, NOT reqwest
use wreq::Client;
use wreq_util::emulation::ClientBuilderExt;

let client = Client::builder()
    .emulate(wreq_util::emulation::KnownVersion::Chrome131)
    .build()?;
```

---

## Project Architecture

```
src/
├── adapters/        # External adapters (HTTP, filesystem)
├── application/    # Use cases, services (CrawlerService, ScraperService)
├── cli/            # CLI argument parsing and commands
├── domain/         # Entities, value objects, domain logic
├── extractor/      # Content extraction (HTML, text)
├── infrastructure/  # AI, Obsidian, detectors, converters
│   ├── ai/         # Semantic cleaning (ONNX embeddings)
│   ├── obsidian/   # Vault detection and parsing
│   └── ...
└── lib.rs          # Main library (ScraperConfig, exports)
```

**Key modules:**
- `src/application/crawler_service.rs` — Crawling logic with rate limiting
- `src/application/scraper_service.rs` — Page scraping with SPA detection
- `src/infrastructure/ai/semantic_cleaner_impl.rs` — AI content cleaning
- `src/infrastructure/obsidian/` — Obsidian vault integration
- `src/cli/` — CLI commands and TUI

---

## Non-Obvious Patterns

### Crate version conflicts (DO NOT try to unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed
- `quick-xml` 0.37 (direct) + 0.38 (via syntect→plist) — both needed
- `scraper` 0.22 → selectors 0.26, `legible` → dom_query → selectors 0.35 — both needed

### HTTP client: `wreq` not `reqwest`

Uses TLS fingerprint emulation (Chrome 131) for WAF evasion. Layer 2 evasion built in.

### WAF detection on HTTP 200

Responses are scanned for 19 WAF signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai). If detected, UA is rotated and retried once. Still blocked → `ScraperError::WafBlocked`.

### AI feature (`--features ai`)

- Loads ~90MB ONNX model (all-MiniLM-L6-v2) into memory
- `SemanticCleanerImpl::new()` is **async** — loads model once, reuses
- `cleaner.clean(html)` is **async** — returns `Vec<DocumentChunk>` with embeddings
- One page → multiple chunks when AI cleaning is active
- Model cached in `~/.cache/rust-scraper/models/`

---

## Testing Rules

- Write tests for all new functionality
- Tests must be deterministic and isolated
- Mock all external dependencies
- Run `cargo nextest run` (never `cargo test`) before marking any task complete
- Use `--test-threads 2` to avoid HDD I/O bottleneck

### HDD Configuration (CRÍTICO)
**Para Intel i5-4590, 8GB RAM, HDD:**

```toml
# .config/nextest.toml
[profile.default]
threads-required = 2 # MÁXIMO 2 hilos - previene thrashing
retries = { backoff = "exponential", count = 2, delay = "1s" }
slow-timeout = { period = "60s", terminate-after = 3 }
```

- **nunca** usar más de 2 threads en desarrollo
- Perfiles nextest: `dev` (rápido), `agent` (conservador), `ci` (completo)

---

## Boundaries

### ✅ Always

- Run `cargo check` before marking any task complete
- Run `cargo clippy -- -D warnings` before committing
- Use `cargo nextest run` (never `cargo test`)
- Use `cargo llvm-cov` (never `cargo tarpaulin`)
- Use `bacon` for background checking (never `cargo-watch`)
- Use `just` recipes for multi-step tasks (audit, coverage, release)

### ⚠️ Ask first

- Adding or removing dependencies
- Changing feature flag structure
- Modifying `Cargo.toml` profiles

### 🚫 Never

- Commit secrets, `.env` files, or credentials
- Use `.unwrap()` in production code — use `?` or `match`
- Force push to main or protected branches
- Modify `target/`, `dist/`, or `build/` directories
- Run `cargo build --release` during development (use `cargo check`)

---

## Resources

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — Architecture details
- [DEVELOPMENT.md](DEVELOPMENT.md) — Dev workflow and tooling
- [justfile](justfile) — Task recipes (check, test, audit, cov)

## Herramientas de Búsqueda

| Herramienta | Uso |
|------------|-----|
| gitnexus_query | Encontrar código por concepto (execution flows) |
| gitnexus_context | Vista 360° de símbolos |
| fff_find_files | Encontrar archivos por nombre |
| fff_grep | Contenido específico |

---

## GitNexus — Code Intelligence

This project is indexed by **rust_scraper**: 3963 symbols, 6951 relationships, 300 execution flows, 118 communities.

| Community | Symbols | Cohesion |
|-----------|---------|----------|
| Application | 30 | 0.85 |
| Domain | 26 | 0.54 |
| Ai | 24-19 | 0.78-1.0 |
| Export | 22-13 | 0.95-0.98 |
| Downloader | 16 | 0.94 |
| Crawler | 12 | 0.63 |
| Tui | 10 | 0.63 |
| Scraper | 10 | 0.73 |

> If any GitNexus tool warns the index is stale, run `gitnexus analyze` in terminal first.

### Always Do

- **MUST run impact analysis** before editing any symbol: `gitnexus_impact({target: "symbolName", direction: "upstream"})`
- **MUST run `gitnexus_detect_changes()`** before committing
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk
- Use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping

### When Debugging

1. `gitnexus_query({query: "<error or symptom>"})` — find execution flows
2. `gitnexus_context({name: "<suspect function>"})` — see callers, callees
3. `READ gitnexus://repo/rust_scraper/process/{processName}` — trace full flow

### When Refactoring

- **Renaming**: MUST use `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` first
- **Extracting/Splitting**: MUST run `gitnexus_impact` before moving code

### Tools Quick Reference

| Tool | Command |
|------|---------|
| query | `gitnexus_query({query: "concept"})` |
| context | `gitnexus_context({name: "symbolName"})` |
| impact | `gitnexus_impact({target: "X", direction: "upstream"})` |
| detect_changes | `gitnexus_detect_changes({scope: "staged"})` |
| rename | `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` |

### Self-Check Before Finishing

1. `gitnexus_impact` was run for all modified symbols
2. No HIGH/CRITICAL risk warnings were ignored
3. `gitnexus_detect_changes()` confirms expected scope
4. All d=1 (WILL BREAK) dependents were updated

---

## SDD Workflow

Este proyecto soporta Spec-Driven Development via skills en `~/.config/kilo/skills/`:

| Skill | Propósito |
|-------|-----------|
| sdd-init | Inicializar contexto, detectar stack |
| sdd-explore | Investigar código existente |
| sdd-propose | Crear propuesta |
| sdd-spec | Escribir especificaciones |
| sdd-design | Diseño técnico |
| sdd-tasks | Lista de tareas |
| sdd-apply | Implementar (con gitnexus_impact) |
| sdd-verify | Verificar contra specs |
| sdd-archive | Archivar cambio |

### Pipeline SDD + GitNexus
1. `just analyze` → iniciar
2. `gitnexus_impact` → antes de editar
3. `gitnexus_detect_changes` → pre-commit
4. `just test-ci` → verificación final

## Rust Best Practices

Este proyecto incluye **50+ reglas de rust-skills** en `.atl/skills/rust-skills/rules/`:

| Categoría | Ejemplos |
|-----------|----------|
| Memory | mem-zero-copy, mem-smallvec, mem-compact-string |
| Performance | perf-release-profile, perf-profile-first, perf-collect-once |
| API Design | api-typestate, api-non-exhaustive, api-serde-optional |
| Async | async-tokio-runtime, async-no-lock-await |
| Testing | test-integration-dir, test-tokio-async, test-proptest-properties |
| Error Handling | err-question-mark, err-lowercase-msg |

### Auto-load de rust-skills
Cuando el agente escribe código **Rust**, cargar automáticamente:
```
skill(name: "rust-skills")
```
(Path: `.atl/skills/rust-skills/`)

---

## Skills

| Skill | Location | Trigger |
|-------|----------|---------|
| **rust-skills** | `~/.config/opencode/skills/rust-skills/SKILL.md` | Any Rust code |
| **gitnexus-exploring** | `.opencode/skills/gitnexus/gitnexus-exploring/SKILL.md` | "How does X work?" |
| **gitnexus-impact-analysis** | `.opencode/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` | "What breaks if I change X?" |
| **gitnexus-debugging** | `.opencode/skills/gitnexus/gitnexus-debugging/SKILL.md` | "Why is X failing?" |
| **gitnexus-refactoring** | `.opencode/skills/gitnexus/gitnexus-refactoring/SKILL.md` | Rename, extract, split |
| **gitnexus-cli** | `.opencode/skills/gitnexus/gitnexus-cli/SKILL.md` | Index, status, clean |
| **gitnexus-guide** | `.opencode/skills/gitnexus/gitnexus-guide/SKILL.md` | Tools, resources |

### Area-Specific Skills

| Area | Skill File |
|------|------------|
| Ai (212 symbols) | `.opencode/skills/generated/ai/SKILL.md` |
| Application (78 symbols) | `.opencode/skills/generated/application/SKILL.md` |
| Domain (74 symbols) | `.opencode/skills/generated/domain/SKILL.md` |
| Export (59 symbols) | `.opencode/skills/generated/export/SKILL.md` |
| Crawler (51 symbols) | `.opencode/skills/generated/crawler/SKILL.md` |
| Tui (27 symbols) | `.opencode/skills/generated/tui/SKILL.md` |
| Obsidian (24 symbols) | `.opencode/skills/generated/obsidian/SKILL.md` |
| Scraper (19 symbols) | `.opencode/skills/generated/scraper/SKILL.md` |

> Index: `gitnexus analyze` · Status: `gitnexus status`

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **rust_scraper** (3893 symbols, 8093 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## When Debugging

1. `gitnexus_query({query: "<error or symptom>"})` — find execution flows related to the issue
2. `gitnexus_context({name: "<suspect function>"})` — see all callers, callees, and process participation
3. `READ gitnexus://repo/rust_scraper/process/{processName}` — trace the full execution flow step by step
4. For regressions: `gitnexus_detect_changes({scope: "compare", base_ref: "main"})` — see what your branch changed

## When Refactoring

- **Renaming**: MUST use `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` first. Review the preview — graph edits are safe, text_search edits need manual review. Then run with `dry_run: false`.
- **Extracting/Splitting**: MUST run `gitnexus_context({name: "target"})` to see all incoming/outgoing refs, then `gitnexus_impact({target: "target", direction: "upstream"})` to find all external callers before moving code.
- After any refactor: run `gitnexus_detect_changes({scope: "all"})` to verify only expected files changed.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Tools Quick Reference

| Tool | When to use | Command |
|------|-------------|---------|
| `query` | Find code by concept | `gitnexus_query({query: "auth validation"})` |
| `context` | 360-degree view of one symbol | `gitnexus_context({name: "validateUser"})` |
| `impact` | Blast radius before editing | `gitnexus_impact({target: "X", direction: "upstream"})` |
| `detect_changes` | Pre-commit scope check | `gitnexus_detect_changes({scope: "staged"})` |
| `rename` | Safe multi-file rename | `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` |
| `cypher` | Custom graph queries | `gitnexus_cypher({query: "MATCH ..."})` |

## Impact Risk Levels

| Depth | Meaning | Action |
|-------|---------|--------|
| d=1 | WILL BREAK — direct callers/importers | MUST update these |
| d=2 | LIKELY AFFECTED — indirect deps | Should test |
| d=3 | MAY NEED TESTING — transitive | Test if critical path |

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/rust_scraper/context` | Codebase overview, check index freshness |
| `gitnexus://repo/rust_scraper/clusters` | All functional areas |
| `gitnexus://repo/rust_scraper/processes` | All execution flows |
| `gitnexus://repo/rust_scraper/process/{name}` | Step-by-step execution trace |

## Self-Check Before Finishing

Before completing any code modification task, verify:
1. `gitnexus_impact` was run for all modified symbols
2. No HIGH/CRITICAL risk warnings were ignored
3. `gitnexus_detect_changes()` confirms changes match expected scope
4. All d=1 (WILL BREAK) dependents were updated

## Keeping the Index Fresh

After committing code changes, the GitNexus index becomes stale. Re-run analyze to update it:

```bash
npx gitnexus analyze
```

If the index previously included embeddings, preserve them by adding `--embeddings`:

```bash
npx gitnexus analyze --embeddings
```

To check whether embeddings exist, inspect `.gitnexus/meta.json` — the `stats.embeddings` field shows the count (0 means no embeddings). **Running analyze without `--embeddings` will delete any previously generated embeddings.**

> Claude Code users: A PostToolUse hook handles this automatically after `git commit` and `git merge`.

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
