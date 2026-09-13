# SSRF defense layers and their kill-switches

Written for #1294 (P6-3). One page, so "which knob lifts which layer, and what is still
protected after I set it" stops being a guess. It is also the reason the CLI and the MCP
server look like they disagree about `127.0.0.1` when they do not.

Nothing here is a relaxation: the deny list is shared and every layer stays armed by
default. The disarmers below exist for tests and for an operator who deliberately points
the tool at an internal target.

## The predicate is one, the layers are four

`domain/ssrf_guard.rs::is_forbidden_ip` is the single deny list — loopback, private,
link-local, unspecified, broadcast, reserved, CGNAT (`100.64.0.0/10`), IPv6
loopback/unique-local/link-local, plus IPv4-mapped, IPv4-compatible, NAT64
(`64:ff9b::/96`), 6to4 (`2002::/16`) and Teredo (`2001:0000::/32`, fail-closed).
The CLI and MCP both consume it. They differ in how many layers consult it.

| # | Layer | Lives in | Sees | CLI | MCP |
| :-- | :--- | :--- | :--- | :--- | :--- |
| 1 | Entry pre-check (resolves the host itself) | `webfang_mcp/src/mcp_server/ssrf.rs` → `validate_url_no_ssrf` | literals **and** hostnames | — (CLI has no equivalent: it validates literals only) | armed by default |
| 2 | Literal entry guard | `webfang_core/src/domain/ssrf_guard.rs` → `reject_forbidden_literal_url`, called from `cli/scrape_flow.rs`, `infrastructure/downloader/fetch_router.rs`, the engine's robots gate (`infrastructure/crawler/robots_utils.rs`), and — since #1301 — the MCP scrape path (`application/scraper_service.rs` pre-check) | IP literals only | armed by default | **armed by default** |
| 3 | Connect-time validating resolver | `webfang_core/src/infrastructure/ssrf.rs` → `ValidatingResolver`, installed by `SsrfGuard::secure_client` | hostnames only — wreq short-circuits literal hosts and never calls a resolver | armed by default | armed by default |
| 4 | Redirect guard | `domain/ssrf_guard.rs::redirect_policy` | literal redirect targets (belt-and-suspenders; hostname hops go through layer 3) | armed by default | armed by default |

