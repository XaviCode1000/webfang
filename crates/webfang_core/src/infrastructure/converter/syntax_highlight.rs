//! Syntax highlighting for code blocks
//!
//! Uses syntect crate for syntax highlighting with the base16-ocean.dark theme.
//! Heavy resources (`SyntaxSet`, `ThemeSet`) are loaded once at startup using `LazyLock`
//! for optimal performance (opt-lazy-initialization).

use regex::Regex;
use std::sync::LazyLock;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

/// Regex for finding code blocks: ```language\ncode\n```
/// Uses (?s) flag for dot-all mode (matches newlines) - more idiomatic than [\s\S]
/// Compiled once at startup (err-no-unwrap-prod)
static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```(\w*)\n(.*?)```").expect("BUG: invalid regex for code blocks")
});

/// `SyntaxSet` loaded once at startup (opt-lazy-initialization)
///
/// Syntect docs explicitly recommend caching this - it's expensive to load (~2-10ms)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// `ThemeSet` loaded once at startup (opt-lazy-initialization)
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Apply syntax highlighting to code blocks in Markdown
///
/// Finds all fenced code blocks (```lang ... ```) and applies syntax highlighting
/// using the base16-ocean.dark theme. Returns HTML-highlighted code blocks.
///
/// Uses `Regex::replace_all()` with closure for correct handling of:
/// - Multiple identical code blocks (no skip bugs)
/// - Single-pass processing (performance)
/// - No manual string mutation (correctness)
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::converter::syntax_highlight::highlight_code_blocks;
///
/// let md = "Text\n```rust\nfn main() {}\n```";
/// let highlighted = highlight_code_blocks(md);
/// assert!(highlighted.contains("<span"));
/// ```
///
/// # Errors
///
/// Returns original Markdown if highlighting fails (fallback pattern).
pub fn highlight_code_blocks(markdown: &str) -> String {
    let theme = &THEME_SET.themes["base16-ocean.dark"];

    // Use replace_all with closure - processes ALL matches in single pass
    // Fixes bug: replace() in loop would skip identical blocks
    CODE_BLOCK_RE
        .replace_all(markdown, |caps: &regex::Captures| {
            let language = caps.get(1).map_or("", |m| m.as_str());
            let code = caps.get(2).map_or("", |m| m.as_str());

            match SYNTAX_SET.find_syntax_by_token(language) {
                Some(syntax) => {
                    match highlighted_html_for_string(code, &SYNTAX_SET, syntax, theme) {
                        Ok(html) => html,              // HTML replaces backticks completely (no wrapping)
                        Err(_) => caps[0].to_string(), // Fallback: keep original Markdown
                    }
                },
                None => caps[0].to_string(), // Unknown language: keep original
            }
        })
        .to_string()
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_rust_code() {
        let md = "```rust\nfn main() {\n    println!(\"Hello\");\n}\n```";
        let highlighted = highlight_code_blocks(md);
        assert!(highlighted.contains("<span"));
    }

    #[test]
    fn test_highlight_python_code() {
        let md = "```python\nprint('Hello')\n```";
        let highlighted = highlight_code_blocks(md);
        assert!(highlighted.contains("<span"));
    }

    #[test]
    fn test_no_code_blocks() {
        let md = "Just plain text without code blocks";
        let highlighted = highlight_code_blocks(md);
        assert_eq!(highlighted, md);
    }

    #[test]
    fn test_empty_string() {
        let md = "";
        let highlighted = highlight_code_blocks(md);
        assert_eq!(highlighted, "");
    }

    #[test]
    fn test_multiple_identical_code_blocks() {
        // BUG FIX: Two identical blocks must both be processed
        // Old code: replace() in loop would skip second block
        let md = "```rust\nfn foo() {}\n```\ntext\n```rust\nfn foo() {}\n```";
        let highlighted = highlight_code_blocks(md);
        // Both should have <pre> tags (2 blocks = 2 <pre> tags)
        // Each block has multiple <span> tags (one per token), so we count <pre>
        assert_eq!(highlighted.matches("<pre").count(), 2);
        // Verify no backticks remain (HTML replaced them completely)
        assert!(!highlighted.contains("```"));
    }

    #[test]
    fn test_html_not_wrapped_in_backticks() {
        // FORMAT FIX: HTML must NOT be wrapped in backticks
        // Old code: format!("```{}\n{}```", language, highlighted)
        let md = "```rust\nfn main() {}\n```";
        let highlighted = highlight_code_blocks(md);
        assert!(!highlighted.contains("```")); // No backticks
        assert!(highlighted.contains("<pre")); // Direct HTML
    }

    #[test]
    fn test_lazy_initialization() {
        // PERFORMANCE: Verify LazyLock statics compile and work
        let _ = &SYNTAX_SET;
        let _ = &THEME_SET;
        // If this compiles, LazyLock is working
    }

    #[test]
    fn test_unknown_language_fallback() {
        // Unknown language should keep original Markdown
        let md = "```unknownlang\nsome code\n```";
        let highlighted = highlight_code_blocks(md);
        assert!(highlighted.contains("```unknownlang"));
    }

    #[test]
    fn test_multiline_code_blocks() {
        // (?s) flag test: dots must match newlines
        let md = "```rust\nfn main() {\n    println!(\"Hello\");\n    println!(\"World\");\n}\n```";
        let highlighted = highlight_code_blocks(md);
        assert!(highlighted.contains("<span"));
    }
}
