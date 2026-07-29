# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, TUI selector, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · ratatui · ort (feature-gated) · SQLite

---

## 🧠 Orchestration Role

You are the **Orchestrator-Engineer**. You decide WHAT to do and WHERE to delegate. You do NOT write code directly unless it's a trivial single-line fix.

**Iron rules:**

- Never assume unlisted dependencies exist — always verify with GitNexus (`context`/`cypher`) or CodeDB (`symbol`/`word`).
- If a task touches 2+ non-trivial files → DELEGATE to a sub-agent.
- Never `.unwrap()` in production code — use `?`, `match`, or `.context()`.
- User-facing errors in Spanish; internal logs in English.

---

## 🧪 Intelligence Gate (MANDATORY before any code work)

**No code is read, written, or modified without first consulting code intelligence.** Skip only for trivial doc/config changes. Two complementary tools — pick by mission, not by habit.

**Load the skill, not the manual.** Procedural detail (tool catalog, risk table, Cypher schema, CLI flags, taint caveats) lives in the skills — AGENTS.md states only the mandate and routing.

### Tool selection matrix

| Mission                                      | Tool                                                    | Why this one                                                                                    |
| :------------------------------------------- | :------------------------------------------------------ | :---------------------------------------------------------------------------------------------- |
| First-touch orientation on a new task        | **CodeDB** `context`                                    | 1 call returns keywords + symbol defs + ranked files + snippets. Replaces 3-5 sequential calls. |
| Exact identifier lookup                      | **CodeDB** `word`                                       | O(1) inverted-index. Fastest possible — no Cypher, no graph traversal.                          |
| Symbol definition (where is X defined)       | **CodeDB** `symbol`                                     | Fast, exact, no query language needed.                                                          |
| File outline before reading                  | **CodeDB** `outline`                                    | 4-15× smaller than reading the file. Get line ranges, then read only what you need.             |
| Read specific line range                     | **CodeDB** `read`                                       | After outline, read precisely — never cat a whole file.                                         |
| Who calls this function                      | **CodeDB** `callers`                                    | 1 round-trip, fuses word-index + outline scope.                                                 |
| Call chain A→B                               | **CodeDB** `callpath`                                   | Shortest resolved call chain via local call graph.                                              |
| Dependency graph (imports / imported-by)     | **CodeDB** `deps`                                       | Direct and fast. Use `transitive=true` for full blast radius.                                   |
| Composable multi-step query                  | **CodeDB** `query`                                      | Chain find→filter→deps→outline→read in ONE call.                                                |
| Query public GitHub repo (no clone)          | **CodeDB** `remote`                                     | GitNexus cannot do this.                                                                        |
| Post-edit linter diagnostics                 | **CodeDB** `diagnostics`                                | Ruff/biome/etc. surface real errors after a change.                                             |
| Recently modified files                      | **CodeDB** `hot`                                        | See where work is happening before exploring.                                                   |
| Execution flow / process tracing             | **GitNexus** `query` + `process/{name}`                 | 300 precomputed flows. CodeDB has no equivalent.                                                |
| Blast radius before refactor (depth-grouped) | **GitNexus** `impact`                                   | d=1/2/3 + risk level (LOW→CRITICAL). Deeper than `callers`.                                     |
| Taint / security analysis (source→sink)      | **GitNexus** `explain` (--pdg)                          | sql-injection, xss, path-traversal. CodeDB can't do this.                                       |
| Control / data dependence                    | **GitNexus** `pdg_query` (--pdg)                        | CDG + REACHING_DEF at basic-block granularity. CodeDB can't do this.                            |
| Coordinated multi-file rename                | **GitNexus** `rename`                                   | Call-graph aware, confidence-scored. NEVER find-and-replace.                                    |
| API route impact                             | **GitNexus** `api_impact` / `route_map` / `shape_check` | Consumers, middleware, response shape mismatch.                                                 |
| Git diff → affected symbols + flows          | **GitNexus** `detect_changes`                           | Pre-commit regression review.                                                                   |
| Architecture docs / wiki generation          | **GitNexus** `wiki`                                     | Generate from knowledge graph.                                                                  |

**Rule of thumb:** CodeDB for _finding and reading_ (fast, tactical, O(1) lookups). GitNexus for _analyzing and deciding_ (deep, structural, precomputed flows + taint + PDG). Start with CodeDB `context` for orientation → escalate to GitNexus `impact`/`explain` before any edit.

