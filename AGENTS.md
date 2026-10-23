# AGENTS.md — WebFang

Production-ready web scraper. Clean Architecture, AI semantic cleaning, sitemap-based crawling.

**Stack:** Rust 1.88 · Tokio · wreq 6 (TLS fingerprint) · SQLite

> **How to use this file.** It is a **router**, not a manual. Every block either states the
> rule (the one you cannot derive from the code or the compiler) or points at **where the rest
> lives**. The long detail lives in Engram and in the harness.
>
> **Nothing is documented twice.** If a fact has a source of truth — a script, a
> `support.json`, a test, a lockfile — this file **links** that source, it does not copy it.
> A copy in prose is a copy that rots.

## 🔎 Consulting Engram

Everything that is not here is searched in Engram: `mem_search` (MCP) or
`engram search "<query>" --project webfang [--scope personal]`.

| I need | Query | Scope | Obs |
|---|---|---|---|
| Worktrees: bootstrap, seeds, cleanup, target dirs | `worktree target dir seed bootstrap envrc` | project | #2307 |
| Workstation build profile: mbx, mold, OOM, timings | `build profile mbx mold cargo target OOM` | personal | #2306 |
| Support lines, hotfix routing, versioning, governance | `support line routing hotfix branch taxonomy versioning` | project | #2308 |
| PR gates, pre-commit, merge, batch, CHANGELOG | `merge-when-green batch merge pre-commit changelog` | project | #2309 |
| Delivery gates: hooks, push to main, cloud verification | `delivery gates hooksPath main-push verify cloud timeout` | project | #2316 |
| SSH :22 → :443 contingency | `ssh port 22 443 identity` | personal | #2311 |
| Contamination, immutable history, hard-deny | `contamination history immutable hard-deny governance` | project | #2312 |
| Native review (RDD): full cycle | `gentle-ai review assess start consent capture acknowledge` | project | #2213 + skill `rdd-defect-workflow` |
| Skill index with exact paths | `skill-registry skills paths` | project | #43 + `.atl/skill-registry.md` (generated per tree) |
| State persistence, `--resume`, StateStore | `StateStore ExportState load_or_default version bak` | project | #216 + `docs/persistence-resume.md` |
| Where project knowledge lives (owner decision) | `knowledge architecture AGENTS.md decision` | project | #370 |
| AGENTS.md best practices | `agents md best practices` | project | #368 |
| Full block → obs index | `knowledge index bloque destino query verificada` | project | #2313 |

**Sources of truth in the repo — link them, do not copy them:**

| Fact | Source of truth |
|---|---|
| Support-line state | `.github/support.json` (`jq`), rendered into `SUPPORT.md` |
| Inter-crate dependency graph | `cargo metadata` + `scripts/check_dependency_direction.sh` |
| MCP tool count | `crates/webfang_mcp/tests/stdio_handshake.rs` |
| Admitted branch topology | `scripts/check-topology.sh` |
| PR rules CI enforces | `.github/workflows/pr-validation.yml` |
| Version pins in docs | `scripts/check_docs_version_pins.sh` |

## 🔧 Commands

```bash
# Verification (the full CI gate; --all-features is mandatory)
cargo check
cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines
cargo fmt --all -- --check          # --check verifies; a bare `cargo fmt` is a fixer
env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps

# Tests
cargo nextest run --test <name>
cargo insta review                  # review every snapshot diff

# Local gate (lane-aware: docs / CI / code / full)
bash scripts/ci_fast_gate.sh

# Manual checks
scripts/merge-when-green.sh <PR>            # wait for green checks, then merge
scripts/ci_status.sh <PR>                   # read-only summary
scripts/ci_pr_overlap.sh <N1> <N2>          # disjoint files? (batch merge)
```

## 🚧 Project rules

What the compiler **cannot** catch. Everything else comes from the harness or the table
above.

- **Never `.unwrap()` in production** — use `?`, `match` or `.context()`.
- **User-facing errors in Spanish; tracing fields and logs in English.**
- **HTTP: always `wreq`, never `reqwest`** — TLS fingerprint impersonation for WAF evasion.
  An agent that suggests `reqwest` is wrong.
- **Do not unify conflicting dependencies** (dashmap 5/6, selectors 0.35/0.37/0.38): they are
  deliberate, three different crates need three different versions.
- **Do not cite line numbers.** Cite the symbol (`WreqDownloader::fetch_inner`) or the path. A
  line-number citation is invalidated by every edit above it in the file.
- **Do not copy mutable state into prose.** If it changes on its own (versions, counts, support
  lines, labels), link the source.
- **Do not touch `CHANGELOG.md`** in a work PR — there is a single writer (the Release PR).
- **Do not touch `docs/`** unless explicitly asked.

## 🏗️ Architecture

### Workspace map (5 crates)

```text
webfang/                          # virtual workspace root (no [package])
├── crates/
│   ├── webfang_core/             # domain + application + infrastructure + cli
│   ├── webfang_mcp/              # MCP server (36 tools, 9 categories)
│   ├── webfang_cli/              # the `webfang` binary (main.rs)
│   ├── webfang_test_utils/       # shared test harness (not published)
│   └── webfang_benchmark/        # benchmark harness (leaf, tooling)
```

- **The CLI code lives in `crates/webfang_core/src/cli/`** (`error.rs`, `scrape_flow.rs`,
  `args/`, `preflight.rs`, `orchestrator.rs`). `crates/webfang_cli/src/main.rs` is the thin
  binary. Look for the module, not the crate.

### Dependency direction (POLICY, enforced by a gate)

