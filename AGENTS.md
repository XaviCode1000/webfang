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
2. **Intelligence mandate** — "Before editing any symbol, run CodeGraph `explore` for callers + impact. Before returning, verify with `cargo check`." In worktrees: absolute path (§2.3).
3. **Verification commands** — the exact `cargo check` / `cargo nextest` / `cargo clippy` commands to run.
4. **Worktree path** — if working in a worktree, the absolute path and the reminder that BOTH intelligence tools need it (§2.3).

### When to delegate vs. inline

| Action | Route |
| :--- | :--- |
| Read 1–3 files to decide or verify | Inline |
| Read 4+ files to understand | Delegate one narrow mapper |
| Write one mechanical, already-understood file | Inline |
| Write 2+ non-trivial files | Delegate one writer |
| Tests, builds, installs, review actions | Fresh worker per action |
| `git`, `gh` state commands | Inline |

---

## 🔬 Intelligence Stack

Two complementary tools. Pick by mission, not by habit. **Load the matching skill (§9), not the manual.**

### 2.1 Strategic routing

| Moment | Tool | Why this one |
| :--- | :--- | :--- |
| "What is this task about?" — first-touch orientation | **CodeDB** `context` | 1 call: keywords + symbol defs + ranked files + snippets. Replaces 3–5 sequential calls. |
| "Show me the code for X" — explore and understand | **CodeGraph** `explore` | Returns verbatim source + call paths + blast radius in ONE call. Eliminates the grep→Read loop. |
| "Where is X defined?" — instant lookup | **CodeDB** `word` / `symbol` | O(1) inverted index. Fastest possible. |
| "Who calls X?" — tactical check | **CodeDB** `callers` | 1 round-trip, fuses word-index + outline scope. |
| "How does X flow through the code?" — deeper view | **CodeGraph** `explore` / `impact` | Call paths + blast-radius summary from the source-graph index. |
| Post-edit linter diagnostics | **CodeDB** `diagnostics` | Surfaces real errors after a change. |
| Query a public GitHub repo (no clone) | **CodeDB** `remote` | CodeGraph cannot do this. |

**Rule of thumb:** CodeDB for *finding and reading* (fast, tactical, O(1)). CodeGraph for *exploring and understanding* (returns source directly, call paths, blast radius).

### 2.2 Non-negotiable gates

- Before editing any symbol → CodeGraph `explore` it (callers + impact). NEVER edit blind.
- Before renaming → check ALL usages first via `codedb_callers` / CodeGraph `explore`.
- Before commit → run `cargo check` + `cargo clippy` + `cargo fmt` and re-read the diff.
- **Legitimate `grep`/`rg` exceptions:** logs, CI output, `.env`/config text, files outside the index — never for source code.

### 2.3 Worktree intelligence — CRITICAL

In worktrees, BOTH tools need the **absolute worktree path** or they silently resolve to the main checkout:

| Tool | Parameter | Example |
| :--- | :--- | :--- |
| CodeDB MCP | `project=` | `project="/home/xavi/Projects/webfang-worktrees/<dir>"` |
| CodeGraph MCP | `projectPath=` | `projectPath="/home/xavi/Projects/webfang-worktrees/<dir>"` |