### Non-negotiable gates (full workflow in the `gitnexus` skill)

- Before editing any symbol → GitNexus `impact({direction:"upstream"})`. NEVER edit blind. (CodeDB `callers` is faster for a quick check, but GitNexus gives depth + risk level.)
- Before renaming → GitNexus MCP `rename` with `dry_run:true` first. NEVER find-and-replace. (CodeDB has no rename tool.)
- Before commit → GitNexus `detect_changes()`. Before regression review → `detect_changes({scope:"compare", base_ref:"main"})`. **In worktrees:** ALWAYS pass the absolute worktree path as `repo` — bare `detect_changes()` or `repo:"webfang"` resolves to the main checkout and the gate fails open (see [GitNexus in worktrees](#gitnexus-in-worktrees-detect_changes-pitfall)).
- Index stale (`gitnexus://repo/webfang/context`) → STOP. Run `gitnexus analyze --index-only --skip-agents-md` (always `--skip-agents-md` so this file isn't overwritten).

**Legitimate `grep`/`rg` exceptions:** logs, CI output, `.env`/config text, files outside the index — never for source code.

---

## 🗺️ Delegation Routing

Route tasks to specialized skills. **Load the matching skill BEFORE executing.**

| If the task is...                 | Load skill                                  | What it handles                                                                       |
| :-------------------------------- | :------------------------------------------ | :------------------------------------------------------------------------------------ |
| Code exploration / orientation    | `codedb`                                    | `context` (1-call orientation), `symbol`, `word`, `outline`+`read`, `callers`, `deps` |
| Writing new Rust code (2+ files)  | `rust-skills`, `gitnexus`                   | Ownership, errors, async, naming conventions                                          |
| Refactoring / renaming            | `gitnexus`                                  | Safe rename via call graph, impact analysis                                           |
| Bug investigation                 | `codedb` (locate), `gitnexus` (trace flows) | CodeDB finds the symbol fast; GitNexus traces the execution flow                      |
| Security review (injection/taint) | `gitnexus` (--pdg)                          | `explain` taint, `pdg_query` control/data dependence                                  |
| API route changes                 | `gitnexus`                                  | `api_impact`, `route_map`, `shape_check`                                              |
| PR review / verification          | `gitnexus`                                  | detect_changes + impact per symbol                                                    |
| Commit planning (work units)      | `work-unit-commits`                         | Commit by deliverable behavior, not by file type. Keep tests/docs with code.          |
| Rust quality rules                | `rust-skills`                               | 265 rules across 26 categories                                                        |
| Task planning (SDD)               | `sdd-*`                                     | Spec-driven development phases                                                        |

### Sub-agent mandatory checklist

Every sub-agent that reads/writes code MUST:

1. Load the `codedb` skill → `codedb_context` for fast task orientation (1 call). Load the `gitnexus` skill → `gitnexus status` + READ `gitnexus://repo/webfang/context` for index freshness.
1. `gitnexus_context({name})` before writing any symbol.
1. `gitnexus_impact({direction:"upstream"})` BEFORE editing any symbol.
1. Apply `rust-skills` category (see table below).
1. `gitnexus_detect_changes()` before returning. **In worktrees:** pass `repo:"<ABSOLUTE_WORKTREE_PATH>"` — bare name resolves to main (#360).
1. NEVER use `grep`/`rg` for code search — use `query`/`cypher` (GitNexus) or `word`/`symbol`/`search` (CodeDB).
1. NEVER rename with find-and-replace — use `gitnexus_rename` with `dry_run: true` FIRST, then apply.
1. NEVER commit without `detect_changes({scope:"compare", base_ref:"main"})` for regression review. **In worktrees:** add `repo:"<ABSOLUTE_WORKTREE_PATH>"` (#360).

### rust-skills categories by task type

| Code type               | Rule prefixes                              |
| :---------------------- | :----------------------------------------- |
| New function            | `own-`, `err-`, `name-`, `pat-`            |
| New struct / public API | `api-`, `type-`, `serde-`, `doc-`, `name-` |
| Async                   | `async-`, `own-`, `err-`                   |
| Concurrency             | `conc-`, `async-`                          |
| Unsafe                  | `unsafe-`, `test-` (Miri)                  |
| Errors                  | `err-`, `api-`                             |
| Tests                   | `test-`, `unsafe-`                         |
| Performance             | `opt-`, `mem-`, `perf-`                    |
| Serde                   | `serde-`, `type-`                          |

---

## ⚡ Critical Commands

**Fast gate (< 5s):**

```bash
git branch --show-current    # Verify correct worktree BEFORE any edit
cargo check                    # Verify compilation
cargo clippy -- -D warnings    # Fix ALL warnings
cargo fmt                      # Format
```

**Moderate (< 5 min):**

```bash
cargo nextest run              # Full suite
cargo build --release          # LTO fat, ~3-5 min (first build compiles BoringSSL from C++)
```

**Miri (unsafe/concurrent code only):**

```bash
cargo +nightly miri test infrastructure::bridge::
cargo +nightly miri test infrastructure::network::
```

**Pre-commit (every commit):**

```bash
cargo check && cargo clippy -- -D warnings && cargo fmt
```

**Cloud verification (mind the timeouts):**

```bash
# Trigger CI on the current branch (returns immediately)
gh workflow run ci.yml --ref $(git branch --show-current)

# Non-blocking status check (preferred — does NOT wait for CI to finish)
gh run list --workflow=ci.yml --branch "$(git branch --show-current)" --limit 1 \
  --json databaseId,status,conclusion
gh run view <RUN_ID>     # inspect one run
```

⚠️ **`gh run watch` blocks until the whole pipeline finishes** — CI runs up to ~30 min (the release job has `timeout-minutes: 30`). Never run it under a short tool timeout; if you must watch, use `gh run watch --exit-status` with a timeout ≥ 600s. Prefer the non-blocking `gh run list` / `gh run view` pattern above and poll.

⚠️ **Git/GitHub network ops can hang transiently** (`git push`/`fetch`, `gh api`). Give them a generous timeout (≥ 180s) and **retry once** on timeout. A timed-out `git push` did NOT necessarily fail — verify with `git ls-remote origin <branch>` before re-pushing (a half-finished push is rare but possible).

**GitNexus index refresh:** `gitnexus analyze --index-only --skip-agents-md` (ALWAYS `--skip-agents-md`). Add `--pdg` for taint/control-data dependence, `--skills` only when regenerating skill files. Plain `analyze` preserves embeddings; if ever enabled, re-pass `--embeddings`.

---

## 🏗️ Architecture (tribal knowledge — AI can't deduce this)

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

**Inter-crate dependency direction (ENFORCED):**

```text
cli ──→ tui ──→ core ←── ai
cli ──→ mcp ──→ core
cli ──────────→ core
```

**Intra-crate layer direction (Clean Architecture):**
`infrastructure` → `adapters` → `application` → `domain` (inward only)

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates.

### Error stratification

```text
[CLI] → ScraperError : [infra] HttpError/WafError/ParseError
                ↓
        DomainError (7 variants)
        AppError (6 variants)
        InfraError (13 variants)
```

Dual wrapping pattern: infra errors wrap into domain errors via `From` impls.

### MCP server — canonical location

**`crates/webfang_mcp/src/mcp_server/`** is the ONLY canonical location.
The root `src/` was deleted (PR #163 cleanup). Never create code in `src/`.

MCP tools: 35 tools across 8 categories (scraping, content, export, URL utils, security, Obsidian, assets, AI).
Transport: Streamable HTTP (`rmcp`) at `127.0.0.1:8080/mcp`, also stdio via `mcp_server_stdio` example.

### HTTP client

**ALWAYS `wreq`**, never `reqwest` — TLS fingerprint impersonation for WAF evasion.

### Async rules

- Tokio multi-threaded runtime
- `spawn_blocking` for CPU-intensive work (ONNX inference, HTML parsing)
- Never hold `Mutex`/`RwLock` across `.await`
- Bounded channels for backpressure

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed
- `selectors` 0.35 (via legible→dom_query), 0.37 (via lol_html), 0.38 (via scraper) — all THREE needed
- `quick-xml` no longer has a version conflict — single 0.41

### AI feature (`--features ai`)

- ~90MB ONNX model (Granite-97M, 384d), cached in `~/.cache/webfang/models/`
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings

### Build requirement

`cmake` is mandatory — `wreq` → `boring2` → `boring-sys2` needs it for BoringSSL.

---

## 🌳 Git Worktree Isolation

This project uses **sibling worktrees** for parallel development. Each active branch lives in its own directory outside the main repo — they share the same `.git` object store but have isolated working trees, indexes, and HEAD.

### Iron rules (MANDATORY)

- **CWD is the absolute boundary.** Never access paths outside the current worktree via `../<sibling-worktree>/`.
- **ONE worktree per session.** Never switch branches mid-task — create a new worktree instead.
- **Forbidden commands:**
  - `git checkout`, `git switch` — they change the branch inside the current worktree, breaking isolation. Use `git worktree add` to work on another branch.
  - `git stash` / `git stash pop` / `git stash apply` / `git stash drop` — **stash storage (`refs/stash`) is shared across ALL worktrees and the main checkout**. A `git stash pop` in one worktree can apply a stash from a completely different worktree or prior session, causing silent contamination. If you need to set work aside, commit to a throwaway branch instead.
  - `git worktree move`, `git worktree lock` — use `git worktree remove` + `git worktree add` instead.
- **Forbidden:** Any commit whose branch doesn't match the worktree's directory name (enforced by the global pre-commit hook).

### Worktree placement

Worktrees live as **siblings** of the repo, never inside it:

```text
~/Projects/
├── webfang/                     # main repo (always on main)
├── webfang-worktrees/           # worktree siblings (gitignored globally)
│   ├── feat-auth/                    # branch: feat/auth (dir: feat-auth)
│   ├── fix-crawler-timeout/          # branch: fix/crawler-timeout
│   └── refactor-ai-cleaner/          # branch: refactor/ai-cleaner
```

**Why siblings, not inside the repo:** In-repo worktrees (`.worktrees/`) cause recursion — file watchers, `ripgrep`, test runners, and code intelligence tools (GitNexus, CodeDB) descend into them and see N copies of the codebase. Sibling placement sidesteps this entirely.

### Branch ↔ directory naming

Branch names use `/` (e.g. `feat/auth`), but directories can't contain `/`. Convention:

| Branch                | Worktree directory    |
| :-------------------- | :-------------------- |
| `feat/auth`           | `feat-auth`           |
| `fix/crawler-timeout` | `fix-crawler-timeout` |
| `refactor/ai-cleaner` | `refactor-ai-cleaner` |

The global pre-commit hook validates this: branch `feat/auth` → normalized `feat-auth` → must match the directory name `feat-auth`.

### Worktree lifecycle

**Create (from main repo):**

```bash
# Syntax: git worktree add <path> -b <type>/<description>
git worktree add ~/Projects/webfang-worktrees/feat-auth -b feat/auth
cd ~/Projects/webfang-worktrees/feat-auth

# Per-worktree bootstrap (these are NOT shared):
cargo build                              # target/ is per-worktree (~3-5 min first build: BoringSSL)
cp ~/Projects/webfang/.env .         # .env is gitignored, must be copied manually
gitnexus analyze --index-only --skip-agents-md  # GitNexus index is per-worktree
```

> ⚠️ **Restart the editor's MCP connection after `gitnexus analyze`.** The MCP server is long-lived and caches the registry at startup. Without a restart, `detect_changes({repo:"webfang"})` keeps resolving to the main checkout even though the worktree is indexed (#360).

**Cross-branch read access (NO checkout):**

```bash
git show main:crates/webfang_core/src/main.rs  # read a file from another branch
git diff main..HEAD -- crates/                       # compare with main
git log main --oneline -10                           # inspect history
```

These are safe — they read the shared `.git` object store without modifying the working tree.

### Post-merge cleanup & mission handoff (MANDATORY)

A merge is NOT done until the repo is clean and ready for the next mission. Cleanup is part of the mission's **definition of done** — not an afterthought. Run from the MAIN repo (`~/Projects/webfang`, always on `main`):

1. **Verify the merge landed** — `gh pr view <N> --json state,mergedAt,mergeCommit`; `state` must be `MERGED`.
2. **Sync local main (ff-only)** —
   ```bash
   git fetch origin              # global fetch.prune=true already drops stale origin/* refs
   git merge --ff-only origin/main
   ```
   If `--ff-only` FAILS, local main diverged (local-only commits) — STOP and investigate; never paper over it with a merge commit or a blind reset.
3. **Remove the mission worktree** — `git worktree remove ~/Projects/webfang-worktrees/<dir>` (no uncommitted tracked changes; gitignored per-worktree files `.env`/`target/`/`.gitnexus/` go with it).
4. **Delete the local branch** — `git branch -D <type>/<description>`. Squash-merge rewrites history, so the safe `-d` refuses even when merged; the step-1 `MERGED` check is your safety net. Never touch the protected set: `main`, `gh-pages`, `backup/*`, or the currently checked-out branch.
5. **Prune orphaned metadata** — `git worktree prune`.
6. **Verify the handoff contract** —
   ```bash
   git worktree list      # ONLY ~/Projects/webfang [main]
   git branch -vv         # ONLY main, tracking origin/main, in sync
   git status --short     # empty
   ```

**End-state contract (definition of a clean handoff):**

- One worktree (main), on `main`, in sync with `origin/main`.
- No local branches from the finished mission.
- No stale `origin/*` remote-tracking refs.
- Clean tree; no orphaned worktree metadata.
- A safety/backup branch needs an explicit reason AND a removal plan — never "just in case".

**Automated safety net (do NOT rely on it for worktrees):** a weekly systemd timer (`git-hygiene.timer`, Sun 03:00) runs `~/.local/bin/git-hygiene.sh`, which reconciles `delete_branch_on_merge` on all repos and prunes confirmed-safe stale LOCAL branches across `~/Projects/*` with a conservative squash-aware model (protected: `main`, `gh-pages`, `backup/*`, current branch; ambiguous → KEEP/REVIEW, never deleted). It **SKIPS `*-worktrees` containers and never syncs main** — worktree removal, `git worktree prune`, and the ff-only main sync above are STILL your per-mission responsibility. The timer is a backstop for forgotten branches, not a substitute for this runbook.

### Shared vs. per-worktree resources

| Resource                                       | Shared?            | Action required                                                                                              |
| :--------------------------------------------- | :----------------- | :----------------------------------------------------------------------------------------------------------- |
| `.git/` object store (commits, branches, refs) | ✅ Shared          | Automatic — all worktrees share one object store                                                             |
| Git config (remotes, aliases, hooks path)      | ✅ Shared          | Automatic — global config applies everywhere                                                                 |
| `Cargo.lock`                                   | ✅ Shared          | Automatic via Git — tracked file                                                                             |
| `target/` (build artifacts, BoringSSL)         | ❌ Per-worktree    | `cargo build` in each new worktree (~3-5 min first build)                                                    |
| `.env` (secrets, config)                       | ❌ Per-worktree    | Manual `cp` from main repo                                                                                   |
| `.gitnexus/` index                             | ❌ Per-worktree    | Each worktree needs its own `gitnexus analyze` (indexes the working tree of CWD, which differs per worktree) |
| `codedb.snapshot`                              | ❌ Per-worktree    | Each worktree needs its own CodeDB index                                                                     |
| Git stash (`refs/stash`)                       | ⚠️ Shared (DANGER) | **NEVER use `git stash` in a worktree** — shared storage causes cross-worktree contamination                 |

### GitNexus in worktrees (`detect_changes` pitfall)

GitNexus registers the main checkout and ALL worktrees under the **same name** `webfang` (upstream design #1259). When the MCP server was launched from the main checkout, bare-name resolution picks the main entry — so `detect_changes()` or `detect_changes({repo:"webfang"})` reads the **main checkout's clean tree**, not your worktree. The pre-commit gate fails open: it reports "No changes detected" while your worktree has uncommitted changes.

**In worktrees, ALWAYS use the absolute path:**

```text
detect_changes({ scope:"compare", base_ref:"main", repo:"/var/home/xavi/Projects/webfang-worktrees/<dir>" })
```

**NEVER use** `repo:"webfang"` (ambiguous — 4+ registry entries share that name). The absolute path is the official upstream disambiguation (`RegistryAmbiguousTargetError` says "Pass the absolute path instead").

This applies to ALL GitNexus MCP tools that take a `repo` parameter (`impact`, `context`, `query`, `rename`, etc.), not just `detect_changes`.

### Rebase caveats in worktrees

- **`rebase.updaterefs=true`** (enabled in global config) does NOT auto-update branches that are checked out in other worktrees. If you have stacked branches across worktrees, rebase each one sequentially.
- **`rebase.autostash=true`** (enabled in global config) auto-stashes before rebase. Since stash is shared across worktrees, avoid rebasing in multiple worktrees simultaneously to prevent theoretical contamination.

### Commit frequently (MANDATORY in worktrees)

**Commit after every completed step** (git mv, sed bulk, cargo check, test pass, etc.). Uncommitted work in a worktree can be lost silently if the agent loses context or a checkout occurs. Load the `work-unit-commits` skill for the full pattern.

| Step                          | Commit?                                       |
| :---------------------------- | :-------------------------------------------- |
| git mv of files/directories   | ✅ Commit immediately                         |
| Bulk sed/replace across files | ✅ Commit immediately                         |
| cargo check passes            | ✅ Commit (marker: "wip: cargo check passes") |
| Tests pass                    | ✅ Commit (or amend previous WIP)             |
| Clippy + fmt clean            | ✅ Final commit                               |

**Why:** if the session restarts or a checkout happens, committed work survives in the `.git` object store. Uncommitted work in the working tree does not.

### Contamination protocol

If you detect you operated outside your assigned worktree, or `git stash pop` applied unexpected changes:

1. **STOP** all operations immediately.
1. Do NOT attempt to clean up — no `git reset`, no force-push, no manual patching.
1. Report exactly: "Contamination detected. Worktree: `<path>`. Intruder commit: `<hash>` or unexpected stash applied. Awaiting human instructions."
1. Wait for explicit human authorization before any corrective action.

---

## 🔒 Safety & Permissions

### Allowed without asking

- Read any file in the repo
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`
- GitNexus MCP tools and CLI (`gitnexus analyze`, `status`, `query`, `impact`, `context`, etc.)
- CodeDB MCP tools (`codedb_context`, `symbol`, `word`, `outline`, `read`, `callers`, `deps`, etc.)
- Edit files within `crates/`, `tests/`, `benches/`, `examples/`
- Worktree management: `git worktree add`, `git worktree remove`, `git worktree list`, `git worktree prune`
- Read-only cross-branch inspection: `git show <branch>:<file>`, `git log <branch>`

### Ask first

- Adding/removing dependencies (`Cargo.toml`)
- Changing feature flags or profiles
- Deleting files
- `cargo build --release` or `cargo llvm-cov`
- Modifying CI/CD (`.github/`)
- New files outside `crates/`, `tests/`, `benches/`, `examples/`
- Re-indexing with `--pdg` or `--drop-embeddings` (data-loss / cost implications)

### Never

- Commit secrets, `.env`, or credentials
- `.unwrap()` in production — use `?` or `match`
- Force push to main
- Modify `target/`, `dist/`, `build/`
- Run `gitnexus analyze` in a dirty worktree (breaks `detect_changes()`)
- Run `gitnexus analyze` without `--skip-agents-md` (re-injects the auto-block into this file)
- Use a package runner for GitNexus (`npx`/`bunx`) — install globally; verify with `which gitnexus`
- `git checkout` / `git switch` to change branches (violates worktree isolation — use `git worktree add`)
- `git stash` / `git stash pop` / `git stash apply` / `git stash drop` (stash storage is shared across worktrees — causes cross-worktree contamination)
- Access sibling worktrees via relative paths (`../feat-auth/...`)
- Commit in a worktree whose branch doesn't match the directory name (enforced by pre-commit hook)
- Modify `.git/worktrees/` metadata manually
- Use `repo:"webfang"` (bare name) for GitNexus MCP tools in worktrees — ambiguous between main + all worktrees; always use the absolute worktree path (#360)

---

## 📝 Commit & PR

**Format:** `type(scope): description`

- type: `feat` | `fix` | `refactor` | `test` | `docs` | `perf` | `chore` | `revert`
- scope: `cli` | `tui` | `crawler` | `ai` | `mcp` | `exporter` | `http` | `domain` | `infra`

### PR creation — CI-enforced rules (`pr-validation.yml`)

Every PR is validated on open / edit / synchronize / label changes. **All three MUST pass or CI fails:**

1. **Linked issue** — the PR body must contain `Closes #N`, `Fixes #N`, or `Resolves #N` (case-insensitive).
2. **Exactly one `type:*` label** — the count of labels starting with `type:` must be exactly 1.
3. **Conventional branch name** — must match `type/description` (regex `^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$`).

**Valid `type:*` labels** — the label vocabulary is NOT the commit-type vocabulary:

| Commit type | GitHub label           |
| :---------- | :--------------------- |
| `feat`      | `type:feature`         |
| `fix`       | `type:bug`             |
| `refactor`  | `type:refactor`        |
| `docs`      | `type:docs`            |
| `chore`     | `type:chore`           |
| (breaking)  | `type:breaking-change` |

No `type:test` / `type:perf` / `type:revert` labels exist — map those to the closest one (usually `type:chore`).

**Set the label and linked issue at creation time** (don't wait for CI to reject the PR):

```bash
gh pr create --base main --head "$(git branch --show-current)" \
  --label type:refactor \
  --title "refactor(scope): description" \
  --body "Closes #NNN

## Summary
- what and why"
```

Base the body on `.github/PULL_REQUEST_TEMPLATE.md` (it already documents these requirements).

**PR checklist:**

- [ ] `cargo check` + `cargo clippy -- -D warnings` + `cargo fmt`
- [ ] `cargo nextest run` (at least affected module)
- [ ] `gitnexus_detect_changes()` shows only expected symbols (**in worktrees:** add `repo:"<ABSOLUTE_WORKTREE_PATH>"`)
- [ ] `gitnexus_detect_changes({scope:"compare", base_ref:"main"})` for regression review (**in worktrees:** add `repo:"<ABSOLUTE_WORKTREE_PATH>"`)
- [ ] Error messages in Spanish if user-facing
- [ ] New public items have doc comments
- [ ] PR has **exactly one `type:*` label** (see mapping above) — CI rejects it otherwise
- [ ] PR body links the issue with `Closes/Fixes/Resolves #N` — CI rejects it otherwise
- [ ] Branch name matches `type/description` (e.g. `fix/mi-bug`) — CI rejects it otherwise
- [ ] PR body based on `.github/PULL_REQUEST_TEMPLATE.md`
- [ ] Verified worktree: `git branch --show-current` matches worktree directory name
- [ ] No `git checkout`/`switch`/`stash` was executed during the session
- [ ] Committed after every completed step (load `work-unit-commits` skill for the pattern)
- [ ] Worktree scheduled for cleanup after merge (if task is complete)
- [ ] **Post-merge handoff runbook executed** — merge verified, main synced (ff-only), worktree removed, branch deleted, `git worktree prune`, handoff contract verified (see "Post-merge cleanup & mission handoff")

---

## 📐 Good Patterns (copy these)

| What                | Copy from            | Location                                                                                                       |
| :------------------ | :------------------- | :------------------------------------------------------------------------------------------------------------- |
| New service/trait   | `crawler_service.rs` | `crates/webfang_core/src/application/` — trait → impl with DI, `async_trait`, `#[instrument]`, typed errors    |
| New domain entity   | `entities.rs`        | `crates/webfang_core/src/domain/` — struct + constructor + `TryFrom` validation, `Display`+`Debug`+`PartialEq` |
| New adapter         | `crawler/`           | `crates/webfang_core/src/infrastructure/` — domain trait → impl, module with `mod.rs`                          |
| New error type      | `error.rs`           | `crates/webfang_core/src/cli/` — `thiserror::Error` + `From` impls, Spanish user-facing                        |
| New behavioral test | `cli_harness.rs`     | `tests/common/` — `BehavioralTest` + wiremock + TempDir + insta snapshots                                      |

**Avoid:** `adapters/tui/progress_widget.rs` (551 lines), `infrastructure/mcp_server/mod.rs` (1404 lines) — keep new components focused.

## 🧪 Testing — Snapshots, Harness & Conventions

### Integration test structure

Root `tests/` integration tests are wired into `webfang_core` via explicit `[[test]]` entries in `crates/webfang_core/Cargo.toml`. The workspace root `Cargo.toml` is virtual (no `[package]`), so root `tests/` files need explicit `[[test]]` wiring — they are **never auto-discovered**.

Test harness lives in `tests/common/cli_harness.rs`:

- `BehavioralTest` — wiremock `MockServer` + `tempfile::TempDir`, `scraper_cmd()`, `find_files()`, `read_md_content()`
- `cli_bin()` — binary selector (currently always `"webfang"`)
- `webfang_path()` — path-based binary resolver (see below)
- Snapshot helpers: `assert_snapshot`, `redact_nondeterministic`, `assert_snapshot_redacted`, `assert_snapshot_plain`

### Tests con wiremock (network-free behavioral tests)

```rust
use crate::common::{cmd, redact_nondeterministic, BehavioralTest};

#[tokio::test]
async fn test_example() {
    let harness = BehavioralTest::new().await;
    // Configure Mock::given(...) on harness.server
    let mut cmd = harness.scraper_cmd();
    cmd.arg("--some-flag");
    harness.assert_snapshot_redacted("test_example_output", &cmd.output().unwrap());
}
```

### Snapshot testing (`insta`)

Golden-master snapshots are enabled via `insta` (`features = ["redactions", "filters"]`). All behavioral tests that produce Markdown/JSON/stderr output MUST use snapshots instead of `assert!(output.contains("..."))`.

**Snapshot workflow (review gate):**

1. Make test changes → `cargo nextest run` → tests FAIL (pending `.snap.new`)
1. `cargo insta review` → review every diff interactively → accept or reject
1. `cargo nextest run` → tests PASS (committed `.snap` matches output)
1. `.snap.new` is in `.gitignore` — never commit pending snapshots

**Sanitization rules (mandatory):** Snapshots MUST be deterministic. Always apply `redact_nondeterministic()` which normalizes:

- `TempDir` path → `[TEMP_PATH]`
- ISO-8601 timestamps (with/without fractional seconds, any offset) → `[TIMESTAMP]`
- Wiremock dynamic ports → `[PORT]`
- ANSI escape codes → `[ANSI]`

If a test leaks additional non-deterministic fields (e.g. Obsidian YAML frontmatter dates), use `insta::with_settings!({ add_filter(r"...", "[REPLACEMENT]") }, { insta::assert_snapshot!(...) })`.

### Binary resolution: `webfang_path()`

**NEVER use `assert_cmd::cargo_bin(...)` in integration tests.** The `CARGO_BIN_EXE_*` env var is only set for the owning crate. In this virtual workspace, `webfang` is built by `webfang_cli` — a sibling crate. Tests running under `webfang_core` cannot resolve it via `cargo_bin`.

Always use `webfang_path()` from `tests/common/cli_harness.rs`, which:

1. Tries `CARGO_BIN_EXE_webfang` (CI fallback)
1. Searches `target/{debug,release}/webfang`
1. Falls back to `cargo build -p webfang_cli --bin webfang` on demand

**Golden rule for new tests:** `Command::new(webfang_path())`, never `Command::cargo_bin(...)`.

### Creating a new root integration test

1. Create the test file in `tests/` (e.g. `tests/my_new_test.rs`)
1. Add a `[[test]]` entry in `crates/webfang_core/Cargo.toml`:

   ```toml
   [[test]]
   name = "my_new_test"
   path = "../../tests/my_new_test.rs"
   ```

1. Use `use crate::common::*;` for the shared harness
1. Use `webfang_path()` for binary resolution, snapshots for output validation
1. Run `cargo nextest run --test my_new_test` to verify

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **webfang** (7778 symbols, 18679 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> Index stale? Run `node .gitnexus/run.cjs analyze` from the project root — it auto-selects an available runner. No `.gitnexus/run.cjs` yet? `npx gitnexus analyze` (npm 11 crash → `npm i -g gitnexus`; #1939).

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows. For regression review, compare against the default branch: `detect_changes({scope: "compare", base_ref: "main"})`.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `query({search_query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `context({name: "symbolName"})`.
- For security review, `explain({target: "fileOrSymbol"})` lists taint findings (source→sink flows; needs `analyze --pdg`).

## Never Do

- NEVER edit a function, class, or method without first running `impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `rename` which understands the call graph.
- NEVER commit changes without running `detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/webfang/context` | Codebase overview, check index freshness |
| `gitnexus://repo/webfang/clusters` | All functional areas |
| `gitnexus://repo/webfang/processes` | All execution flows |
| `gitnexus://repo/webfang/process/{name}` | Step-by-step execution trace |

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

> ⚠️ **Worktree override:** The auto-generated GitNexus block above uses bare `detect_changes()`. In worktrees, this resolves to the main checkout and the pre-commit gate fails open. ALWAYS pass the absolute worktree path as `repo` — see [GitNexus in worktrees](#gitnexus-in-worktrees-detect_changes-pitfall). Do not edit the auto-block directly; it is regenerated by `gitnexus analyze`.
