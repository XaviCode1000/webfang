# Feature: CLI validation fail-closed (issue #1813)

## Objective

Close the four fail-open CLI validation gaps (plus the silently-skipped MCP
test suite) so invalid input is rejected with a typed Spanish config error
(exit 64) instead of panicking, warning-and-defaulting, or parsing-and-ignoring.

## Problem

Four CLI surfaces accept values they should reject and silently do nothing
(`status:approved`, `type:bug`, verified against `main` 2026-10-04):

1. `From<Args>` panics on an invalid rate-limit burst
   (`crates/webfang_core/src/cli/args/mod.rs:492-500`) — two
   `unwrap_or_else(|_| panic!(…))` in a `From` impl. The comment claims
   "the binary validates via `normalize()` before this conversion".
   **CORRECTION (verified in T1):** `normalize()` DOES exist
   (`cli/preflight.rs:772`), and `webfang_cli/src/main.rs:116` calls it
   before the `From` at `:121` — so for the shipped binary the cited safety
   net was substantively true. The panics were reachable only from a
   *programmatic* `Args` built without `ArgSources::capture`.
   The real fail-open is one layer down: `parse_rate_limit_burst`
   (`cli/args/crawler.rs:66-75`) returned `Ok(None)` + a pre-logging
   `record()` note for any non-numeric input, so `--rate-limit-burst banana`
   degraded to the derived default in silence. (Numeric garbage — `0`,
   out-of-`u32` — already hard-errors in Spanish.)

> **Exit-code convention (corrected twice — read carefully).** The issue text
> says "typed config error (exit 64)". That is wrong about this codebase:
> `CliExit::ConfigError` → **78** (`EXIT_CONFIG`, sysexits `EX_CONFIG`) and
> `CliExit::UsageError` → 64 (`cli/error.rs:30-36,383-385`).
>
> **The real rule, established in T2: the exit code is decided by WHERE the
> bound is enforced, not by what is invalid.**
>
> | Stage | Mechanism | Exit | Examples |
> |---|---|---|---|
> | clap boundary (`value_parser`) | `str_fn` binding → `ValueValidation` → `UsageError` | **64** | `--max-tokens` (T2), `--max-pages` 100k cap, `--timeout-secs`, `--download-concurrency`, `--threshold`, `--url` |
> | preflight staging (`stage_*`) | `Err` at `normalize()` → `ConfigError` | **78** | `--rate-limit-burst` (T1) |
>
> So T1 legitimately emits 78 (it defers validation to preflight by design) and
> T2 legitimately emits 64 (it binds a real typed parser). Forcing 78 on a
> spec-driven bound would require a second parallel validator — the exact
> duplication that caused the T1 panic. **One convention per stage, not one
> exit code for the feature.**
2. `--max-tokens` is unbounded and accepts zero
   (`domain/options_spec/ai.rs:49-65`, `kind: ValueKind::uint_unbounded()`).
   The `NumericPolicy { min, max }` machinery already exists in
   `domain/options_spec/mod.rs:146-168` — it just needs wiring.
3. `--adaptive-selectors` does not fail closed. `spec_command.rs:160-162`
   keeps the flag as a hidden compatibility placeholder when the feature is
   off, so it parses and exits 0 doing nothing. `check_adaptive_selectors_feature`
   exists nowhere. Note: the issue's contrast citation (`preflight.rs:751`)
   is stale — that line is now `validate_stage` for `max_pages`.
4. `images`/`documents` collapse. `webfang_core/Cargo.toml:13-15`:
   `default = ["images", "documents"]`, both empty. Nuance vs the issue text:
   `detect_from_url/path` (`adapters/detector/mime.rs:77-104`) already
   collapsed into one ungated function; only the two `get_mime_type` tables
   (`:130` reduced vs `:151` full) still diverge, and `--no-default-features`
   DOES switch to the reduced table — so "unswitchable" overstates it. The
   fix (one feature) is still right; the justification needs rewriting.
5. 22 files under `crates/webfang_mcp/tests/` carry
   `#![cfg(feature = "mcp")]` while `webfang_mcp/Cargo.toml` declares no
   `default` key — plain `cargo nextest run` skips the whole suite silently.

## Why this order

T1 first: it is the only item with panic potential (production code must not
panic). T2+T3 next: same fail-closed family, disjoint files from T1. T5 is a
one-line-class change with a structural (not behavioral) proof. T4 last: it
touches public feature-surface semantics and needs its rationale rewritten
per the mime.rs nuance above.

## Scope

- `crates/webfang_core/src/cli/args/{mod,crawler}.rs` (T1)
- `crates/webfang_core/src/domain/options_spec/ai.rs` (T2)
- `crates/webfang_core/src/cli/{spec_command,preflight}.rs`,
  `crates/webfang_cli/src/main.rs` (T3)