| Crate | May depend on |
|---|---|
| `webfang_core` | — |
| `webfang_mcp` | `webfang_core` |
| `webfang_cli` | `webfang_core` |
| `webfang_test_utils` | `webfang_core` |
| `webfang_benchmark` | `webfang_core`, `webfang_test_utils` |

- The **direction is `cli → core`, `mcp → core`; `cli` does NOT depend on `mcp`**. If you
  need something from the MCP server in the CLI, that is an architecture decision, not an
  import.
- **Dev tier:** any crate may target `webfang_test_utils` from `[dev-dependencies]` (it is
  the shared harness and is never published). An edge from `[dependencies]` is prohibited.
- Gate: `scripts/check_dependency_direction.sh` (runs in the `repo-guards` job of `ci.yml`),
  with its semantics harness `scripts/test_dependency_direction.sh`.

### Intra-crate layers (Clean Architecture)

```text
infrastructure → adapters → application → domain   (inward only)
```

Domain defines ports (traits) → Infrastructure implements them → Application orchestrates.
When writing new code, copy the pattern of the closest equivalent module: trait in
`application/crawler/engine.rs`, domain entity in `domain/entities.rs` with constructor +
`TryFrom` + `Display`/`Debug`/`PartialEq`, adapter in `infrastructure/`, new error in the
layer's error file.

**Avoid:** giant components. No new file should exceed ~500 lines, and if you touch one that
already does, split it. To see the current largest files in the repo:
`find crates -name '*.rs' -not -path '*/tests/*' | xargs wc -l | sort -rn | head`.

### Error stratification

The application error is `ScraperError` (`crates/webfang_core/src/error.rs`) and **everything
converges towards it**: it is not a chain of wrappers, it is a funnel.

```text
DomainError    (domain/error/domain_error.rs,  7 variants)   ─┐
InfraError     (infrastructure/error.rs,        15 variants) ─┤
CrawlError     (domain/error/crawl_error.rs)                 ─┼─ From ─→ ScraperError
DownloadError  (domain/downloader_port.rs)                   ─┘
```

- `DomainError` and `InfraError` implement `From<…> for ScraperError`: they are **inputs** to
  the application error, not a chain of wrappers. `domain/error/domain_error.rs` defines no
  `impl From` at all.
- `CliError` (`crates/webfang_core/src/cli/error.rs`) is **independent**: it has no relation to
  `ScraperError`; it maps to exit codes (`EXIT_CONFIG = 78` for a configuration error).
- `HttpError` is a **domain** enum (`domain/http_error.rs`), not an `InfraError` variant.
  `WafError` **is not an error type**: it is a `FetchBehavior` variant
  (`application/crawler/crawl_task.rs`). `ParseError` does not exist in this repo.
- Full matrix and exit-code rules: `docs/error-classification-matrix.md`.
- A new error variant enters through `From` towards `ScraperError`, and its user-facing
  message goes in Spanish.

### Fetch guard chain (MANDATORY ORDER)

Every fetch path — CLI, engine, MCP tool, benchmark, future adapter — must wire the
protections **in this order**. Each stage assumes the previous one filtered: deviating from
the order is a **bug**, not a style choice (pacing *after* a doomed request burns rate budget
on something the SSRF guard would have rejected; classifying retries *after* reading the body
wastes a full read of a response that will never be accepted).

```text
ValidUrl (entry) → rate limit (pacing) → per attempt:
    timeout → redirect policy → SSRF at the socket dial (every hop)
→ retry classification → body read
```

| # | Stage | Reference implementation | Why |
|---|---|---|---|
| 1 | Entry validation | `ValidUrl` / `try_from_url` (`domain/value_objects.rs`), built at the argv boundary | Untrusted URL text becomes a validated domain type before entering any flow. Never accept a raw `Url`/`String` as a fetch target. |
| 2 | Rate limit (pacing) | Scrape: token bucket in `scrape_urls` (`cli/scrape_flow.rs`, `build_scrape_rate_limiter`), with cancel-aware pacing via `until_ready_or_cancel_observed` (`application/rate_limiter.rs`). Crawl: `RateLimiterConfig` (`application/rate_limiter.rs`), constructed in `application/crawler/engine.rs` from `delay_ms` + budget burst | The wait happens **before** the network is touched: a cancelled wait counts as skipped, never as a blocked fetch. |
| 3a | Timeout | `Client::builder()` in `infrastructure/downloader/wreq_downloader.rs`: request timeout + connect timeout (the connect one is clamped in `infrastructure/downloader/fetch_router.rs`) | Per-request ceiling. |
| 3b | Redirect policy | `redirect_policy()` (`domain/ssrf_guard.rs`): 10-hop limit + synchronous stop when the redirect target is a forbidden literal IP | Every redirect hop re-enters the chain: it is a new dial, not a free pass. |
| 3c | SSRF at the dial | `ssrf_guard()` wrapping the `Client` (`infrastructure/downloader/wreq_downloader.rs`, implementation in `domain/ssrf_guard.rs`): resolver-level check on **every** connection; private / link-local / loopback ranges rejected before the socket exists | Nothing may connect to a forbidden address, neither by DNS name nor through a redirect hop. E2E in `crates/webfang_core/tests/ssrf_rfc1918_e2e_test.rs`: pre-socket rejection of RFC1918/CGNAT/NAT64/mapped targets, exit 69, zero outbound packets. |
| 4 | Retry classification | `fetch_inner` (`infrastructure/downloader/wreq_downloader.rs`): mid-body transients (`ConnectionReset`/`UnexpectedEof`) retry; 429 → `max(Retry-After, exponential backoff)`; 5xx → exponential; 403 → one retry with a rotated UA **only** if the UA is not pinned, capturing the rotated status; **timeouts also retry**; terminal 4xx and builder errors are reported as-is; once retries are exhausted the **last observed status** is reported, never a hardcoded one; WARN only when a retry actually happened | The classification decides the spend: sleep, rotate or fail. It runs **before** the body is read. |
| 5 | Body read | `read_body_capped` (the downloader method in `infrastructure/downloader/wreq_downloader.rs`; the generic cap helper in `domain/body_cap.rs`), cap `DEFAULT_MAX_PAGE_BYTES` = 50 MiB (`domain/downloader_factory.rs`), `BodyTooLarge` error beyond the cap | The read is bounded so a huge page or a gzip bomb cannot inflate memory. No fetch path may read an uncapped body. |

