# Tasks — #785 hybrid shell escalation

- [x] T1 `spa_detector.rs`: `BOILERPLATE_TAGS` + `ANCHOR_TAG` constants with doc comments; `VisibleTextCount` struct; replace `visible_text_chars` with single-pass `count_visible_text` (total + content counts, invisible tags excluded from both)
- [x] T2 `spa_detector.rs`: `SpaReason::BoilerplateOnlyText { total_chars, content_chars }` variant; rewrite `detect_spa` gate to test content chars first, marker enrichment before the boilerplate reason, `InsufficientText` carries content chars (empty-shell path intact)
- [x] T3 `spa_detector.rs` unit tests (deterministic fixtures, no network): `test_shell_with_boilerplate_escalates` (quotes.toscrape.com/js shape → BoilerplateOnlyText), `test_boilerplate_region_text_excluded_without_links` (header/footer regions, no anchors), `test_anchor_text_excluded_outside_regions` (link listing), `test_article_page_with_chrome_stays_static` (REQ-03 guard), `test_content_text_exact_threshold_stays_static` (content exactly 50), `test_boilerplate_shell_with_mount_point_reports_marker` (marker wins)
- [x] T4 `hybrid_router.rs`: NO changes — verified the wildcard `SpaDetected(reason)` arm and the `{reason:?}` verdict event carry the new variant; `cargo check` confirms
- [x] T5 Gates: `cargo check -p webfang_core`, strict CI clippy (0 warnings), `cargo fmt --all` (clean), `cargo nextest run` on spa_detector + hybrid_router modules — see verify.md
- [x] T6 verify.md with evidence; SDD artifacts complete
