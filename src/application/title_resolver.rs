use tracing::debug;
use url::Url;

/// Resolve a possibly-empty extracted title to a guaranteed-non-empty string.
///
/// - Empty/whitespace-only input → `Document: [<host>]` (or `Document: [unknown_host]`).
/// - Non-empty input → returned unchanged (trim-for-check only; `validate()` trims downstream).
pub fn resolve_title(extracted_title: &str, url: &Url) -> String {
    if extracted_title.trim().is_empty() {
        let host = url.host_str().unwrap_or("unknown_host");
        let resolved = format!("Document: [{host}]");
        debug!(
            target: "application::title_resolver",
            url = %url,
            fallback = %resolved,
            "extracted title empty; applied host-based fallback"
        );
        resolved
    } else {
        extracted_title.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_title;
    use url::Url;

    fn parse(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn test_resolve_title_empty_returns_host_fallback() {
        assert_eq!(
            resolve_title("", &parse("https://example.com/page")),
            "Document: [example.com]"
        );
    }

    #[test]
    fn test_resolve_title_whitespace_returns_host_fallback() {
        assert_eq!(
            resolve_title("   ", &parse("https://blog.example.com/post")),
            "Document: [blog.example.com]"
        );
    }

    #[test]
    fn test_resolve_title_mixed_whitespace_returns_host_fallback() {
        assert_eq!(
            resolve_title("  \n  ", &parse("https://example.com")),
            "Document: [example.com]"
        );
    }

    #[test]
    fn test_resolve_title_valid_passes_through_unchanged() {
        assert_eq!(
            resolve_title("My Article Title", &parse("https://example.com")),
            "My Article Title"
        );
    }

    #[test]
    fn test_resolve_title_no_host_returns_unknown_host() {
        assert_eq!(
            resolve_title("", &parse("file:///path")),
            "Document: [unknown_host]"
        );
    }

    #[test]
    fn test_resolve_title_is_deterministic() {
        let url = parse("https://example.com/page");
        let first = resolve_title("", &url);
        let second = resolve_title("", &url);
        assert_eq!(first, second);
        assert_eq!(first, "Document: [example.com]");
    }

    #[test]
    fn test_resolve_title_padded_nonempty_is_not_fallback() {
        // Trim-for-check only: a padded-but-nonempty title is returned as-is,
        // NOT trimmed and NOT replaced with the fallback. This documents the
        // behaviour-preserving semantics (validate() trims downstream).
        assert_eq!(
            resolve_title("  Padded  ", &parse("https://example.com")),
            "  Padded  "
        );
    }
}