- `crates/webfang_core/{Cargo.toml,src/adapters/detector/mime.rs}` (T4)
- `crates/webfang_mcp/{Cargo.toml,tests/*.rs}` (T5)

## Constraints

- User-facing errors in Spanish; tracing fields/logs in English.
- Never `.unwrap()` in production — `?`, `match`, or typed errors.
- `CHANGELOG.md` untouched (release-plz owns it).
- No stacked PRs: sequential delivery, each slice based on `main`.
- Inter-crate direction unchanged; no new dependencies without asking.
- T4 changes feature flags → "Ask first" bucket: decision below is a
  recommendation, maintainer confirms before the slice lands.

## Tasks

- [x] T1 — Burst rejects with a typed error, never a panic.
      **DELIVERED.** Design: single validating authority. `From<Args>` no
      longer parses the burst at all (`rate_burst: None`); preflight's
      `stage_budget_overrides` (reached via `normalize`) is the only
      validator, and its value survives `merge_budget_overrides`.
      `TryFrom<Args>` was infeasible: Rust forbids coexisting `From<Args>` +
      `TryFrom<Args>`, so it would require deleting `From` and editing
      `webfang_cli/src/main.rs:121` (outside the allowed surface).
      Non-numeric arm now hard-`Err`s in Spanish (was: silent degrade +
      pre-log note). Empty/whitespace still means "not set" so
      `WEBFANG_RATE_LIMIT_BURST=""` stays a valid neutralizer.
      Exit **78**. `preflight_notes::record` is now vestigial (lost its only
      producer); full removal touches `cli/mod.rs` + `main.rs` — deferred.
      Callers touched: zero. No `Cargo.toml`/`CHANGELOG.md` change.
      RED→GREEN: 6 new tests failed first (incl. `From` panicking, and
      `main_flow_merge_preserves_valid_burst_override` reproducing main.rs
      step 6b so removing the second parse cannot silently drop a value).
      Verification: `cargo check --all-targets --all-features` 0 errors;
      strict clippy 0 warnings; `cargo fmt --all -- --check` clean;
      `cargo nextest run -p webfang_core` 3534 passed / 21 skipped;
      rustdoc `-D warnings` clean.