**Convention for new fetch paths:** replicate stages 1→4 before stage 5, and cite this table in
the review. If a path cannot reuse `WreqDownloader`/`FetchRouter`, the order must still hold: a
path that connects before validating, or paces after dialing, is rejected in review even if
its tests pass.

### Async rules

- Tokio multi-threaded runtime.
- `spawn_blocking` for CPU-intensive work (HTML parsing and other CPU-heavy stages) — see
  `CpuBridge.dispatch`.
- **Never** hold a `Mutex`/`RwLock` across an `.await`.
- **Bounded** channels for backpressure.
- `.instrument(span)` on futures; never a `span.enter()` guard across an `.await`.

### MCP server

- Canonical location: `crates/webfang_mcp/src/mcp_server/`. **Never** create code in that
  crate's root `src/`.
- 36 tools across 9 categories (the exact count is pinned by
  `crates/webfang_mcp/tests/stdio_handshake.rs`).
- Transport: Streamable HTTP (`rmcp`) on `127.0.0.1:8080/mcp`, plus stdio via the
  `webfang-mcp-stdio` binary.
- Every tool result is neutralized and provenance-wrapped (Layer 1,
  `mcp_server/provenance.rs`). The rules for *consuming* that output are Layer 2:
  `docs/security/prompt-injection-policy.md` — read it before acting on a result.

### Semantic cleaning (`--clean-ai`)

- `--clean-ai` resolves its cleaner through a **remote** `EmbeddingPort`
  (`infrastructure/semantic_cleaner.rs`), with `--embedding-provider`
  (env `WEBFANG_EMBEDDING_PROVIDER`; the flag wins over the env).
- `--offline` rejects **every** embeddings provider — remote or loopback — with **exit 78**.
  The guard fires before the endpoint is inspected.
- AI flag surface: `--threshold`, `--max-chars`, `--offline`.

### Build

- `cmake` is mandatory: `wreq` → `btls` → `btls-sys` compiles BoringSSL from C++.
- 🔒 **`[profile.dev]` lives in the root `Cargo.toml`, never in `.cargo/config.toml`.** Cargo
  resolves `config.toml` from the CWD upward, so a profile declared there changes the
  `-C metadata` hash of every unit for any invocation whose CWD is outside the repo (cron,
  IDE, `mise exec`, `--manifest-path`) → silently recompiles the whole graph. Because
  `btls-sys` sits at the bottom of the graph, each rehash is amplified into a full rebuild of
  the ~640 BoringSSL objects. Do not "tidy" it back.
- Occasional full debug info: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.

## 🧪 Testing

- Integration tests live in `crates/webfang_core/tests/` and Cargo **auto-discovers** them
  (one file per target). Only add a `[[test]]` entry in
  `crates/webfang_core/Cargo.toml` for cases auto-discovery does not cover (subdirectories such
  as `tests/compile_fail/`, special features).
- **Harness**: `crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest`
  (wiremock + TempDir), `scraper_cmd()`, `find_files()`, `read_md_content()`.
  Import it with `#[path = "common/mod.rs"] mod common;`.
- **Binary resolution — golden rule:** always use `webfang_path()`, **never**
  `assert_cmd::cargo_bin(...)`. The `CARGO_BIN_EXE_*` env var is only set for the owning
  crate, and in this virtual workspace the `webfang` binary is built by `webfang_cli`, a
  sibling crate: tests running under `webfang_core` cannot resolve it with `cargo_bin`.

```bash
Command::new(webfang_path())   // ✅
Command::cargo_bin("webfang")   // ❌
```

- **Snapshots (`insta`)**: every test that produces Markdown/JSON/stderr must use snapshots, not
  `assert!(output.contains("..."))`. Cycle: `cargo nextest run` (fails with `.snap.new`) →
  `cargo insta review` (review every diff) → `cargo nextest run` (green). `.snap.new` is
  gitignored: never commit pending snapshots.
- **Mandatory sanitization**: `redact_nondeterministic()` normalizes the TempDir path →
  `[TEMP_PATH]`, ISO-8601 timestamps → `[TIMESTAMP]`, wiremock ports → `[PORT]`, ANSI →
  `[ANSI]`. For extra non-deterministic fields, `insta::with_settings!({ add_filter(...) })`.
- **Six-node diagnostic** for every test you write or modify:
  1. **Observable behavior** — exercise public ports, never internal state.
  2. **Ephemeral adapters** — wiremock for HTTP, TempDir for the filesystem. No real network.
  3. **Semantic assertions** — business invariants through snapshots, not data dumps.
  4. **Effort distribution** — maximum investment in the stable domain logic.
  5. **Arrange simplicity** — an Arrange longer than 5 lines is a production design defect, not
     a test defect.
  6. **Absolute determinism** — injected time and randomness, zero flakiness.
  If the Arrange got complicated, fix the production design, not the test.