Layer 2 and layer 3 are complementary, not redundant: a literal never reaches the
resolver, and a hostname's answer set is only visible at resolution time. That is why
`#1217` added layer 2 instead of widening layer 3. The practical consequence is that the
hatch a local target needs depends on how the target is *spelled*: an IP literal is cut by
layer 2 (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD`), a name like `localhost` by layer 3
(`WEBFANG_DISABLE_SSRF_RESOLVER`), and a target reachable by both spellings needs both.

**The sitemap path is now fully covered (#1382, previously the one uncovered call
site).** `--use-sitemap` used to open real sockets against a loopback seed
(measured: 18 requests with every guard armed) because
`sitemap_discovery.rs::build_discovery_client` goes through
`application/http_client/factory.rs`, which installs layers 3 and 4 only. The
fix restores the AGENTS.md guard-chain order with three layer-1 cuts, all
pre-socket: the seed is guarded in `crawl_with_sitemap_internal` before the
discovery client exists, the resolved sitemap target (explicit `--sitemap-url`
or a robots.txt `Sitemap:` directive) in `resolve_sitemap_url`, and every URL
the parser fetches (initial target + index children) in `parse_with_depth` via
the typed `SitemapError::SsrfLiteralRejected` → `CrawlError::InvalidUrl` (exit
69, same Spanish copy as every other surface). Pinned by
`sitemap_ssrf_e2e_test` (zero outbound requests) and the unit guards in
`sitemap_discovery` tests.

## What a refusal looks like from the outside

Every layer refuses **before a socket exists**, so there is no connection error to read and
nothing to retry. What the caller sees depends on who was holding the URL — same seed, same
policy, five different-looking outcomes:

| Surface | Outcome | Exit |
| :--- | :--- | :--- |
| CLI crawl / scrape / `--single-page`, loopback seed | `URL inválida: SSRF detectado: la IP 127.0.0.1 … está prohibida …` | 69 |
| `--dry-run`, loopback **literal** seed | `Warning: SSRF detectado: …`, naming the cause and both hatches; no `Dry-run:` line (#1391) | 2 |
| `--dry-run`, **hostname** seed resolving into a forbidden range | `Dry-run: 0 URL(s) would be scraped:` — the refusal arrives as `DNS error: name resolution failed`, indistinguishable here from a name that does not exist | 0 |
| MCP crawl/scrape tools, loopback seed | layer 1 answers first: `-32602` / `isError` carrying `SSRF detectado` | — |
| `discover_urls_unified` / `discover_urls_recursive` (library) | `Ok` with an empty `Vec` | — |

The literal `--dry-run` row used to be the operator's surprise: since #1369 plain DOM discovery
pays the whole guard chain — it had ridden a knobless engine entry that built no downloader —
and a refused seed came back as a technical success with zero URLs, because
each refused URL is a legitimate skip inside a crawl. #1391 moved the diagnosis to the boundary
that reports it: the preview asks the entry guard its own verdict
(`domain::ssrf_guard::seed_guard_refusal`, the same check `reject_forbidden_literal_url` makes
minus its `WARN`) and answers with `CliExit::EmptyDiscovery`, the null-result code the sitemap
arms already use. The library contract is deliberately unchanged — `Ok` with an empty list —
and both halves are pinned together: `engine_options_1369.rs`'s
`issue_1381_guard_refused_preview_maps_to_exit_2_while_the_library_stays_ok` asserts `Ok` +
empty AND exit 2 in one test, while
`seed_guard_refusal_agrees_with_the_enforcing_guard_on_every_form` in
`domain/ssrf_guard.rs` keeps the diagnostic verdict from drifting from the enforcement,
armed and hatched. Both sit alongside
`discovery_plain_run_rejects_loopback_seed_with_ssrf_guard_on`, which still asserts
`Ok`, zero URLs and **zero requests** reaching the mock. The refusal itself was never hidden:
the `WARN` (`SSRF literal-IP target rejected at entry (no socket opened)`) prints at default
verbosity, and the engine counts the seed (`crawl completed … errors: 1`) — what changed is that
the exit code now agrees with them. User-facing copy: `docs/src/troubleshooting.md` →
"A local or internal target discovers nothing".

## Kill-switches

| Variable | Lifts | Read |
| :--- | :--- | :--- |
| `WEBFANG_MCP_DISABLE_SSRF=1` | layer 1 (MCP only) | per call |
| `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` | layer 2 | per call |
| `WEBFANG_DISABLE_SSRF_RESOLVER=1` | layer 3 | **once, at client construction** — long-lived clients keep a consistent policy |
| `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1` | layer 4 | once, per `secure_client` call |

Only the exact value `1` disarms anything; `0`, `true`, `yes` all leave the layer armed.

## Test-isolation rule

- Writers: use `EnvGuard::{set,remove,with,clean}` or `env_lock()` for permanent setup (#1126).
  One-time/permanent seeding or cleanup where restore-on-drop does not fit goes through
  `webfang_test_utils::{env_set,env_remove}`, which acquire ENV_LOCK themselves — never a manual
  `env_lock` + raw-mutation pair.
- Readers: hold `EnvGuard` for full read-or-assert window (even pure readers use `EnvGuard::clean`) or `#[serial]`; see #1308.
- Never nest `EnvGuard` in `env_lock()` (mutex not reentrant).
- Spawned children exempt: `Command::env`/`env_remove` fix child env at spawn (see `sanitize_env` in `cli_harness.rs`).
- Canonical names live in `webfang_core::domain::ssrf_guard` (#1348): `WEBFANG_MCP_DISABLE_SSRF_ENV`
  (MCP layer 1) joins `DISABLE_ENTRY_GUARD_ENV` — reference the constants, never restate the strings.
  `EnvGuard::ssrf_hatches_off()` lifts MCP layer 1 + core layer 2 in one step (the double hatch above).
- Mechanical enforcement (#1349): clippy `disallowed-methods` denies `std::env::set_var`/`remove_var`
  in the webfang_core and webfang_mcp src trees, and `env_reader_without_guard_fails_fast`
  (webfang_test_utils) source-scans every `crates/*/src` and `crates/*/tests` file except the owner.
  Reintroducing a raw mutation fails the build by construction.
- #1334 audit outcome: the one confirmed reader-without-guard case was fixed in #1308;
  `index_children_all_fail_exits_69` reads no env (the old #1331 flake was the pre-#1317
  snapshot-based version); the unlocked `env::vars()` reads in `sanitize_env` and the parity test's
  child-env sanitization only REMOVE the same `WEBFANG_*`/`AI_MODEL_ID` names they read, into a
  spawned child whose env is fixed at spawn (exempt by the rule above); `tokio_console_smoke_test`
  reads operator-only `WEBFANG_CONSOLE_*` overrides that no other test writes.

## What MCP actually does with `WEBFANG_MCP_DISABLE_SSRF`

It removes layers 1 and — because layer 2 never sat on this path — **every entry-level
check of an IP literal**. Layers 3 and 4 stay armed, and they cover hostnames and
redirect targets, so:

| Target, with only `WEBFANG_MCP_DISABLE_SSRF=1` | Outcome |
| :--- | :--- |
| `http://127.0.0.1:6379/` | reaches the socket (the failure is a connection error, not an SSRF refusal) |
| `http://redis.internal:6379/` resolving to `127.0.0.1` | refused at connect time by layer 3 |
| a public host that redirects to `http://169.254.169.254/` | stopped by layer 4 (literal) or layer 3 (hostname) |

The CLI has the same shape one layer over: `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` removes
its only literal check, and that is precisely how `tests/common/cli_harness.rs` drives
`127.0.0.1` wiremocks. Neither stack's disarmer is a partial measure on literals, and
that is the fact a reader needs before concluding "MCP blocks loopback, the CLI does not".

**Consequence for test harnesses:** an MCP harness that scrapes a loopback literal must
set **two** variables (`WEBFANG_MCP_DISABLE_SSRF` and `WEBFANG_DISABLE_SSRF_ENTRY_GUARD`)
because MCP has one more entry layer; a CLI harness needs one. `tests/mcp_behavioral_test.rs`
already does exactly this.

## Pinned, not remembered

`crates/webfang_mcp/tests/mcp_ssrf_knob_matrix_test.rs` asserts, in order: the shared
deny-list verdict for both stacks on a table of literals; that the MCP switch alone lets a
literal through; that the core switch alone does **not** disarm MCP's layer. Change one
layer's scope and that suite is where it surfaces.

The CLI↔MCP parity claim above is pinned executably on both sides, by name:
`mcp_entry_env_has_the_same_literal_scope_as_the_cli_entry_env` (MCP's switch) and
`entry_guard_hatch_requires_exact_value_one` in `domain/ssrf_guard.rs` (the CLI's). Each
test names the other in its doc comment, so weakening one scope without the other is a
deliberate edit rather than an oversight.

## Related

- `docs/error-classification-matrix.md` — how a refusal becomes an exit code (CLI) or an
  `isError` / JSON-RPC error (MCP).
- AGENTS.md → "Fetch guard-chain": the order these layers must be wired in. A path that
  connects before validating is rejected in review regardless of its tests.
