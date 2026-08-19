# Tasks — #793 Obscura L2 contract

- [x] T1 — SDD artifacts: explore, proposal, spec, design, tasks, verify
      (this commit).
- [x] T2 — REQ-OBS-01: `obscura_downloader.rs` → `--dump html`, rename local
      var, set `content-type: text/html; charset=utf-8` in `headers`, update
      module + struct docs (stateless scope, REQ-OBS-03).
- [x] T3 — REQ-OBS-01 tests: fake-binary downloader tests — argv capture
      proves `fetch --dump html`, stdout HTML lands in `FetchedPage`,
      content-type marker present; extraction-path survival
      (`extract_with_selector` Matched + `readability::parse` Ok).
- [x] T4 — REQ-OBS-02: `preflight.rs` — `probe_obscura_version`,
      `parse_obscura_version`, `assess_obscura_version`,
      `MINIMUM_OBSCURA_VERSION`; extend `check_obscura_binary` with the three
      outcomes; `obscura_dependency_checked` gains `version` field; doc update
      for `check_js_dependencies`.
- [x] T5 — REQ-OBS-02 tests: pure parse/assess unit tests (no env mutation);
      fake-binary preflight gate tests (0.2.0 pass, 0.1.9 exit-78, garbage +
      non-zero-exit warn-degrade).
- [x] T6 — REQ-OBS-04: `hybrid_router.rs` doc fix (markdown → HTML) +
      `test_layer2_success_stops_escalation` (L3 must not be reached).
- [x] T7 — `docs/src/cli-reference.md`: note 0.2.0 minimum version for
      `--obscura-binary`.
- [x] T8 — Gate: `cargo check -p webfang_core`; strict clippy
      (`-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`);
      `cargo fmt --all`; `cargo nextest run -p webfang_core obscura` +
      preflight + hybrid_router suites.
