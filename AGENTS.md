# AGENTS.md — Rust Scraper

Production-ready web scraper. Clean Architecture, TUI selector, AI semantic cleaning.

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · ratatui · tract-onnx (feature-gated)
**Hardware:** Ryzen 7 5700X (8C/16T), 32GB DDR4, NVMe — local dev for most tasks

### Build dependencies (required)

`cmake` is mandatory — `wreq` → `boring2` → `boring-sys2` needs it to compile BoringSSL.
Without it, nothing compiles. Install before first build:

```bash
# Fedora
sudo dnf install cmake
```

---

## Workflow Phases

### 1. Session Start

```
gitnexus analyze --index-only --skip-agents-md    # Refresh index on a clean tree without touching AGENTS.md
gitnexus analyze --skills --index-only --skip-agents-md  # Regenerate skill files if communities changed
codedb /home/xavi/Projects/rust_scraper status   # Verify CodeDB index is fresh
```

If you see "Index is stale" from any gitnexus tool → stop and run `gitnexus analyze` first.
If `codedb /home/xavi/Projects/rust_scraper status` shows stale index → run `codedb /home/xavi/Projects/rust_scraper index` to rebuild.

Before reindexing, make sure the worktree is clean. If you still need `gitnexus_detect_changes()` later in the session, do not rerun `gitnexus analyze` after editing files.

If `gitnexus analyze` crashes with `Napi::Error` or hangs → clean first:
```bash
gitnexus clean -f && gitnexus analyze --index-only --skip-agents-md
```

**Use** `--skip-agents-md` whenever you want to refresh the index without modifying `AGENTS.md`. Use `--index-only` for pure index mode when you do not want file injection at all.
**Do not** rerun `gitnexus analyze` in a dirty worktree if you still need `gitnexus_detect_changes()` to report your current edits.

### 2. Before Editing Code

```
gitnexus_impact({target: "symbolName", direction: "upstream"})
```

- **LOW/MEDIUM risk** → proceed with changes
- **HIGH risk** → stop, warn user, get approval
- **CRITICAL risk** → stop, require user sign-off

Consult `gitnexus-impact-analysis` skill for full impact analysis protocol (depth groups, confidence scores).

### 3. Before Writing Rust

Load `rust-skills` skill. This is **mandatory** for any Rust code — ownership rules, error handling, async patterns, testing conventions.

### 4. Pre-Commit Protocol (every commit)

```bash
cargo check                    # Must pass
cargo clippy -- -D warnings    # Must pass — fix ALL warnings
cargo fmt                      # Must run
gitnexus_detect_changes()      # Verify only expected symbols changed
```

If `gitnexus_detect_changes()` shows unexpected affected symbols → review before committing.

### 5. Cloud Verification (after commit)

```bash
gh workflow run ci.yml --ref $(git branch --show-current)
gh run watch
```

Only push after CI shows ✅. If CI fails → fix locally, re-commit, re-trigger CI.

---

## Commands

### Local (safe, <5s total)

```bash
cargo check                    # Verify compilation
cargo check --features ai      # With AI feature
cargo clippy -- -D warnings    # Lint
cargo fmt --check              # Format check
cargo fmt                      # Format
```

### Local (moderate, <5 min)

```bash
cargo nextest run              # Full suite, ~1-2 min
cargo nextest run --all-features  # With AI, ~2-3 min
just test-ci                   # Full gate (fmt+clippy+tests), ~3-5 min
cargo build --release          # ~3-5 min (LTO fat)
```

> **Note:** `cargo build --release` uses LTO fat + codegen-units=1 (see `Cargo.toml:257`).
> First clean build compiles BoringSSL from C++ source — expect longer times.
> Incremental builds with sccache are significantly faster.

### Prefer CI (slow, >5 min)

```bash
cargo llvm-cov                 # Coverage instrumentation (~5-8 min)
cargo miri test                # Memory safety interpretation (~10-15 min)
```

---

## Delegation Rules

Sub-agents get a fresh context with no memory. The orchestrator controls context access.

### MANDATORY: Sub-agents MUST use GitNexus + CodeDB

**Every sub-agent that reads, writes, or reviews code MUST use GitNexus and CodeDB tools for code investigation.** The orchestrator enforces this by:

1. Always passing the relevant skill names in the sub-agent prompt
2. Including in the sub-agent prompt: `"You MUST use CodeDB MCP tools for structural code search, gitnexus_impact before editing, and gitnexus_detect_changes before returning. Do NOT grep the project codebase. If CodeDB MCP is unavailable, use the CodeDB CLI fallback with explicit path: codedb /home/xavi/Projects/rust_scraper <command>."`
3. Sub-agents that grep instead of using GitNexus/CodeDB are discipline failures

**Delegate when:**
- Reading 4+ files to understand codebase → exploration sub-agent (with gitnexus-exploring + codedb)
- Writing code across 2+ files → writer sub-agent (with gitnexus-exploring + codedb + rust-skills)
- Running tests or CI → sub-agent
- Multi-step refactoring → sub-agent (with gitnexus-refactoring + codedb + rust-skills)

**Do inline when:**
- Reading 1-3 files for decision/verification
- Single-file mechanical edits
- Git/gh state checks (status, log, diff)

**When delegating, reference skills by name (OpenCode auto-discovers them):**
```
## Skills to load before work
- gitnexus-exploring
- codedb
- rust-skills
```

