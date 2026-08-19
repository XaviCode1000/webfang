# Explore — #785 hybrid never escalates JS shells with visible boilerplate ≥50 chars

## Findings (CodeDB/rg, absolute worktree path per §2.3)

### Detector semantics (`crates/webfang_core/src/infrastructure/downloader/spa_detector.rs`)

- `MIN_VISIBLE_CHARS = 50` (L47). Gate at L150-153: if `visible_text_chars(html) >= 50`
  → `SpaSignal::StaticContent` → router returns without escalation.
- `visible_text_chars` (L84-102) counts ALL non-whitespace text excluding
  `script/style/noscript/template` (INVISIBLE_TEXT_TAGS, L71). It does NOT exclude
  navigation boilerplate (`nav`/`header`/`footer`/`aside`, `<a>` link text).
- SPA mount-point markers only enrich the reason WHEN text is already insufficient
  (L157-161). A fat shell with nav/footer text passes the char gate before markers run.

### Router semantics (`hybrid_router.rs`)

- `evaluate_fetch` (L60-79) matches on `SpaSignal`: `StaticContent` → `Ok(Some(page))`
  (no escalation). `SpaDetected(_)` → `Ok(None)` → Layer 2 → Layer 3.
- `SpaReason` flows through `debug!("SPA detected ({reason:?}) …")` — no structural
  coupling; adding a new variant is safe (single `SpaDetected(reason)` arm, no exhaustive
  per-reason match anywhere).

### Blast radius (rg callers, absolute path)

| Consumer | Uses `SpaReason` exhaustively? | Impact of new variant |
| :--- | :--- | :--- |
| `hybrid_router.rs` | No — `SpaDetected(reason)` wildcard arm | None |
| `spa_detector` tests | Only `matches!` per variant | Add new-variant assertions |
| MCP `detect_spa` tool | Uses `application::spa_detection` (different path, `MIN_CONTENT_CHARS`) | None |

**Conclusion:** `spa_detector` is consumed ONLY by `hybrid_router` + its own tests.
No other crate or module pattern-matches on `SpaReason` variants.

### Real-world fixture (quotes.toscrape.com/js/, fetched live 2026-08-18)

Visible text: `h1` "Quotes to Scrape" (18 chars, inside `<a>`), `p` "Login" (5),
footer "Quotes by: GoodReads.com" (~24), "Made with ❤ by Zyte" (~19). Total ≈ 80.
Content container (`.content` / where quotes render after JS) = 0 chars.
All ~80 chars are nav/header/footer chrome or link text — not content.

### Existing regression test gap

`test_fat_shell_without_markers_escalates` (L293-309) fixture has `<body></body>` =
0 visible chars — exercises the raw byte path but NOT the boilerplate scenario.

## Root cause

The visible-text gate treats ALL visible text as content. Navigation chrome
(nav links, footer credit) on a JS shell exceeds 50 chars, so the detector returns
`StaticContent` and the page is lost with "insufficient content (5 chars)".

## Options considered

| # | Option | Verdict |
| :--- | :--- | :--- |
| A | Raise `MIN_VISIBLE_CHARS` | Rejected — shifts threshold for EVERY page; breaks borderline real articles. |
| B | Exclude boilerplate regions/anchors from the CONTENT count; use content count for the gate; add `BoilerplateOnlyText` reason when total passes but content doesn't | **Chosen** — minimal, targeted, preserves existing semantics for pages with real content. |
| C | Require ≥1 `<p>`/heading to stay static | Rejected — too brittle; many legitimate pages lack `<p>`. |
