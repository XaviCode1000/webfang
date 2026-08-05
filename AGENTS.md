# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, TUI selector, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · ratatui · ort (feature-gated) · SQLite

---

## 🧠 Orchestration & Delegation Methodology

You are the **Orchestrator-Engineer**. You decide WHAT to do, WHERE to delegate, and WHICH tools/skills each agent loads. You do NOT write code directly unless it's a trivial single-line fix.

### Iron rules

- Never assume unlisted dependencies exist — verify with the Intelligence Stack (§2) before any code work.
- If a task touches 2+ non-trivial files → DELEGATE to a sub-agent.
- Never `.unwrap()` in production code — use `?`, `match`, or `.context()`.
- User-facing errors in Spanish; internal logs and tracing fields in English.
- Skip the Intelligence Gate ONLY for trivial doc/config changes.

### Delegation protocol

Every delegation prompt MUST include:

1. **Skills to load** — from the Skill Routing Matrix (§9). The sub-agent has NO memory; it only knows what you tell it.
2. **Intelligence mandate** — "Before editing any symbol, run `impact`. Before returning, run `detect_changes`." In worktrees: absolute path as `repo` (§2.3).
3. **Verification commands** — the exact `cargo check` / `cargo nextest` / `cargo clippy` commands to run.
4. **Worktree path** — if working in a worktree, the absolute path and the reminder that ALL three intelligence tools need it (§2.3).

### When to delegate vs. inline

| Action | Route |
|:---|:---|
| Read 1–3 files to decide or verify | Inline |
| Read 4+ files to understand | Delegate one narrow mapper |
| Write one mechanical, already-understood file | Inline |
| Write 2+ non-trivial files | Delegate one writer |
| Tests, builds, installs, review actions | Fresh worker per action |
| `git`, `gh` state commands | Inline |

---

## 🔬 Intelligence Stack

Three complementary tools. Pick by mission, not by habit. **Load the matching skill (§9), not the manual.**

### 2.1 Strategic routing

| Moment | Tool | Why this one |
|:---|:---|:---|
| "What is this task about?" — first-touch orientation | **CodeDB** `context` | 1 call: keywords + symbol defs + ranked files + snippets. Replaces 3–5 sequential calls. |
| "Show me the code for X" — explore and understand | **CodeGraph** `explore` | Returns verbatim source + call paths + blast radius in ONE call. Eliminates the grep→Read loop. |
| "Can I safely edit X?" — pre-edit blast radius | **GitNexus** `impact` | Depth-grouped (d=1/2/3) + risk level (LOW→CRITICAL) + affected processes. Only tool with precomputed execution flows. |
| "Is there a security issue?" — taint / dependence | **GitNexus** `explain` + `pdg_query` | Only tool with source→sink taint and CDG/REACHING_DEF. Needs `analyze --pdg`. |
| "Rename X across the codebase" | **GitNexus** `rename` | Call-graph aware, confidence-scored. NEVER find-and-replace. MCP-only (no CLI). |
| "What did my changes affect?" — pre-commit | **GitNexus** `detect_changes` | Git diff → affected symbols + execution flows. |
| "Where is X defined?" — instant lookup | **CodeDB** `word` / `symbol` | O(1) inverted index. Fastest possible. |
| "Who calls X?" — quick tactical check | **CodeDB** `callers` | 1 round-trip, fuses word-index + outline scope. |
| "Who calls X?" — deep 360° view | **GitNexus** `context` | Callers + callees + process participation + categorized refs. |
| "How does execution flow through X?" | **GitNexus** `query` + `process/{name}` | 300 precomputed flows. CodeDB and CodeGraph have no equivalent. |
| Post-edit linter diagnostics | **CodeDB** `diagnostics` | Surfaces real errors after a change. |
| Query a public GitHub repo (no clone) | **CodeDB** `remote` | GitNexus and CodeGraph cannot do this. |

**Rule of thumb:** CodeDB for *finding and reading* (fast, tactical, O(1)). CodeGraph for *exploring and understanding* (returns source directly). GitNexus for *analyzing and deciding* (deep, structural, precomputed flows + taint + PDG).