Every sub-agent prompt that involves code MUST include:
```
MANDATORY: Load gitnexus-exploring and codedb skills. Use CodeDB MCP tools
for structural code search (symbol/search/callers/outline/deps) and GitNexus
tools for execution-flow analysis (gitnexus_query, gitnexus_impact,
gitnexus_detect_changes). NEVER grep the project codebase. If CodeDB MCP is
unavailable, fall back to the CodeDB CLI with explicit path:
codedb /home/xavi/Projects/rust_scraper <command>.
```

---

## Code Style

- Error messages in **Spanish** (not English)
- HTTP client is **`wreq`** (not `reqwest`) — TLS fingerprint emulation for WAF evasion

---

## Non-Obvious Patterns

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed
- `quick-xml` 0.37 (direct) + 0.38 (via syntect→plist) — both needed
- `scraper` 0.27 → selectors 0.35, `legible` → dom_query → selectors 0.38 — both needed

### WAF detection on HTTP 200

Responses scanned for WAF signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai). If detected → UA rotation + retry. Still blocked → `ScraperError::WafBlocked`.

### AI feature (`--features ai`)

- Loads ~90MB ONNX model (all-MiniLM-L6-v2) — async init, reused across pages
- Model cached in `~/.cache/rust_scraper/models/`
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings

---

## Boundaries

### Ask first

- Adding or removing dependencies
- Changing feature flags
- Modifying `Cargo.toml` profiles

### Never

- Commit secrets, `.env`, or credentials
- Use `.unwrap()` in production code — use `?` or `match`
- Force push to main
- Modify `target/`, `dist/`, `build/` directories

---

## Skills Reference

| Purpose | Skill | Load when |
|---------|-------|-----------|
| Code exploration | `gitnexus-exploring` | Understanding architecture, tracing flows |
| Impact analysis | `gitnexus-impact-analysis` | Before editing any symbol |
| Debugging | `gitnexus-debugging` | Tracing bugs, error investigation |
| Refactoring | `gitnexus-refactoring` | Rename, extract, split, move |
| PR review | `gitnexus-pr-review` | Reviewing pull requests |
| GitNexus reference | `gitnexus-guide` | Tool/resource/schema questions |
| GitNexus CLI | `gitnexus-cli` | Index, status, clean, wiki commands |
| **Code search** | **`codedb`** | **Structural search, symbols, callers, outlines, file tree** |
| Rust quality | `rust-skills` | Writing Rust — ownership, error handling, async |
| SDD workflow | `sdd-*` skills | Planning/verifying changes |

Skills are auto-discovered by OpenCode from `~/.config/opencode/skills/`. Reference by name only — no paths needed.

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

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/rust_scraper/context` | Codebase overview, check index freshness |
| `gitnexus://repo/rust_scraper/clusters` | All functional areas |
| `gitnexus://repo/rust_scraper/processes` | All execution flows |
| `gitnexus://repo/rust_scraper/process/{name}` | Step-by-step execution trace |

## Skills

| Task | Skill |
|------|-------|
| Understand architecture / "How does X work?" | `gitnexus-exploring` |
| Blast radius / "What breaks if I change X?" | `gitnexus-impact-analysis` |
| Trace bugs / "Why is X failing?" | `gitnexus-debugging` |
| Rename / extract / split / refactor | `gitnexus-refactoring` |
| Review pull requests | `gitnexus-pr-review` |
| Tools, resources, schema reference | `gitnexus-guide` |
| Index, status, clean, wiki CLI commands | `gitnexus-cli` |

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

| Use CodeDB for | Use GitNexus for |
|----------------|------------------|
| Fast structural search (sub-ms) | Deep execution flow analysis |
| File trees, outlines, symbol lookup | Impact analysis (blast radius) |
| Full-text search (trigram) | Process tracing, call chains |
| Dependency graph (import analysis) | Community detection, clusters |

**Use both:** CodeDB for quick lookups, GitNexus for deep analysis.

## CLI Command Reference

| Command | Example |
|---------|---------|
| `codedb <root> tree` | Project orientation — file tree with symbol counts |
| `codedb <root> symbol <name>` | Find where a symbol is defined |
| `codedb <root> search <query>` | Full-text search (supports regex with `--regex`) |
| `codedb <root> callers <name>` | Every call site of a symbol |
| `codedb <root> outline <path>` | Functions/structs/imports in a file |
| `codedb <root> deps <path>` | Dependency graph (`--depends-on`, `--transitive`) |
| `codedb <root> status` | Index freshness and size |
| `codedb <root> hot` | Recently modified files |
| `codedb <root> find <name>` | Fuzzy file-name search |
| `codedb <root> context <task>` | Task-shaped context bundle |

`<root>` = `/home/xavi/Projects/rust_scraper` for this project.

## Never Do

- NEVER use `codedb_edit` when native edit tools work — it's a fallback only
- NEVER use CodeDB for impact analysis — use GitNexus `impact` instead
- NEVER use CodeDB for execution flow tracing — use GitNexus `query`/`context` instead
- NEVER invoke `codedb mcp` manually during normal agent work — use the configured MCP tools. Use CLI only as fallback with explicit `/home/xavi/Projects/rust_scraper` path.

<!-- codedb:end -->