**NEVER use** bare project names in worktrees — ambiguous between main + all worktrees (#360). The absolute path is the official upstream disambiguation.

---

## 🏗️ Architecture & Code Rules

### Workspace structure (6 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure
│   ├── webfang_ai/               # ONNX embeddings, semantic cleaning
│   ├── webfang_tui/              # ratatui TUI selector
│   ├── webfang_mcp/              # MCP server (36 tools)
│   ├── webfang_cli/              # CLI binary (webfang)
│   └── webfang_test_utils/       # shared test utilities (not shipped)
│   └── webfang_benchmark/        # public benchmark harness (tooling leaf)
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
| :--- | :--- |
| `webfang_core` | — |
| `webfang_ai` | `webfang_core` |
| `webfang_tui` | `webfang_core` |
| `webfang_mcp` | `webfang_core`, `webfang_ai` |
| `webfang_cli` | `webfang_core`, `webfang_tui`, `webfang_ai`, `webfang_mcp` |
| `webfang_benchmark` | `webfang_core`, `webfang_test_utils` (leaf; benchmark tooling, no production dependents) |

This is an architectural POLICY, not just what the code happens to do. New code must respect this direction. Verify cross-crate usage with `codedb_deps` or CodeGraph `explore` before adding any inter-crate import.

**CI gate (#513):** `scripts/check_dependency_direction.sh` runs in the `toolchain` job of `ci.yml` and fails on any prohibited inter-crate dependency (including feature-gated optional deps). It parses each crate's `Cargo.toml` `[dependencies]`/`[dev-dependencies]` against the matrix above and prints the effective graph on success. Keep the matrix in the script and this section in sync.

### Intra-crate layers (Clean Architecture)

`infrastructure` → `adapters` → `application` → `domain` (inward only)

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates. When writing new code, follow the existing patterns:

| Writing a... | Copy from | Location |
| :--- | :--- | :--- |
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

MCP tools: 36 tools across 9 categories. Transport: Streamable HTTP (`rmcp`) at `127.0.0.1:8080/mcp`, also stdio via the `webfang-mcp-stdio` binary.

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed.
- `selectors` 0.35 (via legible→dom_query), 0.37 (via lol_html), 0.38 (via scraper) — all THREE needed.
- `quick-xml` — single 0.41, no longer a conflict.

An agent suggesting "clean up duplicate dependencies" must be stopped. These conflicts are intentional.

### AI feature (`--features ai`)

- ONNX models cached in the native hf_hub cache (`~/.cache/huggingface/hub/`): Granite-97M (default, ~390MB, 384d) or Granite-311M (~1.25GB) via `WEBFANG_AI_MODEL_ID` / `--ai-model` (legacy `AI_MODEL_ID` is still honored as a fallback). `--clean-ai` uses hf_hub natively: cache-first when online, strict cache-only when offline.
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings. Embeddings only appear in exports with `--output-vectors`.

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

### Test quality — six-node diagnostic

When writing or modifying tests, apply this 6-node diagnostic:

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
| :--- | :--- |
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

### Placement & naming

Worktrees live as **siblings** of the repo (never inside it — in-repo worktrees cause recursion with file watchers, ripgrep, and code intelligence tools):

```text
~/Projects/
├── webfang/                     # main repo (always on main)
├── webfang-worktrees/           # worktree siblings (gitignored globally)
│   ├── feat-auth/               # branch: feat/auth
│   └── fix-crawler-timeout/     # branch: fix/crawler-timeout
```

Branch `feat/auth` → directory `feat-auth` (`/` → `-`). Worktree/branch matching is a CONVENTION the agent must verify; run `[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"` before each commit in a worktree.

### Worktree lifecycle

**Create (from main repo):**

```bash
git worktree add ~/Projects/webfang-worktrees/feat-auth -b feat/auth
cd ~/Projects/webfang-worktrees/feat-auth

# Per-worktree bootstrap (NONE of these are shared):
cp ~/Projects/webfang/.envrc . && direnv allow     # shared CARGO_TARGET_DIR (gitignored)
cp ~/Projects/webfang/.env .                       # .env is gitignored
codegraph init                                     # CodeGraph: source exploration index
codedb index .                                     # CodeDB: inverted index + outlines
cargo build                                        # fast: reuses shared target via direnv
```

> ⚠️ **`.envrc` + `direnv allow` is mandatory per worktree.** It points `CARGO_TARGET_DIR` at the shared build cache (`~/.cache/cargo-target/webfang`), so BoringSSL and all dependencies compile once, not per worktree. Without it the worktree silently builds into its own `target/` (~3-5 min cold). direnv is installed via mise; the Fish hook lives in `~/.config/fish/conf.d/03-direnv.fish`.

> ⚠️ **Without both indexes, the agent is BLIND in the worktree.** Intelligence tools silently resolve to the main checkout or return empty results. Check that `.codegraph/` and `codedb.snapshot` exist.

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

**No automated safety net is installed.** This file previously described a weekly systemd
`git-hygiene.timer` (Sun 03:00) that pruned confirmed-safe stale local branches. **That
timer does not exist in this environment** — verified 2026-08-30: `systemctl --user
list-timers --all` lists no such unit and `~/.config/systemd/user/` does not exist. Every
step of the runbook above is therefore entirely manual. If the timer is ever installed,
restore the description here with its actual scope.

### Shared vs. per-worktree resources

| Resource | Shared? | Action required |
| :--- | :--- | :--- |
| `.git/` object store | ✅ Shared | Automatic |
| Git config, hooks | ✅ Shared | Automatic |
| `Cargo.lock` | ✅ Shared | Via Git |
| `target/` | ✅ Shared (via direnv) | `.envrc` + `direnv allow` per worktree |
| `.envrc` | ❌ Per-worktree | `cp` from main + `direnv allow` |
| `.env` | ❌ Per-worktree | Manual `cp` from main |
| `.codegraph/` index | ❌ Per-worktree | `codegraph init` |
| `codedb.snapshot` | ❌ Per-worktree | `codedb index .` |
| Git stash (`refs/stash`) | ⚠️ Shared (DANGER) | **NEVER use `git stash`** |

### CodeDB/CodeGraph in worktrees

Both tools resolve projects by name; bare-name resolution picks the main checkout — so queries run from a worktree without the absolute path read the **main checkout**, not your worktree (#360). **In worktrees, ALWAYS use the absolute path** (§2.3).

### Bounded review — delivery gates wired, and what they actually enforce (#1036, #1047, #1050)

RDD is ON (decided by global). Reviews are routed per candidate through the Pi
review tools. `core.hooksPath` points at `scripts/githooks/`, so `pre-commit` and
`pre-push` consult `gentle-ai review validate --gate <gate>` on every delivery.

**What the gate really blocks:** it blocks only while a review lineage governs
*this exact candidate* and has not reached an allowed state. It is **not** a
receipt gate and cannot be:

- `review acknowledge-approved` is terminal and **burns the lineage** — after
  approval there is no durable receipt for any gate to consult.
- Authority is pinned to a candidate tree; **any change to the candidate
  un-governs it**. Observed directly in #1048: the hook printed
  `delivery: unmanaged` while the lineage sat in `correction_required`, because
  the correction commit produced a new candidate identity.
- The provider declares this boundary deliberately: review verdicts are
  model-produced (untrusted actor output), so `gentle-ai` states its gates are
  **"informational and unmanaged; ordinary repository policy decides
  delivery"** (upstream: gentle-pi `README.md:135`, `:272`, `:306`,
  `docs/native-authority-architecture.md:5`, and the policy string embedded in
  the `gentle-ai` binary).

So the wiring is honest friction — no commit while a review of this candidate is
open, and no silent delivery over a hung or failing gate — not a delivery
authorization. Treat "the review passed" as evidence, never as permission.

**Mechanism** — repo-local git config, shared across all worktrees. Fresh clones
must run `git config core.hooksPath scripts/githooks`. Decision matrix:

| validate output | hook decision |
|---|---|
| `gentle-ai` binary absent | ALLOW + warning (a machine without the tool cannot gate anything) |
| validate fails, hangs past `timeout 20s`, or output is unparseable | **BLOCK** (fail-closed) |
| `delivery: unmanaged` | ALLOW — ordinary repository policy applies |
| `allowed: true` | ALLOW |
| any other governed state | **BLOCK** |

The verdict is parsed from JSON, never from the exit code: `validate` exits **0
even when `allowed: false`**. `jq` is preferred with a built-in `sed` scalar
fallback, so a missing `jq` downgrades the parser but never disables the gate.

**Boundary:** initiating a review from a plain shell fails with
`immutable_review_transport_unsupported` — the relay contract is host-only. The
hooks therefore only *consult* an existing verdict; reviews are initiated
through the Pi review tools, which persist the authority `validate` reads.
Measured 2026-08-30: `validate --gate` itself exits 0 and abstains
(`delivery: unmanaged`) for unmanaged candidates, so the wiring does not block
ordinary commits — refuting the earlier "hooks would block every commit"
claim (#1047 fact 2/3).

### Rebase caveats

- `rebase.updaterefs=true` does NOT auto-update branches checked out in other worktrees — rebase each sequentially.
- `rebase.autostash=true` auto-stashes before rebase. Since stash is shared, avoid rebasing in multiple worktrees simultaneously.

### Commit frequently (MANDATORY in worktrees)

Commit after every completed step. Uncommitted work in a worktree can be lost silently if the agent loses context or a checkout occurs. Load the `work-unit-commits` skill for the full pattern.

| Step | Commit? |
| :--- | :--- |
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
- Both intelligence tools: CodeDB MCP, CodeGraph MCP.
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

### Never

- Commit secrets, `.env`, or credentials.
- `.unwrap()` in production — use `?` or `match`.
- Force push to main.
- Modify `target/`, `dist/`, `build/`.
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
   For a **partial slice of an umbrella issue**, use `Closes part of #N` — it passes
   validation and, because GitHub's auto-close requires strict adjacency, does **not**
   close the umbrella. Never write a bare `Closes #N` against an umbrella that has
   remaining scope: it auto-closes the tracker mid-plan, which is exactly how #994 was
   closed after sub-slice 1 of 5 with 0 of 11 acceptance criteria ticked (#1010).
   The cleanest shape remains **one issue per PR**, with the umbrella as an index that
   links child issues rather than a link target.
2. **Exactly one `type:*` label** — count of labels starting with `type:` must be exactly 1.
3. **Conventional branch name** — must match `^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$`.

**Label mapping** (the label vocabulary is NOT the commit-type vocabulary):

| Commit type | GitHub label |
| :--- | :--- |
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

### CHANGELOG policy (written at consolidation — work PRs never touch it)

`CHANGELOG.md` is **out of scope for ordinary work PRs and for every delegated agent**. Do not
add, edit, or "keep it fresh" in a feature/fix branch.

The entries are written **once, centrally, in the consolidation step** — the batch PR that merges
the reviewed work to `main` (see "Batch merge of multiple green PRs" below):

- **Large slice → ONE entry** covering the whole slice.
- **Small slice → ONE entry per issue closed** (closing `Closes #A` + `Closes #B` gets two entries).
- Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), entries under `## [Unreleased]`
  in the matching category (respect the existing emoji category headings).
- Entry text: English, imperative, with the `#<issue>` reference.

```markdown
## [Unreleased]

### 🔧 Fixed

#### Unified filename sanitizer (#911)
- Collapsed the two divergent filename sanitizers into one join-safe helper.
```

> **Why the restriction, not just a preference:** the batch-merge policy requires the merged PRs to
> touch **disjoint files** so N green PRs cost one CI run instead of N. `CHANGELOG.md` is a single
> shared file, so per-PR edits guarantee a conflict in every batch and silently kill the
> optimization. The file stays untouched until one writer — the consolidation PR — owns it.

### Pre-commit gate (every commit)

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines && cargo fmt
```

> ⚠️ **The clippy command MUST match CI exactly.** CI runs the strict gate above, which enables the `#516` complexity ratchets (`clippy::cognitive_complexity` + `clippy::too_many_lines`, thresholds in `clippy.toml`). Running a bare `cargo clippy -- -D warnings` locally will PASS while CI FAILS on any function >100 lines or over the cognitive-complexity limit. Always use the full command above before pushing.

> 🚨 **`--all-features` is a safety flag here, not a strictness preference.** This crate has `chromium`-gated code whose only consumers are behind `#[cfg(feature = "chromium")]`. Running clippy **without** `--all-features` makes those imports look dead, and `clippy --fix` will **delete live code** — `cargo check` with default features then still passes, so the loss is invisible until `--all-features` fails. This bit the main checkout twice during #994 (see #1006). Never run `clippy --fix`, and never wire an auto-fixing tool, against a feature set narrower than the build's. `.pi-lens.json` disables the pi-lens autofix paths for exactly this reason; do not re-enable them.

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
- [ ] Review `git diff --stat main...HEAD` to confirm only expected symbols/files changed
- [ ] Error messages in Spanish if user-facing; new public items have doc comments
- [ ] PR has exactly one `type:*` label + linked issue + conventional branch
- [ ] `CHANGELOG.md` **not** touched by this PR (entries are written once, in the consolidation PR — AGENTS.md → "CHANGELOG policy")
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

- Reads the required status-check contexts from **branch protection**, then polls
  `gh pr checks <N> --json name,bucket` until every one of them has **reported** with a
  terminal bucket, and all report `pass` (exit 2 on failure, 4 if a required context
  never reports). Waiting on `--watch --required` alone is unsafe: it evaluates against
  the checks reported *so far*, so immediately after a push it declared "all required
  checks are GREEN" from a 2-of-3 subset while `CI Gate` had not been queued yet, then
  exited 3 on `BLOCKED` (#1011). If the required-context list cannot be read, the script
  falls back to the old watch behaviour and warns on stderr.
- Verifies `mergeStateStatus` is `CLEAN` or `UNSTABLE` (UNSTABLE with required checks
  green is the repo's normal green state — skipped-by-design jobs push it there, #823).
  `UNKNOWN` is retried for up to 90s rather than treated as a verdict — it means GitHub
  has not finished computing mergeability, the same incomplete-answer class as #1011.
  If `BEHIND`, exits 3 and asks you to rebase (single maintainer, ~30s; no auto-rebase needed).
- A **required** check that reports `skipping` is treated as not-green (exit 2). Required
  checks are expected to run; a skipped one is not evidence of anything.
- Calls `gh pr merge <N> --squash --delete-branch`. This **respects branch protection** —
  required checks must be green at merge time. It is NOT the synchronous-PUT bypass
  (`gh api -X PUT .../pulls/N/merge`) which bypasses required checks and should not be
  used for routine merges.
- Use `--dry-run` to poll and report without merging.

Do NOT rely on `--auto`: it never accepts in this repo configuration. If a future PR
needs auto-merge (e.g. transferring the repo to an organization with rulesets), revisit.

### Batch merge of multiple green PRs (avoid N× CI re-runs)

**Trigger:** the agent detects 2+ open PRs, all with green CI, all targeting `main`.

**Why sequential merging is slow:** branch protection has `strict: true`, so after merging
the first PR, every remaining PR becomes `BEHIND` and each `update branch` (rebase)
re-runs the FULL CI (~27 min). N PRs sequential ≈ N × 27 min. One batch PR ≈ 1 × 27 min.

**Precondition (verify first, no exceptions):**

1. All PRs are `MERGEABLE` with `mergeStateStatus: CLEAN`.
2. **Files touched are fully disjoint** — check with:
   `for pr in <N1> <N2>; do gh pr view $pr --json files --jq '.files[].path'; done`
   Any overlap → do NOT batch; merge sequentially instead.
   (`CHANGELOG.md` must not appear in any of these lists — see "CHANGELOG policy" above. If it
   does, that PR violated the policy and must drop the file before batching.)
3. All PRs share a compatible `type:*` label (e.g. all `fix` → one `type:bug`).

**Procedure:**

```bash
# 1. Branch from current main in a new worktree
git fetch origin && git merge --ff-only origin/main
git worktree add ~/Projects/webfang-worktrees/fix-batch -b fix/batch-<topic>

# 2. Merge each PR's REMOTE head SHA (not the local branch — it may be stale)
#    Get the exact SHA: gh pr view <N> --json headRefOid --jq '.headRefOid'
git merge --no-ff <sha1> -m "Merge <branch> (PR #N1)"
git merge --no-ff <sha2> -m "Merge <branch> (PR #N2)"

# 3. Write the CHANGELOG entries HERE — this is the ONE place they are written.
#    Under `## [Unreleased]`, one entry per merged slice (or per closed issue for small ones).
#    See "CHANGELOG policy" above: no other PR ever touches this file.

# 4. Local gate, push, create the batch PR linking ALL issues
cargo check && cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines && cargo fmt
git push -u origin fix/batch-<topic>
gh pr create --base main --head fix/batch-<topic> --label type:bug \
  --title "fix(batch): ..." --body "Closes #A
Closes #B

## Summary
..."

# 5. Close the original PRs as superseded
for pr in <N1> <N2>; do gh pr close $pr --comment "Superseded by #<batch-PR>"; done

# 6. Delete the now-orphan remote branches (gh pr close does NOT delete them)
git push origin --delete <branch1> <branch2>
```

**Merge method:** use `gh pr merge <batch-PR> --merge` (merge commit), NOT `--squash`.
Squash would crush N independent fixes into one commit, losing per-fix revert
granularity. The merge commit preserves each original commit in main's history.
Note: `merge-when-green.sh` hardcodes `--squash`, so do NOT use it for batch PRs.

> ⚠️ **`UNSTABLE` ≠ failed merge.** With non-required checks failing/skipped,
> `mergeStateStatus` can be `UNSTABLE` while required checks are green; `gh pr merge`
> still merges (respects branch protection). Always verify with
> `gh pr view <N> --json state,mergeCommit` before assuming failure or retrying (#819).

**Issue cleanup is automatic:** the `Closes #N` keywords in the batch PR body close
all linked issues at merge time. Never close them manually before the merge — that
is premature (the fix is not in main yet) and breaks the auto-close trace.

**Post-merge:** run the standard post-merge runbook (ff-only sync, remove batch
worktree, delete local branch, prune). Final state: only `main` locally and remotely,
empty `git status`, all linked issues CLOSED.

**Real example:** PRs #741 + #744 + #745 (disjoint files, all green) → batch PR #746,
merged as `84dc0c1`. Saved ~54 min of CI (3 × 27 min → 1 × 27 min).

---

## 🗺️ Skill Routing Matrix

**Load the matching skill BEFORE executing.** The sub-agent has no memory — if you don't tell it which skill to load, it won't.

| Task | Skills to load | Key behavior |
| :--- | :--- | :--- |
| Any code work (read/write/edit) | `codedb` + `codegraph` | Intelligence Gate: explore impact before edit, `cargo check` before commit |
| Writing Rust code | `rust-skills` (category per task type) | 265 rules across 26 categories. Category prefixes: `own-`, `err-`, `async-`, `api-`, `test-`, etc. |
| **Writing or modifying tests** | `rust-skills(test-)` | 6-node test quality diagnostic: observable behavior, ephemeral adapters, semantic assertions, determinism |
| Planning commits | `work-unit-commits` | Commit by deliverable behavior, not by file type. Keep tests/docs with code |
| Creating PRs | `branch-pr` | Issue-first checks, CI-enforced rules |
| Writing docs / guides | `cognitive-doc-design` | Reduce cognitive load, review-facing docs |
| Refactoring / renaming | `codegraph` + `codedb` | Safe rename via call graph — check ALL callers first (`codedb_callers`), never blind find-and-replace |
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

### Git aliases (use them — agents included)

The maintainer's git config ships these aliases. Prefer them over raw commands; they encode the project's inspection workflow:

| Alias | Expands to | Use for |
| :--- | :--- | :--- |
| `git ddiff` | `-c diff.external=difft diff` | Diff with Difftastic (semantic, tree-sitter-based) — much more readable than Myers diff for Rust refactors |
| `git dshow` | `-c diff.external=difft show --ext-diff` | Show a commit with Difftastic rendering |
| `git dlog` | `-c diff.external=difft log --ext-diff` | History walk with Difftastic per-commit diffs |
| `git lg` | `log --graph --decorate --all` | Branch topology at a glance |
| `git ll` | `log --oneline --decorate --all` | Compact history across all refs |
| `git last` | `log -1 HEAD` | Latest commit summary |
| `git unstage` | `restore --staged` | Unstage files WITHOUT touching the worktree (preferred over any stash-like workaround) |
| `git amend` | `commit --amend --no-edit` | Fold staged changes into the previous commit (work-unit commits discipline) |
| `git root` | `rev-parse --show-toplevel` | Resolve the current worktree root — use it to verify CWD before any edit |

Notes for agents: `ddiff`/`dshow`/`dlog` require `difft` (Difftastic) on PATH. `git root` is the fastest CWD sanity check against the worktree-isolation rules in 🌳 above.

---

## 🚧 Sprint 0 Gate 0 — Freeze + StateStore (sdd/stabilization-sprint0-baseline)

### Freeze policy

- `FREEZE_FEATURES=true` in `.github/workflows/pr-validation.yml` (workflow `env`). When frozen, `type:feature` and `type:breaking-change` are **blocked** with `::error::Gate 0 freeze … Ver sdd/stabilization-sprint0-baseline`.
- Bypass only with **both** `freeze-exception` label **and** CODEOWNER approval via `gh api repos/$REPO/pulls/$PR/reviews` (`APPROVED` count >0). Fail-closed when `gh` empty or non-numeric. **In this single-maintainer repo the bypass is unreachable** (GitHub forbids self-approval) — verified empirically with PR #814.
- `enforce_admins:true` (branch protection, `strict:true`) guarantees admins also blocked. Documented here and in `pr-validation.yml` comment.

### Drain contract (opening the freeze)

Setting `FREEZE_FEATURES="false"` is NOT a bare toggle. It REQUIRES all three, in the same batch:

1. **Linked issue** documenting why the drain is open and what it drains.
2. **Hard deadline**: set `FREEZE_DRAIN_UNTIL` (ISO date `YYYY-MM-DD`) in the workflow `env`. Empty = no active drain.
3. **Closing revert PR** restoring `"true"`, created before or with the drain-opening commit.

Enforcement is fail-closed (#820/#821): once today's UTC date passes `FREEZE_DRAIN_UNTIL`, **every** PR fails `Validate PR metadata` until the flag is restored or the deadline is deliberately extended in a new commit. A forgotten drain cannot silently disable Gate 0.

> ⚠️ **Gotcha:** for `pull_request` events GitHub evaluates the workflow file from the PR's own merge ref, not main's. A batch PR that *contains* `FREEZE_FEATURES="false"` passes its own Gate 0 validation even with a `type:feature` label. This is how drain batches merge — and why the closing revert must exist as its own tracked step.

### StateStore resume contract

- `ExportState { version:1 }` — `#[serde(default="default_version")] pub version:u32`, `default_version()->1`, `new()` sets `1` (`crates/webfang_core/src/domain/entities/export.rs`).
- `StateStore::load_or_default()` (`crates/webfang_core/src/infrastructure/export/state_store.rs: CURRENT_VERSION=1`): stale `version !=1` → `tracing::info!(version, domain)` + fresh `ExportState::new(domain)`; `NotFound` → fresh; corrupt JSON (Serialization) → propagate → `filter_processed_urls` logs via `log_scrape_error` and returns all URLs (re-scrape, no hard error).
- Legacy JSON missing `version` deserializes to `1` via `default_version` (no crash).
- `CrawlCheckpoint` (JSON+CRC32, `checkpoint_interval=100`) is **out-of-scope**: engine-internal, not wired to `--resume`. Checkpoints viejos se invalidan en v-next por `version` mismatch — recrea estado sin crash.
- See `COMPATIBILITY-MATRIX.md` and `docs/test-inventory.md`.
