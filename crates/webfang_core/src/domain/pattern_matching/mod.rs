//! URL pattern matching — dual-mode (host-only + full URL glob)
//!
//! SSRF-safe pattern matching with two modes:
//!
//! - **Host-only patterns** (no `/`): matches against the parsed hostname only.
//!   Examples: `example.com`, `*.example.com`
//! - **Path patterns** (contains `/`): matches against the full normalized URL.
//!   Examples: `*/pricing*`, `/admin/*`, `*.example.com/api/*`
//!
//! Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
//! Following **opt-inline**: Inlined for hot path performance.
//! Following **security-ssrf-prevention**: Parses URL via `url::Url` before
//! comparison — no `.contains()` on raw strings. `Url::parse()` normalizes the
//! URL (strips query params, fragments, etc.), preventing SSRF via crafted URLs.
//!
//! # Security
//!
//! All patterns go through `url::Url::parse()` first. For path patterns, the
//! glob is matched against `url.as_str()` (the normalized URL), NOT the raw
//! input string. This prevents SSRF attacks where malicious URLs like
//! `https://evil.com/?q=example.com/pricing` could bypass filters.

use globset::Glob;
use url::Url;

/// Check if a URL matches a glob-style pattern (dual-mode)
///
/// Three modes depending on pattern shape:
///
/// - **Path pattern** (starts with `/`): matched against the URL path component.
///   Examples: `/pricing`, `/admin/*`, `/api/v2/*`
/// - **Path glob** (starts with `*/`): matched against the URL path component.
///   Examples: `*/pricing*`, `*/cloud*`
/// - **Host pattern** (no leading `/` or `*/`): matched against the parsed hostname
///   for backward-compatible behavior.
///   Examples: `example.com`, `*.example.com`, `*.example.com/*`
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **opt-inline**: Inlined for hot path performance.
/// Following **security-ssrf-prevention**: Always parses URL first; never
/// compares raw strings.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::pattern_matching::matches_pattern;
///
/// // Path pattern: matches URL path
/// assert!(matches_pattern("https://example.com/pricing", "/pricing"));
/// assert!(matches_pattern("https://example.com/admin/settings", "/admin/*"));
/// assert!(!matches_pattern("https://example.com/page", "/admin/*"));
///
/// // Host pattern: matches hostname (backward-compatible)
/// assert!(matches_pattern("https://blog.example.com/page", "*.example.com"));
/// assert!(matches_pattern("https://example.com/page", "example.com"));
/// assert!(!matches_pattern("https://evil.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn matches_pattern(url_str: &str, pattern: &str) -> bool {
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => return false,
    };

    if pattern.is_empty() || pattern == "*" {
        return true;
    }

    // Path pattern: starts with '/' → match against URL path component
    if pattern.starts_with('/') {
        let path = url.path();
        let glob = match Glob::new(pattern) {
            Ok(g) => g.compile_matcher(),
            Err(_) => return false,
        };
        return glob.is_match(path);
    }

    // Path glob: pattern starts with '*/' → match against URL path
    if pattern.starts_with("*/") {
        let glob = match Glob::new(pattern) {
            Ok(g) => g.compile_matcher(),
            Err(_) => return false,
        };
        return glob.is_match(url.path());
    }

    // Host pattern: backward-compatible host-only matching
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    match pattern {
        p if p.starts_with("*.") => {
            // Strip trailing "/*" if present (e.g. "*.example.com/*" → domain = "example.com")
            let rest = &p[2..];
            let domain = rest.strip_suffix("/*").unwrap_or(rest);
            host.ends_with(&format!(".{domain}"))
        },
        p => host == p,
    }
}