- To create an integration test: a file in `crates/webfang_core/tests/` (no `[[test]]`), the
  harness import, `webfang_path()` for the binary, snapshots for the output, and
  `cargo nextest run --test <name>`.

## 🌭 Observability

**Iron rule:** every feature, hot path or behavior change ships with observability. Code that
cannot be traced in production is not finished. The stack is the `tracing` crate + the always
available **FileTraceLayer** (`--trace-file out.jsonl`) + native correlation IDs. No external
metrics backend: neither OpenTelemetry nor any other collector is introduced.

| Situation | Requirement |
|---|---|
| New hot path / operation | `#[instrument(skip(...), fields(url = %url, ...))]` with the fields that identify the operation |
| Error path | `log_scrape_error(&err, url, stage, correlation_id, "context")` — never a bare `warn!`/`eprintln!` |
| New crawl/batch flow | Generate a `CorrelationId` at the entry point and propagate it; each unit of work gets a `.child()` |
| Long-running operation | Periodic progress log + final structured summary (`total`, `succeeded`, `errors`, `duration`, `trace_id`) |
| Async spans | `.instrument(span)` on futures; never a `span.enter()` guard across an `.await` |

- **Structured fields, not string soup:** `tracing::info!(pages = n, url = %url, "msg")`.
- **Correlation:** every event carries a 16-hex `trace_id` that is **ephemeral, an identity
  within the run** (do not persist it or join across runs; the durable cross-run identity is
  the `CorrelationId` UUID). To reconstruct a run: `ROOT=<16-hex root>; jq -c
  'select(.trace_id == "$ROOT")' <file>` — the `trace_id` is at **top level**, not
  `.fields.trace_id` (that inner one is only populated on the error path). `span_close` records
  share the `.span` name, so per-stage counts must exclude `.record == "span_close"`.
- **User-facing errors in Spanish; tracing fields and logs in English.**
- **No new metrics backends:** emit a structured `tracing` event and query it from the JSONL.
  `docs/src/debugging.md` and `scripts/analyze-trace.sh` hold the query cookbook.
- **Deterministic snapshots:** `correlation_id`/`trace_id` are internal and carry
  `#[serde(skip)]`.

## 🔬 Intelligence Stack

Two complementary tools. Pick by mission, not by habit. **They are not skills: they are
CLIs** (`codedb 0.2.5849`, `codegraph 1.6.2`) — invoke them as commands, not by loading a
skill.

| Moment | Tool | Why |
|---|---|---|
| "What is this task about?" — first orientation | **CodeDB** `context` | 1 call: keywords + symbol defs + ranked files + snippets. Replaces 3-5 sequential calls. |
| "Show me the code of X" — explore and understand | **CodeGraph** `explore` | Returns the verbatim source + call paths + blast radius in ONE call. Eliminates the grep→Read loop. |
| "Where is X defined?" — lookup | **CodeDB** `word` / `symbol` | Inverted index, O(1). |
| "Who calls X?" — tactical check | **CodeDB** `callers` | 1 round-trip: fuses the word index with the outline scope. |
| "How does X flow through the code?" — deeper | **CodeGraph** `explore` / `impact` | Call paths + blast radius summary. |
| Post-edit diagnostics | `cargo check` / `cargo clippy` | codedb has no diagnostics tool: the compiler is the gate. |
| Query a public GitHub repo (no clone) | **DeepWiki** MCP (`read_wiki_structure`, `ask_question`) | Separate remote MCP, registered by the codedb installer. codedb itself is local-only. |

**Pocket rule:** CodeDB to *find and read* (tactical, O(1)); CodeGraph to *explore and
understand* (returns source, call paths and blast radius).

### Non-negotiable gates

- Before editing a symbol → `CodeGraph explore` (callers + impact). **Never** edit blind.
- Before renaming → check **all** usages (`codedb callers` / `CodeGraph explore`).
- Before committing → `cargo check` + `cargo clippy` + `cargo fmt --all -- --check`, and
  re-read the diff.
- **Legitimate exceptions to `grep`/`rg`:** logs, CI output, `.env`/config text, and files
  outside the index — never source code.

### Absolute paths in worktrees (CRITICAL)

In a worktree, **both** tools need the **absolute path** of the worktree. With the bare name
they silently resolve to the main checkout:

