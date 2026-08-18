# Spec — #785 hybrid shell escalation

## REQ-HYBRID-SHELL-01 — Boilerplate-only JS shell escalates

**Given** an HTML shell mimicking `quotes.toscrape.com/js/`: nav `<h1><a>Quotes to
Scrape</a></h1>` + `<a>Login</a>`, `<nav>` pager, footer credit (`Quotes by:
GoodReads.com`, `Made with … Zyte`), an empty content container, and zero
paragraph prose (~80 total visible chars, 0 content chars)
**When** `detect_spa(html, false)` runs
**Then** the result is `SpaDetected(BoilerplateOnlyText { total_chars ≥ 50,
content_chars < 50 })` (NOT `StaticContent`), so the hybrid router escalates to
Obscura/Chromium and `hybrid_router.rs` needs no change.

## REQ-HYBRID-SHELL-02 — Boilerplate regions and anchors excluded from content text

**Given** HTML where ALL visible text sits inside `nav`/`header`/`footer`/`aside`
regions, OR inside `<a>` anchors outside those regions, AND the total visible text
exceeds `MIN_VISIBLE_CHARS` (50)
**When** `detect_spa` runs
**Then** every region/anchor character counts toward the total but NOT the content
count, the verdict is `BoilerplateOnlyText`, and `script`/`style`/`noscript`/
`template` text remains excluded from BOTH counts as before (#758).

## REQ-HYBRID-SHELL-03 — Pages with real content text stay StaticContent

**Given** an article page carrying nav + footer chrome AND multiple `<p>`/`<h1>`
paragraphs with ≥50 non-whitespace content chars (plus any page whose content text
is exactly 50 chars)
**When** `detect_spa` runs
**Then** the verdict is `StaticContent` — the gate applies `content >= 50`, chrome
excluded; existing static-page tests (normal page, SSR with hydration, exact-50
threshold) remain green.

## REQ-HYBRID-SHELL-04 — Diagnostics and legacy paths preserved

**Given** a chrome-dressed shell that ALSO contains a known mount point
(`id="root"`), AND the legacy empty-body shell (`<body></body>`, 0 visible chars)
**When** `detect_spa` runs on each
**Then** the mount-point shell reports `MountPoint("React #root")` (marker
enrichment wins before the boilerplate reason), and the empty shell reports
`InsufficientText(0)` exactly as before — the hybrid router's SPA verdict event
`SPA detected ({reason:?})` surfaces the new reason with structured totals
without code changes in `hybrid_router.rs` (reason already interpolated there).