- [x] T2 — `--max-tokens` gets real bounds: `min = 1`, `max = 32_768`, via
      `ValueKind::uint(NumericPolicy::positive(…).capped(32_768, …))`
      (`options_spec/ai.rs`). Enforced end-to-end through the existing spec →
      `value_parser` machinery via a new `args::ai::parse_max_tokens`; exit
      **64** (the clap stage's genuine code — see the exit-code table).
      **Why 32,768 == the default, deliberately:** the ceiling is provably
      dead above that value. The model
      (`granite-embedding-97m-multilingual-r2`, 311m-r2 fallback) documents Max
      Sequence Length 32,768, and `MiniLmTokenizer` truncates with
      `.min(self.max_length)` at the same 32,768 — so nothing can ever reach
      the guard with more, and a higher cap would accept a value that silently
      changes nothing. The operator may only LOWER the guard.
      `min = 1` because `seq_len() > 0` is true for every non-empty chunk, so
      `0` is a guard that rejects all of them.
      Enforced on argv AND env (`WEBFANG_MAX_TOKENS` attaches the same
      parser — not a softer front door). No config-file field exists
      (`ConfigDefaults` has no `max_tokens`), so nothing to enforce there.
      Same programmatic-`Args` narrowing as T1, unreachable in production.
      RED→GREEN: 8 new tests (`tests/max_tokens_bound_test.rs`, 6 behavioral +
      2 unit, all `#![cfg(feature = "ai")]`), 4 RED first including
      `env_max_tokens_zero_fails_closed`; 22/22 GREEN under
      `--features ai`. Boundary triangulation: 32768 and 1 both accepted,
      32769 and 0 rejected.
      Verification: check/clippy/fmt/rustdoc clean; `webfang_core` 3534 passed
      (unchanged — T2 is invisible without the `ai` feature, see below);
      `webfang_ai --features ai` 226 passed; `burst` filter 38/38 (no T1
      regression); `--features ai,adaptive-selectors` 3608 passed.
- [ ] T3 — `--adaptive-selectors` on a non-adaptive build fails closed with a
      message naming the feature (mirror the `preflight.rs` rejection style,
      not the hidden-placeholder path in `spec_command.rs:160-162`).
      Behavioral test: flag on a `--no-default-features`-style build without
      `adaptive-selectors` → exit 64 naming `adaptive-selectors`.
- [ ] T4 — Collapse `images` + `documents` into one feature; verify
      `--no-default-features` actually disables it (unified `get_mime_type`
      table, single gate). Rewrite the justification per the mime.rs nuance
      (divergent tables, not unswitchable features).
- [ ] T5 — MCP integration tests run under default `nextest` (remove the
      per-file `cfg(feature = "mcp")` gates or add the `default` key —
      whichever keeps `--no-default-features` semantics honest). Proof is
      structural: `cargo nextest list` shows the 22 files in a default run.
- [ ] T6 — Verification chain + sequential PRs (`type:bug`, `Closes #1813`
      on the final slice, `Closes part of #1813` before that).

## Authorized scope

Issue #1813 carries `status:approved`. Covered: the five acceptance criteria
above plus their tests. NOT covered: new features, dependency changes,
CI workflow edits, support-line backports.

## Acceptance criteria (from #1813)

- [ ] Invalid rate-limit burst → typed config error (exit 64), never a panic
- [ ] `--max-tokens` rejects zero and has a documented maximum
- [ ] `--adaptive-selectors` on a non-adaptive build fails closed naming the feature
- [ ] `images` + `documents` collapsed; `--no-default-features` disables it
- [ ] `webfang_mcp` integration tests run in the default `nextest` invocation

## Applicable checks (per task)

`cargo check` → strict clippy
(`--all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`)
→ `cargo fmt --all -- --check` → `cargo nextest run` (affected module at
minimum) → `bash scripts/ci_fast_gate.sh` GREEN before push/PR.
Test-first: RED observed before GREEN wherever a runnable deterministic test
exists (T1–T4); T5 is structural (list output), recorded as the exception.

## Route declaration

- T1–T5: **direct inline**. Trigger evidence: delegation was attempted and the
  runtime refused the launch (`task` → provider error: free-tier subagents
  "can only be used from within OpenCode"). No mapping/worker topology is
  available in this session, so the parent executes bounded inline batches
  (≤3 calls, bounded ranges) instead of silently continuing as if delegation
  had happened. Revisit if delegation becomes available.

## Delivery strategy

`ask-on-risk` (default). Forecast: five small slices, each far below the
~400-line heuristic — no chaining needed, strictly sequential PRs off `main`.
Slice 1 = T1 (`Closes part of #1813`); slices 2–4 likewise; final slice
carries `Closes #1813`.

## Progress

- 2026-10-04: feature document created (branch `fix/cli-validation-fail-closed`,
  worktree `fix-cli-validation-fail-closed`). Read-only analysis of all five
  points done; no source writes yet. Next: T1.
- 2026-10-04: worktree bootstrapped (`.envrc` → isolated
  `~/.cache/cargo-target/fix-cli-validation-fail-closed`, `codegraph init`,
  `codedb reindex` head `ca3c5dff` / 940 files; `seed_target.sh` reported
  `cold`, reason `no-seed` — expected and correct).
- 2026-10-04: **T1 delivered by delegated writer** (absolute-path
  intelligence, isolated target). Three parent premises were refuted and
  corrected in this document: `normalize()` exists; `ConfigError` is exit
  78, not 64; clap does NOT validate this arg (`spec_command.rs:451` binds
  `value_parser!(String)`), so pipeline validation was always the only gate.

### Open items carried forward

- **T6 verification must pass feature flags.** `webfang_core`'s default set is
  `["images", "documents"]` — it excludes `ai`. A bare
  `cargo nextest run -p webfang_core` compiles the T2 test file to ZERO tests
  and still reads green. T6 must run `--features ai` (and
  `ai,adaptive-selectors` for the two `--help` snapshots) or T2 ships
  unverified.
- **New finding, NOT in #1813's scope:** `cargo nextest run -p webfang_ai`
  reports "no tests to run" on a clean tree — `webfang_ai` declares no
  `default` feature, so its entire suite is `cfg(feature = "ai")`-gated and
  silently skipped. This is the SAME defect class as acceptance criterion 5
  (F-25 / the `webfang_mcp` gates) but in a crate that criterion does not
  name. Findings do not authorize scope expansion → raise as a follow-up
  issue for the maintainer rather than absorbing it here.
- **Semantic narrowing (T1 + T2):** a programmatic `Args` that sets
  `crawler.rate_limit_burst` / `ai.max_tokens` *without* going through clap now
  bypasses the bound and receives the derived default. No production caller
  does this (`webfang_mcp` builds `CrawlOptions` directly), but it is a real
  narrowing of the previous panic-y behavior.
- `cli::preflight_notes` is vestigial; removal deferred (needs `cli/mod.rs`
  + `main.rs`, outside every slice's surface).
- `NumericPolicy::above_max_message` cannot interpolate the offending value
  (`&'static str`); clap's own error frame supplies the number. Accepted.

## Next step

T3 — `--adaptive-selectors` must fail closed on a non-adaptive build with a
message naming the feature. Same writer discipline; the open question is
whether it can reuse the preflight stage (exit 78) or must bind a parser
(exit 64) — decide from the mechanism, and report which.
