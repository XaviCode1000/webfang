# Proposal — #785 escalate JS shells whose visible text is boilerplate only

## Chosen approach

Make the visible-text gate boilerplate-aware in `spa_detector.rs`:

- Split `visible_text_chars` into `count_visible_text` returning
  `VisibleTextCount { chars_total, chars_content }` in ONE parse pass.
- Boilerplate = text inside `nav`/`header`/`footer`/`aside` regions OR inside
  any `<a>` anchor (even outside those regions).
- Gate on `chars_content` (content-only) for `StaticContent`.
- New `SpaReason::BoilerplateOnlyText { total_chars, content_chars }` when total
  passes the gate but content does not — so the router escalates the shell.
- Mount-point marker enrichment runs first (preserves `MountPoint` diagnostics).

`hybrid_router.rs` is NOT touched — its `SpaDetected(reason)` arm already handles
the new variant via wildcard.

## Scope (in)

- `crates/webfang_core/src/infrastructure/downloader/spa_detector.rs` — constants,
  `VisibleTextCount`, `count_visible_text`, `detect_spa` gate rewrite, new
  `SpaReason::BoilerplateOnlyText`, 6 new unit tests.
- SDD artifacts in `sdd/785-hybrid-shell-escalation/`.

## Scope (out)

- `hybrid_router.rs` routing logic (no change needed).
- `application::spa_detection` / MCP `detect_spa` tool (separate path, separate
  `MIN_CONTENT_CHARS`; out of scope for this infra-tier detector fix).
- Raising `MIN_VISIBLE_CHARS` (would hurt legitimate borderline pages).

## Threshold tradeoff

`MIN_VISIBLE_CHARS = 50` unchanged. The threshold now applies to CONTENT text
(boilerplate-free). A page with ≥50 chars of `<p>`/heading prose stays static.
A listing page whose only text is anchor link titles will escalate — conservative:
escalation costs one extra render pass but still yields the content, so false
escalation is safe (no data loss), while false `StaticContent` loses the page.

## Risk

- `SpaReason` gains a variant — verified no exhaustive `match` on it outside
  `spa_detector.rs` (rg callers, absolute path). Hybrid router wildcard handles it.
- Static pages with a large `<aside>` or anchor-heavy layout could escalate
  unnecessarily — acceptable tradeoff (see above); documented in spec scenario 4.