/// Glob/substring match a URL against a pattern (MCP `match_url_pattern` tool).
///
/// Unlike [`matches_pattern`] (crawler's SSRF-safe dual-mode filter), this is a
/// general-purpose matcher for the MCP surface. It supports:
///
/// - **Exact full URL** — `https://host/path` against the same URL → `true`.
/// - **Globstar / wildcards** — `**/catalogue/*`, `*page*` (every `*` is a full
///   wildcard matching across separators).
/// - **Scheme-less host** — `host/path` patterns match the same raw input.
/// - **Substring** — `*word*` and bare substrings match anywhere in the URL.
///
/// Scheme-less inputs are normalized with a synthetic `https://` prefix so the
/// target string stays intact for matching (we match against the *original*
/// input, not a parsed/normalized URL, to honor exact/substring cases).
#[inline]
#[must_use]
pub fn match_url_pattern(url: &str, pattern: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }

    // Treat the raw input as the match target so exact/substring/scheme-less
    // cases behave as callers expect.
    let target = url;

    if has_glob_metachars(pattern) {
        match glob_to_regex(pattern) {
            Ok(re) => return re.is_match(target),
            Err(_) => return false,
        }
    }

    // No metachars: exact equality or substring match (covers scheme-less host).
    target == pattern || target.contains(pattern)
}

