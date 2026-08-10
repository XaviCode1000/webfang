//! URL Path Types Module
//!
//! Provides type-safe handling for:
//! - Domain extraction from URLs
//! - Safe filename generation from URL paths
//! - Output path construction with folder structure
//!
//! This follows the type-no-stringly principle - instead of passing raw Strings
//! where a domain or path is expected, we use newtypes that guarantee validity.
//!
//! # Security
//!
//! Includes Windows reserved names check to prevent crashes on Windows systems.
//! See: <https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file>

use std::path::PathBuf;
use thiserror::Error;

use crate::domain::DomainError;
use crate::OutputFormat;

/// Windows reserved device names (case-insensitive)
/// https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
///
/// These names cannot be used as file names on Windows, regardless of extension.
/// Attempting to create files with these names will crash on Windows.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Domain extracted from URL, validated and sanitized.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Domain(String);

impl Domain {
    /// Extract a validated [`Domain`] from a URL string.
    ///
    /// Strips the `www.` prefix for consistency. Returns a [`DomainError`]
    /// if the URL is malformed or has no host.
    pub fn from_url(url: &str) -> Result<Self, DomainError> {
        let parsed = url::Url::parse(url).map_err(|e| DomainError::InvalidUrl(e.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| DomainError::InvalidUrl("URL has no host".to_string()))?;
        if host.is_empty() {
            return Err(DomainError::InvalidUrl("Host is empty".to_string()));
        }
        // Remove "www." prefix for consistency
        let clean = host.strip_prefix("www.").unwrap_or(host);
        Ok(Self(clean.to_string()))
    }

    #[allow(dead_code)]
    /// Create a [`Domain`] from a raw string without validation.
    pub fn new_unchecked<S: Into<String>>(s: S) -> Self {
        Self(s.into())
    }

    /// Returns the domain as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the domain and returns the inner string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// URL path prepared for filesystem-safe conversion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UrlPath {
    raw: String,
    is_root: bool,
    ends_with_slash: bool,
}

impl UrlPath {
    /// Create an [`UrlPath`] from a raw URL path string.
    ///
    /// Normalizes the path: ensures leading `/`, strips query/fragment,
    /// removes trailing slashes (except root `/`).
    pub fn from_url_path(path: &str) -> Self {
        let clean = path.split('?').next().unwrap_or(path);
        let clean = clean.split('#').next().unwrap_or(clean);
        let normalized = if clean.is_empty() || !clean.starts_with('/') {
            format!("/{clean}")
        } else {
            clean.to_string()
        };
        let is_root = normalized == "/";
        let ends_with_slash = normalized.ends_with('/') && !is_root;
        let without_trailing = if normalized.ends_with('/') && !is_root {
            normalized.trim_end_matches('/').to_string()
        } else {
            normalized
        };
        Self {
            raw: without_trailing,
            is_root,
            ends_with_slash,
        }
    }

    /// Create an [`UrlPath`] from a full URL string.
    ///
    /// Parses the URL and extracts its path component.
    pub fn from_url(url: &str) -> Result<Self, UrlPathError> {
        let parsed = url::Url::parse(url).map_err(|e| UrlPathError::InvalidUrl(e.to_string()))?;
        Ok(Self::from_url_path(parsed.path()))
    }

    /// Create an [`UrlPath`] from a full URL, preserving query + fragment.
    ///
    /// Unlike [`Self::from_url`], this appends a sanitized query/fragment suffix
    /// to `raw` so that URLs differing only only by query params or fragments
    /// produce distinct filenames (no silent overwrite).
    ///
    /// Special characters (`?`, `&`, `:`, `=`, `#`, `/`) in the query/fragment
    /// are replaced with `_`. `is_root` and `ends_with_slash` reflect the path only.
    pub fn from_url_with_query(url: &str) -> Result<Self, UrlPathError> {
        let parsed = url::Url::parse(url).map_err(|e| UrlPathError::InvalidUrl(e.to_string()))?;
        let path = parsed.path();
        let mut base = Self::from_url_path(path);

        let query_part = parsed.query().unwrap_or("");
        let fragment = parsed.fragment().unwrap_or("");

        if query_part.is_empty() && fragment.is_empty() {
            return Ok(base);
        }

        let mut suffix = String::new();
        if !query_part.is_empty() {
            suffix.push('_');
            suffix.push_str(&Self::sanitize_query_part(query_part));
        }
        if !fragment.is_empty() {
            suffix.push('_');
            suffix.push_str(&Self::sanitize_query_part(fragment));
        }

        base.raw.push_str(&suffix);
        base.is_root = false;
        base.ends_with_slash = false;
        Ok(base)
    }

