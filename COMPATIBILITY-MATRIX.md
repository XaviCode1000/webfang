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

Roadmap stale claim "7 sitemap tests ignored" is **incorrect**. Reality: **1 ignored** at `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs:1218` (`#[ignore = "requires network — hits real DNS for invalid-host-xyz-12345.com"]`, by design) + **18 active** tests.

## Versioning note (StateStore)

`ExportState { version:1 }` (see `crates/webfang_core/src/domain/entities/export.rs`). Old JSON without `version` loads via `#[serde(default="default_version")] → 1` (no crash); stale `version:0` is discarded with `info!` + fresh state. Checkpoints viejos se invalidan en v-next por `version` mismatch — recrea estado (re-scrape) sin crash; migración no requerida en Sprint 0. `CrawlCheckpoint` (JSON+CRC32, `checkpoint_interval=100`) is engine-internal, not wired to `--resume`.

## Harness

```bash
bash scripts/check_compatibility.sh --ci-required   # 6 required combos (CI)
bash scripts/check_compatibility.sh --all           # +2 pairwise (local/nightly)
```

## Inventory

Full 37-row `#[ignore]` catalog: [`docs/test-inventory.md`](docs/test-inventory.md) (generated via `rg -n "#\[ignore" crates/ --glob '!target'`).

## CI integration

- `feature-matrix` job runs `bash scripts/check_compatibility.sh --ci-required` after `cargo hack --each-feature`.
- Single required context (loop, not matrix strategy) keeps `strict:true` simple.
- Pairwise excluded from required gate to avoid +10-15 min.

## References

- SDD: `sdd/stabilization-sprint0-baseline`
- Gate 0: `FREEZE_FEATURES=true` in `.github/workflows/pr-validation.yml` + `AGENTS.md` Freeze policy
- `scripts/check_dependency_direction.sh` still enforces inter-crate direction
- Stack: `wreq` (TLS fingerprint), `Tokio`, `ort` (feature-gated), `SQLite`

