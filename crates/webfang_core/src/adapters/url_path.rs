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
    pub fn from_url(url: &str) -> Result<Self, DomainError> {
        let parsed = url::Url::parse(url).map_err(|e| DomainError::InvalidUrl(e.to_string()))?;
        let host = parsed.host_str().ok_or(DomainError::NoHost)?;
        if host.is_empty() {
            return Err(DomainError::EmptyHost);
        }
        // Remove "www." prefix for consistency
        let clean = host.strip_prefix("www.").unwrap_or(host);
        Ok(Self(clean.to_string()))
    }

    #[allow(dead_code)]
    pub fn new_unchecked<S: Into<String>>(s: S) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("URL has no host")]
    NoHost,
    #[error("Host is empty")]
    EmptyHost,
}

/// URL path prepared for filesystem-safe conversion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UrlPath {
    raw: String,
    is_root: bool,
    ends_with_slash: bool,
}

impl UrlPath {
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

    pub fn from_url(url: &str) -> Result<Self, UrlPathError> {
        let parsed = url::Url::parse(url).map_err(|e| UrlPathError::InvalidUrl(e.to_string()))?;
        Ok(Self::from_url_path(parsed.path()))
    }

    /// Generate a unique filename from the full URL path, avoiding collisions.
    ///
    /// Unlike the old behavior that mapped ALL trailing-slash URLs to `index.md`
    /// (causing collisions), this converts the full path into a unique filename:
    /// - `/` → `index.md`
    /// - `/blog/post1/` → `blog-post1.md`
    /// - `/blog/post2/` → `blog-post2.md`
    /// - `/docs/api/v2/users/` → `docs-api-v2-users.md`
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

        format!("{final_name}.{extension}")
    }

    /// Get directory part (everything except last component)
    pub fn to_directory(&self) -> String {
        if self.is_root {
            return String::new();
        }
        let path_trimmed = self.raw.trim_start_matches('/');
        // Find the parent directory (everything before the last /)
        if let Some(last_slash) = path_trimmed.rfind('/') {
            format!("{}/", &path_trimmed[..last_slash])
        } else {
            String::new()
        }
    }

    fn sanitize_path_segment(s: &str) -> String {
        const INVALID: &[char] = &['\\', ':', '*', '?', '"', '<', '>', '|', ' '];
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else if INVALID.contains(&c) {
                    '_'
                } else {
                    c
                }
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl std::fmt::Display for UrlPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlPathError {
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
    pub fn from_url(url: &str) -> Result<Self, OutputPathError> {
        let domain = Domain::from_url(url)?;
        let parsed =
            url::Url::parse(url).map_err(|e| OutputPathError::InvalidUrl(e.to_string()))?;
        let path = UrlPath::from_url_path(parsed.path());
        Ok(Self { domain, path })
    }

    #[allow(dead_code)]
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
    /// `/blog/post1/` → `blog-post1.md` (not `index.md`)
    pub fn to_full_path(&self) -> String {
        self.to_full_path_with_format(None)
    }

    /// Full path with explicit output format.
    pub fn to_full_path_with_format(&self, format: Option<OutputFormat>) -> String {
        let folder = self.to_folder_path();
        let filename = self.path.to_safe_filename_with_format(format);
        format!("{folder}{filename}")
    }

    pub fn to_pathbuf(&self) -> PathBuf {
        PathBuf::from(self.to_full_path())
    }

    pub fn domain(&self) -> &Domain {
        &self.domain
    }

    pub fn path(&self) -> &UrlPath {
        &self.path
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutputPathError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("Domain error: {0}")]
    Domain(#[from] DomainError),
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
        assert_eq!(path.to_safe_filename(), "docs-api.md");
        assert_eq!(path.to_directory(), "docs/");
    }

    #[test]
    fn test_url_path_nested_no_trailing() {
        let path = UrlPath::from_url_path("/docs/api");
        assert_eq!(path.to_safe_filename(), "docs-api.md");
        assert_eq!(path.to_directory(), "docs/");
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

        assert_eq!(path1.to_safe_filename(), "blog-post1.md");
        assert_eq!(path2.to_safe_filename(), "blog-post2.md");
        assert_eq!(path3.to_safe_filename(), "blog.md");

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
            "./output/geminicli.com/docs/docs-api.md"
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
}
