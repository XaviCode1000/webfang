# #1294 — MCP contract-boundary remnants: exploration brief (SDD phase 1)

Author: AGENT-2 (worktree `fix-mcp-boundary`, branch `fix/mcp-boundary`, base `08eee306`).
Status: **exploration only — no production code, no build executed.**
Indexes verified for this worktree: `codegraph` (485 files, project resolves to the worktree),
`codedb "$PWD" status` → root == worktree, head == 08eee306.

Every claim below is a `file:line` citation from this tree or from the pinned framework source
(`rmcp 1.8.0`, Cargo.lock; `~/.cargo/registry/src/index.crates.io-*/rmcp-1.8.0`). Runtime
confirmation is the repro step and needs one heavy build turn (see §Repro).

---

## Item-by-item findings

### P5-1 — unknown JSON-RPC method → 422 instead of `-32601`

**Verdict hypothesis: framework-owned, and the reported 422 is a probe artifact, not a missing
method-not-found implementation.** Two different cases were conflated:

| Case | What happens | Owner + citation |
| :--- | :--- | :--- |
| POST on a session-less `/mcp` whose message is not `initialize` | `422 Unexpected message, expect initialize request` (plain text, never reaches JSON-RPC) | rmcp, `streamable_http_server/tower.rs:1149,1160` → `common/server_side_http.rs:164-169` (`unexpected_message_response` = `UNPROCESSABLE_ENTITY`) |
| unknown method **inside an established session** | JSON-RPC error `-32601` over HTTP 200 | rmcp `handler/server.rs:329` (`on_custom_request` default → `ErrorCode::METHOD_NOT_FOUND`); webfang's `McpHandler` does not override it (`mcp_server/mod.rs:157-199`) |

`stateful_mode` defaults to `true` (`tower.rs:113`), and webfang builds the service with
`Default::default()` config (`mcp_server/server.rs:93-98`), so the session gate is armed.

**Test-gap evidence (webfang-owned part):** the existing test accepts the wrong behavior as a
pass — `crates/webfang_mcp/tests/mcp_behavioral_test.rs:595-625` asserts `-32601` *only if*
`status.is_success()` and then comments "HTTP error status is also acceptable". A probe that never
handshakes therefore passes forever. The same file's `test_no_session_id_handled:580-591` also
lists 422 as acceptable; and `mcp_lifecycle_test.rs:352-393` **pins 422 as the contract** for the
missing-session case (its body assertion is `body.contains("initialize")`).

Disposition options: (a) close as framework-owned + a strict test that handshakes first and
asserts `-32601`, and tighten the permissive assertion; (b) add a response-translating tower layer
that rewrites rmcp's 415/422 into JSON-RPC errors. **Recommend (a)** — (b) fights the framework,
re-litigates its status codes on every rmcp bump, and would contradict `mcp_lifecycle_test.rs`
which already documents 422 as intended.

### P5-2 — JSON-RPC 1.0 → 415 instead of a protocol error

**Framework-owned, exactly located.** rmcp deserializes the body with
`expect_json` → `serde_json::from_reader::<_, ClientJsonRpcMessage>`; **any** deserialization
failure (including the missing/incorrect `jsonrpc:"2.0"` discriminator that `JsonRpcVersion2_0`
requires) is answered with `UNSUPPORTED_MEDIA_TYPE` and the text
`fail to deserialize request body {e}` (`common/server_side_http.rs:170-186`). Semantically 400
would be right; 415 is rmcp's own mislabel. Two *other* 415/406 gates precede it in the same
handler: `Accept` must contain both `application/json` and `text/event-stream` (else 406), and
`Content-Type` must start with `application/json` (else 415) — `tower.rs:1018-1050`.

Disposition: close framework-owned with a named test pinning 415 + the exact reason, and a
doc-comment in `mcp_server/server.rs` naming the three HTTP-level gates we do not own. Same
recommendation as P5-1: no translation layer.

### P6-3 — SSRF divergence: CLI sitemap works on loopback, MCP blocks

**The *policy* does not diverge; the *layers and their kill-switches* do.** Both stacks forbid
loopback through the same predicate — `domain/ssrf_guard.rs:96-107` (`v4.is_loopback()` …), which
MCP imports (`mcp_server/ssrf.rs:12`) and core uses for literal-IP entry rejection
(`reject_forbidden_literal_url`, `ssrf_guard.rs:373-391`) and for the connect-time resolver
(`infrastructure/ssrf.rs:207-215`). The discovery client is guarded in both stacks
(`application/http_client/factory.rs:107` → `secure_client`, used by
`crawler/sitemap_discovery.rs:221-233`).