    /// Generate a unique filename from the full URL path, avoiding collisions.
    ///
    /// Unlike the old behavior that mapped ALL trailing-slash URLs to `index.md`
    /// (causing collisions), this converts the full path into a unique filename:
    /// - `/` → `index.md`
    /// - `/blog/post1/` → `blog-post1_.md`
    /// - `/blog/post2/` → `blog-post2_.md`
    /// - `/docs/api/v2/users/` → `docs-api-v2-users_.md`
    /// - Trailing-slash URLs get a `_` suffix to avoid colliding with `/blog/post1`
    ///
    /// # Security
    ///
    /// Checks Windows reserved names (CON, PRN, AUX, etc.) and appends `_safe` suffix
    /// to prevent crashes on Windows systems.
    ///
    /// Uses OutputFormat to determine file extension (default: .md for Markdown).
    pub fn to_safe_filename(&self) -> String {
        self.to_safe_filename_with_format(None)
    }

    /// Generate filename with explicit output format.
    ///
    /// When format is provided, uses the appropriate extension:
    /// - Markdown → .md
    /// - Json → .json
    /// - Text → .txt
    pub fn to_safe_filename_with_format(&self, format: Option<OutputFormat>) -> String {
        let extension = match format {
            Some(OutputFormat::Json) => "json",
            Some(OutputFormat::Text) => "txt",
            Some(OutputFormat::Markdown) | None => "md",
        };

        if self.is_root {
            return format!("index.{extension}");
        }
        let path_trimmed = self.raw.trim_start_matches('/');
        // Convert /docs/api/v2/users/ → docs-api-v2-users
        let slug = path_trimmed
            .trim_end_matches('/')
            .replace('/', "-")
            .replace(' ', "_");
        let sanitized = Self::sanitize_path_segment(&slug);

        // Check Windows reserved names (case-insensitive)
        let upper = sanitized.to_uppercase();
        let is_reserved = WINDOWS_RESERVED.iter().any(|&r| r == upper);
        let final_name = if is_reserved {
            format!("{sanitized}_safe")
        } else {
            sanitized
        };

        // Distinguish trailing-slash URLs from their slash-less counterparts
        // (e.g. /a/ vs /a) so the writer does not silently overwrite one with
        // the other. `ends_with_slash` is computed from the original input and
        // survives the `raw` trimming above, so it reliably flags directory URLs.
        let final_name = if self.ends_with_slash && !final_name.is_empty() {
            format!("{final_name}_")
        } else {
            final_name
        };

        format!("{final_name}.{extension}")
    }

    /// Get directory part (everything except last component)
    pub fn to_directory(&self) -> String {
        if self.is_root {
            return String::new();
        }
        let path_trimmed = self.raw.trim_start_matches('/');
        if let Some(last_slash) = path_trimmed.rfind('/') {
            let dir = &path_trimmed[..last_slash];
            // Sanitize each component (defense in depth): neutralizes exotic chars
            // and dot-segments so a hostile path cannot escape the output folder even
            // if upstream URL normalization is bypassed.
            let sanitized = dir
                .split('/')
                .filter(|component| !component.is_empty())
                .map(|component| {
                    if component == "." || component == ".." {
                        "_".to_string()
                    } else {
                        Self::sanitize_path_segment(component)
                    }
                })
                .collect::<Vec<_>>()
                .join("/");
            format!("{sanitized}/")
        } else {
            String::new()
        }
    }

