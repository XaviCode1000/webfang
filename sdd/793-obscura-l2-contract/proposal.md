# Proposal — #793 Obscura L2 contract: HTML dump, version gate, stateless scope

## Chosen approach

Three coordinated changes on top of the #787 binary-wiring seams (no duplication):

1. **`--dump html`** in `ObscuraDownloader::fetch_inner`, storing real HTML in
   `FetchedPage.html`, plus an honest `content-type: text/html; charset=utf-8`
   entry in the otherwise-empty `headers` map. This reuses the exact key/value
   shape that `InspectionContext::from_lowercase_headers` and the WAF engine's
   `is_html_content_type` already consume — no new `FetchedPage` field, no
   ripple through stubs/test doubles.
2. **Minimum-version contract in preflight**: after `resolve_obscura_binary`
   (#787) finds the file, preflight runs `<binary> --version` once and parses a
   semver-like version. Policy:
   - binary missing → unchanged `CliExit::ConfigError` (exit 78),
   - `--version` fails or unparseable → `tracing::warn!` + continue
     (best-effort; unknown/custom builds are not hard-blocked),
   - parsed < 0.2.0 → `CliExit::ConfigError` (exit 78) naming found vs required.
   Parsing is a pure function (`parse_obscura_version` + `assess_obscura_version`)
   unit-tested without env mutation; the probe injects nothing global (binary
   path comes from the #787 resolver).
3. **Stateless L2 as a documented scope decision** (no `--storage-dir` wiring):
   L2 stays sessionless; L3/Chromiumoxide is the cookie path via `CookieBridge`.
   Justified in design.md §4.

## Scope (in)

- `crates/webfang_core/src/infrastructure/downloader/obscura_downloader.rs` —
  `--dump html`, content-type marker, module docs incl. stateless decision,
  fake-binary tests (dump-args assertion, extraction-path survival).
- `crates/webfang_core/src/cli/preflight.rs` — version probe + assessment,
  extended `obscura_dependency_checked` event with a structured `version` field,
  unit tests for parsing/assessment/gate.
- `crates/webfang_core/src/infrastructure/downloader/hybrid_router.rs` — module
  doc fix ("markdown extraction" → HTML) + REQ-OBS-04 branch test (L2-ok stops
  escalation).
- `docs/src/cli-reference.md` — one-line note on the 0.2.0 minimum version.

## Scope (out)

- `--storage-dir` / cookie sharing for L2 (documented decision, design.md §4).
- Markdown as an alternative router output format (issue's open evaluation;
  not needed once HTML is honest).
- Any change to `FetchedPage` shape, L1/L3 downloaders, or WAF engine.

## Risk

- Existing hybrid preflight tests write non-executable fake binaries; the new
  probe now attempts a spawn on them and degrades to warn-continue (no chmod,
  probe fails → Unknown). Behavior-preserving; deterministic.
- The `--version` probe spawns one short-lived process at startup — same
  pattern as the existing Full-strategy Chrome probe (`binary_reports_version`).