The real deltas:

1. **MCP adds one extra entry layer that CLI does not have**: `validate_url_no_ssrf` performs its
   own `lookup_host` DNS round-trip and rejects forbidden answers with
   `McpError::invalid_params` (`-32602`, Spanish message) — `mcp_server/ssrf.rs:41-105`. CLI's
   entry guard rejects **literals only** (hostnames are deferred to connect-time resolution,
   `ssrf_guard.rs:361-365`). So a hostname that resolves to `127.0.0.1` fails at MCP entry with a
   clean typed error, and fails in CLI at dial with an infra error → different error surface, same
   decision. The MCP doc-comment already calls itself "fast-fail typed UX … NOT the enforcement
   point" (`ssrf.rs:28-37`) → **intended layering, not a bug.**
2. **The disarmer sets differ, and one name is a trap.** Core exposes three per-layer envs
   (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD` / `_REDIRECT_GUARD` / `_RESOLVER`,
   `ssrf_guard.rs:58,66,82`); MCP has one, `WEBFANG_MCP_DISABLE_SSRF`, that lifts **only** the MCP
   pre-check. Lifting loopback for MCP therefore needs **two** variables where CLI needs one —
   which is exactly what the MCP harness does (`tests/mcp_behavioral_test.rs:37-40`), while the
   CLI harness sets one (`tests/common/cli_harness.rs:145-150`). `bin/mcp_server_http.rs:108-111`
   logs "SSRF protection disabled (test mode)" when only that one is set, which overstates it.
3. Corollary, verified in the framework comment: IP literals never reach the validating resolver
   ("wreq short-circuits IP-literal hosts before calling any custom resolver",
   `infrastructure/ssrf.rs:105-108`) — which is why the separate entry guard exists (#1217) and
   why one env per layer is the right shape.

**Proposed disposition (policy = intended, fix the honesty of the knob + document):** keep the
block; when `WEBFANG_MCP_DISABLE_SSRF` is set, log at WARN which layers remain armed and name the
core envs; state the 4-layer/4-knob matrix in one doc; pin with named tests (MCP blocks loopback
by default; the MCP env lifts only its own layer; CLI↔MCP reach the same verdict on the same
input). No policy relaxation, no shared-knob unification that would touch the guard-chain order.

### P6-5 + F-10 — CLI exit codes vs MCP `isError`; permanent-vs-transient taxonomy partial

**F-10 as stated is stale in one specific way: the taxonomy helper exists, is total where the
contract says it must be, and is called by nobody.**

* `CliExit` — 10 variants, sysexits-shaped (`cli/error.rs:129-161`).
* `default_exit_code_for_class` / `cli_exit_for_class` (`cli/error.rs:177-197`) return `None` for
  `PermanentFatal` and `DomainRecoverable` **by contract** (variant-dependent / outcome-dependent),
  documented against `docs/error-classification-matrix.md`.
* Production call sites of either helper: **zero** (grep over `crates/` returns only the
  definitions and their own unit tests, `cli/error.rs:491-551`). Exit decisions live in ~138
  hand-written `CliExit::` constructions.
* The one class-driven site I found, `cli/export_flow.rs:203-238`, consumes `classify()` for
  **control flow** (abort vs fallback vs count), not for an exit code; forcing the helper there
  would be wrong.

⇒ "Finish the taxonomy" as a mass adoption is a CLI-wide refactor with real exit-code risk, and it
is not the MCP boundary this issue is about. Recommend: **document the boundary + add a sync
guard** — a table-driven test asserting the matrix doc's class→exit rows equal
`default_exit_code_for_class`, and that exactly the two variant-dependent classes return `None`.
That converts "partial" from an unfalsifiable claim into a checked invariant.

**P6-5 (CLI exit vs MCP `isError`)** is the same boundary seen from the other side: MCP has no
exit channel at all. Today: 40 `CallToolResult::error` sites vs 18 `McpError::*` sites across the
handler modules (per-file counts collected), i.e. the "domain failure vs unroutable request" split
rmcp itself prescribes (`handler/server.rs:280-297`) is applied ad-hoc per handler with no
project-level statement of which classes project to which channel. Recommend documenting the
projection (one table: ErrorClass → CLI exit → MCP channel) and pinning 2–3 representative
equivalences with named tests (SSRF, robots-blocked, network-failure), not inventing a new MCP
error envelope in a bug issue.

### NS-01 — `McpState.inspector` never wired in MCP production

**Confirmed real gap, cheapest fix in the mission.**

* Field + default `None`: `mcp_server/state.rs:83-84,164`; builder `with_inspector`
  `state.rs:205-208`; tests assert the *default* is `None` (`state.rs:521-545`) — so today's tests
  pin the gap.
* Consumer: `handlers/scraping.rs:155` `let inspector = self.state.inspector.as_deref();` →
  passed to `scraper_service::scrape_with_config` (`scraping.rs:160-170`). With `None`, CSS-selector
  diagnostics silently degrade.
* Production call sites of `with_inspector`: **none**. The only production wiring of the real
  inspector is the CLI: `webfang_cli/src/main.rs:43,428` → `DefaultDomInspector::new()`.
* Both MCP binaries build state without it: `src/bin/mcp_server_http.rs:100-103`
  (`McpState::from_container(…).with_downloader(…).with_export_roots(…)`), and the stdio binary
  likewise.

Fix: wire `DefaultDomInspector` in both composition roots (mcp→core is an allowed direction) + a
named test that the built state carries `Some(inspector)`. Note the existing default-`None` unit
tests must be re-read so the change is not mistaken for breaking them (`with_inspector` stays
opt-in at the builder level; the *binaries* are what change).

### NS-02 — `discover_sitemap` advertises a sitemap URL, returns page URLs

**Confirmed contract lie, and the wiremock suite freezes the behavior, not the name.**

* Advertised: `#[tool(description = "Auto-discover a website's sitemap URL by checking robots.txt
  and common locations (/sitemap.xml, /sitemap_index.xml, etc.).")]` and doc-comment
  "Auto-discover sitemap URL from robots.txt or common locations" —
  `handlers/scraping.rs:633-637`.
* Actual: calls `crawl_with_sitemap_resolved(url, None, …)` and returns
  `discovered[].url` — the **pages inside the sitemap**, serialized as a JSON array of strings
  (`scraping.rs:655-676`). The real sitemap-URL discoverer, `discover_sitemap_url`, is **private**
  (`application/crawler/sitemap_discovery.rs:513`), so the tool cannot currently even report it.
* The behavior is frozen by an explicitly named contract: `sitemap_discovery.rs:23-24`
  ("Kept for the frozen `discover_sitemap_wiremock` contract") +
  `tests/discover_sitemap_wiremock.rs:1-21` asserts `Vec<String>` of 2 entries.

Disposition options: (a) correct the tool description + doc-comment to what it returns (zero wire
change, no rename); (b) change the payload to `{sitemap_url, urls}` (needs plumbing the resolved
URL out of `crawl_with_sitemap_resolved`, breaks the frozen wire contract); (c) rename the tool
(breaking for every consumer). **Recommend (a)** and record (b)/(c) as deliberate non-goals in the
commit message, because the issue text ("returns the sitemap URL (or rename)") presumes a wire
change the frozen contract argues against.

### NS-04 — `concurrency`: MCP-only param whose advertised default is wrong

**Confirmed, statically provable.**

* Advertised: `/// Concurrency limit (default: 4)` on `ScrapeBatchParams::concurrency`
  (`mcp_server/params.rs:363-364`). schemars renders the doc comment as the schema `description`,
  so "default: 4" is what the agent sees; there is no `#[schemars(default …)]`, so no machine-
  readable `default`, and no `minimum`/`maximum` even though `validate()` enforces `1..=64`
  (`params.rs:405-407`).
* Effective: `handlers/scraping.rs:255-258` — `ScraperConfig::default()` unless overridden, and
  `domain/config.rs:113` sets `scraper_concurrency: 3`. It is used unclamped at
  `application/scraper_service.rs:779` (`buffer_unordered(config.scraper_concurrency)`).
  **Announced 4, applied 3.**
* Why the existing parity machinery misses it: the bridge documents `urls`/`concurrency` as
  MCP-only, hence outside the OptionsSpec parity table (`schema_bridge.rs:99`), and
  `tests/options_spec_parity_test.rs` only asserts parity for *overlapping* params
  (`options_spec_parity_test.rs:1-16`). So the one param with no CLI twin has no default check.

Fix: make the advertised default derive from the same source the handler uses (bridge override or
a shared const), announce bounds 1..=64 in-schema (mirroring `require_range_u64`), and add a named
test "announced default == effective default when the field is omitted". Small, self-contained,
and it extends machinery that already exists (`default_overrides_for_tool`,
`merged_input_schema`).

**The fix already has a canonical precedent in this tree** — which changes the shape of the work:
`schema_bridge.rs:123-158` defines `DefaultOverride::{Set,Unset}` for exactly this bug class, and
`#940 F1/F2` states the rule ("schema truth outranks spec-default propagation", `schema_bridge.rs:115-120`).
`crawl_site` already advertises `handlers::scraping::CRAWL_SITE_DEFAULT_MAX_DEPTH/_MAX_PAGES`
instead of the spec's 2/10, and `scrape_batch` already carries one override
(`("delay_ms", Set(0))`, `schema_bridge.rs:155-157`). So NS-04 = add a
`SCRAPE_BATCH_DEFAULT_CONCURRENCY` const next to the existing ones, register
`("concurrency", Set(…))` on `scrape_batch`, correct the `params.rs` doc comment, and add the parity
row. Cost: minimal, no new mechanism.

**Bounds are the one part that would need a new mechanism.** `apply_default_overrides` only writes
`"default"` (`schema_bridge.rs:172-190`); there is no way today to advertise `minimum`/`maximum`,
and `concurrency` has no OptionsSpec twin to inherit them from. A `SetBounds` variant is ~15 lines
plus parity coverage. Treat it as an explicit optional sub-slice, not part of the default fix.

### Side finding (out of scope, flagged for triage, NOT for this issue)

The **stdio** binary builds its state with neither `with_downloader` nor `with_inspector`:
`src/bin/mcp_server_stdio.rs:198` → `McpState::from_container(container).with_export_roots(…)`,
while the HTTP binary injects the bounded shared downloader
(`src/bin/mcp_server_http.rs:100-103`). #1120's rationale is explicitly about *long-lived servers*
("the server process outlives any single crawl", `mcp_server/mod.rs:70-73`) and a stdio server is
exactly that, so stdio appears to keep the per-call connection-pool churn #1120 was filed to remove.
Separate root (composition/parity of transports), separate issue — NS-01's slice D only touches the
inspector unless you tell me otherwise.

---

## Slice plan (by root, each independently reviewable, all < 400 lines)

| Slice | Root | Items | Deliverable shape | Est. |
| :--- | :--- | :--- | :--- | :--- |
| **A** | transport (framework-owned) | P5-1, P5-2 | tests only + `server.rs` doc-comment naming the 3 HTTP gates we don't own; tighten the 2 permissive assertions | ~150–200 |
| **B** | policy (SSRF layering) | P6-3 | honest WARN on partial disarmer + named layer tests + doc section | ~150–250 |
| **C** | advertised contract (schema/tool description) | NS-04, NS-02 | schema_bridge default+bounds, parity test, description fix | ~150–250 |
| **D** | composition root | NS-01 | inspector wiring in both binaries + test | ~80–150 |
| **E** | error boundary (docs + invariant) | P6-5, F-10 | matrix↔helper sync guard test + projection table | ~100–200 |

A/C/D are pure-code; B/E are the policy+docs slices. None touches the guard-chain order, `wreq`, or
`CHANGELOG.md`. Batch-merge eligibility: all five must stay file-disjoint (`ci_pr_overlap.sh`).

## Repro plan (needs exactly one heavy turn)

Static evidence is already in hand for all 8 items; what's missing is runtime confirmation of
P5-1's `-32601`-after-handshake and of the P6-3 knob matrix. Cheapest vehicle is not a manual
server: it is the existing in-process harness (`build_mcp_router` + `axum::serve` on an ephemeral
port, `tests/mcp_behavioral_test.rs:63-88`) extended by one focused test file that fails before
the fix and passes after. One build, one run, and the repro doubles as the regression net:

```bash
export CARGO_TARGET_DIR=~/.cache/cargo-target/fix-mcp-boundary
env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN \
  CARGO_BUILD_JOBS=2 \
  RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads" \
  cargo nextest run -p webfang_mcp --features mcp --test <repro_test_name>
```

Cost warning: the isolated target dir is cold, so this first invocation pays BoringSSL + the full
dependency graph. `~/.cache/cargo-target/webfang/debug/webfang` shows an mtime of 05:07 today, i.e.
another agent is likely using the shared cache; I will not seed from it with hardlinks without
explicit authorization.

## Open decisions for the orchestrator

1. Approve dispositions: P5-1/P5-2 = framework-owned + strict tests, **no** response-translation
   middleware; P6-3 = intended policy + honest-knob fix; F-10 = document + sync guard instead of a
   CLI-wide `cli_exit_for_class` adoption; NS-02 = description fix only.
2. Artifact home: this file sits at `docs/research/1294-mcp-boundary-exploration.md`; there is no
   `openspec/` in this tree. Approve the location or name the right one (new file outside
   `crates/`/`tests/` needs your authorization per AGENTS.md §Safety).
3. Turn for the single repro build+run, with a long window.