### 2.2 Non-negotiable gates

- Before editing any symbol → GitNexus `impact({direction:"upstream"})`. NEVER edit blind.
- Before renaming → GitNexus `rename` with `dry_run:true` first.
- Before commit → GitNexus `detect_changes()`. For regression review → `detect_changes({scope:"compare", base_ref:"main"})`.
- Index stale (`gitnexus://repo/webfang/context`) → STOP. Run `gitnexus analyze --index-only --skip-agents-md`.
- **Legitimate `grep`/`rg` exceptions:** logs, CI output, `.env`/config text, files outside the index — never for source code.

### 2.3 Worktree intelligence — CRITICAL

In worktrees, ALL three tools need the **absolute worktree path** or they silently resolve to the main checkout:

| Tool | Parameter | Example |
|:---|:---|:---|
| GitNexus MCP | `repo:` | `repo:"/var/home/xavi/Projects/webfang-worktrees/<dir>"` |
| CodeDB MCP | `project=` | `project="/var/home/xavi/Projects/webfang-worktrees/<dir>"` |
| CodeGraph MCP | `projectPath=` | `projectPath="/var/home/xavi/Projects/webfang-worktrees/<dir>"` |

**NEVER use** `repo:"webfang"` (bare name) in worktrees — ambiguous between main + all worktrees (#360). The absolute path is the official upstream disambiguation.

### 2.4 GitNexus operations

**Index refresh (post-commit/merge):**

```bash
gitnexus analyze --index-only --skip-agents-md              # structural graph, preserves embeddings, 0 tokens
gitnexus analyze --index-only --skip-agents-md --embeddings  # + re-embeds ONLY new/changed nodes (incremental hash compare)
```

- Plain `analyze` preserves existing embeddings without generating new ones.
- `--embeddings` is incremental: content-hash comparison skips unchanged nodes. Cost ∝ diff, not repo size.
- **NEVER** `--drop-embeddings` unless switching embedding model/dimension — it wipes the entire cache.
- ALWAYS `--skip-agents-md` so this file isn't overwritten by the auto-block regeneration.
- Add `--pdg` only for taint/control-data dependence layers. Add `--skills` only when regenerating skill files.

**Embeddings (remote, 2048d):** `.gitnexusrc` (gitignored) pins `embeddingBaseUrl` (OpenRouter) + `embeddingModel` (nemotron-vl-1b) + `pdg`/`skipAgentsMd`/`embeddings` defaults. Dims and API key are **env-only** (rc no los soporta): `GITNEXUS_EMBEDDING_DIMS=2048` + `GITNEXUS_EMBEDDING_API_KEY`. Sin esas env vars, cae al ONNX local (384d) y dispara dimension mismatch.

---

## 🏗️ Architecture & Code Rules

### Workspace structure (5 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure
│   ├── webfang_ai/               # ONNX embeddings, semantic cleaning
│   ├── webfang_tui/              # ratatui TUI selector
│   ├── webfang_mcp/              # MCP server (35 tools)
│   └── webfang_cli/              # CLI binary (webfang)
```

### Inter-crate dependency direction (ENFORCED POLICY)

```text
cli ──→ tui ──→ core ←── ai
cli ──→ mcp ──→ core
cli ──────────→ core
cli ──→ ai   (feature-gated, #433)
mcp ──→ ai   (feature-gated, #433)
```

Full allow-matrix (effective build graph):

| Crate | May depend on |
|:---|:---|
| `webfang_core` | — |
| `webfang_ai` | `webfang_core` |
| `webfang_tui` | `webfang_core` |
| `webfang_mcp` | `webfang_core`, `webfang_ai` |
| `webfang_cli` | `webfang_core`, `webfang_tui`, `webfang_ai`, `webfang_mcp` |

This is an architectural POLICY, not just what the code happens to do. New code must respect this direction. Verify cross-crate usage with `codedb_deps` or GitNexus `impact` before adding any inter-crate import.

**CI gate (#513):** `scripts/check_dependency_direction.sh` runs in the `toolchain` job of `ci.yml` and fails on any prohibited inter-crate dependency (including feature-gated optional deps). It parses each crate's `Cargo.toml` `[dependencies]`/`[dev-dependencies]` against the matrix above and prints the effective graph on success. Keep the matrix in the script and this section in sync.

### Intra-crate layers (Clean Architecture)

`infrastructure` → `adapters` → `application` → `domain` (inward only)

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates. When writing new code, follow the existing patterns:

| Writing a... | Copy from | Location |
|:---|:---|:---|
| New service/trait | `crawler_service.rs` | `application/` — trait → impl with DI, `async_trait`, `#[instrument]`, typed errors |
| New domain entity | `entities.rs` | `domain/` — struct + constructor + `TryFrom` validation, `Display`+`Debug`+`PartialEq` |
| New adapter | `crawler/` | `infrastructure/` — domain trait → impl, module with `mod.rs` |
| New error type | `error.rs` | `cli/` — `thiserror::Error` + `From` impls, Spanish user-facing |
| New behavioral test | `cli_harness.rs` | `tests/common/` — `BehavioralTest` + wiremock + TempDir + insta snapshots |

**Avoid:** `adapters/tui/progress_widget.rs` (551 lines), `infrastructure/mcp_server/mod.rs` (1404 lines) — keep new components focused.

### Error stratification

```text
[CLI] → ScraperError : [infra] HttpError/WafError/ParseError
                ↓
        DomainError (7 variants)
        AppError (6 variants)
        InfraError (13 variants)
```

Dual wrapping pattern: infra errors wrap into domain errors via `From` impls. New error variants MUST follow this chain — never bubble raw infrastructure errors to the CLI layer.

### HTTP client

**ALWAYS `wreq`**, never `reqwest` — TLS fingerprint impersonation for WAF evasion. This is non-negotiable. An agent suggesting `reqwest` as an alternative is wrong.

### Async rules

- Tokio multi-threaded runtime.
- `spawn_blocking` for CPU-intensive work (ONNX inference, HTML parsing) — see `CpuBridge.dispatch`.
- Never hold `Mutex`/`RwLock` across `.await`.
- Bounded channels for backpressure.

### MCP server — canonical location

**`crates/webfang_mcp/src/mcp_server/`** is the ONLY canonical location. The root `src/` was deleted (PR #163 cleanup). Never create code in `src/`.

MCP tools: 35 tools across 8 categories. Transport: Streamable HTTP (`rmcp`) at `127.0.0.1:8080/mcp`, also stdio via `mcp_server_stdio` example.

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed.
- `selectors` 0.35 (via legible→dom_query), 0.37 (via lol_html), 0.38 (via scraper) — all THREE needed.
- `quick-xml` — single 0.41, no longer a conflict.

An agent suggesting "clean up duplicate dependencies" must be stopped. These conflicts are intentional.

### AI feature (`--features ai`)

- ONNX models cached in `~/.cache/webfang/ai_models/`: Granite-97M (default, ~390MB, 384d) or Granite-311M (~1.25GB) via `AI_MODEL_ID` / `--ai-model`.
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings.

### Build requirement

`cmake` is mandatory — `wreq` → `boring2` → `boring-sys2` needs it for BoringSSL. First build compiles BoringSSL from C++ (~3–5 min).

---

## 🧪 Testing Methodology

### Framework & harness

Root `tests/` integration tests are wired into `webfang_core` via explicit `[[test]]` entries in `crates/webfang_core/Cargo.toml`. The workspace root `Cargo.toml` is virtual (no `[package]`), so root `tests/` files need explicit `[[test]]` wiring — they are **never auto-discovered**.

Test harness lives in `tests/common/cli_harness.rs`:

- `BehavioralTest` — wiremock `MockServer` + `tempfile::TempDir`, `scraper_cmd()`, `find_files()`, `read_md_content()`.
- Snapshot helpers: `assert_snapshot`, `redact_nondeterministic`, `assert_snapshot_redacted`, `assert_snapshot_plain`.

### Binary resolution: `webfang_path()`

**NEVER use `assert_cmd::cargo_bin(...)` in integration tests.** The `CARGO_BIN_EXE_*` env var is only set for the owning crate. In this virtual workspace, `webfang` is built by `webfang_cli` — a sibling crate. Tests running under `webfang_core` cannot resolve it via `cargo_bin`.

Always use `webfang_path()` from `tests/common/cli_harness.rs`. **Golden rule:** `Command::new(webfang_path())`, never `Command::cargo_bin(...)`.

### Snapshot testing (`insta`)

All behavioral tests that produce Markdown/JSON/stderr output MUST use snapshots instead of `assert!(output.contains("..."))`.

**Workflow:** make changes → `cargo nextest run` (tests FAIL with `.snap.new`) → `cargo insta review` (review every diff) → `cargo nextest run` (PASS). `.snap.new` is gitignored — never commit pending snapshots.

**Sanitization (mandatory):** always apply `redact_nondeterministic()` which normalizes: TempDir path → `[TEMP_PATH]`, ISO-8601 timestamps → `[TIMESTAMP]`, wiremock ports → `[PORT]`, ANSI escapes → `[ANSI]`. For additional non-deterministic fields, use `insta::with_settings!({ add_filter(...) })`.

### Test quality — contract-based-test-audit

When writing or modifying tests, **load the `contract-based-test-audit` skill**. It enforces a 6-node diagnostic:

1. **Observable behavior** — test public ports only, never internal state.
2. **Ephemeral adapters** — wiremock for HTTP (no real network), TempDir for filesystem.
3. **Semantic assertions** — validate business invariants via snapshots, not raw data dumps.
4. **Effort distribution** — maximum test investment on stable domain logic.
5. **Arrange simplicity** — complex Arrange (>5 lines setup) = production design flaw.
6. **Absolute determinism** — injected time/randomness, zero flakiness.

If the Arrange phase is complex, fix the production design, not the test.

### Creating a new root integration test

1. Create the test file in `tests/`.
2. Add a `[[test]]` entry in `crates/webfang_core/Cargo.toml`: `name = "my_test"`, `path = "../../tests/my_test.rs"`.
3. Use `use crate::common::*;` for the shared harness, `webfang_path()` for binary resolution, snapshots for output validation.
4. Run `cargo nextest run --test my_test` to verify.

---

## 🔭 Observability (MANDATORY for every change)

**Iron rule:** any new feature, hot path, or behavior change MUST ship with observability. Code that cannot be traced in production is not done. There is no OpenTelemetry (removed in #356) — the stack is the `tracing` crate + the always-available **FileTraceLayer** (`--trace-file out.jsonl`) + native **correlation IDs**.

### Required for new/changed code

| Situation | Requirement |
|:---|:---|
| New hot path / operation | `#[instrument(skip(...), fields(url = %url, ...))]` with the fields that identify the operation |
| Error path | `log_scrape_error(&err, url, stage, correlation_id, "context")` — never a bare `warn!`/`eprintln!` |
| New crawl/batch flow | Generate a `CorrelationId` at entry and propagate it; each unit of work gets `.child()` |
| Long-running op | Periodic progress log + final structured summary (`total`, `succeeded`, `errors`, `duration`, `trace_id`) |
| Async spans | Use `.instrument(span)` on futures — never hold a `span.enter()` guard across `.await` |

### Conventions

- **Structured fields, not string soup:** `tracing::info!(pages = n, url = %url, "msg")` — never `format!` data into the message.
- **Correlation:** every event/span carries its `trace_id`, so a whole operation is reconstructable with `jq 'select(.fields.trace_id == "...")'`.
- **User-facing errors in Spanish; tracing fields/logs in English.**
- **No new metrics backends:** do not reintroduce OpenTelemetry or any external collector. Emit a structured tracing event and query it from the JSONL.
- **Snapshots stay deterministic:** `correlation_id`/`trace_id` are internal and `#[serde(skip)]` on scraped output; redact via `redact_nondeterministic()`.

See `docs/debugging.md` and `scripts/analyze-trace.sh` for the full query cookbook. The observability module lives in `crates/webfang_core/src/infrastructure/observability/`.

---

## 🌳 Git Worktree Isolation

This project uses **sibling worktrees** for parallel development. Each active branch lives in its own directory outside the main repo — shared `.git` object store, isolated working trees, indexes, and HEAD.

### Iron rules (MANDATORY)

- **CWD is the absolute boundary.** Never access paths outside the current worktree via `../<sibling-worktree>/`.
- **ONE worktree per session.** Never switch branches mid-task — create a new worktree instead.
- **Forbidden commands:**
  - `git checkout`, `git switch` — they change the branch inside the current worktree. Use `git worktree add`.
  - `git stash` / `git stash pop` / `git stash apply` / `git stash drop` — **stash storage (`refs/stash`) is shared across ALL worktrees**. A `pop` in one worktree can apply a stash from a completely different session. If you need to set work aside, commit to a throwaway branch.
  - `git worktree move`, `git worktree lock` — use `remove` + `add` instead.
- **Forbidden:** any commit whose branch doesn't match the worktree's directory name (enforced by global pre-commit hook).

### Placement & naming

Worktrees live as **siblings** of the repo (never inside it — in-repo worktrees cause recursion with file watchers, ripgrep, and code intelligence tools):

```text
~/Projects/
├── webfang/                     # main repo (always on main)
├── webfang-worktrees/           # worktree siblings (gitignored globally)
│   ├── feat-auth/               # branch: feat/auth
│   └── fix-crawler-timeout/     # branch: fix/crawler-timeout
```

Branch `feat/auth` → directory `feat-auth` (`/` → `-`). The global pre-commit hook validates this mapping.

### Worktree lifecycle

**Create (from main repo):**

```bash
git worktree add ~/Projects/webfang-worktrees/feat-auth -b feat/auth
cd ~/Projects/webfang-worktrees/feat-auth

# Per-worktree bootstrap (NONE of these are shared):
cargo build                                        # target/ (~3-5 min first build: BoringSSL)
cp ~/Projects/webfang/.env .                       # .env is gitignored
gitnexus analyze --index-only --skip-agents-md     # GitNexus: graph + flows
codegraph init                                     # CodeGraph: source exploration index
codedb index .                                     # CodeDB: inverted index + outlines
```

> ⚠️ **Without all three indexes, the agent is BLIND in the worktree.** Intelligence tools silently resolve to the main checkout or return empty results. Verify with `gitnexus status`, and check that `.codegraph/` and `codedb.snapshot` exist.

> ⚠️ **Restart the editor's MCP connection after indexing.** The MCP server caches the registry at startup. Without a restart, tools keep resolving to the main checkout (#360).

**Cross-branch read access (NO checkout):**

```bash
git show main:crates/webfang_core/src/main.rs      # read a file from another branch
git diff main..HEAD -- crates/                     # compare with main
git log main --oneline -10                         # inspect history
```

### Post-merge cleanup & mission handoff (MANDATORY)

A merge is NOT done until the repo is clean and ready for the next mission. Cleanup is part of the **definition of done**. Run from the MAIN repo (`~/Projects/webfang`, always on `main`):

1. **Verify the merge landed** — `gh pr view <N> --json state,mergedAt,mergeCommit`; `state` must be `MERGED`.
2. **Sync local main (ff-only)** — `git fetch origin && git merge --ff-only origin/main`. If `--ff-only` FAILS, local main diverged — STOP and investigate; never paper over it.
3. **Remove the mission worktree** — `git worktree remove ~/Projects/webfang-worktrees/<dir>`.
4. **Delete the local branch** — `git branch -D <type>/<description>`. Squash-merge rewrites history, so safe `-d` refuses; the step-1 `MERGED` check is your safety net. Never touch: `main`, `gh-pages`, `backup/*`, or the current branch.
5. **Prune orphaned metadata** — `git worktree prune`.
6. **Verify the handoff contract** — `git worktree list` (ONLY main), `git branch -vv` (ONLY main, in sync), `git status --short` (empty).

**Automated safety net:** a weekly systemd timer (`git-hygiene.timer`, Sun 03:00) prunes confirmed-safe stale LOCAL branches. It SKIPS `*-worktrees` containers and never syncs main — worktree removal and ff-only sync are STILL your per-mission responsibility.

### Shared vs. per-worktree resources

| Resource | Shared? | Action required |
|:---|:---|:---|
| `.git/` object store | ✅ Shared | Automatic |
| Git config, hooks | ✅ Shared | Automatic |
| `Cargo.lock` | ✅ Shared | Via Git |
| `target/` | ❌ Per-worktree | `cargo build` (~3-5 min first) |
| `.env` | ❌ Per-worktree | Manual `cp` from main |
| `.gitnexus/` index | ❌ Per-worktree | `gitnexus analyze --index-only --skip-agents-md` |
| `.codegraph/` index | ❌ Per-worktree | `codegraph init` |
| `codedb.snapshot` | ❌ Per-worktree | `codedb index .` |
| Git stash (`refs/stash`) | ⚠️ Shared (DANGER) | **NEVER use `git stash`** |

### GitNexus in worktrees (`detect_changes` pitfall)

GitNexus registers the main checkout and ALL worktrees under the **same name** `webfang` (upstream #1259). Bare-name resolution picks the main entry — so `detect_changes()` or `detect_changes({repo:"webfang"})` reads the **main checkout's clean tree**, not your worktree. The pre-commit gate fails open: "No changes detected" while your worktree has uncommitted changes.

**In worktrees, ALWAYS use the absolute path** (§2.3). This applies to ALL GitNexus MCP tools that take a `repo` parameter.

### Bounded Review (4R) in worktrees

The gentle-ai bounded review hook resolves the target repo from the OpenCode session CWD. When the session runs from main but changes live in a worktree, the binding mismatches and every lens refuses to launch.

- **Option A (proper):** set `GENTLE_AI_REVIEW_CWD=<absolute worktree path>` in the OpenCode server environment BEFORE starting the session.
- **Option B (no restart):** launch lenses as `general` agents — the hook only intercepts `review-*` agent types with a `GENTLE_AI_REVIEW_BINDING` prefix.

The project's PR workflow does NOT require a gentle-ai review receipt — only cargo gates, `detect_changes`, linked issue, one `type:*` label, conventional branch.

### Rebase caveats

- `rebase.updaterefs=true` does NOT auto-update branches checked out in other worktrees — rebase each sequentially.
- `rebase.autostash=true` auto-stashes before rebase. Since stash is shared, avoid rebasing in multiple worktrees simultaneously.

### Commit frequently (MANDATORY in worktrees)

Commit after every completed step. Uncommitted work in a worktree can be lost silently if the agent loses context or a checkout occurs. Load the `work-unit-commits` skill for the full pattern.

| Step | Commit? |
|:---|:---|
| git mv of files/directories | ✅ Immediately |
| Bulk sed/replace across files | ✅ Immediately |
| cargo check passes | ✅ Marker: "wip: cargo check passes" |
| Tests pass | ✅ Or amend previous WIP |
| Clippy + fmt clean | ✅ Final commit |

### Contamination protocol

If you detect you operated outside your assigned worktree, or `git stash pop` applied unexpected changes:

1. **STOP** all operations immediately.
2. Do NOT attempt to clean up — no `git reset`, no force-push, no manual patching.
3. Report exactly: "Contamination detected. Worktree: `<path>`. Intruder commit: `<hash>` or unexpected stash applied. Awaiting human instructions."
4. Wait for explicit human authorization before any corrective action.

---

## 🔒 Safety & Permissions

### Allowed without asking

- Read any file in the repo.
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`.
- All three intelligence tools: GitNexus MCP/CLI, CodeDB MCP, CodeGraph MCP.
- Edit files within `crates/`, `tests/`, `benches/`, `examples/`.
- Worktree management: `git worktree add`, `remove`, `list`, `prune`.
- Read-only cross-branch inspection: `git show <branch>:<file>`, `git log <branch>`.

### Ask first

- Adding/removing dependencies (`Cargo.toml`).
- Changing feature flags or profiles.
- Deleting files.
- `cargo build --release` or `cargo llvm-cov`.
- Modifying CI/CD (`.github/`).
- New files outside `crates/`, `tests/`, `benches/`, `examples/`.
- Re-indexing with `--pdg` or `--drop-embeddings` (data-loss / cost implications).

### Never

- Commit secrets, `.env`, or credentials.
- `.unwrap()` in production — use `?` or `match`.
- Force push to main.
- Modify `target/`, `dist/`, `build/`.
- Run `gitnexus analyze` in a dirty worktree (breaks `detect_changes()`).
- Run `gitnexus analyze` without `--skip-agents-md` (re-injects the auto-block into this file).
- Use a package runner for GitNexus (`npx`/`bunx`) — install globally; verify with `which gitnexus`.
- `git checkout` / `git switch` to change branches (use `git worktree add`).
- `git stash` in any form (shared storage causes cross-worktree contamination).
- Access sibling worktrees via relative paths (`../feat-auth/...`).
- Commit in a worktree whose branch doesn't match the directory name.
- Use `repo:"webfang"` (bare name) for intelligence tools in worktrees — always absolute path (#360).

---

## 📝 Commit, PR & CI

**Format:** `type(scope): description`

- type: `feat` | `fix` | `refactor` | `test` | `docs` | `perf` | `chore` | `revert`
- scope: `cli` | `tui` | `crawler` | `ai` | `mcp` | `exporter` | `http` | `domain` | `infra`

### PR creation — CI-enforced rules (`pr-validation.yml`)

Every PR is validated on open / edit / synchronize / label changes. **All three MUST pass:**

1. **Linked issue** — PR body must contain `Closes #N`, `Fixes #N`, or `Resolves #N`.
2. **Exactly one `type:*` label** — count of labels starting with `type:` must be exactly 1.
3. **Conventional branch name** — must match `^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$`.

**Label mapping** (the label vocabulary is NOT the commit-type vocabulary):

| Commit type | GitHub label |
|:---|:---|
| `feat` | `type:feature` |
| `fix` | `type:bug` |
| `refactor` | `type:refactor` |
| `docs` | `type:docs` |
| `chore` | `type:chore` |
| (breaking) | `type:breaking-change` |

No `type:test` / `type:perf` / `type:revert` labels exist — map to closest (usually `type:chore`).

**Set label and linked issue at creation time:**

```bash
gh pr create --base main --head "$(git branch --show-current)" \
  --label type:refactor \
  --title "refactor(scope): description" \
  --body "Closes #NNN

## Summary
- what and why"
```

Base the body on `.github/PULL_REQUEST_TEMPLATE.md`.

### Pre-commit gate (every commit)

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines && cargo fmt
```

> ⚠️ **The clippy command MUST match CI exactly.** CI runs the strict gate above, which enables the `#516` complexity ratchets (`clippy::cognitive_complexity` + `clippy::too_many_lines`, thresholds in `clippy.toml`). Running a bare `cargo clippy -- -D warnings` locally will PASS while CI FAILS on any function >100 lines or over the cognitive-complexity limit. Always use the full command above before pushing.

### Cloud verification

```bash
# Trigger CI (returns immediately)
gh workflow run ci.yml --ref $(git branch --show-current)

# Non-blocking status check (preferred)
gh run list --workflow=ci.yml --branch "$(git branch --show-current)" --limit 1 \
  --json databaseId,status,conclusion
```

⚠️ `gh run watch` blocks up to ~30 min. Never run under a short tool timeout. Prefer the non-blocking pattern above.

⚠️ Git/GitHub network ops can hang transiently. Give generous timeout (≥ 180s) and retry once. A timed-out `git push` did NOT necessarily fail — verify with `git ls-remote origin <branch>`.

### PR checklist

- [ ] `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt`
- [ ] `cargo nextest run` (at least affected module)
- [ ] `detect_changes()` shows only expected symbols (worktrees: absolute path)
- [ ] `detect_changes({scope:"compare", base_ref:"main"})` for regression review
- [ ] Error messages in Spanish if user-facing; new public items have doc comments
- [ ] PR has exactly one `type:*` label + linked issue + conventional branch
- [ ] Verified worktree: `git branch --show-current` matches directory name
- [ ] No `git checkout`/`switch`/`stash` was executed during the session
- [ ] Post-merge handoff runbook will be executed after merge

### Automated merge workflow (single maintainer)

GitHub's auto-merge feature (`gh pr merge --auto`) is **broken for this repo**: classic
branch protection + no rulesets means `enablePullRequestAutoMerge` returns HTTP 422 /
`GraphQL: Auto merge is not allowed for this repository` (verified empirically, Aug 2026;
matches the open community thread orgs/community#190610 and ravenblackx's May 2026 report).
The fix GitHub announced for March 2026 has not landed for this repo profile.

The automation path that works:

1. Open the PR (`gh pr create ...`).
2. Walk away while CI runs (~6m34s to merge-ready; full wall: ~8m41s).
3. Run the automation script:

   ```bash
   scripts/merge-when-green.sh <PR-NUMBER>
   ```

The script:
- Polls `gh pr checks <N> --watch --required --fail-fast` until all required checks are
  SUCCESS or one FAILS/CANCELS (exit 2 on failure).
- Verifies `mergeStateStatus == CLEAN`. If `BEHIND`, exits 3 and asks you to rebase
  (single maintainer, ~30s; no auto-rebase needed).
- Calls `gh pr merge <N> --squash --delete-branch`. This **respects branch protection** —
  required checks must be green at merge time. It is NOT the synchronous-PUT bypass
  (`gh api -X PUT .../pulls/N/merge`) which bypasses required checks and should not be
  used for routine merges.
- Use `--dry-run` to poll and report without merging.

Do NOT rely on `--auto`: it never accepts in this repo configuration. If a future PR
needs auto-merge (e.g. transferring the repo to an organization with rulesets), revisit.

---

## 🗺️ Skill Routing Matrix

**Load the matching skill BEFORE executing.** The sub-agent has no memory — if you don't tell it which skill to load, it won't.

| Task | Skills to load | Key behavior |
|:---|:---|:---|
| Any code work (read/write/edit) | `gitnexus` + `codedb` | Intelligence Gate: impact before edit, detect_changes before commit |
| Writing Rust code | `rust-skills` (category per task type) | 265 rules across 26 categories. Category prefixes: `own-`, `err-`, `async-`, `api-`, `test-`, etc. |
| **Writing or modifying tests** | **`contract-based-test-audit`** + `rust-skills(test-)` | 6-node diagnostic: observable behavior, ephemeral adapters, semantic assertions, determinism |
| Planning commits | `work-unit-commits` | Commit by deliverable behavior, not by file type. Keep tests/docs with code |
| Creating PRs | `branch-pr` | Issue-first checks, CI-enforced rules |
| Writing docs / guides | `cognitive-doc-design` | Reduce cognitive load, review-facing docs |
| Refactoring / renaming | `gitnexus` | Safe rename via call graph (`dry_run:true` first), impact analysis |
| Security review | `gitnexus` (--pdg) | `explain` taint + `pdg_query` control/data dependence |
| SDD planning phases | `sdd-*` | Spec-driven development: explore → propose → spec → design → tasks → apply → verify → archive |

### Critical commands reference

**Fast gate (< 5s):**

```bash
git branch --show-current    # Verify correct worktree BEFORE any edit
cargo check                  # Verify compilation
cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines  # Fix ALL warnings (matches CI strict gate, #516 ratchets)
cargo fmt                    # Format
```

**Moderate (< 5 min):**

```bash
cargo nextest run            # Full suite
cargo build --release        # LTO fat, ~3-5 min
```

**PR automation (single maintainer):**

```bash
scripts/merge-when-green.sh <PR-N>          # Wait for green checks, squash-merge, delete branch
scripts/merge-when-green.sh <PR-N> --dry-run # Poll and report; do not merge
```

**Miri (unsafe/concurrent code only):**

```bash
cargo +nightly miri test infrastructure::bridge::
cargo +nightly miri test infrastructure::network::
```


