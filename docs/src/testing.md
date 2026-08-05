# Testing Guide

End-to-end (E2E) tests live as integration test crates under `tests/` and invoke the
real `webfang` binary via [`assert_cmd`](https://docs.rs/assert_cmd). Mock HTTP servers
([`wiremock`](https://docs.rs/wiremock)) stand in for target sites and `tempfile::TempDir`
captures scrape output.

## Test crates

| Crate | File | Gate | What it covers |
|:------|:-----|:-----|:---------------|
| `behavioral` | `tests/behavioral/main.rs` | default features | Single-page scrape, CLI help, unreachable host, slow server, obsidian frontmatter |
| `cli_binary` | `tests/cli_binary_test.rs` | default features | `--version`, `--help`, network-error exit codes |
| `cli_behavioral` | `tests/cli_behavioral_test.rs` | `feature = "images"` **and** `feature = "documents"` | Obsidian tag/metadata/wiki-link conversion, CSS-selector extraction, full-page extraction |

`cli_behavioral` is `#![cfg(all(feature = "images", feature = "documents"))]`. It is built
and run by default; with `--no-default-features` it is skipped entirely (no `compile_error!`).

## Running tests

```bash
# all E2E crates
cargo nextest run --test behavioral --test cli_binary --test cli_behavioral

# a single crate
cargo nextest run --test cli_behavioral

# a single test (libtest, prints the full snapshot diff on mismatch)
cargo test --test cli_behavioral test_selector_h3_extracts_only_h3
```

Ignored tests (e.g. optional live-site checks) are excluded by default; run them with
`cargo nextest run --test behavioral --run-ignored ignored-only`.

## Snapshot testing with `insta`

Content assertions use [`insta`](https://insta.rs) snapshots instead of brittle
`content.contains(...)` checks, so a full output change is reviewed as a diff rather than
a silent boolean flip.

### Review gate (RED → GREEN)

`cargo insta` is **not installed** in this environment. Use the env-var workflow instead:

1. **RED** — first run fails because the `.snap` is missing or differs, and a
   `*.snap.new` pending file is written next to it:

   ```bash
   cargo nextest run --test cli_behavioral
   ```

2. **GREEN** — regenerate and accept the pending snapshots, then re-run with no flag to
   confirm they are now stable (no new `*.snap.new` should appear):

   ```bash
   INSTA_UPDATE=always cargo nextest run --test cli_behavioral
   cargo nextest run --test cli_behavioral        # must stay green
   ```

3. Inspect the generated `*.snap` files, then stage them with the code change.

> `*.snap.new` is git-ignored (see `.gitignore`). Never commit a `*.snap.new`; commit the
> accepted `*.snap`.

### Where snapshots live

`insta` resolves the snapshot directory from the module where `assert_snapshot!` *expands*.
The thin `assert_snapshot_*` wrappers therefore live at each test crate's **root module** so
snapshots land where the suite expects:

- `tests/behavioral/snapshots/` — root `behavioral` snapshots
- `tests/behavioral/cli/snapshots/` — obsidian snapshots (local helper inside `cli/obsidian_test.rs`)
- `tests/snapshots/` — `cli_binary__*.snap` and `cli_behavioral__*.snap`

## Redaction conventions

Scrape output embeds per-run, machine-specific, and non-deterministic values. A shared
helper, `tests/common/cli_harness.rs::redact_nondeterministic`, collapses them **before**
snapshotting so approved snapshots stay stable across machines and runs:

| Leak | Redacted to |
|:-----|:------------|
| `TempDir` absolute path | `<OUT_DIR>` |
| ANSI color escape sequences | (stripped) |
| ISO-8601 timestamps (`timestamp_utc`, `scrapeDate`, `scrape_date`, …) with or without fractional seconds and any offset/Z | `<TIMESTAMP>` |
| Wiremock `127.0.0.1:<port>` | `127.0.0.1:<PORT>` |

`cli_behavioral` additionally emits a bare `date:` frontmatter field (date only, no time
component) that the helper cannot catch, so `assert_content_snapshot` applies an insta
`add_filter` for `date: \d{4}-\d{2}-\d{2}` → `date: [DATE]` (see
`tests/cli_behavioral_test.rs`).

### Adding a new snapshot test

1. Build the scrape output through the shared harness (`BehavioralTest` / `cmd`).
2. Call the crate's `assert_snapshot_*` wrapper (root module) or, for free-text content,
   `assert_content_snapshot` in `cli_behavioral`.
3. If a new non-deterministic field appears, extend `redact_nondeterministic` (centralized)
   rather than adding a per-test hack.
4. Generate + accept via `INSTA_UPDATE=always`, then verify with a plain run.

## Lint

```bash
cargo clippy -p webfang_core --test behavioral --test cli_binary --test cli_behavioral -- -D warnings
```

Gate clippy on the specific test crates (not `--tests`): `webfang_core`'s own lib
tests have a pre-existing `tokio::time::pause` failure that requires the `test-util`
feature and is out of scope for E2E changes.

## Coverage exclusions (LCOV)

Defensive error paths — invariants by design — must not drag down the
codecov/patch target (80% on new lines). Annotate them with LCOV exclusion
markers (issue #527).

### Policy

Only annotate arms that "should not happen in normal operation":

- internal / mutex-poisoning / integer-overflow invariants
- compile-time-constant failures (CSS selectors, regexes, hardcoded URLs)
- panic/expect paths guarded by proven invariants (e.g. `NonZeroU32` after a
  zero-check, `chunks_exact` slice conversion)

NEVER annotate business paths: reachable errors like HTTP connection failures,
parse errors, or config validation. Reachable error handling is exercised by
tests and counted like any other code.

### Syntax

- Single statement: `// LCOV_EXCL_LINE` on its OWN comment line immediately
  ABOVE the code line — never inline on the code line.
- Multi-line arm/block: `// LCOV_EXCL_START` above the block and
  `// LCOV_EXCL_STOP` below it, each on its own line.
- Every marker site carries a justification comment starting with
  `// defensive: <variant> <rationale>` — merged into the marker line or as a
  preceding line.

### Safety net

Excluded paths are still mutation-tested: a surviving mutant in the weekly
cargo-mutants baseline, or in a PR touching gated hot paths (`cargo-mutants`
PR diff), is reported. The markers only affect coverage accounting — they do
not affect mutant survival.

### Hot-path rule

In files under `.cargo/mutants.toml` globs, markers MUST be own-line comments
(never inline) so the diff adds no mutable code lines.

Never lower `codecov.yml` thresholds; use markers per path instead.

## Known Issues

### Sitemap Discovery Regression (Pre-existing)
Seven behavioral tests are marked `#[ignore]` due to a pre-existing crawler regression
where auto-discovered sitemaps exit with code 2 on mock-server scenarios.
This is NOT related to the `insta` snapshot migration and was exposed when the
root test suite was wired in PR-0 (these tests were previously unwired and never ran).

Affected tests: `crawl_test.rs` (4 tests), `robots_test.rs` (1 test), and 2 tests
in `cli_behavioral_test.rs` — all tagged with
`#[ignore = "Pre-existing stale test, out of scope for insta migration"]`.