| Tool | Parameter |
|---|---|
| CodeDB MCP | `project="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |
| CodeGraph MCP | `projectPath="/home/xavi/Projects/Rust/webfang-worktrees/<dir>"` |

Never use the bare repo name in a worktree: it is ambiguous between main and every worktree.

**CodeDB CLI — the root is POSITIONAL-FIRST:** `codedb <abs-root> <cmd>` (`usage: codedb
[root] <command> [args...]`). Only `mcp` accepts the reversed order (`codedb mcp <path>`).
`codedb status <path>` and `codedb reindex <path>` **ignore the trailing path** and operate
on the cwd project with exit 0 — always verify with `codedb <abs-root> status`: `root` must be
the worktree path and `head` must match `git -C <abs-root> rev-parse --short HEAD`.

## 🌳 Worktree policy

Every active branch lives in its own **sibling** directory of the repo (never inside it: in-repo
worktrees break file watchers, ripgrep and the code intelligence tools). The `.git` object store
is shared; working tree, index and HEAD are isolated.

### Iron rules

- **The CWD is the absolute boundary.** Never reach paths outside your worktree through
  `../<other-worktree>/`.
- **One worktree per session.** Never switch branches mid-task: create another worktree.
- **The mission record travels with the branch.** `odd/tasks/<feature>.md` is created and
  edited INSIDE the mission worktree, never in the main checkout: written in the main
  checkout it stays orphaned in `main` (untracked) and the finished work never reaches the
  tree (#1972).
- **`.git/worktrees/` is Git's internal state.** Never edit it by hand: use
  `git worktree add` / `remove` / `prune` / `repair`.
- **Forbidden commands:**
  - `git checkout`, `git switch` — they change the branch inside the current worktree. Use
    `git worktree add`. Since git 2.44 upstream enforces this: `git checkout -B <branch>`
    refuses (exit 128) if the branch is in use in another worktree. The hatch is
    `git checkout --ignore-other-worktrees -B <branch>` (the flag goes **before** `-B`; after
    it, the flag is parsed as a refspec and the command still fails). It is the same class of
    escape as `--force`: explicit human authorization only.
  - `git stash` / `stash pop` / `stash apply` / `stash drop` — the storage (`refs/stash`) is
    **shared by ALL worktrees**: a `pop` in one worktree can apply another session's stash. If
    you need to set something aside, commit it on a throwaway branch.
  - `git worktree move`, `git worktree lock` — use `remove` + `add`.
  - `git worktree add --force` — it bypasses Git's native guard that refuses a branch already
    checked out in another worktree. Two agents on the same branch are exactly the failure that
    guard prevents.

### Placement, naming and the identity proxy

```text
~/Projects/Rust/
├── webfang/                     # main repo (always on main)
└── webfang-worktrees/           # repo siblings (globally gitignored)
    ├── feat-auth/               # branch: feat/auth
    └── fix-crawler-timeout/     # branch: fix/crawler-timeout
