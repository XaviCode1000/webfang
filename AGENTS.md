# AGENTS.md — Rust Scraper

Production-ready web scraper. Clean Architecture, TUI selector, AI semantic cleaning.

**Stack:** Rust 1.88 · Tokio · wreq (TLS fingerprint) · ratatui · tract-onnx (feature-gated)
**Hardware:** Intel i5-4590 (4C), 8GB DDR3, HDD — cloud CI for heavy work

---

## Workflow Phases

### 1. Session Start

```
gitnexus analyze                    # Refresh index (re-run after branch switch)
gitnexus analyze --skills           # Regenerate skill files if communities changed
```

If you see "Index is stale" from any gitnexus tool → stop and run `gitnexus analyze` first.

If `gitnexus analyze` crashes with `Napi::Error` or hangs → clean first:
```bash
gitnexus clean -f && gitnexus analyze
```

**Never** run `gitnexus analyze --skip-agents-md` or add `--no-stats` — we want AGENTS.md to stay in sync.

### 2. Before Editing Code

```
gitnexus_impact({target: "symbolName", direction: "upstream"})
```

- **LOW/MEDIUM risk** → proceed with changes
- **HIGH risk** → stop, warn user, get approval
- **CRITICAL risk** → stop, require user sign-off

Consult `gitnexus-master` skill for full impact analysis protocol (depth groups, confidence scores).

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

### 5. Before Finishing (self-check)

1. `cargo check` passes
2. No clippy warnings
3. `gitnexus_impact` was run for modified symbols
4. No HIGH/CRITICAL risk ignored
5. `gitnexus_detect_changes()` confirms expected scope

### 6. Cloud Verification (after commit)

```bash
gh workflow run ci.yml --ref $(git branch --show-current)
gh run watch
```

Only push after CI shows ✅. If CI fails → fix locally, re-commit, re-trigger CI.

---

## Commands

### Local (safe, <30s total)

```bash
cargo check                    # Verify compilation
cargo check --features ai      # With AI feature
cargo clippy -- -D warnings    # Lint
cargo fmt --check              # Format check
cargo fmt                      # Format
```

### Forbidden on this machine (HDD + 8GB RAM — WILL freeze system)

| Command | Why | Alternative |
|---------|-----|-------------|
| `cargo nextest run` | 680 tests, 5-10 min, 100% CPU | `gh workflow run ci.yml` |
| `cargo nextest run --all-features` | AI model (90MB) loads | CI |
| `cargo build --release` | 10+ min optimization | CI |
| `cargo build` | Slower than `cargo check` | `cargo check` |
| `just test-ci` | Full gate, 10+ min | `gh workflow run ci.yml` |
| `cargo llvm-cov` | Instrument + test, 15+ min | CI |
| `cargo miri test` | Interprets instructions, 30+ min | CI |

**Exception:** single test for debugging is allowed:
```bash
cargo nextest run --test-threads 2 -E 'test(specific_test_name)'
```

---

## Delegation Rules

Sub-agents get a fresh context with no memory. The orchestrator controls context access.

### MANDATORY: Sub-agents MUST use GitNexus

**Every sub-agent that reads, writes, or reviews code MUST use GitNexus tools for code investigation.** The orchestrator enforces this by:

1. Always passing `gitnexus-master/SKILL.md` in the skill paths
2. Including in the sub-agent prompt: `"You MUST use gitnexus_query to find code, gitnexus_impact before editing, and gitnexus_detect_changes before returning. Do NOT grep the project codebase."`
3. Sub-agents that grep instead of using GitNexus are discipline failures

**Delegate when:**
- Reading 4+ files to understand codebase → exploration sub-agent (with gitnexus-master)
- Writing code across 2+ files → writer sub-agent (with gitnexus-master + rust-skills)
- Running tests or CI → sub-agent
- Multi-step refactoring → sub-agent (with gitnexus-master + rust-skills)

**Do inline when:**
- Reading 1-3 files for decision/verification
- Single-file mechanical edits
- Git/gh state checks (status, log, diff)

**When delegating, pass skill paths explicitly:**
```
## Skills to load before work
- /path/to/gitnexus-master/SKILL.md
- /path/to/rust-skills/SKILL.md
```

Every sub-agent prompt that involves code MUST include:
```
MANDATORY: Load gitnexus-master skill. Use MCP tools (gitnexus_query,
gitnexus_impact, gitnexus_detect_changes) — NEVER shell out to `gitnexus`
CLI for analysis. MCP tools are instant. CLI is only for analyze/clean/wiki.
NEVER grep the project codebase when gitnexus_query works.
```

---

## Code Style

- Error messages in **Spanish** (not English)
- HTTP client is **`wreq`** (not `reqwest`) — TLS fingerprint emulation for WAF evasion
- Never use `.unwrap()` in production — use `?` or `match`

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

### Always

- `cargo check` before marking tasks complete
- `cargo clippy -- -D warnings` before committing
- Run `cargo fmt` before committing

### Ask first

- Adding or removing dependencies
- Changing feature flags
- Modifying `Cargo.toml` profiles

### Never

- Commit secrets, `.env`, or credentials
- Use `.unwrap()` in production code
- Force push to main
- Modify `target/`, `dist/`, `build/` directories
- Run any command from the forbidden table above

---

## Skills Reference

| Purpose | Skill | Load when |
|---------|-------|-----------|
| Code intelligence | `gitnexus-master` | Any code work — has full MCP tools, CLI commands, impact protocol |
| Rust quality | `rust-skills` | Writing Rust — ownership, error handling, async, testing |
| SDD workflow | `sdd-*` skills | Planning/verifying changes |

Skills contain tool details (parameters, flags, schemas). This file contains workflow (when, sequence).

---

## Resources

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — Architecture details
- [justfile](justfile) — Task recipes
- [docs/wiki/](docs/wiki/) — GitNexus auto-generated documentation

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **rust_scraper** (4346 symbols, 8004 relationships, 190 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `gitnexus analyze` in terminal first. If it crashes (Napi::Error), run `gitnexus clean -f && gitnexus analyze`.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/rust_scraper/context` | Codebase overview, check index freshness |
| `gitnexus://repo/rust_scraper/clusters` | All functional areas |
| `gitnexus://repo/rust_scraper/processes` | All execution flows |
| `gitnexus://repo/rust_scraper/process/{name}` | Step-by-step execution trace |

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
