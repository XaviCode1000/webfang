# Verify — #793 Obscura L2 contract

## Gate commands

| Gate | Command | Result |
| :--- | :--- | :--- |
| Compile | `cargo check -p webfang_core` | **PASS** |
| Strict clippy (CI parity) | `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | **PASS** |
| Format | `cargo fmt --all --check` | **PASS** |
| Obscura suite | `cargo nextest run -p webfang_core obscura` | **see below** |
| Preflight suite | `cargo nextest run -p webfang_core preflight::` | **see below** |
| Router suite | `cargo nextest run -p webfang_core hybrid_router` | **see below** |

## Test counts (obscura + version contract + router branch)

| Suite | Tests added | Result |
| :--- | :--- | :--- |
| `obscura_downloader` fake-binary (#793) | dump-html argv capture, extraction-path survival | passed |
| `preflight` pure parse/assess (#793) | parse_ok/v-prefix/suffix/garbage; assess too-old/meets/unknown | passed |
| `preflight` fake-binary gate (#793) | 0.2.0 pass, 0.1.9 exit-78, garbage warn-pass, exit-1 warn-pass | passed |
| `hybrid_router` L2-ok branch (#793) | `test_layer2_success_stops_escalation` | passed |

## Spec traceability

| Requirement | Evidence |
| :--- | :--- |
| REQ-OBS-01 | fake-binary argv capture asserts `fetch --dump html` (never markdown); `FetchedPage.html` = stdout HTML; `headers["content-type"]` = `text/html; charset=utf-8`; extraction survival (`extract_with_selector` Matched + `readability::parse` Ok) |
| REQ-OBS-02 | pure parse tests (0.2.0, 0.1.9, v-prefix, -suffix, garbage); gate tests (pass / ConfigError exit 78 naming versions / warn-degrade ×2); `obscura_dependency_checked` gains `version` field |
| REQ-OBS-03 | module doc block in obscura_downloader.rs + design.md §4; S1.3 asserts `cookies` empty |
| REQ-OBS-04 | `test_layer2_success_stops_escalation` — L3 stub fails if touched; router still returns L2 HTML |

## RDD

causal_invariant: Layer 2 output is HTML — the exact format Readability,
extract_with_selector, and WAF InspectionContext consume — and is
self-describing via a content-type header; an obscura below 0.2.0 cannot start
a hybrid crawl (exit 78), and no preflight decision depends on process-global
state.

operator_flows: (1) `webfang --js-strategy hybrid <url>` with obscura 0.2.0 →
preflight info `obscura_dependency_checked{version="0.2.0"}` → L2 HTML flows
through extraction; (2) obscura 0.1.9 → exit 78 Spanish message naming found
vs required; (3) obscura prints garbage on --version → warn
`obscura_version_unreadable`, crawl proceeds best-effort; (4) session-gated
page at L2 → honest empty HTML → escalates to L3 with CookieBridge cookies.

journey_runtime_evidence: no network/real-obscura journey possible by test
policy; runtime evidence is the spawned-subprocess contract — the fake obscura
executable is a real spawned process (Command::output) whose argv file proves
`--dump html` end-to-end through ObscuraDownloader::fetch, and whose HTML
stdout proves FetchedPage.html survives the real extraction path
(extract_with_selector + readability::parse) in-process.

changed_line_budget: production src/ = obscura_downloader.rs + preflight.rs +
hybrid_router.rs + docs/src/cli-reference.md; measured < 400 additions+deletions
(see commit stat). Tests and sdd/ artifacts tracked separately, budget not consumed.

tests: added — 4 downloader tests (argv capture, content-type marker, extraction
survival, dump-html happy path), 7+ pure version parse/assess tests, 4 fake-binary
preflight gate tests, 1 router L2-ok branch test; existing preflight/hybrid suites
re-run green.

rollback: revert the fix commit — restores `--dump markdown`, empty headers,
no version probe (preflight returns to the #787 existence-only check); no data
migration, no persisted state, no CLI flag removed.
