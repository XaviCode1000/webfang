# Spec — #793 Obscura L2 contract

## REQ-OBS-01 — Layer 2 dumps HTML, not markdown

`ObscuraDownloader` SHALL invoke `obscura fetch --dump html` and store the
result in `FetchedPage.html`. The returned `FetchedPage` SHALL carry a
`content-type: text/html; charset=utf-8` entry in `headers` (lowercase key,
the shape `InspectionContext::from_lowercase_headers` consumes), so downstream
Readability, CSS-selector extraction, and WAF inspection all receive honest
HTML. `status` remains 200; `cookies` remains empty (see REQ-OBS-03).

Scenarios:
- S1.1: fetch through a fake obscura binary → the process receives
  `fetch --dump html <url>` (never `markdown`), and `FetchedPage.html` contains
  the HTML printed on stdout.
- S1.2: the returned HTML survives the real extraction path:
  `extract_with_selector(html, "article", None)` matches, and
  `readability::parse(html)` yields article content.
- S1.3: `FetchedPage.headers["content-type"]` equals `text/html; charset=utf-8`.

## REQ-OBS-02 — Binary minimum-version contract (>= 0.2.0)

Hybrid preflight SHALL verify the resolved obscura binary reports a version
>= 0.2.0. Policy:
- binary missing (path or PATH) → unchanged `CliExit::ConfigError` (exit 78).
- `--version` exits non-zero or output has no parseable MAJOR.MINOR.PATCH →
  `tracing::warn!` + preflight continues (best-effort for unknown builds).
- parsed version < 0.2.0 → `CliExit::ConfigError` (exit 78), Spanish message
  naming found version, required version, and `--obscura-binary` /
  `WEBFANG_OBSCURA_BINARY`.

Parsing is a pure function of the raw `--version` output (first whitespace
token that is semver-like; `v` prefix, `-`/`+` suffixes on patch tolerated).
Tests MUST NOT mutate process-global environment.

Scenarios:
- S2.1: `parse_obscura_version("obscura 0.2.0")` → `(0,2,0)`; `"obscura 0.1.9"`
  → `(0,1,9)`; `v`-prefixed and `-rc` suffixed tokens parse; garbage/empty → none.
- S2.2: assessment of `(0,1,9)` → too-old; `(0,2,0)` exact → meets minimum;
  `(0,3,0)`/`(1,0,0)` → meets; missing/unparseable → unknown.
- S2.3: preflight with a fake binary printing `obscura 0.2.0` passes; one
  printing `obscura 0.1.9` yields exit-78 ConfigError naming both versions; one
  printing garbage or exiting non-zero passes with a warning.
- S2.4: the `obscura_dependency_checked` event carries a structured `version`
  field (`0.2.0`-style string, or `unknown`).

## REQ-OBS-03 — Stateless Layer 2 is a documented scope decision

Layer 2 SHALL remain sessionless: no cookies injected, no shared storage
directory. Module docs and design.md SHALL state that session-gated pages are
expected to escalate to Layer 3 (Chromiumoxide), which injects `CookieBridge`
cookies filtered by domain. `--storage-dir` wiring is explicitly out of scope.

Scenarios:
- S3.1: module doc of `obscura_downloader.rs` documents the sessionless
  contract and the L3 cookie path.
- S3.2: `FetchedPage.cookies` from Layer 2 stays `vec![]` (S1.3 evidence).

## REQ-OBS-04 — Layer-2 success stops escalation

The hybrid router SHALL return a non-empty Layer-2 page immediately and MUST
NOT invoke Layer 3.

Scenarios:
- S4.1: L1 returns an SPA shell, L2 returns non-empty HTML, L3 is configured
  to fail → the router returns L2 content (L3 failure is never observed).