    /// Sanitize a query or fragment string for use in a filename.
    ///
    /// Replaces special characters (`?`, `&`, `:`, `=`, `#`, `/`) with `_`.
    fn sanitize_query_part(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                '?' | '&' | ':' | '=' | '#' | '/' => '_',
                c => c,
            })
            .collect()
    }

    fn sanitize_path_segment(s: &str) -> String {
        // Whitelist: only alphanumeric + '-' '_' '.' survive. Everything else
        // (including path separators '/', '\\', and shell metacharacters) maps to '_'.
        // Defense in depth — callers also pre-replace '/', so this guards reuse.
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    #[allow(dead_code, missing_docs)] // Phase 0 triage — internal API surface
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl std::fmt::Display for UrlPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

/// Errors that can occur when constructing an [`UrlPath`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlPathError {
    /// The URL string could not be parsed.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

/// Complete output path: domain + file path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputPath {
    domain: Domain,
    path: UrlPath,
}

impl OutputPath {
    /// Construct an [`OutputPath`] from a full URL.
    ///
    /// Extracts the domain and path, returning an error on malformed URLs.
    pub fn from_url(url: &str) -> Result<Self, OutputPathError> {
        let domain = Domain::from_url(url)?;
        let parsed =
            url::Url::parse(url).map_err(|e| OutputPathError::InvalidUrl(e.to_string()))?;
        let path = UrlPath::from_url_path(parsed.path());
        Ok(Self { domain, path })
    }

    /// Construct an [`OutputPath`] from a full URL, preserving query + fragment.
    ///
    /// Uses [`UrlPath::from_url_with_query`] so that URLs differing only by
    /// query params or fragments produce distinct filenames.
    pub fn from_url_with_query(url: &str) -> Result<Self, OutputPathError> {
        let domain = Domain::from_url(url)?;
        let path = UrlPath::from_url_with_query(url)?;
        Ok(Self { domain, path })
    }

    #[allow(dead_code)]
    /// Create an [`OutputPath`] from a domain and path directly.
    pub fn new(domain: Domain, path: UrlPath) -> Self {
        Self { domain, path }
    }

    /// Folder path: ./output/{domain}/{dir}/
    pub fn to_folder_path(&self) -> String {
        let dir = self.path.to_directory();
        if dir.is_empty() {
            format!("./output/{}/", self.domain)
        } else {
            format!("./output/{}/{}", self.domain, dir)
        }
    }

    /// Full path: ./output/{domain}/{dir}/{filename}
    ///
    /// Always uses unique filename mapping to avoid collisions:
    /// `/blog/post1/` → `blog-post1_.md` (not `index.md`)
    pub fn to_full_path(&self) -> String {
        self.to_full_path_with_format(None)
    }

    /// Full path with explicit output format.
    pub fn to_full_path_with_format(&self, format: Option<OutputFormat>) -> String {
        let folder = self.to_folder_path();
        let filename = self.path.to_safe_filename_with_format(format);
        format!("{folder}{filename}")
    }

    /// Convert this output path to a [`PathBuf`].
    pub fn to_pathbuf(&self) -> PathBuf {
        PathBuf::from(self.to_full_path())
    }

    /// Returns a reference to the [`Domain`] component.
    pub fn domain(&self) -> &Domain {
        &self.domain
    }

    /// Returns a reference to the [`UrlPath`] component.
    pub fn path(&self) -> &UrlPath {
        &self.path
    }

    /// Relative path to the images directory for this output path.
    ///
    /// Returns `images/` for root URLs, or `{dir}/images/` for nested paths.
    pub fn images_relative_path(&self) -> String {
        let dir = self.path.to_directory();
        if dir.is_empty() {
            "images/".to_string()
        } else {
            format!("{dir}images/")
        }
    }
}

impl std::fmt::Display for OutputPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_full_path())
    }
}

