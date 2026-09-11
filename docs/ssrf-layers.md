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
| 2 | Literal entry guard | `webfang_core/src/domain/ssrf_guard.rs` → `reject_forbidden_literal_url`, called from `cli/scrape_flow.rs`, `infrastructure/downloader/fetch_router.rs`, and — since #1301 — the MCP scrape path (`application/scraper_service.rs` pre-check) | IP literals only | armed by default | **armed by default** |
| 3 | Connect-time validating resolver | `webfang_core/src/infrastructure/ssrf.rs` → `ValidatingResolver`, installed by `SsrfGuard::secure_client` | hostnames only — wreq short-circuits literal hosts and never calls a resolver | armed by default | armed by default |
| 4 | Redirect guard | `domain/ssrf_guard.rs::redirect_policy` | literal redirect targets (belt-and-suspenders; hostname hops go through layer 3) | armed by default | armed by default |

Layer 2 and layer 3 are complementary, not redundant: a literal never reaches the
resolver, and a hostname's answer set is only visible at resolution time. That is why
`#1217` added layer 2 instead of widening layer 3.

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
- Readers: hold `EnvGuard` for full read-or-assert window (even pure readers use `EnvGuard::clean`) or `#[serial]`; see #1308.
- Never nest `EnvGuard` in `env_lock()` (mutex not reentrant).
- Spawned children exempt: `Command::env`/`env_remove` fix child env at spawn (see `sanitize_env` in `cli_harness.rs`).

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