```

The convention is `/ → -`: `feat/auth` → `feat-auth`. **The invariant that matters is that the
commit lands on the branch intended for the task or the PR.** The name proxy is the default
verification — run this before every commit in a worktree:

```bash
[ "$(basename "$PWD")" = "$(git branch --show-current | tr '/' '-')" ] || echo "MISMATCH"
```

### Identity-exception protocol

A failed proxy does **not** prove you are in the wrong worktree — but the local branch name
also does not prove you are in the right one. Committing under a directory/name mismatch is
allowed only when **all three** hold:

1. **Externally anchored expectation.** The intended branch is known from a source fixed outside
   this worktree *before* committing: PR metadata
   (`gh pr view --json headRefOid,headRefName`), task metadata, an upstream ref, or an explicit
   human instruction. A branch that only exists inside the worktree proves nothing.
2. **Verifiable binding.** The worktree is observably attached to that branch:
   `git symbolic-ref --short HEAD` and `git worktree list` agree (path → branch). With an
   upstream, `git rev-parse @{u}` is an extra signal.
3. **Ancestry, not equality.** The pre-commit HEAD must be at or a descendant of the expected
   tip/base — a strict `HEAD == tip` check breaks after the first commit of a sequence, so
   ancestry is the correct test.

Every exception commit records the evidence in its body (compact trailer):

```text
Worktree-Identity-Exception: directory-name mismatch
Intended-Branch: <branch>
Expected-Head: <sha>
Evidence: <external source> + git symbolic-ref + git worktree list
```

If any condition fails, MISMATCH is a hard stop.

### Per-worktree bootstrap

The `.envrc` is **per worktree**, never shared, and it is written **from the tree's own
name** (never derived from main's: a `sed` over main's left a perfectly valid
`CARGO_TARGET_DIR` pointing at MAIN's target, silently). It is **exactly 5 lines**:

```bash
TREE="$(basename "$PWD")"
cat > .envrc <<EOF
export CARGO_TARGET_DIR=\$HOME/.cache/cargo-target/$TREE
export CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER
export CARGO_LLVM_COV_TARGET_DIR=\$HOME/.cache/cargo-target/$TREE-llvm-cov
PATH_add \$HOME/.local/share/mbx/bin
EOF
direnv allow
cp ~/Projects/Rust/webfang/.env .        # .env is gitignored
```

`$TREE` is substituted when the file is written; only `$HOME` stays literal. A literal
`$TREE` left in the file makes the isolation guard reject the build with
`reason=unregistered-target`.

Then: index the worktree (`codegraph init`; `codedb "$PWD" reindex && codedb "$PWD" status`)
and, optionally, `bash scripts/seed_target.sh` (seeds an already-compiled target; `cold` is a
normal result, `refused` is the only one that is not a build). The operational detail, the
mistakes already paid for and the seed contract: **obs #2307**.

**Without both indexes the agent is blind in the worktree**: the tools resolve to the main
checkout or return nothing. Restart the MCP connection after indexing (the server caches the
registry at startup).

### Frequent commits (mandatory in worktrees)

Commit **after every completed step**: a `git mv`, a bulk replace, a passing `cargo check`, the
tests in green. Uncommitted work in a worktree can be lost silently if you lose context or a
checkout happens. The full pattern (what goes into a commit and what does not) is in the
`work-unit-commits` skill.

### Post-merge cleanup (the merge is not done until the repo is ready)

From the **main** repo:

1. **Verify the merge landed** — `gh pr view <N> --json state,mergedAt,mergeCommit`; `state`
   must be `MERGED`.
2. **Sync main (ff-only)** — `git fetch origin && git merge --ff-only origin/main`. If
   `--ff-only` fails, main diverged: **STOP** and investigate, do not paper over it.
3. **Remove the mission's worktree** — `git worktree remove ~/Projects/Rust/webfang-worktrees/<dir>`.
4. **Delete the local branch** — `git branch -D <type>/<desc>` (squash rewrites history, so the
   safe `-d` refuses; the step-1 check is your safety net). Never touch `main`, `gh-pages`,
   `backup/*` or the current branch.
5. **Prune orphaned metadata** — `git worktree prune`.
6. **Delete the mission's target dir** — by **ownership**: the directory you named (its exact
   basename, or the custom name you exported). **Never** enumerate the parent hunting for
   extra candidates: agents name their targets freely and git never sees those names.
   Re-check with a **moving window** (file count sampled twice 60 s apart with delta 0, plus
   zero live `cargo`/`rustc`/`cargo-nextest`: `pgrep -x`, one name per call).
   The removal is a **standalone** command, with the literal absolute path
   (`/home/xavi/.cache/cargo-target/<name>`), never with `$HOME`/`~` and never chained: `rm -rf ~`
   is hard-deny and blocks the whole chain. That is why steps 3-5 (git) run FIRST and complete;
   the `rm` goes after, on its own line.
7. **Verify the handoff** — `git worktree list` and `git branch -vv` with no leftovers **of the
   mission you merged**, and `git status --short` empty. Other worktrees and branches belong to
   other missions: their presence is neither a failed handoff nor your business.

### Shared vs per-worktree resources

| Resource | Shared? | Action |
|---|---|---|
| `.git/` object store | ✅ | automatic |
| Git config, hooks | ✅ | automatic |
| `Cargo.lock` | ✅ | via Git |
| `target/` | ❌ | `.envrc` + `direnv allow` per worktree; required by `scripts/ci_fast_gate.sh` |
| `.envrc` | ❌ | written per worktree; gitignored |
| `.env` | ❌ | `cp` from main |
| `.codegraph/` and `codedb.snapshot` indexes | ❌ | index per worktree |
| Seeds (`~/.cache/cargo-target/seeds/`) | ✅ read-only | consumed by `seed_target.sh`; never written by a worktree |
| Quarantine (`~/.cache/cargo-target/quarantine/`) | ⚠️ | explicitly retired objects: **never** a build target, never consumed nor cleaned during a normal bootstrap |
| `refs/stash` | ⚠️ **DANGER** | **NEVER** `git stash` |

### Reserved `~/.cache/cargo-target/` namespace

```text
~/.cache/cargo-target/
├── <worktree-target>/   ← build state owned by one agent
├── seeds/               ← RESERVED: published seed store
└── quarantine/          ← RESERVED: retired objects, never a build target
```

`seeds/` and `quarantine/` are reserved namespaces and can never be a worktree's
`CARGO_TARGET_DIR`. Do not invent new categories (`scratch/`, `shared/`, `cache/`, `tmp/`): the
guard rejects three known identities and accepts anything else, so a name nobody thought of
gets in through the back door. A denylist does not stop invention; the explicit namespace does.

### Contamination

If you detect that you operated outside your worktree, or that a `git stash pop` applied
unexpected changes:

1. **STOP** every operation immediately.
2. Do **NOT** try to clean up: no `git reset`, no force-push, no manual patching.
3. Report exactly: *"Contamination detected. Worktree: `<path>`. Intruder commit:
   `<hash>` or unexpected stash applied. Awaiting human instructions."*
4. Wait for explicit human authorization before any corrective action.

### Pushed history is immutable

Once a commit exists on `origin/<branch>`, its SHA is public and any other worktree or CI run
may hold it. Rewriting it (`git reset HEAD~N` + recommit) desynchronizes local from remote, and
the only way back is a force-push — which the harness **hard-denies** with no approval path.
The correction lands **on top**, as a new commit.

| Situation | ❌ | ✅ |
|---|---|---|
| Pushed commit is defective | `git reset HEAD~N` + recommit | a new commit that describes the fix |
| Pushed commit needs a better message | `git commit --amend` over pushed history | a new commit (or `--amend` only if it is not pushed) |
| Commit **not** pushed | — | `git commit --amend` |

⚠️ The gate is asymmetric: `git reset --hard` is hard-denied, but a bare `git reset HEAD~N` is
not gated at all. You can walk into a rewrite freely and only discover at the end that the push
is blocked.

## 🏷️ Branching, releases and support lines

- **Taxonomy — only these exist:** `main` (development; bases `feat/*`, `fix/*`,
  `refactor/*`, `perf/*`, `docs/*`, `test/*`, `chore/*`, + `ci/*`, `build/*`, `style/*`,
  `revert/*`) · `release/X.Y` (temporary stabilization for a MINOR/MAJOR, cut by the maintainer
  from an explicit main SHA; accepts only `fix/*`; deleted when `vX.Y.0` ships; a PATCH never
  creates one) · `support/X.Y` (maintenance line, created on demand from the line's latest tag
  when it needs a patch while main is ahead; accepts only `hotfix/*`; deleted at EOL — but
  `support.json` is the source of truth, not the branch) · `hotfix/*` (MUST be based on
  `support/X.Y`; PR base = `support/X.Y`, never main).
- **The state of the lines is not here.** Read `.github/support.json` (jq). The full model, the
  hotfix routing and the governance scripts: **obs #2308**.
- **Forbidden operations (review-blocking, enforced in CI):** merging `main` INTO
  `support/*` · cherry-picking `main` → `support/*` without an approved issue · tagging
  `vX.Y.Z+1` from `main` while it is ahead · a hotfix PR with base `main` · creating
  `support/*` "just in case" · editing `SUPPORT.md` by hand · running `release-plz` on a support
  branch · batching PRs with different `baseRefName` (disjoint files is not enough).
- **Versioning:** version bumps are release acts, never part of a feature/fix commit.
  Inter-crate pins stay at `^old`; only `[workspace.package]` moves. `release-plz` governs
  `main`; on `support/*` the PATCH is manual, inside the hotfix PR.

## 🔒 Permissions and limits

### Allowed without asking

- Read any file in the repo.
- `cargo check`, `cargo clippy`, `cargo fmt`, `cargo nextest run`.
- Both intelligence tools (CodeDB, CodeGraph).
- Edit inside `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Worktree management: `git worktree add`, `remove`, `list`, `prune`.
- Read-only cross-branch inspection: `git show <branch>:<file>`, `git log <branch>`.

### Ask first

- Adding or removing dependencies (`Cargo.toml`).
- Changing feature flags or profiles.
- Deleting files.
- `cargo build --release` or `cargo llvm-cov`.
- Modifying CI/CD (`.github/`).
- New files outside `crates/`, `crates/*/tests/`, `benches/`, `examples/`.
- Merging a PR (the maintainer decides the merge).

### Never

- Commit secrets, `.env` or credentials.
- `.unwrap()` in production.
- Force-push to main.
- Modify `target/`, `dist/`, `build/`.
- `git checkout` / `git switch` to change branches (use `git worktree add`).
- `git stash` in any form.
- Reach sibling worktrees through relative paths (`../feat-auth/...`).
- Commit in a worktree with a branch/directory mismatch without the exception protocol.
- Use the bare repo name for the intelligence tools in a worktree.
- Rewrite pushed history (`git reset` / `rebase` over commits already on
  `origin/<branch>`).

### Hard-deny: there is no approval path

Some command classes open a dialog the human approves. Others are **hard-deny** and never show
a dialog. Conversational authorization **cannot** change a hard-deny decision, because there is
no dialog to press.

| Class | Examples |
|---|---|
| **Confirm** (dialog → approve → runs) | `git push`, `git rebase`, forced `git branch -D`, `npm publish`, `pi remove` |
| **Hard-deny** (nothing is approvable) | force-push (`--force`, `--force-with-lease`, `-f`), `git reset --hard`, `git clean -f -d`, `rm -rf /` or `~`, `chmod -R 777`, `chown -R` |

When a hard-deny blocks: **do not re-ask for permission in conversation** (it is structurally
useless and burns a turn); **do not route around the pattern** by splitting the command,
reordering flags or wrapping it in a script (the list is deliberately non-configurable);
**report the block once**, with the exact command for the human and a non-destructive
equivalent you can execute yourself.

## 📝 Commits, PRs and CI

**Format:** `type(scope): description` — type: `feat` | `fix` | `refactor` | `test` | `docs` |
`perf` | `chore` | `revert` — scope: `cli` | `crawler` | `ai` | `mcp` | `exporter` | `http` |
`domain` | `infra`.

**The 5 PR gates** (job `Validate PR metadata`, required, no `continue-on-error`):

1. **Linked issue** — the body must carry `Closes #N` / `Fixes #N` / `Resolves #N`. For a
   partial slice of an umbrella: `Closes part of #N` (it passes validation and **does not**
   close the umbrella). A bare `Closes #N` against an umbrella with remaining scope auto-closes
   it mid-plan.
