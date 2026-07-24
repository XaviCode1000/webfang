//! Aggressive content processor — download pipeline adapter.
//!
//! Wraps the `bridge.rs` pipeline logic into a [`ContentProcessor`] impl:
//! 1. `clean_html` (lol_html) — element-level boilerplate removal + attribute stripping
//! 2. Aggressive `strip_html_tags` — lowercases HTML, removes complete script/style
//!    blocks, skips to `>` for other tags, joins with whitespace
//!
//! Self-contained: copies the logic rather than importing private functions
//! from `bridge.rs`, so each adapter is independent.

use crate::domain::content_processor::ContentProcessor;
use crate::infrastructure::converter::html_cleaner::clean_html;

/// Aggressive content processor for the download pipeline.
///
/// Removes all boilerplate (nav, header, footer, scripts, styles) via lol_html,
/// then aggressively strips remaining HTML tags including complete block-level
/// removal of script/style/noscript elements. Best for download pipelines where
/// noise reduction is paramount.
pub struct AggressiveProcessor;

impl ContentProcessor for AggressiveProcessor {
    fn process(&self, html: &str) -> String {
        let cleaned_html = clean_html(html);
        strip_html_tags(&cleaned_html)
    }

    fn name(&self) -> &str {
        "aggressive"
    }
}

/// Aggressive tag stripper that removes complete script/style blocks
/// and lowercases HTML for case-insensitive block detection.
fn strip_html_tags(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let lbytes = lower.as_bytes();
    let n = html.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        if lbytes[i] == b'<' {
            let rest = &lower[i..];
            if rest.starts_with("<script") {
                if let Some(rel) = rest.find("</script>") {
                    i += rel + "</script>".len();
                    continue;
                } else {
                    break; // unterminated script: drop the rest
                }
            }
            if rest.starts_with("<style") {
                if let Some(rel) = rest.find("</style>") {
                    i += rel + "</style>".len();
                    continue;
                } else {
                    break;
                }
            }
            // Regular tag: skip to '>'.
            i += 1;
            while i < n && lbytes[i] != b'>' {
                i += 1;
            }
            if i < n {
                i += 1; // consume '>'
            }
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            // Push one char on its proper UTF-8 boundary.
            let next = html[i..]
                .char_indices()
                .nth(1)
                .map(|(j, _)| i + j)
                .unwrap_or(n);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_aggressive() {
        assert_eq!(AggressiveProcessor.name(), "aggressive");
    }

    #[test]
    fn strip_removes_script_blocks() {
        let result = strip_html_tags("<p>Hello</p><script>alert('xss')</script><p>World</p>");
        assert!(!result.contains("alert"), "script body removed: {result}");
        assert!(
            result.contains("Hello") && result.contains("World"),
            "visible text preserved: {result}"
        );
    }

    #[test]
    fn strip_removes_style_blocks() {
        let result = strip_html_tags("<p>Content</p><style>.red{color:red}</style>");
        assert!(
            !result.contains("color:red"),
            "style content removed: {result}"
        );
        assert!(result.contains("Content"), "text preserved: {result}");
    }

    #[test]
    fn strip_no_raw_tags() {
        let result = strip_html_tags("<p>Hello <b>bold</b> world</p>");
        assert!(!result.contains('<'), "no raw tags: {result}");
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_strips_boilerplate_and_tags() {
        let p = AggressiveProcessor;
        let result = p.process("<nav>Menu</nav><p>Real content</p><footer>Footer</footer>");
        assert!(!result.contains("Menu"), "nav removed: {result}");
        assert!(!result.contains("Footer"), "footer removed: {result}");
        assert!(
            result.contains("Real content"),
            "content preserved: {result}"
        );
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_handles_empty_input() {
        let p = AggressiveProcessor;
        assert_eq!(p.process(""), "");
    }
}
