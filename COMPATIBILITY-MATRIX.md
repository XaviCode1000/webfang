# Compatibility Matrix — Sprint 0 Gate 0

Verifies `webfang` across all release-critical feature combinations.
Executed locally via `scripts/check_compatibility.sh` and retained in CI as a single
required context. See `docs/test-inventory.md` for ignored-test catalog and sitemap correction.

## Release combos (CI-required — 6)

| Combo | Features flag | compile | start | --help | crawl | resume | failure-path |
|-------|---------------|---------|-------|--------|-------|--------|--------------|
| default | `(default)` `images,documents` | pass | pass | exit 0 | wiremock `BehavioralTest` | fresh+corrupt round-trip | 65/74/77 |
| no-default | `--no-default-features` | pass | pass | exit 0 | wiremock | fresh+corrupt | 65/74/77 |
| ai | `--features ai` | pass | pass | exit 0 | wiremock | fresh+corrupt | 65/74/77 |
| chromium | `--features chromium` | pass | pass | exit 0 | wiremock | fresh+corrupt | 65/74/77 |
| mcp | `--features mcp` | pass | pass | exit 0 | wiremock | fresh+corrupt | 65/74/77 |
| full | `--all-features` | pass | pass | exit 0 | wiremock | fresh+corrupt | 65/74/77 |

*Isolated feature checks retained:* `ci.yml` `feature-matrix` still runs `cargo hack check --each-feature --workspace --no-dev-deps`.

## Pairwise spot-checks (local/nightly — not `strict:true` gate)

| Combo | Features flag | Note |
|-------|---------------|------|
| ai+persistence | `--features ai,persistence` | local `scripts/check_compatibility.sh --all` |
| mcp+chromium | `--features mcp,chromium` | local/nightly only — saves ~10 min CI |

Exhaustive `2^10` / `--feature-powerset` (1024) is **not** run in CI by design.

## Column definitions

- **compile**: `cargo check -p webfang_cli --features <flag> --tests` (and `cargo hack --each-feature` for isolated).
- **start**: `cargo build -p webfang_cli --features <flag>`.
- **--help**: `./target/debug/webfang --help` exits 0.
- **crawl**: wiremock `BehavioralTest` via `webfang_path()` + `TempDir` (no real network).
- **resume**: pre-seeded `state/<domain>.json` (valid v1) + corrupted JSON degrade; `StateStore::load_or_default` discards stale `version:0` with `info!` + fresh state, corrupt → re-scrape via `log_scrape_error` (not hard error).
- **failure-path**: exits `65` (`--output-vectors` without vectors), `74` (`--resume` bad state-dir), `77` (all-blocked via mocked sitemap/robots).

## Sitemap correction

Roadmap stale claim "7 sitemap tests ignored" is **incorrect**. Reality: **1 ignored** — `test_parse_from_url_depth_one_attempts_fetch` in `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs` (`#[ignore = "requires network — hits real DNS for invalid-host-xyz-12345.com"]`, by design) + **18 active** tests.

## Versioning note (StateStore)