2. **The linked issue carries `status:approved`** before the PR is opened. The label is
   protected: **never** self-approve — request the maintainer's approval. If the PR was opened
   before the label landed, validation does not re-evaluate on its own: `gh run rerun <id>
   --failed`.
3. **Exactly one `type:*` label.** The label vocabulary is **not** the commit-type vocabulary,
   and it is not written here: the map lives in `scripts/auto_label_type.sh` (the single
   source of truth — `bash scripts/auto_label_type.sh --map` prints it), which is also what
   the auto-label job of `pr-validation.yml` applies from the PR title.
4. **Conventional branch name** — the regex lives in `pr-validation.yml`; do not restate it
   here (a regex citation rots: the workflow is the source of truth).
5. **No AI attribution** in the PR's commits.

**Pre-commit (must replicate CI exactly):**

```bash
cargo check && cargo clippy --all-targets --all-features -- -D warnings \
  -W clippy::cognitive_complexity -W clippy::too_many_lines \
  && cargo fmt --all -- --check \
  && env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
```

- `--all-features` is a **safety flag**: there is `chromium`-gated code whose only consumers sit
  behind `#[cfg(feature = "chromium")]`, and running clippy without it makes those imports
  look dead — `clippy --fix` would **delete** them, and `cargo check` with default features
  still passes. Never run `clippy --fix` against a narrower feature set.
- A bare `cargo fmt` is a **fixer** (it rewrites the tree and exits 0 whether it changed
  anything or not): green from it is not evidence. Verify with `--check` and commit the result.
- The fourth command replicates CI's `doc-quality` job (it denies rustdoc lints). `check`,
  `clippy` and `fmt` never document an item.

**Checklist before opening the PR:**

