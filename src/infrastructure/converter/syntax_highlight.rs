//! Syntax highlighting for code blocks
//!
//! Uses syntect crate for syntax highlighting with the base16-ocean.dark theme.
//! Compiled once at startup using once_cell::Lazy.

use once_cell::sync::Lazy;
use regex::Regex;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

/// Regex for finding code blocks: ```language\ncode\n```
/// Compiled once at startup (err-no-unwrap-prod)
static CODE_BLOCK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"```(\w*)\n([\s\S]*?)```").expect("BUG: invalid regex for code blocks")
});

/// Apply syntax highlighting to code blocks in Markdown
///
/// Finds all fenced code blocks (```lang ... ```) and applies syntax highlighting
/// using the base16-ocean.dark theme. Returns HTML-highlighted code blocks.
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::converter::syntax_highlight::highlight_code_blocks;
///
/// let md = "Text\n```rust\nfn main() {}\n```";
/// let highlighted = highlight_code_blocks(md);
/// assert!(highlighted.contains("<span"));
/// ```
pub fn highlight_code_blocks(markdown: &str) -> String {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["base16-ocean.dark"];

    let mut result = markdown.to_string();

    for cap in CODE_BLOCK_RE.captures_iter(markdown) {
        let language = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let code = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        let syntax = syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        let highlighted = highlighted_html_for_string(code, &syntax_set, syntax, theme)
            .unwrap_or_else(|_| code.to_string());

        if let Some(full_match) = cap.get(0) {
            let replacement = format!("```{}\n{}```", language, highlighted);
            result = result.replace(full_match.as_str(), &replacement);
        }
    }

    result
}

#[cfg(test)]
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
}