`ExportState { version:1 }` (see `crates/webfang_core/src/domain/entities/export.rs`). Old JSON without `version` loads via `#[serde(default="default_version")] → 1` (no crash); stale `version:0` is discarded with `warn!` + fresh state, and the pre-migration file is preserved as a `.bak` sibling (#1587). Checkpoints viejos se invalidan en v-next por `version` mismatch — recrea estado (re-scrape) sin crash; migración no requerida en Sprint 0. `CrawlCheckpoint` (JSON+CRC32, `checkpoint_interval=100`) is engine-internal, not wired to `--resume`.

## Persisted version markers (#1617)

A documented contract that nothing enforces is the defect, so each marker below names the thing that FAILS when the contract is broken — not the field that records it.

| Artifact | Marker | Enforced by | Failure mode |
| :--- | :--- | :--- | :--- |
| `ExportState` | `version: u32` (1) | `StateStore::load_or_default` | stale → `warn!` + `.bak` + fresh v1; corrupt → degrade to re-scrape |
| `CrawlCheckpoint` | `version: u32` (2) + CRC32 header | `accept_version` / CRC verify | stale → `warn!` + `.bak` + fresh; integrity failure → `warn!` + `.bak` + fresh |
| RecordStore | envelope `version` (2) | `RecordStore::load_under_lock` | v1 → **backup-first** migration to v2; foreign → `.bak` + `UnsupportedVersion` → fresh (never a silent downgrade rewrite) |
| SQLite (`persistence`) | `PRAGMA user_version` = `SCHEMA_VERSION` (1) | `enforce_schema_version`, called by **both** `setup_schema` impls | foreign marker → fail closed **before** any DDL; `0` (pre-marker DB) → adopted and stamped |
| JSONL output | `checksum_sha256` = `CHECKSUM_FIELD` | `serialized_uses_the_pinned_checksum_field_name` + counted `warn!` in both index readers | rename → the test fails; at runtime a file with no checksum field indexes zero hashes and says so |

**SQLite `user_version` semantics.** `SCHEMA_VERSION = 1` is SQLite's own on-disk header slot, so it travels with the file and survives DDL. The gate REFUSES rather than migrates: there is no migration mechanism in this crate and adding one is a design change, not a defect fix. Two consequences are deliberate and must not be "fixed" away:

- The gate runs **before** the DDL. Creating tables first would mean the build had already written to a file it cannot read.
- `user_version = 0` is **adopted**, not refused — that is every database written before the marker existed, and refusing them would turn the marker into a breaking change for all existing installs.

Bump `SCHEMA_VERSION` only together with a migration, and only after that migration exists.

**Backup policy is symmetric (#1617).** Every rewrite that drops bytes a previous run wrote preserves them as a `.bak` sibling first, backing up the caller's exact rejected bytes rather than re-reading the file (a re-read races a concurrent writer into preserving the replacement). Covered: StateStore stale version, checkpoint stale version, checkpoint CRC32 mismatch, checkpoint deserialization failure, record-store corrupt envelope, record-store corrupt record map, record-store unsupported version (the **downgrade** direction — an older binary was destroying a newer one's records), and JSONL torn-tail truncation. An existing `.bak` is never overwritten, so the first abandoned bytes win. `backup_sibling` extends the artifact's own extension: `state.json → state.json.bak`, `export.jsonl → export.jsonl.bak`.

## Harness

```bash
bash scripts/check_compatibility.sh --ci-required   # 6 required combos (CI)
bash scripts/check_compatibility.sh --all           # +2 pairwise (local/nightly)
```

The binary under test is resolved from `$CARGO_TARGET_DIR`, falling back to
`./target` when that variable is unset — the same convention as
`scripts/gen-cli-reference.sh`. Every worktree in this repo builds into its own
`~/.cache/cargo-target/<tree>` (isolated target dirs, #1267), so a hardcoded
`./target/debug/webfang` made the probes report `exit 127` (command not found)
in exactly the context the harness is meant to run in, which reads as a
contract failure and is not one (#1698).

## Inventory

Full 32-row `#[ignore]` catalog (26 test attributes + 6 doc/comment mentions): [`docs/test-inventory.md`](docs/test-inventory.md) (generated via `rg -n "#\[ignore" crates/ --glob '!target'`).

## CI integration

- `feature-matrix` job runs `bash scripts/check_compatibility.sh --ci-required` after `cargo hack --each-feature`.
- Single required context (loop, not matrix strategy) keeps `strict:true` simple.
- Pairwise excluded from required gate to avoid +10-15 min.

## References

- SDD: `sdd/stabilization-sprint0-baseline`
- Gate 0 freeze: **retired 2026-09-07** (#1241) — see `AGENTS.md` §"Gate 0 freeze — RETIRED"
- `scripts/check_dependency_direction.sh` still enforces inter-crate direction
- Stack: `wreq` (TLS fingerprint), `Tokio`, `ort` (feature-gated), `SQLite`