/// Errors that can occur when constructing an [`OutputPath`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutputPathError {
    /// The URL string could not be parsed.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    /// The domain portion of the URL is invalid.
    #[error("Domain error: {0}")]
    Domain(#[from] DomainError),
    /// The path portion of the URL is invalid.
    #[error("Path error: {0}")]
    Path(#[from] UrlPathError),
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_from_url_basic() {
        let domain = Domain::from_url("https://geminicli.com/docs").unwrap();
        assert_eq!(domain.as_str(), "geminicli.com");
    }

    #[test]
    fn test_domain_from_url_with_www() {
        let domain = Domain::from_url("https://www.example.com/page").unwrap();
        assert_eq!(domain.as_str(), "example.com");
    }

    #[test]
    fn test_domain_from_url_invalid() {
        assert!(Domain::from_url("not-a-url").is_err());
    }

    #[test]
    fn test_url_path_from_root() {
        let path = UrlPath::from_url_path("/");
        assert_eq!(path.to_safe_filename(), "index.md");
    }

    #[test]
    fn test_url_path_simple() {
        let path = UrlPath::from_url_path("/docs");
        assert_eq!(path.to_safe_filename(), "docs.md");
        assert_eq!(path.to_directory(), "");
    }

    #[test]
    fn test_url_path_nested_trailing_slash_unique() {
        // Trailing-slash URLs now produce unique filenames (no index.md collision)
        let path = UrlPath::from_url_path("/docs/api/");
        assert_eq!(path.to_safe_filename(), "docs-api_.md");
        assert_eq!(path.to_directory(), "docs/");
    }

    #[test]
    fn test_url_path_nested_no_trailing() {
        let path = UrlPath::from_url_path("/docs/api");
        assert_eq!(path.to_safe_filename(), "docs-api.md");
        assert_eq!(path.to_directory(), "docs/");
    }

    /// /a/ and /a are distinct resources but both would serialize to "a.md"
    /// without the trailing-slash marker, causing silent data loss.
    fn assert_trailing_slash_filename_marker() {
        let dir = UrlPath::from_url_path("/a/");
        let file = UrlPath::from_url_path("/a");
        assert_ne!(dir.to_safe_filename(), file.to_safe_filename());
        assert_eq!(dir.to_safe_filename(), "a_.md");
        assert_eq!(file.to_safe_filename(), "a.md");
    }

    #[test]
    fn test_to_safe_filename_distinguishes_trailing_slash() {
        assert_trailing_slash_filename_marker();

        let nested_dir = UrlPath::from_url_path("/docs/api/");
        let nested_file = UrlPath::from_url_path("/docs/api");
        assert_ne!(
            nested_dir.to_safe_filename(),
            nested_file.to_safe_filename()
        );
        assert!(nested_dir.to_safe_filename().contains("api_"));
        assert_eq!(nested_dir.to_safe_filename(), "docs-api_.md");
        assert_eq!(nested_file.to_safe_filename(), "docs-api.md");
    }

    #[test]
    fn test_url_path_with_query_string() {
        let path = UrlPath::from_url_path("/docs?foo=bar");
        assert_eq!(path.to_safe_filename(), "docs.md");
    }

    #[test]
    fn test_url_path_sanitize_invalid_chars() {
        let path = UrlPath::from_url_path("/docs with spaces");
        assert!(!path.to_safe_filename().contains(' '));
    }

    #[test]
    fn test_url_path_blog_collision_avoidance() {
        // Verify no collision between different trailing-slash URLs
        let path1 = UrlPath::from_url_path("/blog/post1/");
        let path2 = UrlPath::from_url_path("/blog/post2/");
        let path3 = UrlPath::from_url_path("/blog/");

        assert_eq!(path1.to_safe_filename(), "blog-post1_.md");
        assert_eq!(path2.to_safe_filename(), "blog-post2_.md");
        assert_eq!(path3.to_safe_filename(), "blog_.md");

        // All must be unique
        assert_ne!(path1.to_safe_filename(), path2.to_safe_filename());
        assert_ne!(path1.to_safe_filename(), path3.to_safe_filename());
        assert_ne!(path2.to_safe_filename(), path3.to_safe_filename());
    }

    #[test]
    fn test_output_path_full_url_unique() {
        let output = OutputPath::from_url("https://geminicli.com/docs/api/").unwrap();
        assert_eq!(output.to_folder_path(), "./output/geminicli.com/docs/");
        assert_eq!(
            output.to_full_path(),
            "./output/geminicli.com/docs/docs-api_.md"
        );
    }

    #[test]
    fn test_output_path_root_url() {
        let output = OutputPath::from_url("https://geminicli.com/").unwrap();
        assert_eq!(output.to_folder_path(), "./output/geminicli.com/");
        assert_eq!(output.to_full_path(), "./output/geminicli.com/index.md");
    }

    #[test]
    fn test_output_path_simple() {
        let output = OutputPath::from_url("https://example.com/docs").unwrap();
        assert_eq!(output.to_folder_path(), "./output/example.com/");
        assert_eq!(output.to_full_path(), "./output/example.com/docs.md");
    }

    #[test]
    fn test_output_path_domain() {
        let output = OutputPath::from_url("https://geminicli.com/docs").unwrap();
        assert_eq!(output.domain().as_str(), "geminicli.com");
    }

    #[test]
    fn test_output_path_images_relative() {
        let output = OutputPath::from_url("https://example.com/docs/api/").unwrap();
        assert_eq!(output.images_relative_path(), "docs/images/");
    }

    #[test]
    fn test_output_path_images_root() {
        let output = OutputPath::from_url("https://example.com/").unwrap();
        assert_eq!(output.images_relative_path(), "images/");
    }

    // ========================================================================
    // TASK-002: Windows Reserved Names Tests
    // ========================================================================

    #[test]
    fn test_windows_reserved_con() {
        let url = UrlPath::from_url_path("/CON");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "CON_safe.md");
    }

    #[test]
    fn test_windows_reserved_prn() {
        let url = UrlPath::from_url_path("/PRN");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "PRN_safe.md");
    }

    #[test]
    fn test_windows_reserved_aux() {
        let url = UrlPath::from_url_path("/AUX");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "AUX_safe.md");
    }

    #[test]
    fn test_windows_reserved_nul() {
        let url = UrlPath::from_url_path("/NUL");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "NUL_safe.md");
    }

    #[test]
    fn test_windows_reserved_com1() {
        let url = UrlPath::from_url_path("/COM1");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "COM1_safe.md");
    }

    #[test]
    fn test_windows_reserved_com9() {
        let url = UrlPath::from_url_path("/COM9");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "COM9_safe.md");
    }

    #[test]
    fn test_windows_reserved_lpt1() {
        let url = UrlPath::from_url_path("/LPT1");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "LPT1_safe.md");
    }

    #[test]
    fn test_windows_reserved_lpt9() {
        let url = UrlPath::from_url_path("/LPT9");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "LPT9_safe.md");
    }

    #[test]
    fn test_windows_reserved_case_insensitive() {
        // Should be case-insensitive
        let url = UrlPath::from_url_path("/con");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "con_safe.md");

        let url2 = UrlPath::from_url_path("/Con");
        let filename2 = url2.to_safe_filename();
        assert_eq!(filename2, "Con_safe.md");
    }

    #[test]
    fn test_windows_reserved_nested_path() {
        // Last component is CON — now full path is checked
        let url = UrlPath::from_url_path("/docs/page/CON");
        let filename = url.to_safe_filename();
        // Full path becomes "docs-page-CON", which doesn't match reserved
        assert_eq!(filename, "docs-page-CON.md");
    }

    #[test]
    fn test_non_reserved_names_unchanged() {
        let url = UrlPath::from_url_path("/docs");
        let filename = url.to_safe_filename();
        assert_eq!(filename, "docs.md");

        let url2 = UrlPath::from_url_path("/config");
        let filename2 = url2.to_safe_filename();
        assert_eq!(filename2, "config.md");
    }

    #[test]
    fn test_filename_never_contains_path_separator() {
        for input in ["/a/b/c", "/docs/page%20x", "/con/x"] {
            let path = UrlPath::from_url_path(input);
            let filename = path.to_safe_filename();
            assert!(
                !filename.contains('/'),
                "separator leaked for {input}: {filename}"
            );
            assert!(
                !filename.contains('\\'),
                "backslash leaked for {input}: {filename}"
            );
        }
    }

    #[test]
    fn test_to_directory_collapses_double_slash() {
        // A path with duplicate slashes (//double//slash) must not produce empty
        // directory segments on disk. The directory must be a single-slash path
        // with no `//` runs, and the resulting filename must contain no slash.
        let output = OutputPath::from_url("http://example.com//double//slash").unwrap();
        let dir = output.path().to_directory();
        assert!(
            !dir.contains("//"),
            "directory must not contain empty segments: {dir}"
        );
        let full = output.to_full_path();
        assert!(
            !full.contains("//"),
            "full path must not contain empty segments: {full}"
        );
        assert_eq!(dir, "double/");
        assert_eq!(full, "./output/example.com/double/double--slash.md");
    }

    #[test]
    fn test_to_directory_collapses_leading_and_trailing_slash_runs() {
        let output = OutputPath::from_url("http://example.com///a///b///c").unwrap();
        let dir = output.path().to_directory();
        assert!(!dir.contains("//"), "directory leaked a slash run: {dir}");
        assert_eq!(dir, "a/b/");
        let full = output.to_full_path();
        assert!(!full.contains("//"), "full path leaked a slash run: {full}");
    }

    #[test]
    fn test_trailing_slash_still_distinct_from_file() {
        // Regression for Bug 5: /a/ and /a must remain distinct resources after
        // the slash-collapse fix. The filename marker (`a_.md`) is covered by
        // `assert_trailing_slash_filename_marker`; here we only assert the
        // directory collapse (Bug 4 regression) is preserved.
        assert_trailing_slash_filename_marker();
        let dir = UrlPath::from_url_path("/a/");
        let file = UrlPath::from_url_path("/a");
        assert_eq!(dir.to_directory(), file.to_directory());
    }

    #[test]
    fn test_directory_neutralizes_traversal() {
        let path = UrlPath::from_url_path("/a/../b/leaf");
        let dir = path.to_directory();
        assert!(!dir.contains(".."), "traversal segment leaked: {dir}");
        assert_eq!(dir, "a/_/b/");
    }

    // ========================================================================
    // BUG-001: Query/Fragment Preservation Tests
    // ========================================================================

    #[test]
    fn test_from_url_with_query_distinguishes_ids() {
        let path1 = UrlPath::from_url_with_query("http://host/item.html?id=1").unwrap();
        let path2 = UrlPath::from_url_with_query("http://host/item.html?id=2").unwrap();
        let path3 = UrlPath::from_url_with_query("http://host/item.html?id=3").unwrap();

        assert_ne!(path1.to_safe_filename(), path2.to_safe_filename());
        assert_ne!(path1.to_safe_filename(), path3.to_safe_filename());
        assert_ne!(path2.to_safe_filename(), path3.to_safe_filename());
    }

    #[test]
    fn test_from_url_with_query_preserves_fragment() {
        let path = UrlPath::from_url_with_query("http://host/page.html#section-a").unwrap();
        let filename = path.to_safe_filename();
        assert!(
            filename.contains("section-a"),
            "fragment not preserved in filename: {filename}"
        );
    }

    #[test]
    fn test_from_url_with_query_no_query_matches_from_url() {
        // URLs without query/fragment behave identically to from_url
        let with_query = UrlPath::from_url_with_query("http://host/docs/api").unwrap();
        let plain = UrlPath::from_url("http://host/docs/api").unwrap();
        assert_eq!(with_query.to_safe_filename(), plain.to_safe_filename());
    }

    #[test]
    fn test_from_url_with_query_sanitizes_special_chars() {
        let path = UrlPath::from_url_with_query("http://host/page.html?a=1&b=2").unwrap();
        let filename = path.to_safe_filename();
        // ? and & and = should be replaced with _
        assert!(!filename.contains('?'), "? leaked: {filename}");
        assert!(!filename.contains('&'), "& leaked: {filename}");
        assert!(!filename.contains('='), "= leaked: {filename}");
    }

    #[test]
    fn test_from_url_with_query_combined_query_and_fragment() {
        let path = UrlPath::from_url_with_query("http://host/doc.html?v=2#changelog").unwrap();
        let filename = path.to_safe_filename();
        assert!(filename.contains("v_2"), "query not preserved: {filename}");
        assert!(
            filename.contains("changelog"),
            "fragment not preserved: {filename}"
        );
    }

    #[test]
    fn test_output_path_from_url_with_query_distinguishes() {
        let op1 = OutputPath::from_url_with_query("https://example.com/item?id=1").unwrap();
        let op2 = OutputPath::from_url_with_query("https://example.com/item?id=2").unwrap();
        assert_ne!(op1.to_full_path(), op2.to_full_path());
    }
}
