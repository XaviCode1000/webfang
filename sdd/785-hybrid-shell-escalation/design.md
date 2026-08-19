# Design — #785 boilerplate-aware visible-text gate

## Data model

```rust
/// One classification pass over the visible text of an HTML document.
struct VisibleTextCount {
    chars_total: usize,   // all visible non-whitespace chars (boilerplate included)
    chars_content: usize, // chars OUTSIDE boilerplate regions and anchors
}

pub enum SpaReason {
    MountPoint(String),
    InsufficientText(usize), // now reports CONTENT chars (boilerplate-free)
    BoilerplateOnlyText { total_chars: usize, content_chars: usize }, // NEW
}
```

`BoilerplateOnlyText` is a struct variant carrying both counts — the router's
`debug!("SPA detected ({reason:?})")` event already interpolates the reason, so
the heuristic is observable in `--trace-file` JSONL with zero router changes.

## Constants

| Constant | Value | Meaning |
| :--- | :--- | :--- |
| `MIN_VISIBLE_CHARS` | `50` (unchanged, aligned with `application::spa_detection::MIN_CONTENT_CHARS`) | threshold — now applied to CONTENT text |
| `BOILERPLATE_TAGS` | `["nav", "header", "footer", "aside"]` | region tags whose text is chrome |
| `ANCHOR_TAG` | `"a"` | link text is navigation, excluded anywhere |
| `INVISIBLE_TEXT_TAGS` | `["script", "style", "noscript", "template"]` (unchanged) | excluded from both counts |

## Control flow — `count_visible_text` (replaces `visible_text_chars`)

Single `scraper::Html::parse_document` + ONE descendants walk. For each text node:

1. Walk ancestors once: set `invisible` if inside `INVISIBLE_TEXT_TAGS`, set
   `boilerplate` if inside `BOILERPLATE_TAGS` or any `<a>`.
2. `invisible` → `continue` (counts nothing — same semantics as #758).
3. Otherwise count non-whitespace chars `n`; `chars_total += n`, and
   `chars_content += n` only when NOT boilerplate.

## Control flow — `detect_spa` gate (WAF path untouched)

1. WAF verdict first (REQ-WAF-10/07, unchanged).
2. `let text = count_visible_text(html);`
3. `text.chars_content >= MIN_VISIBLE_CHARS` → `StaticContent` (REQ-HYBRID-SHELL-03).
4. Mount-point markers → `MountPoint` (enrichment order preserved — REQ-04).
5. `text.chars_total >= MIN_VISIBLE_CHARS` → `BoilerplateOnlyText { .. }`
   (total passes, content doesn't — REQ-01/02).
6. Fallback → `InsufficientText(text.chars_content)` (empty-shell path preserved).

Step 4 before step 5 deliberately: known shells keep their mount-point diagnosis.

## Complexity guard

The ancestor scan is a straight `for` loop with guard-clause `continue`s inside one
flat function — cognitive complexity stays under the CI ratchet (30), which is why
the invisible/boilerplate scan is NOT recursed factored out but written as an inline
`match` over element names.

## Threshold tradeoff (documented)

Anchor-text exclusion makes a bare link-listing page (only `<a>` item titles, no
paragraph prose) escalate. That is the chosen bias: escalation costs one extra
Obscura/Chromium round-trip and STILL yields the content, whereas a false
`StaticContent` verdict fails extraction and loses the page (#785). Pages with even
50 chars of non-anchor region text (headings/paragraphs/list text) stay static.