- [ ] `bash scripts/ci_fast_gate.sh` GREEN
- [ ] `cargo check` + `cargo clippy --all-targets --all-features` (full command above) + `cargo fmt --all -- --check`
- [ ] `cargo doc` with `RUSTDOCFLAGS=-D warnings` if library code changed
- [ ] `cargo nextest run` (at least the affected module)
- [ ] `git diff --stat main...HEAD` reviewed: only the expected symbols/files changed
- [ ] User-facing errors in Spanish; new public items carry doc comments
- [ ] Exactly one `type:*` label + linked issue **with** `status:approved` + conventional branch
- [ ] `CHANGELOG.md` **not** touched by this PR
- [ ] Worktree verified: `git branch --show-current` matches the directory
- [ ] No `git checkout`/`switch`/`stash` during the session

**Merge:** `scripts/merge-when-green.sh <PR> --queue` — waits for the required checks,
enqueues, and waits for `MERGED`. The queue is the normal path to `main`; no direct merge.
Never `--auto` or `--admin`; no strategy flag — the queue owns the merge method. `BEHIND` is
not fatal. Batches are queue groups: same base, disjoint files
(`scripts/ci_pr_overlap.sh`); the queue groups them and does not resolve content conflicts.

**Breaking change (maintainer decision):** if the queue method is squash, do not enqueue:
squash drops the `!` and the script exits 5. Land it with a merge commit outside the queue
(queue temporarily disabled), or switch the queue method to `MERGE` for that landing and
switch back. Never leave the `!` only in the PR title.

**Hooks and verification:** the `pre-commit` and `pre-push` consult the review gate; on a fresh
clone you must enable the wiring with `git config core.hooksPath scripts/githooks`. To wait
for CI prefer the non-blocking pattern (`gh run list --workflow=ci.yml …`) and never
`gh run watch` under a short timeout; a `git push` that timed out did not necessarily fail
(verify with `git ls-remote`). Detail: **obs #2316**.

**There are no stacked PRs in this repo.** `scripts/check-topology.sh` only admits the
head-prefix → base pairs of its table, and **no feature head prefix may target another
working branch** (`fix/*` → `release/X.Y` and `hotfix/*` → `support/X.Y` are admitted, because
those are versioned lines, not working branches). The path that works is **sequential
delivery**: slice 1 as a normal PR on top of `main` → merge → slice 2 from the new `main` →
normal PR → repeat. Never retarget an open PR onto a working branch to "shrink the diff": that
move made a healthy PR unmergeable once, and rebuilding on `main` is always available and always
legal.

## 🧾 Native review (RDD)

The native review is an **outside view**, never a delivery authorization: after the
acknowledgement, commit / push / PR / release stay under ordinary repository policy.

- The **candidate** is one reviewable slice — a work-unit commit or a PR slice. Never a TODO,
  never the accumulated branch.
- The full cycle (assess → status → start → consent → collect → capture → acknowledge), the
  4 lenses, the failure envelopes and the recovery paths: **obs #2213** and the
  `rdd-defect-workflow` skill.
- **Consent is per candidate**, it is not the global switch's kill switch. Never fabricate an
  approval and never re-enable the switch for the user.

## 🗺️ Skills and CLIs

- **Skill index:** `.atl/skill-registry.md` — generated **per tree** and gitignored: it does not
  exist in a fresh clone until you run `gentle-ai skill-registry refresh` in that tree. It is
  an **index** for delegators: the `SKILL.md` is the source of truth and you must **inject
  exact paths**, not summaries. The registry skips `sdd-*`, `_shared` and `skill-registry` by
  contract.
- **CodeDB and CodeGraph are CLIs, not skills** — see § Intelligence Stack.
- Relevant skills (the registry defines the scope, not this file): `rust-skills` (265 rules /
  26 categories; category prefixes: `own-`, `err-`, `async-`, `api-`, `test-`, …),
  `branch-pr`, `work-unit-commits`, `chained-pr`, `rdd-defect-workflow`,
  `cognitive-doc-design` (user scope) and `codebase-refactoring`, `dry-refactoring`, `jscpd`,
  `webfang-architecture` (repo scope).
- **Every delegation to a subagent must include:** the skill paths to read, the intelligence
  mandate (explore impact before editing; `cargo check` before returning), the exact
  verification commands, and the absolute worktree path.

## 💾 Engram (correct usage)

The Engram server resolves the active session from the working directory. With several
concurrent agent sessions in this repo, `mem_save` / `mem_judge` can fail with `multiple
active runtime sessions match the current project and directory`.

- **Never invent a `session_id`.** Register one with `mem_session_start` (explicit unique ID)
  and pass that same `session_id` to every `mem_save` / `mem_judge` of the session.
- **Close the session when you are done** (`mem_session_end(id, summary)`). Since engram v2.1.0
  every registration writes a 30-minute runtime lease, and an expired lease is excluded from
  candidate resolution: a session whose runtime never renews stops being a candidate **by
  itself**, 30 minutes after its last renewal. Closing earlier only matters while the runtime
  is still alive.
- If `mem_save` returns `judgment_required: true`, judge **each** candidate with **its own**
  `judgment_id` (the one in `candidates[]`), never with the top-level one.
- On **any** engram error, run `engram --help` first (then `engram <cmd> --help` for the command
  that failed) before retrying or inventing flags. The CLI is the fallback when the MCP fails:
  `engram save`, `engram search`, `engram context`.
- Diagnosing the session collision:
  `engram doctor --check ambiguous_active_runtime_sessions --project webfang` (diagnostic
  only; `doctor repair` does not cover that check). If the collision is between two
  **genuinely alive** sessions, that is by design: end the one that finished, or pass
  `session_id`.
- **Project knowledge lives here**, not in the repo: decisions, direction, audit findings and
  reasoning. The repo holds the **rules** and their sources of truth.
  (Owner decision: obs #370.)