/// Returns `true` if `pattern` contains glob metacharacters.
#[inline]
#[must_use]
fn has_glob_metachars(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// Convert a glob pattern to an anchored `regex::Regex`.
///
/// `**` and `*` both map to `.*` (full wildcard, matching across separators),
/// `?` maps to `.`, and all other characters are regex-escaped.
fn glob_to_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    let mut re = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
                re.push_str(".*");
            },
            '?' => re.push('.'),
            other => {
                for e in regex::escape(&other.to_string()).chars() {
                    re.push(e);
                }
            },
        }
    }
    re.push('$');
    regex::Regex::new(&re)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== SSRF PREVENTION TESTS ==========

    #[test]
    fn test_matches_pattern_ssrf_bypass_attempt() {
        assert!(!matches_pattern(
            "https://evil.com/?q=example.com/path",
            "*.example.com/*"
        ));

        assert!(!matches_pattern(
            "https://attacker.com/?redirect=example.com/admin",
            "*.example.com/*"
        ));

        assert!(!matches_pattern(
            "https://malicious.com/redirect?url=example.com/secret",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_matches_pattern_real_subdomain() {
        assert!(matches_pattern(
            "https://blog.example.com/post",
            "*.example.com/*"
        ));

        assert!(matches_pattern(
            "https://sub.example.com/page",
            "*.example.com"
        ));

        assert!(matches_pattern(
            "https://deep.sub.example.com/page",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_matches_pattern_with_port() {
        assert!(matches_pattern(
            "https://blog.example.com:8080/path",
            "*.example.com/*"
        ));

        assert!(matches_pattern(
            "https://blog.example.com:443/post",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_matches_pattern_ipv4() {
        assert!(matches_pattern(
            "http://192.168.1.1:8080/path",
            "192.168.1.1"
        ));
    }

    #[test]
    fn test_matches_pattern_ipv6() {
        assert!(matches_pattern("http://[::1]:8080/path", "[::1]"));
    }

    #[test]
    fn test_matches_pattern_invalid_url() {
        assert!(!matches_pattern("not-a-url", "*.example.com/*"));
        assert!(!matches_pattern("://missing-scheme.com", "*"));
        assert!(!matches_pattern("", "*"));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("https://example.com/page", "*"));
        assert!(matches_pattern("https://any.domain.com/page", "*"));
    }

    #[test]
    fn test_matches_pattern_empty() {
        assert!(matches_pattern("https://example.com", ""));
    }

    #[test]
    fn test_matches_pattern_no_match() {
        assert!(!matches_pattern("https://other.com/page", "example.com"));
        assert!(!matches_pattern("https://evil.com/page", "*.example.com/*"));
    }

    #[test]
    fn test_matches_pattern_exact_host() {
        assert!(matches_pattern("https://example.com/page", "example.com"));
        assert!(!matches_pattern(
            "https://sub.example.com/page",
            "example.com"
        ));
    }

    #[test]
    fn test_matches_pattern_prefix_wildcard() {
        assert!(matches_pattern(
            "https://blog.example.com/admin/users",
            "*.example.com/*"
        ));
        assert!(matches_pattern(
            "https://admin.example.com/users",
            "*.example.com/*"
        ));
        // Root domain does NOT match *.example.com/* (host-only: must be subdomain)
        assert!(!matches_pattern(
            "https://example.com/admin/users",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_matches_pattern_slash_wildcard() {
        assert!(matches_pattern(
            "https://blog.example.com/admin/users",
            "*.example.com/*"
        ));
        assert!(matches_pattern(
            "https://admin.example.com/users",
            "*.example.com/*"
        ));
        assert!(!matches_pattern(
            "https://example.com/admin/users",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_matches_pattern_path_pattern() {
        // Path patterns start with '/' → match against URL path component
        assert!(matches_pattern(
            "https://example.com/admin/settings",
            "/admin/*"
        ));
        assert!(matches_pattern(
            "https://example.com/admin/users",
            "/admin/*"
        ));
        assert!(!matches_pattern("https://example.com/page", "/admin/*"));
    }

    #[test]
    fn test_matches_pattern_path_pattern_exact() {
        assert!(matches_pattern("https://example.com/pricing", "/pricing"));
        assert!(!matches_pattern(
            "https://example.com/pricing/page",
            "/pricing"
        ));
    }

    #[test]
    fn test_matches_pattern_path_pattern_ssrf_safe() {
        // SSRF: query params should NOT match path patterns
        assert!(!matches_pattern(
            "https://evil.com/?q=target.com/pricing",
            "/target.com/pricing"
        ));
    }

    #[test]
    fn test_matches_pattern_host_pattern_unchanged() {
        assert!(matches_pattern("https://example.com/page", "example.com"));
        assert!(!matches_pattern(
            "https://sub.example.com/page",
            "example.com"
        ));
        assert!(matches_pattern(
            "https://sub.example.com/page",
            "*.example.com"
        ));
    }

    #[test]
    fn test_matches_pattern_path_glob_with_leading_wildcard() {
        // Patterns with '/' (but not starting with '/') match against URL path
        assert!(matches_pattern("https://example.com/pricing", "*/pricing*"));
        assert!(matches_pattern(
            "https://example.com/cloud-scraper",
            "*/cloud*"
        ));
        assert!(!matches_pattern("https://example.com/about", "*/pricing*"));
    }

    #[test]
    fn test_matches_pattern_path_glob_preserves_existing_behavior() {
        // Path patterns starting with '/' still work
        assert!(matches_pattern(
            "https://example.com/admin/settings",
            "/admin/*"
        ));
        assert!(!matches_pattern("https://example.com/page", "/admin/*"));

        // Host patterns without '/' still work
        assert!(matches_pattern("https://example.com/page", "example.com"));
        assert!(matches_pattern(
            "https://blog.example.com/page",
            "*.example.com"
        ));
    }

    // ========== MCP match_url_pattern (issue #596) ==========

    #[test]
    fn test_match_url_pattern_exact_full_url() {
        let url = "https://books.toscrape.com/catalogue/page-1.html";
        assert!(match_url_pattern(url, url));
    }

    #[test]
    fn test_match_url_pattern_globstar() {
        let url = "https://books.toscrape.com/catalogue/page-1.html";
        assert!(match_url_pattern(url, "**/catalogue/*"));
    }

    #[test]
    fn test_match_url_pattern_substring() {
        let url = "https://books.toscrape.com/catalogue/page-1.html";
        assert!(match_url_pattern(url, "*page*"));
    }

    #[test]
    fn test_match_url_pattern_scheme_less_host() {
        let url = "https://books.toscrape.com/catalogue/page-1.html";
        assert!(match_url_pattern(
            url,
            "books.toscrape.com/catalogue/page-1.html"
        ));
    }

    #[test]
    fn test_match_url_pattern_no_match() {
        let url = "https://books.toscrape.com/catalogue/page-1.html";
        assert!(!match_url_pattern(url, "nonexistent"));
        assert!(!match_url_pattern(url, "**/admin/*"));
    }
}
