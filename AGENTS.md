# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · ort (feature-gated) · SQLite

---

## 🧠 Orchestration & Delegation Methodology

You are the **Orchestrator-Engineer**. You decide WHAT to do, WHERE to delegate, and WHICH tools/skills each agent loads. You do NOT write code directly unless it's a trivial single-line fix.

### Iron rules

- Never assume unlisted dependencies exist — verify with the Intelligence Stack (§2) before any code work.
- If a task touches 2+ non-trivial files → DELEGATE to a sub-agent.
- Never `.unwrap()` in production code — use `?`, `match`, or `.context()`.
- User-facing errors in Spanish; internal logs and tracing fields in English.
- Skip the Intelligence Gate ONLY for trivial doc/config changes.
- rust-analyzer diagnostics are advisory; `cargo check --all-targets --all-features` is the source of truth for E0308/BoxFuture methods — see `crates/webfang_core/src/application/crawler/ports.rs` caveat (#1034).
- Do NOT reintroduce a repo-level rust-analyzer features config (#1094): the dotted `.rust-analyzer.toml` is never read — upstream matches the exact filename `rust-analyzer.toml` (`global_state.rs` name/extension filter) — and `cargo.features` has no effect from any `rust-analyzer.toml` either (upstream rust-lang/rust-analyzer#18114 still open; #18197 was a 2024 flycheck fix, NOT per-file features support, contrary to #1094's original evidence). Verified empirically on rust-analyzer 0.3.3025: only the LSP client config applies features — set `rust-analyzer.cargo.features = "all"` in the editor/client, never in a repo file.

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
| Post-edit diagnostics | `cargo check` / `cargo clippy` | codedb has NO diagnostics tool (removed upstream) — the compiler is the gate (§2.2). |
| Query a public GitHub repo (no clone) | **DeepWiki** MCP (`read_wiki_structure`, `ask_question`) | Separate remote MCP registered by the codedb installer. codedb itself is local-only. |

**Rule of thumb:** CodeDB for *finding and reading* (fast, tactical, O(1)). CodeGraph for *exploring and understanding* (returns source directly, call paths, blast radius).

### 2.2 Non-negotiable gates

- Before editing any symbol → CodeGraph `explore` it (callers + impact). NEVER edit blind.
- Before renaming → check ALL usages first via `codedb_callers` / CodeGraph `explore`.
- Before commit → run `cargo check` + `cargo clippy` + `cargo fmt --all -- --check` and re-read the diff. Bare `cargo fmt` rewrites files and **exits 0 unconditionally** — it is a fixer, never a verification.
- **Legitimate `grep`/`rg` exceptions:** logs, CI output, `.env`/config text, files outside the index — never for source code.

### 2.3 Worktree intelligence — CRITICAL

In worktrees, BOTH tools need the **absolute worktree path** or they silently resolve to the main checkout:

| Tool | Parameter | Example |
| :--- | :--- | :--- |
| CodeDB MCP | `project=` | `project="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |
| CodeGraph MCP | `projectPath=` | `projectPath="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |

**NEVER use** bare project names in worktrees — ambiguous between main + all worktrees (#360). The absolute path is the official upstream disambiguation.

**CodeDB CLI root is POSITIONAL-FIRST:** `codedb <abs-root> <cmd>` (`src/cli_args.zig:parsePositional`, `usage: codedb [root] <command> [args...]`). ONLY `mcp` accepts the reversed order (`codedb mcp <path>`, upstream #503). `codedb status <path>` and `codedb reindex <path>` IGNORE the trailing path with exit 0 and silently operate on the cwd project — always verify with `codedb <abs-root> status`: `root` must equal the worktree path and `head` must equal `git -C <abs-root> rev-parse --short HEAD`.

---

## 💾 Engram Persistent Memory — correct usage (agents)

The engram MCP server resolves the active session from the working directory. With
several concurrent agent sessions on this repo, `mem_save` / `mem_judge` fail with
`multiple active runtime sessions match the current project and directory`:

- **Never guess or invent a `session_id`.** Register one first with `mem_session_start`
  (explicit unique ID, e.g. `opencode-<task>-<YYYYMMDD>`), then pass that same
  `session_id` to every `mem_save` / `mem_judge` call for the rest of the session.
- **Close it when the task ends: `mem_session_end(id, summary)`.** Good hygiene, **not** a
  hard requirement: since engram v2.1.0 every session registration (MCP `mem_session_start`
  and HTTP `POST /sessions` alike) writes a **30-minute `runtime_lease_expires_at`**, and an
  **expired lease is excluded** from candidate resolution. A session whose runtime never
  renews it therefore stops being a candidate **by itself, 30 minutes** after its last
  renewal — no cleanup needed. Closing early only matters while your runtime is still alive.
  The 27 stale sessions (`opencode-*`, `codebuff-*`, `pipeline-*`, `webfang-audit-*`…)
  piled up here **before leases existed** (created 2026-09-19..23, pre-v2.1.0) and were
  closed on 2026-09-26.
- If `mem_save` returns `judgment_required: true`, judge every candidate with its own
  `judgment_id` from `candidates[]` — never the top-level one.
- **Diagnose this specific failure** with
  `engram doctor --check ambiguous_active_runtime_sessions --project webfang`. It is
  diagnostic-only (`doctor repair` does not cover this check): it lists the session IDs
  involved. If the collision is between two sessions that are **both genuinely alive**, that
  is by design — end the finished one, or pass `session_id` (bullet above). Full mechanism in
  the engram session-guard section of the `fedora-maintenance` vault, doc 12.
- **On ANY engram error, run `engram --help` first** (then `engram <cmd> --help` for
  the failing command) before retrying or inventing flags. The CLI is also the
  fallback when MCP fails: `engram save "<title>" "<content>" --project webfang`,
  `engram search <query>`, `engram context [project]`.

---

## 🏗️ Architecture & Code Rules

### Workspace structure (6 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure
│   ├── webfang_ai/               # ONNX embeddings, semantic cleaning
│   ├── webfang_mcp/              # MCP server (36 tools)
│   ├── webfang_cli/              # CLI binary (webfang)
│   └── webfang_test_utils/       # shared test utilities (not shipped)
│   └── webfang_benchmark/        # public benchmark harness (tooling leaf)
```

### Inter-crate dependency direction (ENFORCED POLICY)

```text
ai ───→ core
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
| `webfang_mcp` | `webfang_core`, `webfang_ai` |
| `webfang_cli` | `webfang_core`, `webfang_ai`, `webfang_mcp` |
| `webfang_test_utils` | `webfang_core` (leaf; test harness, `publish = false`) |
| `webfang_benchmark` | `webfang_core`, `webfang_test_utils` (leaf; benchmark tooling, no production dependents) |

This is an architectural POLICY, not just what the code happens to do. New code must respect this direction. Verify cross-crate usage with `codedb_deps` or CodeGraph `explore` before adding any inter-crate import.

**Dev tier (#1825):** any crate may target `webfang_test_utils` from `[dev-dependencies]` — it is the shared test harness and never ships. A `[dependencies]` edge into it remains prohibited. The gate extractor also recognizes the dotted-key form (`webfang_core.workspace = true`).

**CI gate (#513):** `scripts/check_dependency_direction.sh` runs in the `repo-guards` job of `ci.yml` (ci.yml:225-226) and fails on any prohibited inter-crate dependency (including feature-gated optional deps). It parses each crate's `Cargo.toml` `[dependencies]`/`[dev-dependencies]` against the matrix above and prints the effective graph on success. A semantics harness (`scripts/test_dependency_direction.sh`) runs as the next step and fails on any fixture where a forbidden edge passes or a documented edge is rejected (#1825). Keep the matrix in the script and this section in sync.

### Intra-crate layers (Clean Architecture)

`infrastructure` → `adapters` → `application` → `domain` (inward only)

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates. When writing new code, follow the existing patterns:

| Writing a... | Copy from | Location |
| :--- | :--- | :--- |
| New service/trait | `crawler/engine.rs` | `application/` — trait → impl with DI, `async_trait`, `#[instrument]`, typed errors |
| New domain entity | `entities.rs` | `domain/` — struct + constructor + `TryFrom` validation, `Display`+`Debug`+`PartialEq` |
| New adapter | `crawler/` | `infrastructure/` — domain trait → impl, module with `mod.rs` |
| New error type | `error.rs` | `cli/` — `thiserror::Error` + `From` impls, Spanish user-facing |
| New behavioral test | `cli_harness.rs` | `crates/webfang_core/tests/common/` — `BehavioralTest` + wiremock + TempDir + insta snapshots |

**Avoid:** oversized components such as `infrastructure/mcp_server/mod.rs` (1404 lines) — keep new components focused.

### Error stratification

```text
[CLI] → ScraperError : [infra] HttpError/WafError/ParseError
                ↓
        DomainError (7 variants)
        InfraError (13 variants)
```

Dual wrapping pattern: infra errors wrap into domain errors via `From` impls. New error variants MUST follow this chain — never bubble raw infrastructure errors to the CLI layer.

### HTTP client

**ALWAYS `wreq`**, never `reqwest` — TLS fingerprint impersonation for WAF evasion. This is non-negotiable. An agent suggesting `reqwest` as an alternative is wrong.

### Fetch guard-chain (MANDATORY order)

Every fetch path — CLI, engine, MCP tool, benchmark, future adapter — must wire the protections in this exact order. Each stage assumes the previous one filtered: deviating from the order is a **bug, not a style choice** (e.g. pacing *after* a doomed request burns rate budget on something SSRF would have rejected; classifying retries *after* reading the body wastes a full read on a response that will never be accepted).

```text
ValidUrl (entry) → rate limit (pre-fetch pacing) → per-attempt request:
    timeout → redirect policy → SSRF at socket dial (every hop)
→ retry classification → body read
```

| Stage | Reference implementation | Rationale |
| :--- | :--- | :--- |
| 1. Entry validation | `ValidUrl` (`domain/value_objects.rs`), built via `try_from_url` at the argv boundary (#1240, P0) | Untrusted URL text becomes a validated domain type before entering any flow. Never accept a raw `Url`/`String` as a fetch target. |
| 2. Rate limit (pacing) | Scrape path: token bucket in `scrape_urls` (`cli/scrape_flow.rs` — lands with PR #1249, tracking #1255: cancel-aware `until_ready_or_cancel`, zero overhead at `delay_ms = 0`). Crawl path: `rate_limiter_config` (`crawler/engine.rs:152`) from `delay_ms` + budget-tier burst. | The wait happens BEFORE the network is touched — a cancelled wait counts as skipped (#509), never as a blocked fetch. |
| 3a. Timeout | `Client` builder (`downloader/wreq_downloader.rs:169-170`): request timeout + connect timeout (connect is clamped) | Per-request ceiling; see stage 4 for why a timeout is terminal, not retriable. |
| 3b. Redirect policy | `redirect_policy` (`domain/ssrf_guard.rs`): 10-hop limit + synchronous stop on redirects whose target is a literal forbidden IP | Every redirect hop re-enters the chain — it is a new dial, not a free pass. |
| 3c. SSRF at socket dial | `ssrf_guard()` wrapping the `Client` (`wreq_downloader.rs:180`, `domain/ssrf_guard.rs`): resolver-level check on EVERY connection — private/link-local/loopback ranges rejected before the socket exists | Nothing may connect to a forbidden address, including via DNS names and redirect hops (E2E-proven: `crates/webfang_core/tests/ssrf_rfc1918_e2e_test.rs`, PR #1251 — pre-socket rejection of RFC1918/CGNAT/NAT64/mapped targets, exit 69, zero outbound packets). |
| 4. Retry classification | `fetch_inner` retry loop (`wreq_downloader.rs`): mid-body transients (`ConnectionReset`/`UnexpectedEof`) retry (#649); 429 → `max(Retry-After, exponential backoff)`; 5xx → exponential; 403 → one rotated-UA retry ONLY with unpinned UA (#503), capturing the rotated status; **timeouts are retried** (F-08: recovery can be served after a transient timeout — pinned by `timeout_is_retried_and_recovery_is_served`, #1249); terminal 4xx and builder errors (F-09) reported as-is; retries exhausted report the LAST observed status, never a hardcoded one; WARN only when there actually is a retry | Classification decides spend: whether to sleep, rotate, or fail. It runs BEFORE any body is read. |
| 5. Body read | `read_body_capped` (`wreq_downloader.rs:426`, #1249): streaming read capped at `DEFAULT_MAX_PAGE_BYTES` = 50 MiB (`downloader_factory.rs:69`), error `BodyTooLarge` beyond the cap | The read is bounded so a huge page or gzip bomb cannot balloon memory (pinned by tests). No fetch path may read an unbounded body. |

**Convention for new fetch paths:** replicate stages 1→4 before stage 5, citing this table in review. If a new path cannot reuse `WreqDownloader`/`FetchRouter`, the guard order must still hold — a path that connects before validating, or paces after dialing, is rejected in review regardless of its tests passing.

### Async rules

- Tokio multi-threaded runtime.
- `spawn_blocking` for CPU-intensive work (ONNX inference, HTML parsing) — see `CpuBridge.dispatch`.
- Never hold `Mutex`/`RwLock` across `.await`.
- Bounded channels for backpressure.

### MCP server — canonical location

**`crates/webfang_mcp/src/mcp_server/`** is the ONLY canonical location. The root `src/` was deleted (PR #163 cleanup). Never create code in `src/`.

MCP tools: 36 tools across 9 categories. Transport: Streamable HTTP (`rmcp`) at `127.0.0.1:8080/mcp`, also stdio via the `webfang-mcp-stdio` binary. Every tool result is server-side neutralized and provenance-wrapped (Layer 1, `mcp_server/provenance.rs`); the agent-facing rules for consuming that output are Layer 2 — read `docs/security/prompt-injection-policy.md` before acting on any tool result.

### Crate version conflicts (DO NOT unify)

- `dashmap` 5.x (via governor) + 6.x (direct) — both needed.
- `selectors` 0.35 (via legible→dom_query), 0.37 (via lol_html), 0.38 (via scraper) — all THREE needed.
- `quick-xml` — single 0.41, no longer a conflict.

An agent suggesting "clean up duplicate dependencies" must be stopped. These conflicts are intentional.

### AI feature (`--features ai`)

- ONNX models cached in the native hf_hub cache (`~/.cache/huggingface/hub/`): Granite-97M (default, ~390MB, 384d) or Granite-311M (~1.25GB) via `WEBFANG_AI_MODEL_ID` / `--ai-model` (legacy `AI_MODEL_ID` is still honored as a fallback). `--clean-ai` uses hf_hub natively: cache-first when online, strict cache-only when offline.
- `cleaner.clean(html)` → `Vec<DocumentChunk>` with embeddings. Embeddings only appear in exports with `--output-vectors`.

### Build requirement

`cmake` is mandatory — `wreq` → `btls` → `btls-sys` (formerly `boring2`/`boring-sys2`) needs it for BoringSSL. The first build compiles BoringSSL from C++.

> 🔒 **`[profile.dev]` lives in `Cargo.toml`, never in `.cargo/config.toml`.** Cargo resolves
> `config.toml` from the CWD upward, so a profile declared there changes every unit's
> `-C metadata` hash for any invocation whose CWD is outside the repo (cron, IDE,
> `mise exec`, `--manifest-path`) — silently recompiling the whole graph into the target
> dir. Verified 2026-09-29: after the move, `cargo check --workspace` is a 0.27 s no-op
> in-repo and 0.25 s from `/tmp`. Because `boring-sys2` sits at the bottom of the graph, each
> such rehash is amplified into a full 639-object BoringSSL rebuild — `main`'s target dir
> had accumulated 202 of them before it moved to a seeded target of its own. Do not "tidy" this key back into
> `.cargo/config.toml`. One-off full debuginfo stays per-invocation:
> `CARGO_PROFILE_DEV_DEBUG=true cargo build`.

> ⏱️ **Measured cost, do not inflate it (2026-09-16, 16-core workstation, ccache active via
> `/usr/lib64/ccache/cc`, warm registry, `--offline`, dev profile with
> `debug = "line-tables-only"`):** a cold `cargo build -p webfang_core` in a virgin target
> dir finishes in **85 s**, and that window contains BoringSSL's whole C++ compile (639
> objects, 96 MB of `.o` under `.../btls-sys-*/out/`). A cold `cargo build --workspace` is
> **2 m 23 s** (430 units, 3.1 GB). So BoringSSL is tens of seconds, not "3-5 min", on a
> developer machine. The minutes-scale figures still alive in `.github/workflows/`
> (sanitizers.yml ~10 min, benches.yml and mutants.yml ~10-15 min) describe **GitHub
> runners with no ccache and, for sanitizers, std rebuilt from source** — they are not a
> local baseline, and neither is this one. Quote the context, never the number alone.

---

## 🧪 Testing Methodology

### Framework & harness

Integration tests live in `crates/webfang_core/tests/` and are auto-discovered by Cargo by default — one file per test target. Explicit `[[test]]` entries in `crates/webfang_core/Cargo.toml` exist only for cases auto-discovery does not cover (subdirectories such as `tests/compile_fail/`, special features).

Test harness lives in `crates/webfang_core/tests/common/cli_harness.rs`:

- `BehavioralTest` — wiremock `MockServer` + `tempfile::TempDir`, `scraper_cmd()`, `find_files()`, `read_md_content()`.
- Snapshot helpers: `assert_snapshot`, `redact_nondeterministic`, `assert_snapshot_redacted`, `assert_snapshot_plain`.

Import it via `#[path = "common/mod.rs"] mod common;` + `use common::cli_harness::{...}` (see `tests/crash_matrix_test.rs`).

### Binary resolution: `webfang_path()`

**NEVER use `assert_cmd::cargo_bin(...)` in integration tests.** The `CARGO_BIN_EXE_*` env var is only set for the owning crate. In this virtual workspace, `webfang` is built by `webfang_cli` — a sibling crate. Tests running under `webfang_core` cannot resolve it via `cargo_bin`.

Always use `webfang_path()` from `crates/webfang_core/tests/common/cli_harness.rs`. **Golden rule:** `Command::new(webfang_path())`, never `Command::cargo_bin(...)`.

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

### Creating a new integration test

1. Create the test file in `crates/webfang_core/tests/` — no `[[test]]` entry needed (Cargo auto-discovers it).
2. Only for cases auto-discovery does not cover (subdirectory, special features), add a `[[test]]` entry in `crates/webfang_core/Cargo.toml`: `name = "my_test"`, `path = "tests/compile_fail/my_test.rs"`.
3. Import the shared harness via `#[path = "common/mod.rs"] mod common;` + `use common::cli_harness::{...}`, use `webfang_path()` for binary resolution, snapshots for output validation.
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
- **Correlation:** every event/span carries a top-level `trace_id` equal to the root span Id (16-hex, **ephemeral identity-within-run** — do not persist it or join across runs; the durable cross-run identity is the `CorrelationId` UUID). Reconstruct a whole run with `ROOT=<16-hex root>; jq -c 'select(.trace_id == "$ROOT")' <file>` (top-level, NOT `.fields.trace_id` — that inner field is only populated on the error path). Note `span_close` records share `.span` names, so counts/stage queries must exclude `.record == "span_close"`.
- **User-facing errors in Spanish; tracing fields/logs in English.**
- **No new metrics backends:** do not reintroduce OpenTelemetry or any external collector. Emit a structured tracing event and query it from the JSONL.
- **Snapshots stay deterministic:** `correlation_id`/`trace_id` are internal and `#[serde(skip)]` on scraped output; redact via `redact_nondeterministic()`.

See `docs/src/debugging.md` and `scripts/analyze-trace.sh` for the full query cookbook. The observability module lives in `crates/webfang_core/src/infrastructure/observability/`.

---

## 🌳 Git Worktree Isolation

This project uses **sibling worktrees** for parallel development. Each active branch lives in its own directory outside the main repo — shared `.git` object store, isolated working trees, indexes, and HEAD.

### Iron rules (MANDATORY)

- **CWD is the absolute boundary.** Never access paths outside the current worktree via `../<sibling-worktree>/`.
- **ONE worktree per session.** Never switch branches mid-task — create a new worktree instead.
- **`.git/worktrees/` is Git's internal state.** Never create, edit, or delete entries there by hand — use `git worktree add/remove/prune/repair`.
- **Forbidden commands:**
  - `git checkout`, `git switch` — they change the branch inside the current worktree. Use `git worktree add`. **Since git 2.44 this is enforced upstream, not only by local policy**: `git checkout -B <branch>` refuses a branch that is in use in another worktree (`fatal: '<branch>' is already used by worktree at …`, exit 128), and upstream marks it a breaking change — `-B` used to override the guard "by mistake". The escape hatch is `git checkout --ignore-other-worktrees -B <branch>` (exit 0), and **the flag must come before `-B`**: placed after `-B` it is parsed as the refspec and the command still fails with exit 128. So all three forms, on a branch held by another worktree: bare `checkout -B <branch>` → 128; `checkout -B --ignore-other-worktrees <branch>` → 128; `checkout --ignore-other-worktrees -B <branch>` → 0. Verified on git 2.55.0 against a real linked worktree — note that `git init <dir>` creates an independent repository and does **not** reproduce the guard, so the check must use `git worktree add`. Treat the flag exactly like `--force` below: explicit human authorization only.
  - `git stash` / `git stash pop` / `git stash apply` / `git stash drop` — **stash storage (`refs/stash`) is shared across ALL worktrees**. A `pop` in one worktree can apply a stash from a completely different session. If you need to set work aside, commit to a throwaway branch.
  - `git worktree move`, `git worktree lock` — use `remove` + `add` instead.
  - `git worktree add --force` — it bypasses Git's native guard that refuses a branch already checked out in another worktree. Two agents on the same branch is exactly the failure that guard prevents. Only with explicit human authorization. (`--ignore-other-worktrees` is the same class of escape for `checkout -B`; see above.)

### Placement & naming

Worktrees live as **siblings** of the repo (never inside it — in-repo worktrees cause recursion with file watchers, ripgrep, and code intelligence tools):

```text
~/Projects/Rust/
├── webfang/                     # main repo (always on main)
├── webfang-worktrees/           # worktree siblings (gitignored globally)
│   ├── feat-auth/               # branch: feat/auth
│   └── fix-crawler-timeout/     # branch: fix/crawler-timeout
```

Branch `feat/auth` → directory `feat-auth` (`/` → `-`). The binding invariant is: **every commit must land on the intended branch for the task or PR**. The directory-name proxy is the default verification; run `[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"` before each commit in a worktree. If the proxy reports MISMATCH, do not commit unless the identity-exception protocol below is fully satisfied.

### Identity-exception protocol (proxy failed, invariant still provable)

A failed name proxy is not proof of a wrong worktree — but the local branch name alone is not proof of the right one either. Committing under a directory-name mismatch is allowed only when all three hold:

1. **Externally-anchored expectation.** The intended branch is known from a source fixed outside this worktree before committing: PR metadata (`gh pr view --json headRefOid,headRefName`), task metadata, upstream ref, or explicit human instruction. A branch name that exists only inside the worktree proves nothing.
2. **Verifiable binding.** The worktree is observably attached to that branch: `git symbolic-ref --short HEAD` plus `git worktree list` agree on worktree path → branch. Optional extra signal when the branch has an upstream: `git rev-parse @{u}`.
3. **Ancestry, not equality.** Pre-commit HEAD must be at or descended from the expected tip/base — a strict `HEAD == expected tip` check breaks after the first commit in a sequence, so ancestry is the correct test.

Every exception commit records the evidence in its body (compact trailer):

```
Worktree-Identity-Exception: directory-name mismatch
Intended-Branch: <branch>
Expected-Head: <sha>
Evidence: <external source> + git symbolic-ref + git worktree list
```

If any condition fails, MISMATCH remains a hard stop.

### Worktree lifecycle

**Create (from main repo):**

```bash
git worktree add ~/Projects/Rust/webfang-worktrees/feat-auth -b feat/auth
cd ~/Projects/Rust/webfang-worktrees/feat-auth

# Per-worktree bootstrap (NONE of these are shared), run INSIDE the worktree:
# The worktree's .envrc is WRITTEN FROM ITS OWN NAME, never derived by rewriting
# main's. That used to be a `sed` over main's file, which made main's target name
# a load-bearing input to every future worktree: once main moved off
# `cargo-target/webfang`, the sed stopped matching and emitted a perfectly VALID
# CARGO_TARGET_DIR pointing at MAIN's target. Silent, and only caught later by
# ci_fast_gate.sh refusing the build. A bootstrap that fails quietly into a
# broken isolation policy is worse than one that fails loudly, so the coupling
# is gone rather than updated.
TREE="$(basename "$PWD")"
cat > .envrc <<EOF
export CARGO_TARGET_DIR=$HOME/.cache/cargo-target/$TREE
export CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER
export CARGO_LLVM_COV_TARGET_DIR=$HOME/.cache/cargo-target/$TREE-llvm-cov
PATH_add $HOME/.local/share/mbx/bin
EOF
direnv allow     # gitignored; carries the per-tree cache policy

# mbx (mr-boxington) wraps Cargo so a warm worktree compiles ~40% less. The
# PATH_add line above is not optional, and it must come first: `~/.cargo/bin`
# is ahead of the wrapper by rustup's default, so without it every `cargo`
# invocation resolves to the rustup shim and **silently bypasses mbx**. There
# is no error and no warning — the build looks normal and simply runs cold,
# which is how a bad measurement gets taken. `mise activate bash --shims` also
# puts the wrapper first, but only in login shells; agents, cron and other
# non-login, non-interactive shells never run it, so direnv is the one route
# that covers them all. Scoped per-worktree on purpose: the global mise config
# already routes mise-activated interactive shells, and this keeps the wrapper
# out of unrelated Rust projects. Any fix attempt must be verified with
# `command -v cargo`, never by trusting the install succeeded.

# There is deliberately NO second check here. The only enforcement is
# scripts/ci_fast_gate.sh, and it works by canonical identity, not by name.
# A basename test here was wrong twice over: it would reject a legitimate
# ~/.cache/cargo-target/x/webfang, and it would accept a symlink resolving to
# main's target — which is the exact failure the gate exists to close. The tree
# name above is the bootstrap CONVENTION, not a condition of validity; the
# mandatory condition is that a worktree's target is independent of every other
# tree's, and in particular is neither main's target nor inside the seed store.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:?}"
#   .envrc is the ONLY place this policy can live: mise.toml is byte-identical in
#   every tree, so it cannot tell main from a worktree. Every tree — main
#   included — gets its own CARGO_TARGET_DIR; there is no longer a "shared" target
#   dir in this repo. Every WORKTREE also sets CARGO_INCREMENTAL=0 and unsets the
#   sccache wrapper; main keeps CARGO_INCREMENTAL=1 on measured grounds.
#   The snippet above is documentation, not enforcement: scripts/ci_fast_gate.sh
#   is the check that actually runs, and it fails closed when CARGO_TARGET_DIR
#   is unset in any tree, or when a worktree's CARGO_TARGET_DIR points at main's
#   target.

cp ~/Projects/Rust/webfang/.env .                       # .env is gitignored
codegraph init                                     # CodeGraph: source exploration index
codedb reindex && codedb status                    # CodeDB: root MUST be $PWD, head MUST match git rev-parse --short HEAD
# — same without cd: codedb "$PWD" reindex && codedb "$PWD" status
# Index lives in BOTH ./codedb.snapshot AND ~/.codedb/projects/<hash>/ (see data: in status).

# OPTIONAL, never required. Seeding reuses what a previous build already compiled
# (measured: 162 s cold -> 18 s seeded, for a 0.7 s clone). If there is no
# compatible seed, or the filesystem cannot clone one, it reports `cold` and you
# build normally — see "Seed contract" below.
bash scripts/seed_target.sh

cargo build                                        # cold or seeded; both are correct, and the script says which
```

**During normal bootstrap, never delete, clean, reuse, or repoint a build at anything under `~/.cache/cargo-target/quarantine/`.** Quarantine removal is gated by `scripts/quarantine_age.sh` and fresh ownership/use evidence collected at deletion time; the quarantine runbooks are authoritative for that procedure.

> ⚠️ **`.envrc` + `direnv allow` is mandatory per worktree, and is now ENFORCED.** In a **worktree** it points `CARGO_TARGET_DIR` at a per-tree isolated dir (`~/.cache/cargo-target/<tree-name>`), which is what #1267 requires. The cost of isolation depends on whether a compatible seed exists: with one, a worktree builds in a measured 18 s against 162 s cold, because the 639 BoringSSL objects and the heavy dependencies come from the seed; with no compatible seed it pays the full cold build, measured 2 m 23 s for `cargo build --workspace`. Either way that is cheap enough that it must never be used as an argument to share a target dir between concurrent builds (#1267). **There is no shared target dir in this repo any more.** `main` used to keep one, and that single exception is what let 46 dead worktrees accumulate in a 478 G target dir that Cargo cannot attribute by ownership. `main` now has its own `~/.cache/cargo-target/main`, seeded like any other tree.
>
> `main` is the one tree that keeps `CARGO_INCREMENTAL=1` while the seed contract pins `0`. Measured cost: the first build over a freshly seeded target spends one extra workspace rebuild (20 s) because the incremental flag changes the fingerprints; every build after that is unaffected, and the 669 BoringSSL C++ objects are reused under either setting. The seed contract deliberately does NOT hash the installation paths, so the same compiler in two places stays the same seed.
>
> Without `.envrc`, cargo falls back to an in-repo `target/`. This is no longer silent: `scripts/ci_fast_gate.sh` fails closed with exit 2 when `CARGO_TARGET_DIR` is unset, and names `direnv allow` as the fix. That guard exists because the in-repo fallback is invisible by construction — `.gitignore` has `target`, so a leaked 33 G in-repo target dir leaves `git status` clean and no cleanup step can attribute it. Measured 2026-09-29 on `main`: six such dirs, 39 G logical / 18 G physical on btrfs+zstd.
>
> ⚠️ **A defined `CARGO_TARGET_DIR` is not enough — the gate enforces a worktree target registry (#1679).** The policy lives in `scripts/check_target_isolation.sh`: the gate enumerates ALL live worktrees via `git worktree list --porcelain -z` and reads each tree's `.envrc` declaration (`export CARGO_TARGET_DIR=`, last line wins, with `$HOME`/`~` expansion — sibling `.envrc` files use literal `$HOME/...`) as the ownership registry, comparing everything by canonical path identity. A target owned by ANY other live worktree is hard-rejected with `reason=target-owned-by-other-worktree` and has **no opt-out** — `--allow-unregistered-target` cannot excuse it, because two trees building the same profile into one directory get identical output filenames (#1267: an E2E run silently executes the other tree's binary). A target nobody declares is rejected with `reason=unregistered-target` unless `--allow-unregistered-target` is passed to `scripts/ci_fast_gate.sh` (per-invocation flag; deliberately never an env var, which would propagate exactly the inherited ambient state this guard exists to stop). The old main-only check is subsumed — main is just another registry entry, and an unbootstrapped main still refuses (`reason=main-unbootstrapped`). Trees without `.envrc` (tool-generated detached worktrees) are non-owners and are skipped, so the normal bootstrap-then-gate flow never needs the flag.
>
> ⚠️ **Concurrent agent builds must NOT share a target dir (#1267).** Two worktrees building the same binary profile into one target dir concurrently overwrite each other's `debug/webfang` (same `-C metadata` hash ⇒ same output filename), so E2E runs silently execute the other tree's binary — stale links report as fresh, failures misattribute. Every tree builds into its own target dir. Isolated-build recipe (verified 2026-09-09 after 8 opaque worker deaths): `export CARGO_TARGET_DIR=~/.cache/cargo-target/<worktree>` (home disk — `/tmp` tmpfs is only 16 GB and a full workspace target dir starves both the build and sccache), `env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN` (sccache's Rust cache key embeds the target-dir path, so an identical source in a new isolated dir scores ZERO hits - verified with a private cache and a positive control: same dir hits, different dir misses, leaving duplicate objects for one unit. Raising `SCCACHE_CACHE_SIZE` cannot fix that, it only fits the duplicates; the wrapper also breaks `--json` parsing on isolated dirs; an exported stable toolchain shadows `rust-toolchain.toml` and its rust-lld rejects the BFD-only link flags below — F-52 evidence), `CARGO_BUILD_JOBS=2` plus `RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads"` (test-binary links OOM-die otherwise). The orchestrator assigns the isolated path per gate; workers never invent their own. Step 6 of the post-merge runbook deletes them - measured cost of NOT doing it: 52 GB of dead build state from three already-merged trees, invisible to `git status` and to `git worktree prune`.

> ⚠️ **Without both indexes, the agent is BLIND in the worktree.** Intelligence tools silently resolve to the main checkout or return empty results. Check that `.codegraph/` and `codedb.snapshot` exist.

> ⚠️ **Restart the editor's MCP connection after indexing.** The MCP server caches the registry at startup. Without a restart, tools keep resolving to the main checkout (#360).

**Cross-branch read access (NO checkout):**

```bash
git show main:crates/webfang_core/src/main.rs      # read a file from another branch
git diff main..HEAD -- crates/                     # compare with main
git log main --oneline -10                         # inspect history
```

### Post-merge cleanup & mission handoff (MANDATORY)

A merge is NOT done until the repo is clean and ready for the next mission. Cleanup is part of the **definition of done**. Run from the MAIN repo (`~/Projects/Rust/webfang`, always on `main`):

1. **Verify the merge landed** — `gh pr view <N> --json state,mergedAt,mergeCommit`; `state` must be `MERGED`.
2. **Sync local main (ff-only)** — `git fetch origin && git merge --ff-only origin/main`. If `--ff-only` FAILS, local main diverged — STOP and investigate; never paper over it.
3. **Remove the mission worktree** — `git worktree remove ~/Projects/Rust/webfang-worktrees/<dir>`.
4. **Delete the local branch** — `git branch -D <type>/<description>`. Squash-merge rewrites history, so safe `-d` refuses; the step-1 `MERGED` check is your safety net. Never touch: `main`, `gh-pages`, `backup/*`, or the current branch.
5. **Prune orphaned metadata** - `git worktree prune`.
6. **Delete the mission's isolated target dir** - `rm -rf ~/.cache/cargo-target/<worktree-dir-name>`.
   Git owns nothing here, so neither `worktree remove` nor `prune` touches it, and `git status`
   stays clean while it grows. **The sound safety test is ownership, not exclusion:** delete the
   directory *you* named for the worktree you removed in step 3 - its exact basename, or the custom
   name you exported into `CARGO_TARGET_DIR` yourself. Never enumerate that parent hunting for
   extra candidates, and never treat an unrecognized name as evidence of death: agents name their
   targets freely and git never sees those names, and one worktree can build under several shortened
   names (observed: `fix-preflight-diagnostics-and-stale-lock` has built into both
   `~/.cache/cargo-target/fix-preflight-diagnostics` and `.../fix-preflight-doccheck`, neither of
   which matches any worktree or branch). Re-check right before deleting, with a **moving
   window**: sample the file count twice 60 s apart and require delta 0, plus zero live
   `cargo`/`rustc`/`cargo-nextest` (`pgrep -x`, one name per call - `pgrep -x 'a;b'` never matches).
   The fixed "no writes in the last 30 minutes" test this step used to publish is NOT sufficient: it
   cannot tell a finished build from an agent that is thinking between builds. And absence of a
   branch is *anti-correlated* with being dead - work that has not been pushed yet has no branch
   locally or on the remote by definition, so that criterion passes hardest over the newest, most
   active caches. Executed as written it deleted a live mission's 24 GB cache and cost its owner a
   3m15s check and a 706s behavioral rebuild; all four old criteria were satisfied at the time
   (#1449). Expect logical size to read ~2x the physical one (btrfs `zstd:1` plus `du` counting
   uncompressed `st_blocks`).
7. **Verify the handoff contract** — `git worktree list` shows no worktree and `git branch -vv`
   shows no branch **for the mission you just merged**, and `git status --short` is empty. Other
   worktrees and branches are expected: missions run in parallel in this repo, so their presence is
   neither a failed handoff nor your business. Delete only what step 6 can attribute to you.

**No automated safety net is installed.** This file previously described a weekly systemd
`git-hygiene.timer` (Sun 03:00) that pruned confirmed-safe stale local branches. **That
timer does not exist in this environment** — verified 2026-08-30: `systemctl --user
list-timers --all` lists no such unit and `~/.config/systemd/user/` does not exist. Every
step of the runbook above is therefore entirely manual. If the timer is ever installed,
restore the description here with its actual scope.

### Seed contract

A **seed** is a deliberately published build reference that a new worktree can
clone to skip most of its first compile. It is an optimisation, never a
requirement: every rule below has a cold-build answer, and no agent workflow
depends on a seed existing.

Three different things, and conflating them is the bug this section exists to
prevent:

```text
worktree target   mutable state, owned by one agent, disposable
seed              shared reference, published on purpose, never written by a consumer
cargo cache       possible future upstream mechanism, not a dependency today
```

Cargo is developing a cross-workspace cache upstream (2026 goal: cross-workspace
recompilation and disk duplication). That is context for the future, not
something this bootstrap relies on or waits for.

**The rules.**

- Your `CARGO_TARGET_DIR` is **yours alone** (`~/.cache/cargo-target/<tree-name>`). Never build into `main`'s target, and never two worktrees into one dir — that is #1267, where an E2E run silently executes the other tree's binary.
- `scripts/seed_target.sh` **consumes** a seed. It never publishes one. Publication is a separate, explicit step run by a maintainer; a worktree that finds no seed must not create one. Publishing from a worktree would turn a read into a mutation and put two trees racing for the same reference.
- A seed is used **only** when its compatibility key matches yours exactly. The key covers toolchain, target triple, profile, flags, cargo config, features and `Cargo.lock`. It deliberately does **not** cover your commit, branch, worktree path or workspace identity — those are exactly the things that must not stop one worktree reusing another's compiled dependencies.
- **The key and the build come from one recipe, never from the ambient environment.** `--features`, `--profile` and `--target` change the key *and* reach `cargo`; there is no option that changes only the key. The toolchain and incremental-compilation settings are pinned by the recipe rather than inherited, so a stray `RUSTUP_TOOLCHAIN` in your shell moves neither the key nor the build. Arbitrary cargo arguments after `--` are **refused**, not passed through: an argument that changes which units get compiled has to be part of the recipe, or it changes the build without changing the key that describes it.
- **`CARGO_TARGET_DIR` pointing at a seed is rejected**, by `scripts/ci_fast_gate.sh`, before Cargo runs, and regardless of whether that seed is healthy. The decision is on the path's identity alone. Building into a reference would write your units into the tree every later worktree copies from.
- **If `build-dir` is configured, do not seed.** Cargo keeps build-script output in a separate location with an internal layout, and that is where the bulk of what a seed saves lives. It is stable since Rust 1.91, so this is a policy decision about what we can reason about, not a workaround. The check covers every config source Cargo reads, including ones outside the repo.
- Cloning uses `reflink=always`, never `auto`. `auto` silently falls back to a full copy on a filesystem without copy-on-write, which is slower and quietly not what you asked for. If the clone fails for any reason, you get a cold build.

**When there is no seed, or no compatible one:** build cold and carry on. Do not go
hunting for another seed, do not fall back to `main`'s target, do not edit a seed,
and do not publish one from your worktree. A performance optimisation that turns
into a workflow dependency is a regression, and the cold path is fully supported —
it is the path every first build before any seed existed took.

**Reading the result.** `seed_target.sh` prints one line, and it is worth reading:

```text
seed: seeded  reason=…    → reuse happened; the build is genuinely faster
seed: cold    reason=…    → correct build, no reuse; nothing is wrong
seed: refused reason=…    → stop and read the message; do not build
```

`cold` is a normal outcome, not a failure. `reason` names which condition applied
(no seed for this key, incompatible seed, clone unavailable, …), which is how you
tell "there simply isn't one" from "the one here cannot be used".

`refused` is the one outcome that is not a build at all: the clone failed **and
its leftovers could not be removed**, so there is no clean target dir to build
over. The script prints the exact `chmod`/`rm` to run. Fix that, then re-run —
do not treat it as a cold build, because the target it found was not clean.

**Not in this contract, on purpose:** which build artifacts a seed contains or
how it is produced. `scripts/test_seed_contamination.sh` asserts the seed's
observable behaviour, not cargo's internal layout, and neither should you.

### Shared vs. per-worktree resources

| Resource | Shared? | Action required |
| :--- | :--- | :--- |
| `.git/` object store | ✅ Shared | Automatic |
| Git config, hooks | ✅ Shared | Automatic |
| `Cargo.lock` | ✅ Shared | Via Git |
| `target/` | ✅ Shared (via direnv) | `.envrc` + `direnv allow` per worktree; enforced by `ci_fast_gate.sh` |
| `.envrc` | ❌ Per-worktree | `cp` from main + `direnv allow`; gitignored by the repo (`.gitignore:125`), not only by a personal global ignore |
| `.env` | ❌ Per-worktree | Manual `cp` from main |
| `.codegraph/` index | ❌ Per-worktree | `codegraph init` |
| `codedb.snapshot` + `~/.codedb/projects/<hash>/` | ❌ Per-worktree | `codedb reindex` inside the worktree |
| Seeds (`~/.cache/cargo-target/seeds/`) | ✅ Shared, read-only | Consumed by `seed_target.sh`; never written by a worktree; `ci_fast_gate.sh` rejects it as a `CARGO_TARGET_DIR` |
| Quarantine (`~/.cache/cargo-target/quarantine/`) | ⚠️ Deliberate, do not touch | Historical objects explicitly removed from the active build system by rename. This currently contains the former shared `main` target and obsolete seeds. Quarantine contents are not valid active build targets and must not be consumed, republished, or cleaned as part of normal bootstrap. Both this store and the seed store are rejected by `ci_fast_gate.sh` before Cargo runs. Deletion is a separate, explicitly authorized operation. Age only makes an object eligible for deletion; it does not authorize deletion. See `odd/tasks/target-quarantine-runbook.md` and `odd/tasks/seed-reclamation-runbook.md`. |
| Git stash (`refs/stash`) | ⚠️ Shared (DANGER) | **NEVER use `git stash`** |

### `~/.cache/cargo-target/` namespace

Treat the cache root as a reserved namespace with exactly these semantic categories:

```text
~/.cache/cargo-target/
├── <worktree-target>/   ← build state owned by one worktree
├── seeds/                ← RESERVED: published seed store
└── quarantine/           ← RESERVED: retired objects, never a build target
```

A worktree target may use the bootstrap-generated tree name or an explicitly configured custom name, but it remains **worktree-owned build state**. Do not invent additional semantic categories or use names such as `scratch/`, `shared/`, `cache/`, or `tmp/` for another kind of state under this root.

`seeds/` and `quarantine/` are reserved namespaces and must never be used as a worktree `CARGO_TARGET_DIR`. Follow their respective runbooks for lifecycle operations.

This exists because a denylist cannot stop invention. `ci_fast_gate.sh` refuses three known identities — `main`'s target, `seeds/`, `quarantine/` — and accepts anything else, so a name nobody had thought of yet is a name the guard will happily let you build into. Measured 2026-09-30: `~/.cache/cargo-target/scratch` is accepted with exit 0, and during that investigation 40 GB were written into `quarantine/` itself by a probe that had not read this section. The gate protects what it knows about; what it does not know about is bounded by the namespace being stated, not by the denylist growing.

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

**Never match a gate on git's human-facing prose.** Git's advice text ("hint:",
"warning:") is not a stable interface. Its wording has already changed across
releases — as of 2.44 every conditional advice message carries an off-switch hint,
and 2.48 changed that hint to name the newer `git config set` command instead of the
older spelling (observable on 2.55.0, where a malformed-ref error prints
`hint: Disable this message with "git config set advice.refSyntax false"`). It moves to
**stderr** while every gate captures stdout. The hooks here are already safe by
construction: their git calls
are porcelain or format-driven (`git rev-parse --show-toplevel`, `git rev-list`,
`git log -1 --format=%s`) and everything else is `gh`/`gentle-ai` JSON read with
`--jq`. Keep it that way — a new `git ...` call added to a gate must consume a
machine-readable field, and if it ever genuinely needs advice silenced, the
global `--no-advice` (git ≥ 2.46) does that without touching config.

> **Source.** git release notes for 2.44 ("All conditional \"advice\" messages show how to
> turn them off") and 2.48 ("The advice messages now tell the newer 'git config set' command
> to set the advice.token configuration variable to squelch a message"). Search the release
> notes **by version**, not by path: the file extension is not stable across releases — older
> tags carry `Documentation/RelNotes/2.x.0.txt`, while 2.55 and later carry `.adoc`, so a
> hardcoded extension 404s on roughly half the range.

**Boundary:** initiating a review from a plain shell fails with
`immutable_review_transport_unsupported` — the relay contract is host-only. The
hooks therefore only *consult* an existing verdict; reviews are initiated
through the Pi review tools, which persist the authority `validate` reads.
Measured 2026-08-30: `validate --gate` itself exits 0 and abstains
(`delivery: unmanaged`) for unmanaged candidates, so the wiring does not block
ordinary commits — refuting the earlier "hooks would block every commit"
claim (#1047 fact 2/3).

### Main-push gate (#1091)

`pre-push` now chains two gates: `review-gate.sh` (called without `exec`, exit
code propagated) and `main-push-gate.sh` (`exec`'d last so git's stdin ref list
reaches it). The second blocks any local push that would advance
`refs/heads/main` with commits that did not arrive through a PR merge: a merge
commit must list ≥ 1 associated PR (`gh api repos/{owner}/{repo}/commits/<sha>/pulls`),
a plain commit must end in `(#N)` with `gh pr view N --json state` = `MERGED`.

| case | decision |
| :--- | :--- |
| push does not target `refs/heads/main` | SKIP (silent) |
| `refs/heads/main` created (all-zero remote sha) | SKIP + warning |
| merge commit with 0 associated PRs | **BLOCK** (exit 1) |
| plain commit without trailing `(#N)`, or PR not MERGED | **BLOCK** (exit 1) |
| `gh` absent, gh network/auth failure, rev-list cannot enumerate | SKIP + warning (fail-open) |

`--no-verify` remains the documented hatch. This hook is defense-in-depth, not
the primary control: the reflog shows 0 local pushes to main in 172 updates,
and every BLOCK line names the bypass.

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

### Pushed history is immutable — corrections land on top

Once a commit exists on `origin/<branch>`, its SHA is public and any other worktree or CI
run may hold it. Rewriting it (`git reset` to an earlier point, then recommitting)
desynchronizes local from remote, and the only way back is a force-push — which the harness
hard-denies with no approval path (see "Hard-deny shell policy" below). Land the correction
as a new commit on top instead.

| Situation | Wrong | Right |
| :--- | :--- | :--- |
| Pushed commit found defective | `git reset HEAD~N` + recommit | new commit on top describing the fix |
| Pushed commit needs a better message | `git commit --amend` on pushed history | new commit, or amend only unpushed history |
| Unpushed commit needs fixing | `git commit --amend` ✅ | ✅ |

> ⚠️ **The gate is asymmetric: easy to rewrite pushed history, impossible to undo it.**
> `git reset --hard` is hard-denied, but a plain `git reset HEAD~N` is not gated at all. An
> agent can therefore walk into a rewrite freely and then discover the exit is blocked.
> Observed 2026-09-28 on `fix/crossplatform-portability` (PR #1651): two pushed commits were
> reset and recommitted after a verifier caught the `WEBFANG_CONFIG` override landing in the
> CLI's private resolver twin instead of the canonical one. The rewrite was correct, the
> method forced a force-push, and the normal push was correctly rejected.

### Contamination protocol

If you detect you operated outside your assigned worktree, or `git stash pop` applied unexpected changes:

1. **STOP** all operations immediately.
2. Do NOT attempt to clean up — no `git reset`, no force-push, no manual patching.
3. Report exactly: "Contamination detected. Worktree: `<path>`. Intruder commit: `<hash>` or unexpected stash applied. Awaiting human instructions."
4. Wait for explicit human authorization before any corrective action.

---

## 🏷️ Branching + Releases + Hotfix + Backport + Support + EOL

### Validity status (read first — this section phases in)

- **ACTIVE today:** `main` as the only development line; tags `v*` from `main`; `release.yml` preflight (RC vs stable channel + `tag == Cargo.toml` fail-fast); `support.json` + `SUPPORT.md` (**declared 2.1 STABLE / 2.0 EOL, and drifted — v2.2.0 through v2.4.1 are published and undeclared; see "DRIFTED today" below**); process labels (`release:cut`, `support:create`, `support:extended`, `breaking:*`, `migration:*`); the agent routing below.
- **ENFORCING today:** the branch-topology check is a **required gate**, not a warning. `scripts/check-topology.sh` runs as the step `Validate branch topology (enforcing)` inside the job `Validate PR metadata` in `pr-validation.yml`, with **no `continue-on-error`** — enforcement landed in #1502 (`d4f8fa79`, 2026-09-21), which also extended the conventional-branch regex to admit `hotfix/*`, `release/*` and `support/*` so no enforced arm is dead on arrival. `Validate PR metadata` is in the required status-check list for `main`, so a misrouted PR fails a required check within seconds and is unmergeable. Plan the base branch **before** opening the PR; never retarget an open PR onto another working branch (see "No stacked PRs in this repo" below).
- **WARN-ONLY today:** the `--scope=published` half of `check_support_drift.sh` in `support-drift.yml` reports without failing. Read its output; do not rely on it as a gate yet.
- **DRIFTED today (#1676, open):** `support.json` was never rotated after 2.1. Four releases (v2.2.0, v2.3.0, v2.3.1, v2.4.0/v2.4.1) shipped with it declaring a line three minors back. `check_support_drift.sh` now reports this on every PR; its `published` scope is advisory **until the maintainer backfills the undeclared minors**, at which point it becomes enforcing in that same commit. Nothing an agent runs creates, extends, or EOLs a line: that is a governance decision with an approved issue behind it.
- **NOT YET:** no `release/*` or `support/*` branch exists. Do not create one speculatively — support lines are materialized on demand (see below), never "just in case".

### Mental model: version first, branch second

Never start from "where do I create my worktree". Start from the issue:

```text
issue #N
  → affected version?
  → .github/support.json (jq, no LLM needed)
  → line state?
  → correct branch
  → worktree
```

`main` is always DEVELOPMENT (vNext) — never a release line and never a hotfix target. A hotfix branch physically cannot contain future code when it is based on `support/X.Y`: the safety is structural, not disciplinary.

### Agent routing (execute in order, stop at the first match)

```text
1. No version named (or version == main HEAD, no later work)? → main, normal flow.
2. Named version, main NOT ahead? → main, normal flow.
3. Named version, main ahead? → read support.json for line X.Y:
   EOL → STOP. Comment "línea EOL, requiere excepción del maintainer". Create nothing.
   MAINTENANCE → only if the bug is security; otherwise STOP as EOL.
   STABLE → step 4.
4. support/X.Y exists? → new worktree on it, hotfix/* based on support/X.Y.
   Missing? → create support/X.Y from tag vX.Y.latest FIRST (cut-support-branch.sh,
   inside chore/support-*, never on main), then as above. Never branch a hotfix from main.
5. Hotfix PR base MUST be support/X.Y — verify with `gh pr view --json baseRefName`.
   Base = main on a hotfix PR is a review-blocking error.
6. After merge: forward-port to main the same day (cherry-pick -x, same issue) if the
   affected code still exists there; else record `Backport: not-applicable (<reason>)`.
7. Fix lands on the OLDEST supported affected line first, then cherry-picks upward
   (fix-oldest-first). Never merge main INTO support/* (that drags the future into stable).
8. Patch tags (vX.Y.Z+1) are cut on the support line via cut-patch-tag.yml (manual
   dispatch), never by tagging main while it is ahead.
```

### Advisory duty (agents propose, maintainer disposes)

Anything in this policy that requires a maintainer decision (EOL exception, cutting `release/X.Y`, `release:cut` on a risky minor, whether a fix applies to another line, `support:extended`) MUST reach the maintainer as a proposal — never as a bare question and never as a silent stop:

1. What you determined (facts: affected versions, line states from `support.json`).
2. Options considered (2–3, one-line trade-off each).
3. Explicit recommendation FIRST, marked as such — including the cost of being wrong.
4. What you need: approval label, exception issue, or a choice between the options.

Example: "Bug X affects 2.4 (MAINTENANCE, non-security) → options: (a) no backport per policy **[recommended**: matches N-1 security-only; cost if wrong: 2.4 users stay exposed], (b) exception via `support:extended` [cost: opens a maintenance line for one fix]. Need: your call — (a) executes with no further action."

Silence, a bare "what should I do?", and deciding governance matters alone are all failures of this duty.

### Branch taxonomy (only these exist)

- `main` — development. Bases: `feat/*`, `fix/*`, `refactor/*`, `perf/*`, `docs/*`, `test/*`, `chore/*` (+ `ci/*`, `build/*`, `style/*`, `revert/*` per branch naming).
- `release/X.Y` — TEMPORARY stabilization for a MINOR/MAJOR. Cut by the maintainer from an explicit main SHA. Accepts only `fix/*` based on it. No features. Deleted when `vX.Y.0` ships. PATCH releases never create one.
- `support/X.Y` — maintenance line, created ON DEMAND from the line's latest tag when (and only when) it needs a patch while main is ahead. Accepts only `hotfix/*`. Deleted at EOL — a deleted support branch IS the EOL marker, but `support.json` is the source of truth, not the branch.
- `hotfix/*` — MUST be based on `support/X.Y` (or the affected tag when materializing the support branch in the same move). PR base MUST be `support/X.Y`, never `main`.

### Line states (`accepts` in support.json is machine-checkable — prefer jq over prose)

| State | Features | Bugfix | Security | Releases |
|---|---|---|---|---|
| DEVELOPMENT (`main`) | yes | yes | yes | no (only via cut) |
| STABILIZATION (`release/X.Y`) | **no** | yes (blockers) | yes | `rc.N` |
| STABLE (latest minor) | no | yes (via support) | yes | patch `Z+1` |
| MAINTENANCE (previous minor) | no | no | yes (best-effort) | security patch |
| EOL | no | no | no (explicit exception only) | no |

Support window (structural, no calendar): at most 2 live lines. Publishing `vX.(Y+1).0` demotes the previous STABLE to MAINTENANCE (security-only) and EOLs the previous MAINTENANCE automatically (`rotate-stable.sh`). Exceptions via approved issue, traced as `extended_by` / `support:extended` — an EOL line never silently revives.

**The declaration is derived, and it is checked.** `support.json` is a function of the published `v*` tags, so it drifts by construction unless something reads it. `scripts/check_support_drift.sh` is that reader (`support-drift.yml`, every PR):

| Scope | Asserts | Fail-closed on |
| :--- | :--- | :--- |
| `--scope=declared` | valid schema; `accepts` coherent with each line's state (STABLE `bugfix`+`security`, MAINTENANCE `security`, EOL none); **exactly one** STABLE, ≤1 MAINTENANCE, ≤2 live lines; `SUPPORT.md` matches a fresh render | unreadable/invalid JSON, unknown `state`, missing renderer |
| `--scope=published` | declared STABLE is the newest published minor; every minor at or above the tracking floor is declared; MAINTENANCE is the immediately-previous published minor; every `latest` exists as a tag | shallow clone, no tags, unparseable `v*` tag |

The tracking floor is the **oldest declared line**: minors published before it (1.x) predate support tracking, the same fact 2.0 records as `eol_reason: "baseline: predates support tracking"`, and demanding they be declared would invent history. `-rc.N` tags are candidates, not publications, so they never move the window.

Run it locally before committing any support mutation: `scripts/check_support_drift.sh` (both scopes; needs tags — a shallow clone is refused, not skipped).

### Release candidates

- PATCH: no RC, straight tag on the support line.
- MINOR/MAJOR: `vX.Y.0-rc.N` (`prerelease:true`, resolved by `release.yml` preflight) only when the maintainer declares a stabilization window (state migrations, export format, ONNX/ai changes). During RC, `release/X.Y` takes fixes only; `main` stays open.

### Cutting a release (labels are signals, authority is the maintainer)

- `MAJOR` → always cuts `release/X.Y`. `PATCH` → never does.
- `MINOR` → cuts `release/X.Y` when it touches state format/compat, SQLite migrations, behavior incompatible with existing installs, major AI/ONNX changes, operational protocol/API changes, or any issue explicitly marked release-risk. Labels (`breaking:*`, `migration:*`) and the categories above are triage *signals*; the *decision* is the maintainer applying `release:cut`. A forgotten label must never silently skip stabilization for a risky minor.

### Versioning authority

- `release-plz` owns versioning on `main` (Release PR → tag `v*` → binaries). It NEVER runs on `support/*`: both `release-pr` and `update` fail on `cargo package` for the unpublished path+version deps (same #1337 codepath, verified by pre-flight) — and `release-pr` targets the default branch by upstream design (#2159 open).
- On `support/*`: manual PATCH via `bump-support-patch.sh` INSIDE the hotfix PR (fix commit(s) first, then the script enforces the release contract — tag uniqueness, line `accepts`, lock in sync, CHANGELOG entry, snapshot green — and produces the single `chore: bump` commit). Never run `release-plz` against a support branch.
- Version bumps are release acts, never part of feature/fix commits. Inter-crate pins stay at `^old` by #1339 precedent — only the root `[workspace.package]` version moves.

### Governance scripts (all fail-closed; safe to re-run)

Run inside a `chore/support-*` branch (base `main`); mutations travel by normal PR (issue + `status:approved` + `type:chore`). Never on a `main` or `support/*` checkout. `SUPPORT.md` is rendered (`render-support-md.sh`), never hand-edited — same single-writer pattern as `CHANGELOG.md`.

| Script | Event |
|---|---|
| `cut-support-branch.sh X.Y` | Materialize branch from the line's latest tag (refuses EOL/unknown lines) |
| `rotate-stable.sh X.Y` | On publishing `vX.(Y+1).0`: demote + auto-EOL, new STABLE at the head of the list (idempotent per argument). **Fails closed** when `vX.Y.0` has no tag, when there is no STABLE to demote, or when published-but-undeclared minors sit between the current STABLE and `X.Y` — that is a governance gap, not something to rotate over silently |
| `eol-line.sh X.Y` | Declare EOL (deletes branch remote+local, marks entry) |
| `render-support-md.sh [--check]` | Regenerate `SUPPORT.md`, or verify it against a fresh render writing nothing (`--check` is the CI gate; the generator stays the single writer) |
| `check_support_drift.sh [--scope=…]` | Read-only, fail-closed: `support.json` vs its tags and vs its own semantics (`--scope=published\|declared\|both`); run by `support-drift.yml` on every PR |
| `test_support_drift.sh` | Semantics harness for the two scripts above (25 cases: drift fails, consistency passes, blind spots fail closed) |
| `bump-support-patch.sh X.Y.Z [pr#]` | Manual patch bump with release-contract checks (see above) |
| `check-topology.sh` | Head-prefix → base validation. **Required gate** — invoked as step `Validate branch topology (enforcing)` by `pr-validation.yml`; the script itself is policy-agnostic, so the enforcement lives entirely in that step |

### Merge Queue (durable statement, not environment-dependent)

The repository currently does not use GitHub Merge Queue; `merge-when-green.sh` + batch merge (same-`baseRefName` required within a batch) is the integration mechanism. Re-evaluate if repository ownership/plan changes.

### Forbidden operations (review-blocking, CI-enforced at enforcement time)

- Merging `main` INTO `support/*`. Cherry-picking `main` → `support/*` without an approved issue.
- Tagging `vX.Y.Z+1` from `main` while it is ahead of `vX.Y.Z`. A hotfix PR with base `main`.
- Creating `support/*` "just in case". Editing `SUPPORT.md` by hand. Running `release-plz` on a support branch.
- Batching PRs with different `baseRefName` (disjoint files are not sufficient).

---

## 🔒 Safety & Permissions

### Allowed without asking

- Read any file in the repo.
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`.
- Both intelligence tools: CodeDB MCP, CodeGraph MCP.
- Edit files within `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Worktree management: `git worktree add`, `remove`, `list`, `prune`.
- Read-only cross-branch inspection: `git show <branch>:<file>`, `git log <branch>`.

### Ask first

- Adding/removing dependencies (`Cargo.toml`).
- Changing feature flags or profiles.
- Deleting files.
- `cargo build --release` or `cargo llvm-cov`.
- Modifying CI/CD (`.github/`).
- New files outside `crates/`, `crates/*/tests/`, `benches/`, `examples/`.

### Never

- Commit secrets, `.env`, or credentials.
- `.unwrap()` in production — use `?` or `match`.
- Force push to main.
- Modify `target/`, `dist/`, `build/`.
- `git checkout` / `git switch` to change branches (use `git worktree add`).
- `git stash` in any form (shared storage causes cross-worktree contamination).
- Access sibling worktrees via relative paths (`../feat-auth/...`).
- Commit in a branch/directory-mismatched worktree without satisfying the identity-exception protocol.
- Use `repo:"webfang"` (bare name) for intelligence tools in worktrees — always absolute path (#360).
- Rewrite pushed history (`git reset` / `rebase` over commits that exist on `origin/<branch>`) — see "Pushed history is immutable".

### Hard-deny shell policy (no approval path)

The harness classifies every `bash` command before running it. Some classes open an
interactive confirmation the human approves; others are **hard-deny** and never prompt at all
(`gentle-pi/extensions/gentle-ai.ts` → `DENIED_BASH_PATTERNS`, evaluated before any config and
before the UI layer). Conversational authorization from the user cannot change a hard-deny
decision, because no dialog is ever shown.

| Class | Examples | Human sees |
| :--- | :--- | :--- |
| **Confirm** | `git push`, `git rebase`, forced `git branch -D`, `npm publish`, `pi remove` | dialog → approve → runs |
| **Hard-deny** | force-push (`--force`, `--force-with-lease`, `-f`), `git reset --hard`, `git clean -f -d`, `rm -rf /` or `~`, `chmod -R 777`, `chown -R` | nothing — structurally unapprovable |

What an agent must do when a hard-deny block lands:

1. **Do not re-ask for permission in conversation.** It is structurally useless — there is no
   prompt to press — and repeating it burns a turn on a decision that cannot change.
2. **Do not route around the pattern** by splitting the command, reordering flags, or wrapping
   it in a script. The list is deliberately non-configurable; evasion defeats its purpose.
3. **Report the block once**, with the exact command for the human to run, plus a
   non-destructive equivalent the agent *can* execute. Then let the human choose.

---

## 📝 Commit, PR & CI

**Format:** `type(scope): description`

- type: `feat` | `fix` | `refactor` | `test` | `docs` | `perf` | `chore` | `revert`
- scope: `cli` | `crawler` | `ai` | `mcp` | `exporter` | `http` | `domain` | `infra`

### PR creation — CI-enforced rules (`pr-validation.yml`)

Every PR is validated on open / edit / synchronize / label changes. **All four MUST pass:**

1. **Linked issue** — PR body must contain `Closes #N`, `Fixes #N`, or `Resolves #N`.
   For a **partial slice of an umbrella issue**, use `Closes part of #N` — it passes
   validation and, because GitHub's auto-close requires strict adjacency, does **not**
   close the umbrella. Never write a bare `Closes #N` against an umbrella that has
   remaining scope: it auto-closes the tracker mid-plan, which is exactly how #994 was
   closed after sub-slice 1 of 5 with 0 of 11 acceptance criteria ticked (#1010).
   The cleanest shape remains **one issue per PR**, with the umbrella as an index that
   links child issues rather than a link target.
2. **Linked issue carries `status:approved`** — every issue linked via
   `Closes/Fixes/Resolves #N` must carry the protected `status:approved` label BEFORE
   the PR is opened (enforced since 2026-09-11). The label is protected: agents must
   never self-approve — request the maintainer's approval and let them (or an actor
   with verified `MAINTAIN`/`ADMIN` on the target host) add it. Verify with
   `gh issue view N --json labels`. External contributions: the maintainer approves
   the issue first, then the PR can pass validation.
   Operational trap (#1338): if the PR is opened before the label lands, validation
   fails with `Toda issue vinculada debe tener el label 'status:approved'` and does
   NOT re-evaluate on its own — after adding the label, force a re-run with
   `gh run rerun <validation-run-id> --failed` (a push also retriggers it).
3. **Exactly one `type:*` label** — count of labels starting with `type:` must be exactly 1.
4. **Conventional branch name** — must match `^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$`.

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

### CHANGELOG policy (release-plz writes it — work PRs never touch it)

`CHANGELOG.md` is **out of scope for ordinary work PRs and for every delegated agent**. Do not
add, edit, or "keep it fresh" in a feature/fix branch.

**Since the release-plz automation (#1177), the `CHANGELOG.md` `[Unreleased]` section and every
`## [x.y.z]` release section are written automatically** by release-plz (git-cliff) from the
Conventional Commit history, in the automated **Release PR** (`chore/release-*`, see
"Release automation" below). Humans only edit CHANGELOG.md inside that Release PR (entry polish
before merging) or for one-off baseline cuts such as the v2.0.0 consolidation (#1176).

The old rules that still apply:

- **Work PRs never touch `CHANGELOG.md`** — write good Conventional Commit titles instead
  (`feat:`, `fix:`, `refactor:`, ...); that is what the changelog generator reads.
- Breaking changes must be declared in the commit body footer (`BREAKING CHANGE: <why>`) or in
  the title (`feat!:`) so both the version bump and the ⚠️ section are generated correctly.
- Batch merges of N green PRs still keep **disjoint files** per PR — `CHANGELOG.md` must not
  appear in any of them.

```markdown
## [Unreleased]

### 🔧 Fixed

#### Unified filename sanitizer (#911)
- Collapsed the two divergent filename sanitizers into one join-safe helper.
```

> **Why the restriction, not just a preference:** the batch-merge policy requires the merged PRs to
> touch **disjoint files** so N green PRs cost one CI run instead of N. `CHANGELOG.md` is a single
> shared file, so per-PR edits guarantee a conflict in every batch and silently kill the
> optimization. The file stays untouched until one writer — now the automated Release PR — owns it.

### Release automation (release-plz, tags + binaries, no crates.io)

Releases are automated end-to-end as of #1177. Distribution is **binaries-only** (GitHub
Releases); nothing is published to crates.io.

```text
merge PR to main
   └─> release-plz-pr job: opens/updates the Release PR (chore/release-*)
         version bump ([workspace.package] in Cargo.toml) + CHANGELOG.md (cliff.toml)
   merge Release PR (human review — the ONE place to polish changelog text)
   └─> release-plz-release job: pushes tag v{{ version }} (single lockstep version)
       └─> tag push fires release.yml natively (push: tags — App token, no suppression)
           └─> release.yml: 4 binaries + SHA256SUMS + GitHub Release
               (linux x86_64/aarch64, macOS Apple Silicon, Windows x86_64 —
               Intel macOS is not built: ONNX Runtime dropped x64 macOS as of
               1.24.1, so the `ai` feature has no prebuilt to link against)
```

> ⚠️ **The `v*` tag push IS the hand-off — `release.yml` triggers on `push: tags` natively.**
> Both release-plz jobs mint a GitHub App token, and GitHub only suppresses workflow runs
> for `GITHUB_TOKEN`-generated events, so App-pushed tags fire the trigger like any human
> push. History: under `GITHUB_TOKEN` the same tags never fired it — `v2.1.0` shipped its
> binaries only through a **manual** dispatch and `v2.1.1` ended with a tag and **no Release
> at all** (#1478) — and the hand-off was an explicit `workflow_dispatch` call from a
> `dispatch-release` job. Do not reintroduce that job: with the native trigger live it would
> run `release.yml` twice per release.

Configuration lives in `release-plz.toml` (workspace: `git_only = true`, `git_tag_name = "v{{
version }}"`, `version_group = "webfang"` on every processed crate, `publish = false`) and
`cliff.toml` (Keep a Changelog sections with the emoji vocabulary).

- All crates share ONE version and ONE tag per release — the tag must keep the `v*` shape or
  the dispatcher filter and `release.yml` preflight reject it (`git_tag_name` is load-bearing).
- The hand-off from `release-plz` to `release.yml` is the `push: tags` trigger, never a
  dispatch call (see the note above). A tag is trusted only if it both sits at the pushed
  commit and carries the `release-plz` fingerprint (tagger `github-actions[bot]` pre-migration
  or the App bot since slice 2 + subject `chore: Release package …`), so historical or human
  tags are never built and published.
- `webfang_benchmark` and `webfang_test_utils` are excluded (`release = false`).
- Breaking changes: declare `BREAKING CHANGE: <why>` in the commit footer → major bump; `feat:`
  → minor; `fix:`/`perf:` → patch.
- The tag must never be created by hand anymore (except emergency recoveries); release-plz owns it.
- Both release-plz jobs mint a GitHub App token (the full migration closed #1205):
  App-opened Release PRs deliver `pull_request` events so checks run without a human-token
  rerun, and App-pushed tags fire `push: tags` with no dispatch job. The `chore/release-*`
  exemption stays in `pr-validation.yml` (#1474) and #1177 carries `status:approved`.

### Pre-commit gate (every commit)

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines && cargo fmt --all -- --check && env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
```

> ⚠️ **The clippy command MUST match CI exactly.** CI runs the strict gate above, which enables the `#516` complexity ratchets (`clippy::cognitive_complexity` + `clippy::too_many_lines`, thresholds in `clippy.toml`). Running a bare `cargo clippy -- -D warnings` locally will PASS while CI FAILS on any function >100 lines or over the cognitive-complexity limit. Always use the full command above before pushing.

> ⚠️ **`cargo fmt` MUST be run as `cargo fmt --all -- --check` in every verification chain.** A bare `cargo fmt` is a *fixer*: it rewrites the working tree and exits **0 whether or not it changed anything**, so a green run proves nothing about what is committed. Reporting "cargo fmt ✓" from it is not verification — an agent that runs it, sees exit 0 and moves on leaves the rewrite **uncommitted**, and CI's `cargo fmt --all -- --check` then fails on exactly that file. Observed on PR #1493: `crates/webfang_core/src/domain/mod.rs` was dirty in the worktree (the fix already applied) while the pushed commit was still unformatted, so local passed and CI failed. Fix with `cargo fmt --all` if needed, then **verify** with `--check`, then commit the result.

> 🚨 **`--all-features` is a safety flag here, not a strictness preference.** This crate has `chromium`-gated code whose only consumers are behind `#[cfg(feature = "chromium")]`. Running clippy **without** `--all-features` makes those imports look dead, and `clippy --fix` will **delete live code** — `cargo check` with default features then still passes, so the loss is invisible until `--all-features` fails. This bit the main checkout twice during #994 (see #1006). Never run `clippy --fix`, and never wire an auto-fixing tool, against a feature set narrower than the build's. `.pi-lens.json` disables the pi-lens autofix paths for exactly this reason; do not re-enable them.

> 📚 **The fourth command mirrors the CI `doc-quality` job, which denies rustdoc lints (`rustdoc::redundant_explicit_links`, missing docs) via `RUSTDOCFLAGS=-D warnings`.** `cargo check`, `clippy`, and `fmt` never document an item, and the fast gate's `lane_docs` only validates Markdown links — without this step a denied-by-default rustdoc lint is invisible locally and only fails CI (#1435). It is path-gated inside `scripts/ci_fast_gate.sh` (runs only when `crates/*/src/**.rs` changed; the full lane always runs it), so docs/CI-only commits skip it.

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

#### SSH port 22 outage → per-invocation 443 override

Port 22 to GitHub is observed **flapping** on this network (timeout, then a healthy `Hi <user>!` minutes later), so this is a contingency, not a permanent configuration. **Never pin 443** in `~/.ssh/config` or `remote.origin.url`: a permanent pin hides the moment :22 recovered, and done as a minimal snippet it silently breaks identity for every repo and every agent on the machine.

When :22 times out, override the transport **for one invocation** and keep `origin`:

```bash
GIT_SSH_COMMAND="ssh -p 443 -o Hostname=ssh.github.com" git push origin <branch>
```

**Why per-invocation `origin` and not an explicit URL** (`git push ssh://git@ssh.github.com:443/OWNER/REPO.git`): a push to a bare URL lands on the remote but does **not** update `refs/remotes/origin/*` — verified against a local bare repo, where a push to `origin` created the tracking ref and a push to a URL did not. This repo depends on those refs in two places — the post-merge runbook step *"Sync local main (ff-only)"*, and the *"No stacked PRs"* sequential-delivery list — both of which run `git merge --ff-only origin/main` and STOP when it fails. So a URL push would manufacture the next confusing STOP. (Referenced by section name on purpose: a line-number citation in this file is invalidated by any edit above it, including these ones.)

**The identity trap:** `ssh.github.com` does **not** match a `Host github.com` block in `~/.ssh/config`, so `IdentityFile` / `IdentitiesOnly` are silently lost (`ssh -G ssh.github.com` reports `identitiesonly no` and only the default identity names). SSH then authenticates via the **agent** alone — true while the key happens to be loaded, fatal with a cold agent. Pass the identity explicitly:

```bash
GIT_SSH_COMMAND="ssh -p 443 -o Hostname=ssh.github.com -o IdentitiesOnly=yes -i ~/.ssh/<your_github_key>" git push origin <branch>
```

⚠️ Substitute your own key path — this file is shared, so a literal `-i ~/.ssh/id_ed25519_github` would be machine-specific.

The same override covers `fetch`, `ls-remote`, `gh repo clone` and `gh pr checkout` (`gh config get git_protocol` = `ssh` here). `gh` **API** calls (`gh pr view`, `gh run list`, `gh pr merge`) are HTTPS and unaffected: a :22 outage breaks git transport, not the merge automation.

**The verification gap:** the `git ls-remote origin <branch>` above uses the same SSH transport as the push, so **when :22 is down the verification step is unreachable too**. That is an inference from the shared transport, not a measurement — the two were never observed hanging together. Fall back to the override, or to the API:

```bash
GIT_SSH_COMMAND="ssh -p 443 -o Hostname=ssh.github.com" git ls-remote origin <branch>
gh api repos/{owner}/{repo}/git/ref/heads/<branch> --jq '.object.sha'
```

### PR checklist

- [ ] `bash scripts/ci_fast_gate.sh` GREEN (lane-aware local gate: runs the cargo gates below only when code changed)
- [ ] `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all -- --check` (never a bare `cargo fmt` — it exits 0 after rewriting)
- [ ] rustdoc covered when library source changed: `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` GREEN (path-gated inside `ci_fast_gate.sh` on `crates/*/src/**.rs`; the full lane always runs it)
- [ ] `cargo nextest run` (at least affected module)
- [ ] Review `git diff --stat main...HEAD` to confirm only expected symbols/files changed
- [ ] Error messages in Spanish if user-facing; new public items have doc comments
- [ ] PR has exactly one `type:*` label + linked issue WITH `status:approved` + conventional branch
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
2. Walk away while CI runs (~6m34s to merge-ready; full wall: ~8m41s). Quick status any time: `scripts/ci_status.sh <PR-N>` (read-only).
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
- Merges with `gh pr merge <N> --squash` (or `--merge` for batch PRs). This
  **respects branch protection** — required checks must be green at merge time. It is
  NOT the synchronous-PUT bypass (`gh api -X PUT .../pulls/N/merge`) which bypasses
  required checks and should not be used for routine merges.
    - Deletes the **remote** head branch separately, via `git ls-remote` pre-check then
      `git push origin --delete`, and only for same-owner PRs. It deliberately never passes
      `--delete-branch` to `gh pr merge`, for two reasons:
      1. **Linked-worktree invariant.** In this repo's flow the head branch is checked out
         in a sibling worktree, and Git refuses to delete a branch that is any worktree's
         HEAD. `gh` would return rc=1 *after a successful merge* trying to delete the local
         branch, so the exit code would lie. Remote cleanup here + local cleanup in the
         runbook keep exit codes truthful.
      2. **Noisy absent-ref deletes.** `--delete-branch` deletes server-side as part of the
         merge, while `git push --delete` on an already-absent ref prints a
         `[remote rejected]` error even when nothing is wrong — hence the pre-check.
      Do not "fix" this by adding `--delete-branch` manually; it desyncs the runbook.
- Use `--dry-run` to poll and report without merging.

Do NOT rely on `--auto`: it never accepts in this repo configuration. If a future PR
needs auto-merge (e.g. transferring the repo to an organization with rulesets), revisit.

### No stacked PRs in this repo — slice large changes sequentially

**A stacked/chained PR cannot be merged here, and the gate rejects it in seconds.**
`scripts/check-topology.sh` admits exactly these head-prefix → base pairs:

| Head prefix | Allowed base |
| :--- | :--- |
| `hotfix/*` | `support/*` |
| `fix/*` | `main`, `release/*` |
| `feat/*` `refactor/*` `perf/*` `docs/*` `test/*` `chore/*` `style/*` `build/*` `ci/*` `revert/*` | `main` only |
| `release/*` `support/*` `release-plz-*` `dependabot/*` `renovate/*` | exempt (own flow) |

There is therefore **no head prefix whose base may be another working branch**. Both
strategies the `chained-pr` skill offers are structurally impossible here:

- *"Stacked PRs to main"* — the skill's own diagram requires slice *N+1* to be built on
  slice *N* and retargeted onto it. A `fix/*` slice retargeted onto a `fix/*` parent is
  rejected: `fix/* debe apuntar a main o release/X.Y, no a <parent>`.
- *"Feature Branch Chain"* — the tracker branch is a `feat/*` head whose base must be
  `main`, and every child PR targets the tracker or its parent. The first child already
  violates the table above.

**Observed, not theoretical.** PR #1665 (`fix/mcp-session-cap`, stacked on #1664's
`fix/mcp-session-cap-design`) failed the required check ~15 s after the run started, with
the annotation `fix/* debe apuntar a main o release/X.Y, no a fix/mcp-session-cap-design`.
It was closed; the change was rebuilt against the new `main` and merged as #1672.

**The mechanism that does work is sequential delivery:**

1. Ship slice 1 as an ordinary PR based on `main`; merge it.
2. Rebase or rebuild slice 2 from the **new** `main` — `git rebase origin/main` on a fresh
   branch, or cherry-pick the slice commits — and open it based on `main`.
3. Repeat. Each PR is a normal one-work-unit PR; reviewability is preserved because the
   slices are separate, not because they are stacked.

**Do not let the harness skill override the repo gate.** `chained-pr` activates on
"PRs over 400 lines, stacked PRs, review slices" and proposes a tracker branch or a
retarget. In this repo the 400-line concern is real and the stacked answer is wrong: the
equivalent control is the **sequential** pattern above, plus `work-unit-commits` so each
slice is already a self-contained commit. If a large change genuinely cannot be split
into slices that each land on `main`, that is a maintainer decision (a policy change to
`check-topology.sh` or an explicit exception), not something to discover at PR time.

**Never retarget an open PR onto a working branch to "make the diff small".** That is the
single move that turned a healthy PR into an unmergeable one in the #1664/#1665/#1672
sequence. Rebasing onto `main` and opening a new branch is always available and always legal.

### Batch merge of multiple green PRs (avoid N× CI re-runs)

Canonical procedure: `docs/merge-queue-manual.md` (helpers + strict-mode cost rationale).
Summary below — the doc wins on conflict.

**Trigger:** the agent detects 2+ open PRs, all with green CI, all targeting `main`.

**Why sequential merging is slow:** branch protection has `strict: true`, so after merging
the first PR, every remaining PR becomes `BEHIND` and each `update branch` (rebase)
re-runs the FULL CI (~27 min). N PRs sequential ≈ N × 27 min. One batch PR ≈ 1 × 27 min.

**Precondition (verify first, no exceptions):**

1. All PRs are `MERGEABLE` with `mergeStateStatus: CLEAN`.
2. **Files touched are fully disjoint** — check with:
   `scripts/ci_pr_overlap.sh <N1> <N2> [...]` (exit 0 = disjoint, 1 = overlap with paths printed, 2 = usage/non-open/non-main PR).
   Any overlap → do NOT batch; merge sequentially instead.
   (`CHANGELOG.md` must not appear in any of these lists — see "CHANGELOG policy" above. If it
   does, that PR violated the policy and must drop the file before batching.)
3. All PRs share a compatible `type:*` label (e.g. all `fix` → one `type:bug`).

**Procedure:**

```bash
# 1. Branch from current main in a new worktree (or let the helper do it)
git fetch origin && git merge --ff-only origin/main
git worktree add ~/Projects/Rust/webfang-worktrees/fix-batch -b fix/batch-<topic>
# Helper alternative (validates names/SHAs, merges, never auto-resolves):
# scripts/ci_batch_branch.sh fix/batch-<topic> <sha1> <sha2> [--dry-run]

# 2. Merge each PR's REMOTE head SHA (not the local branch — it may be stale)
#    Get the exact SHA: gh pr view <N> --json headRefOid --jq '.headRefOid'
git merge --no-ff <sha1> -m "Merge <branch> (PR #N1)"
git merge --no-ff <sha2> -m "Merge <branch> (PR #N2)"

# 3. Write the CHANGELOG entries HERE — this is the ONE place they are written.
#    Under `## [Unreleased]`, one entry per merged slice (or per closed issue for small ones).
#    See "CHANGELOG policy" above: no other PR ever touches this file.

# 4. Local gate (lane-aware: cheap for docs/CI, full for code), push, create the batch PR linking ALL issues
bash scripts/ci_fast_gate.sh
git push -u origin fix/batch-<topic>
gh pr create --base main --head fix/batch-<topic> --label type:bug \
  --title "fix(batch): ..." --body "Closes #A
Closes #B

## Summary
..."

# 5. Merge ONLY when the batch PR itself is green — see "Merge method" below
scripts/merge-when-green.sh <batch-PR> --merge

# 6. ONLY AFTER the merge landed: close the original PRs as superseded
#    Verify first: gh pr view <batch-PR> --json state,mergeCommit
for pr in <N1> <N2>; do gh pr close $pr --comment "Superseded by #<batch-PR>"; done

# 7. ONLY AFTER the merge landed: delete the now-orphan remote branches
#    (gh pr close does NOT delete them)
git push origin --delete <branch1> <branch2>
```

> ⚠️ **Steps 6-7 are cleanup, not part of the batch at all — never run them before step 5
> merges.** Closing the constituent PRs and deleting their remote branches is what removes the
> fallback: if the batch's own CI then goes red there is no PR to fall back to, and the slices
> survive only as local refs that a later `worktree remove` can erase. Observed 2026-09-13 with
> the 5-slice batch (PR #1397): steps 6-7 were executed while `Coverage` was still `pending`, per
> the previous wording of this very list, which placed them right after PR creation. Recovery cost
> one `git push origin <SHA>:refs/heads/<branch>` per slice from refs that happened to still exist.
> The order is the fix: green → merge → close → delete.

**Merge method:** use `scripts/merge-when-green.sh <batch-PR> --merge` (merge commit),
NOT `--squash`. Squash would crush N independent fixes into one commit, losing per-fix
revert granularity. The merge commit preserves each original commit in main's history.
The `--merge` flag exists for exactly this case (#1033): going through the script keeps
a batch merge under all four of its guards — required-context reporting (#1011),
`UNKNOWN` retry, refusing a **required** check that reports `skipping`, and the
`CLEAN`/`UNSTABLE` vs `BEHIND`/`BLOCKED` check. Merging a batch by hand silently drops
them, and a batch is the highest-risk case there is.

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
cargo fmt --all -- --check   # Verify formatting (bare `cargo fmt` rewrites files and exits 0 => never evidence)
```

**Moderate (< 5 min):**

```bash
cargo nextest run            # Full suite
cargo build --release        # LTO fat, ~3-5 min
```

**Local lane gate (pre-push, path-aware):**

```bash
bash scripts/ci_fast_gate.sh       # docs/CI/code/full lanes; GREEN required before push or PR
bash scripts/ci_test_budget.sh     # advisory: affected areas + estimated CI lane set
bash scripts/ci_metrics.sh         # read-only SLO snapshot (see docs/ci-slo.md)
```

CI itself classifies every PR via the `change-scope` job
(`scripts/ci_path_classifier.sh`): docs-only/CI-only PRs skip the
compile-heavy entry points, AI/MCP lanes trigger only on affected paths, and
`CI Gate` stays fail-closed for skipped producers. `cargo-mutants (PR diff)`
remains required only as the temporary context until Tier 1 is proven stable.

**PR automation (single maintainer):**

```bash
scripts/merge-when-green.sh <PR-N>            # Wait for green checks, squash-merge (default)
scripts/merge-when-green.sh <PR-N> --merge    # Merge commit — use this for batch PRs
scripts/merge-when-green.sh <PR-N> --dry-run  # Poll and report; do not merge
scripts/ci_pr_overlap.sh <N1> <N2>            # exit 0 = disjoint files, safe to batch
scripts/ci_batch_branch.sh <branch> <sha...>  # local integration branch helper (--dry-run first)
scripts/ci_status.sh <PR-N>                   # compact required-check + mergeability summary (read-only)
```
    
All three delete the **remote** head branch after a successful merge, via a
`git ls-remote`-guarded `git push origin --delete` — never with
`gh pr merge --delete-branch` (see "Automated merge workflow"), and never the
local branch or worktree, which the post-merge runbook owns.

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

## 🚧 Sprint 0 — StateStore resume contract (sdd/stabilization-sprint0-baseline)

### Gate 0 freeze — RETIRED (2026-09-07, #1241)

The `FREEZE_FEATURES` gate that blocked `type:feature` / `type:breaking-change` PRs has been removed
from `.github/workflows/pr-validation.yml`, together with `FREEZE_DRAIN_UNTIL`, the drain contract, and
`scripts/test_freeze_gate.sh`. The Sprint 0 stabilization baseline it protected is complete: all six
gates ratified and the ADR-0012 intra-crate allowlist at its terminal state (19 entries → 2, both
declared permanent).

Two facts from that regime stay in force as general knowledge, because they outlive the gate:

- **A freeze bypass is unreachable in this single-maintainer repo.** Any future gate that requires a
  CODEOWNER *approval* (`gh api repos/$REPO/pulls/$PR/reviews`, `APPROVED` count > 0) cannot be
  satisfied by this maintainer — GitHub forbids self-approval. Verified empirically with PR #814.
  Design future gates on labels or artifact checks, not on approvals nobody can grant.
- **For `pull_request` events, GitHub evaluates the workflow file from the PR's own merge ref, not
  `main`'s.** A PR that changes a gate can therefore pass its own new (or relaxed) validation. This is
  not a freeze-specific quirk — it applies to every `pr-validation.yml` edit, and it is why policy
  changes need review of the *diff*, not just of the resulting check status.

To reinstate a freeze for a future stabilization sprint: `git revert` the retirement PR (#1241) and
re-read that issue's rationale for why the parked machinery was deleted rather than left switched off.

### StateStore resume contract

- `ExportState { version:1 }` — `#[serde(default="default_version")] pub version:u32`, `default_version()->1`, `new()` sets `1` (`crates/webfang_core/src/domain/entities/export.rs`).
- `StateStore::load_or_default()` (`crates/webfang_core/src/infrastructure/export/state_store.rs: CURRENT_VERSION=1`): stale `version !=1` → `tracing::warn!(version, domain, path)` + pre-migration `.bak` sibling + fresh `ExportState::new(domain)` (#1587); `NotFound` → fresh; corrupt JSON (Serialization) → propagate → `filter_processed_urls` logs via `log_scrape_error` and returns all URLs (re-scrape, no hard error).
- Legacy JSON missing `version` deserializes to `1` via `default_version` (no crash).
- `CrawlCheckpoint` (JSON+CRC32, `checkpoint_interval=100`) is **out-of-scope**: engine-internal, not wired to `--resume`. Checkpoints viejos se invalidan en v-next por `version` mismatch — `warn!` + `.bak` y recrea estado sin crash (#1587).
- See `COMPATIBILITY-MATRIX.md` and `docs/test-inventory.md`.
