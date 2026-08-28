//! MCP parameter structs — shared request parameter types
//!
//! These structs define the input parameters for MCP tool invocations.
//!
//! Every struct in this module carries:
//!
//! * `#[serde(deny_unknown_fields)]` — rejects unexpected JSON keys at the
//!   deserialization boundary (defence against schema-drift typos and typosquat
//!   parameter names that could otherwise pass through to handlers).
//! * `pub fn validate(&self) -> Result<(), McpError>` — semantic validation
//!   (URL scheme, path traversal, oversize blobs, numeric bounds). Handlers
//!   call this at the top of every tool body (`params.validate()?`); the
//!   wiring landed in slice 2 of issue #512.

use rmcp::ErrorData as McpError;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use webfang_core::domain::options_spec::{self as options_spec, export};

use crate::mcp_server::validation::{
    invalid_params, require_http_url, require_max_len, require_max_value_u64, require_non_empty,
    require_one_of, require_range_u64, require_safe_domain, require_safe_filename,
    require_safe_name, require_safe_path, require_safe_path_allow_absolute, require_safe_seed,
    MAX_BLOB_LEN,
};

/// Accepted `format` values for the export tools, derived from the
/// OptionsSpec SSOT (`export::EXPORT_FORMAT`) — the same closed set the CLI
/// `--export-format` flag accepts. Never duplicated as a literal here.
#[must_use]
fn export_formats() -> &'static [&'static str] {
    export::EXPORT_FORMAT.enum_variants().unwrap_or(&[])
}

/// THE single enforcement point for the MCP `max_pages` parameter, shared
/// by `crawl_site` and `scrape_with_options` (#940): bounds come from the
/// OptionsSpec SSOT (`crawler::MAX_PAGES`) via its `check_bound` policy.
/// `check_bound` returns a typed [`BoundError`] (issue #948 F7) — no
/// re-comparison of the value, no policy re-access, the MCP-stable wording
/// renders directly from the variant.
///
/// # Errors
/// Returns `McpError::invalid_params` when `value` violates the spec's
/// inclusive bounds.
pub(crate) fn validate_max_pages(value: u32) -> Result<(), McpError> {
    let raw = u64::from(value);
    if let Err(bound) = options_spec::crawler::MAX_PAGES.check_bound(raw) {
        return Err(invalid_params("max_pages", bound.to_string()));
    }
    Ok(())
}

/// THE single enforcement point for the MCP `max_depth` parameter of
/// `crawl_site` (#940 F3, #948 F7): bounds come exclusively from the
/// OptionsSpec SSOT (`crawler::MAX_DEPTH`) via its `check_bound` policy.
/// The typed [`BoundError`] carries the bound as data so the
/// MCP-stable wording renders without re-comparing the value or
/// re-reading the policy.
///
/// # Errors
/// Returns `McpError::invalid_params` when `value` violates the spec's
/// inclusive bounds.
pub(crate) fn validate_max_depth(value: u64) -> Result<(), McpError> {
    if let Err(bound) = options_spec::crawler::MAX_DEPTH.check_bound(value) {
        return Err(invalid_params("max_depth", bound.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod ssot_tests {
    use webfang_core::domain::options_spec;

    /// The SSOT accessor must yield the export variants; guards against the
    /// spec entry silently losing its Enum kind (which would empty this set).
    #[test]
    fn export_formats_derive_from_options_spec() {
        assert_eq!(super::export_formats(), &["jsonl", "vector", "auto"][..]);
    }

    /// Both `max_pages` parameters enforce the shared spec bounds: 0 is below
    /// the inclusive minimum, 100_001 above the inclusive cap — both rejected.
    #[test]
    fn max_pages_bounds_are_enforced_for_both_tools() {
        let crawl = super::CrawlSiteParams {
            url: "https://example.com".into(),
            max_depth: None,
            max_pages: Some(0),
        }
        .validate();
        assert!(crawl.is_err(), "crawl_site max_pages 0 must be rejected");
        let scrape = super::ScrapeWithOptionsParams {
            url: "https://example.com".into(),
            max_pages: Some(0),
            download_images: None,
            download_documents: None,
            selector: None,
            ignore_robots: None,
        }
        .validate();
        assert!(
            scrape.is_err(),
            "scrape_with_options max_pages 0 must be rejected"
        );
        let over = super::validate_max_pages(100_001);
        assert!(
            over.is_err(),
            "max_pages 100_001 must be rejected: {over:?}"
        );
    }

    /// Inclusive boundaries accept; violation messages keep today's stable
    /// MCP wording rendered from the spec bounds (no CLI flag names).
    #[test]
    fn max_pages_boundaries_accept_and_messages_stay_stable() {
        assert_eq!(super::validate_max_pages(1), Ok(()));
        assert_eq!(super::validate_max_pages(100_000), Ok(()));

        let err = super::validate_max_pages(0).expect_err("below minimum must fail");
        assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
        assert!(
            err.message.contains("must be at least 1"),
            "below-min message must keep today's wording, got: {err:?}"
        );

        let err = super::validate_max_pages(100_001).expect_err("above cap must fail");
        assert!(
            err.message.contains("must be at most 100000"),
            "above-cap message must keep today's wording, got: {err:?}"
        );
    }

    /// `max_depth` boundaries flow through `crawler::MAX_DEPTH`: zero
    /// stays valid (seed-only semantics) and the cap is accepted.
    #[test]
    fn max_depth_zero_and_cap_accept() {
        assert_eq!(super::validate_max_depth(0), Ok(()), "0 = seed-only crawl");
        assert_eq!(
            super::CrawlSiteParams {
                url: "https://example.com".into(),
                max_depth: Some(0),
                max_pages: None,
            }
            .validate(),
            Ok(())
        );
    }

    /// Drift-killer (#940 F3): the MCP rejection boundary is COMPUTED FROM
    /// the spec entry at runtime. Changing the spec cap moves the MCP
    /// boundary with it — a reintroduced duplicated literal would fail
    /// this test instead of silently disagreeing with the SSOT.
    #[test]
    fn max_depth_validation_boundary_follows_the_spec_cap() {
        let options_spec::ValueKind::Uint {
            policy: Some(policy),
        } = options_spec::crawler::MAX_DEPTH.kind
        else {
            panic!("crawler::MAX_DEPTH must carry its numeric policy");
        };
        let cap = policy.max.expect("MAX_DEPTH must record the shared cap");

        // Exactly at the spec cap: accepted.
        assert_eq!(super::validate_max_depth(cap), Ok(()));
        // One past the spec cap: rejected with the stable MCP wording
        // rendered from the SAME bound — not from a local literal.
        let err =
            super::validate_max_depth(cap + 1).expect_err("one past the spec cap must be rejected");
        assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
        assert!(
            err.message
                .contains(format!("must be at most {cap}").as_str()),
            "message must render the spec cap, got: {err:?}"
        );
    }

    /// The user-facing message keeps TODAY'S exact wording ("must be at
    /// most 10") — identical to what `require_max_value_u64` emitted
    /// before the spec routing (#940 F3).
    #[test]
    fn max_depth_message_keeps_todays_mcp_wording() {
        let err = super::validate_max_depth(11).expect_err("above cap must fail");
        assert_eq!(err.message, "must be at most 10");
        assert!(matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS));
    }
}

/// Parameters for the `scrape_url` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScrapeUrlParams {
    /// URL to scrape (must start with http:// or https://)
    pub url: String,
}

impl ScrapeUrlParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL
    /// or exceeds the maximum length.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

/// Parameters for the `scrape_with_options` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScrapeWithOptionsParams {
    /// URL to scrape
    pub url: String,
    /// Maximum pages to crawl (default: 1)
    pub max_pages: Option<u32>,
    /// Download images if found (default: false)
    pub download_images: Option<bool>,
    /// Download documents if found (default: false)
    pub download_documents: Option<bool>,
    /// CSS selector for content extraction (default: "body").
    ///
    /// When provided, the scraper extracts only the HTML matching this
    /// selector. If the selector matches zero elements or is invalid, the
    /// full page HTML is returned with a diagnostic explaining the failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Bypass the robots.txt check for this request (default: false).
    ///
    /// When false or omitted, the scrape is rejected with an error when the
    /// target site's robots.txt disallows the URL (#697).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_robots: Option<bool>,
}

impl ScrapeWithOptionsParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL,
    /// `selector` exceeds 1024 bytes, or `max_pages` violates the shared spec
    /// bounds (`crawler::MAX_PAGES`, 1..=100_000).
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        if let Some(sel) = &self.selector {
            require_max_len("selector", sel, 1024)?;
        }
        if let Some(p) = self.max_pages {
            validate_max_pages(p)?;
        }
        Ok(())
    }
}

/// Parameters for the `scrape_batch` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScrapeBatchParams {
    /// List of URLs to scrape
    pub urls: Vec<String>,
    /// Concurrency limit (default: 4)
    pub concurrency: Option<usize>,
    /// Bypass the robots.txt check for every URL in the batch (default: false).
    ///
    /// When false or omitted, URLs disallowed by robots.txt fail their
    /// individual scrape with an error (#697).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_robots: Option<bool>,
}

impl ScrapeBatchParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `urls` is empty, any URL is not
    /// http(s), `concurrency` exceeds 64, or `concurrency` is less than 1.
    pub fn validate(&self) -> Result<(), McpError> {
        if self.urls.is_empty() {
            return Err(McpError::invalid_params(
                "urls must not be empty",
                Some(Value::String("urls".to_string())),
            ));
        }
        for u in &self.urls {
            require_http_url("urls[]", u)?;
        }
        if let Some(c) = self.concurrency {
            require_range_u64("concurrency", c as u64, 1, 64)?;
        }
        Ok(())
    }
}

/// Parameters for the `crawl_site` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct CrawlSiteParams {
    /// Base URL to crawl
    pub url: String,
    /// Maximum crawl depth (default: 3, hard cap 10)
    #[schemars(range(min = 1, max = 10))]
    pub max_depth: Option<u8>,
    /// Maximum pages to crawl (default: 100)
    pub max_pages: Option<u32>,
}

impl CrawlSiteParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL,
    /// `max_depth` violates the shared spec cap (`crawler::MAX_DEPTH`, 0..=10),
    /// or `max_pages` violates the shared spec bounds
    /// (`crawler::MAX_PAGES`, 1..=100_000).
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        if let Some(d) = self.max_depth {
            validate_max_depth(u64::from(d))?;
        }
        if let Some(p) = self.max_pages {
            validate_max_pages(p)?;
        }
        Ok(())
    }
}

/// Parameters for the `crawl_with_sitemap` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct CrawlWithSitemapParams {
    /// Base URL of the website
    pub url: String,
    /// Optional explicit sitemap URL
    pub sitemap_url: Option<String>,
}

impl CrawlWithSitemapParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` (or `sitemap_url` when
    /// present) is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        if let Some(s) = &self.sitemap_url {
            require_http_url("sitemap_url", s)?;
        }
        Ok(())
    }
}

/// Parameters for the `discover_urls` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct DiscoverUrlsParams {
    /// URL to extract links from
    pub url: String,
}

impl DiscoverUrlsParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

/// Parameters for the `detect_spa` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct DetectSpaParams {
    /// URL to check for SPA content
    pub url: String,
}

impl DetectSpaParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

/// Parameters for the `clean_html` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct CleanHtmlParams {
    /// Raw HTML to clean
    pub html: String,
}

impl CleanHtmlParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` is empty or exceeds the
    /// maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("html", &self.html)?;
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        Ok(())
    }
}

/// Parameters for the `convert_html_to_markdown` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct HtmlToMarkdownParams {
    /// HTML to convert
    pub html: String,
}

impl HtmlToMarkdownParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` is empty or exceeds the
    /// maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("html", &self.html)?;
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        Ok(())
    }
}

/// Parameters for the `extract_links` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExtractLinksParams {
    /// HTML to extract links from
    pub html: String,
    /// Base URL for resolving relative links
    pub base_url: String,
}

impl ExtractLinksParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` exceeds the maximum blob
    /// size or `base_url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        require_http_url("base_url", &self.base_url)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct HighlightCodeParams {
    /// Markdown with code blocks
    pub markdown: String,
}

impl HighlightCodeParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `markdown` is empty or exceeds the
    /// maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("markdown", &self.markdown)?;
        require_max_len("markdown", &self.markdown, MAX_BLOB_LEN)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConvertWikiLinksParams {
    /// Markdown content
    pub markdown: String,
    /// Base domain for link conversion (e.g. example.com, without scheme)
    pub base_domain: String,
}

impl ConvertWikiLinksParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `markdown` exceeds the maximum
    /// blob size or `base_domain` is not a bare domain string.
    pub fn validate(&self) -> Result<(), McpError> {
        require_max_len("markdown", &self.markdown, MAX_BLOB_LEN)?;
        require_safe_domain("base_domain", &self.base_domain)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateUrlParams {
    /// URL to validate
    pub url: String,
}

impl ValidateUrlParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractDomainParams {
    /// URL to extract domain from
    pub url: String,
}

impl ExtractDomainParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct NormalizeUrlParams {
    /// URL to normalize
    pub url: String,
}

impl NormalizeUrlParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatchUrlPatternParams {
    /// URL to check
    pub url: String,
    /// Glob pattern to match against
    pub pattern: String,
}

impl MatchUrlPatternParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL,
    /// `pattern` is empty, or `pattern` exceeds 1024 bytes.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        require_non_empty("pattern", &self.pattern)?;
        require_max_len("pattern", &self.pattern, 1024)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct IsInternalLinkParams {
    /// URL to check
    pub url: String,
    /// Seed domain to compare against (bare domain, e.g. example.com, or a full http(s) URL)
    pub seed_domain: String,
}

impl IsInternalLinkParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL
    /// or `seed_domain` is not a well-formed bare domain string.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        require_safe_seed("seed_domain", &self.seed_domain)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetectWafParams {
    /// HTML body to scan for WAF signatures
    pub html: String,
}

impl DetectWafParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` is empty or exceeds the
    /// maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("html", &self.html)?;
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        Ok(())
    }
}

/// Parameters for the `export_file` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ExportFileParams {
    /// Output directory path
    pub output_dir: String,
    /// Filename (without extension)
    pub filename: String,
    /// Export format: jsonl, vector, auto
    #[serde(alias = "format")]
    pub content_format: String,
    /// Content to export (written to the output file)
    pub content: String,
}

impl ExportFileParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `output_dir` is not a safe path
    /// (absolute paths allowed here — issue #600 — but the handler enforces
    /// the server export-root gate before any write, #756), `filename` is
    /// empty or too long, `content_format` is not one of the allowed formats,
    /// or `content` exceeds the maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_safe_path_allow_absolute("output_dir", &self.output_dir)?;
        require_safe_filename("filename", &self.filename)?;
        require_one_of("content_format", &self.content_format, export_formats())?;
        require_max_len("content", &self.content, MAX_BLOB_LEN)?;
        Ok(())
    }
}

/// Parameters for the `detect_obsidian_vault` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct DetectVaultParams {
    /// Explicit vault path (optional)
    pub vault_path: Option<String>,
}

impl DetectVaultParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `vault_path` is present but not a
    /// safe path (absolute paths allowed; `..` traversal still rejected).
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(p) = &self.vault_path {
            require_safe_path_allow_absolute("vault_path", p)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildObsidianUriParams {
    /// Vault name
    pub vault_name: String,
    /// File path within vault
    pub file_path: String,
}

impl BuildObsidianUriParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `vault_name` is empty or too long,
    /// or `file_path` is not a safe relative path.
    pub fn validate(&self) -> Result<(), McpError> {
        require_safe_name("vault_name", &self.vault_name)?;
        require_safe_path("file_path", &self.file_path)?;
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchObsidianParams {
    /// Search query
    pub query: String,
    /// Optional vault path to search in
    pub vault_path: Option<String>,
    /// Maximum results (default: 10)
    pub limit: Option<usize>,
}

impl SearchObsidianParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `query` is empty or exceeds 1024
    /// bytes, `vault_path` (when present) is not a safe path (absolute paths
    /// allowed — issue #600), or `limit` exceeds 1000.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("query", &self.query)?;
        require_max_len("query", &self.query, 1024)?;
        if let Some(p) = &self.vault_path {
            require_safe_path_allow_absolute("vault_path", p)?;
        }
        if let Some(l) = self.limit {
            require_max_value_u64("limit", l as u64, 1000)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadAssetsParams {
    /// HTML containing asset references
    pub html: String,
    /// Base URL for resolving relative asset paths
    pub base_url: String,
    /// Download images (default: true)
    pub images: Option<bool>,
    /// Download documents (default: false)
    pub documents: Option<bool>,
    /// Output directory for downloaded assets.
    ///
    /// When omitted, defaults to the scraper output directory
    /// (`output/` relative to the current working directory). Only used when
    /// the server has no shared downloader injected; a shared downloader
    /// writes to its own configured output directory.
    pub output_dir: Option<String>,
}

impl DownloadAssetsParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` exceeds the maximum blob
    /// size, `base_url` is not a valid http(s) URL, or `output_dir` (when
    /// present) is not a safe path (absolute paths allowed — issue #600).
    pub fn validate(&self) -> Result<(), McpError> {
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        require_http_url("base_url", &self.base_url)?;
        if let Some(d) = &self.output_dir {
            require_safe_path_allow_absolute("output_dir", d)?;
        }
        Ok(())
    }
}

// Params for tools that accept free-form JSON input
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerateFrontmatterParams {
    /// Document title
    pub title: Option<String>,
    /// Source URL
    pub url: Option<String>,
    /// Author name
    pub author: Option<String>,
    /// Excerpt or summary
    pub excerpt: Option<String>,
    /// Tags
    pub tags: Option<Vec<String>>,
}

impl GenerateFrontmatterParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if any optional string exceeds its
    /// per-field length cap, `url` (when present) is not a valid http(s) URL,
    /// `tags` has more than 64 entries, or any tag exceeds 64 bytes.
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(u) = &self.url {
            require_http_url("url", u)?;
        }
        if let Some(t) = &self.title {
            require_max_len("title", t, 512)?;
        }
        if let Some(a) = &self.author {
            require_max_len("author", a, 256)?;
        }
        if let Some(e) = &self.excerpt {
            require_max_len("excerpt", e, 4096)?;
        }
        if let Some(tags) = &self.tags {
            require_max_value_u64("tags", tags.len() as u64, 64)?;
            for tag in tags {
                require_max_len("tags[]", tag, 64)?;
            }
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerateRichMetadataParams {
    /// Document content for analysis
    pub content: Option<String>,
}

impl GenerateRichMetadataParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `content` exceeds the maximum blob
    /// size.
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(c) = &self.content {
            require_max_len("content", c, MAX_BLOB_LEN)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportJsonlParams {
    /// Output directory path
    pub output_dir: Option<String>,
    /// Filename (without extension)
    pub filename: Option<String>,
}

impl ExportJsonlParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `output_dir` (when present) is
    /// not a safe path (absolute paths allowed here — issue #600 — but the
    /// handler enforces the server export-root gate before any write, #756)
    /// or `filename` (when present) is not a single flat component (no `..`,
    /// `/`, or subdirectory — issue #601).
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(d) = &self.output_dir {
            require_safe_path_allow_absolute("output_dir", d)?;
        }
        if let Some(f) = &self.filename {
            require_safe_filename("filename", f)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExportVectorParams {
    /// Output directory path
    pub output_dir: Option<String>,
    /// Filename (without extension)
    pub filename: Option<String>,
}

impl ExportVectorParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `output_dir` (when present) is
    /// not a safe path (absolute paths allowed here — issue #600 — but the
    /// handler enforces the server export-root gate before any write, #756)
    /// or `filename` (when present) is not a single flat component (no `..`,
    /// `/`, or subdirectory — issue #601).
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(d) = &self.output_dir {
            require_safe_path_allow_absolute("output_dir", d)?;
        }
        if let Some(f) = &self.filename {
            require_safe_filename("filename", f)?;
        }
        Ok(())
    }
}

/// Parameters for the `process_export_pipeline` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ProcessExportPipelineParams {
    /// URL to scrape and export
    pub url: Option<String>,
    /// Export format
    #[serde(alias = "format", alias = "export_format")]
    pub pipeline_format: Option<String>,
}

impl ProcessExportPipelineParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` (when present) is not a
    /// valid http(s) URL or `pipeline_format` (when present) is not one of the
    /// allowed formats.
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(u) = &self.url {
            require_http_url("url", u)?;
        }
        if let Some(f) = &self.pipeline_format {
            require_one_of("pipeline_format", f, export_formats())?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyWafIntegrityParams {
    /// HTML body to inspect
    pub html: Option<String>,
    /// Optional JSON object of HTTP headers to check (e.g. {"server": "cloudflare"})
    pub headers: Option<std::collections::HashMap<String, String>>,
    /// Optional HTTP status code for context-aware detection (REQ-WAF-09).
    /// When omitted, inspection runs in degraded mode (backward compatible):
    /// fingerprint evidence never blocks without a correlated WAF status.
    #[serde(default)]
    pub status: Option<u16>,
    /// Optional Content-Type header for the body-scan gate (REQ-WAF-09).
    /// When omitted, the body is scanned (degraded mode).
    #[serde(default)]
    pub content_type: Option<String>,
}

impl VerifyWafIntegrityParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `html` (when present) exceeds the
    /// maximum blob size or `status` (when present) is not a valid HTTP status
    /// code in the inclusive range [100,599].
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(h) = &self.html {
            require_max_len("html", h, MAX_BLOB_LEN)?;
        }
        if let Some(s) = self.status {
            // HTTP status codes are in [100,599]; 0 and out-of-range values are
            // not valid status codes (issue #606). Reject at the param layer so
            // the inspector never receives a meaningless status.
            require_range_u64("status", u64::from(s), 100, 599)?;
        }
        Ok(())
    }
}

/// Serde default for `interactive_only = true` (spec R3).
fn default_true() -> bool {
    true
}

/// Snapshot serialization formats for `get_accessibility_snapshot` (spec R3).
#[derive(Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotFormatParams {
    /// Interactive-only `@eN` refs with a `token_estimate` (default).
    #[default]
    Compact,
    /// Playwright MCP AXSnapshot format — not implemented yet.
    PlaywrightMcp,
}

/// Parameters for the `get_accessibility_snapshot` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct GetAccessibilitySnapshotParams {
    /// URL to snapshot (must start with http:// or https://)
    pub url: String,
    /// Optional case-insensitive substring to filter nodes by name or role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Emit only interactive nodes (default: true)
    #[serde(default = "default_true")]
    pub interactive_only: bool,
    /// Snapshot format (default: compact)
    #[serde(default)]
    pub format: SnapshotFormatParams,
}

impl GetAccessibilitySnapshotParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL
    /// or `selector` exceeds 1024 bytes.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        if let Some(sel) = &self.selector {
            require_max_len("selector", sel, 1024)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // ISSUE #600 — absolute paths MUST be accepted for output_dir / vault_path
    // across every affected tool (these structs are `pub(crate)`, so the
    // contract is exercised here rather than from an external test crate).
    // `..` traversal stays rejected. The shared `require_safe_path_allow_absolute`
    // helper is already covered in `validation::tests`; these pin the per-struct
    // `validate()` wiring without duplicating the helper's assertion body.
    // ========================================================================

    const ABS_DIR: &str = "/tmp/webfang_verify";

    /// Exercise every `output_dir`/`vault_path` param that was switched to
    /// `require_safe_path_allow_absolute` (issue #600) and assert absolute
    /// paths are accepted.
    #[test]
    fn issue_600_absolute_paths_accepted() {
        let jsonl = ExportJsonlParams {
            output_dir: Some(ABS_DIR.to_string()),
            filename: Some("out".to_string()),
        };
        let vector = ExportVectorParams {
            output_dir: Some(ABS_DIR.to_string()),
            filename: Some("out".to_string()),
        };
        let assets = DownloadAssetsParams {
            html: "<html></html>".to_string(),
            base_url: "https://example.com".to_string(),
            images: Some(true),
            documents: Some(false),
            output_dir: Some(ABS_DIR.to_string()),
        };
        let obsidian = SearchObsidianParams {
            query: "hello".to_string(),
            vault_path: Some("/home/user/vault".to_string()),
            limit: None,
        };
        for (name, params) in [
            ("export_jsonl", jsonl.validate()),
            ("export_vector", vector.validate()),
            ("download_assets", assets.validate()),
            ("search_obsidian", obsidian.validate()),
        ] {
            assert!(params.is_ok(), "{name}: absolute path must be accepted");
        }
    }

    /// Traversal must still be rejected even inside an absolute output_dir.
    #[test]
    fn issue_600_traversal_still_rejected() {
        let params = ExportJsonlParams {
            output_dir: Some("/tmp/webfang_verify/../etc".to_string()),
            filename: Some("out".to_string()),
        };
        assert!(
            params.validate().is_err(),
            "traversal must still be rejected"
        );
    }

    #[test]
    fn get_accessibility_snapshot_params_unknown_field_rejected() {
        let json = serde_json::json!({
            "url": "https://example.com",
            "foo": "bar"
        });
        let res = serde_json::from_value::<GetAccessibilitySnapshotParams>(json);
        assert!(
            res.is_err(),
            "deny_unknown_fields must reject foo, got: {res:?}"
        );
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("foo"), "error must mention foo, got: {msg}");
    }

    #[test]
    fn get_accessibility_snapshot_params_kebab_case_format() {
        let params: GetAccessibilitySnapshotParams = serde_json::from_value(serde_json::json!({
            "url": "https://example.com",
            "format": "playwright-mcp"
        }))
        .expect("kebab-case playwright-mcp must deserialize");
        assert_eq!(params.format, SnapshotFormatParams::PlaywrightMcp);
        let params2: GetAccessibilitySnapshotParams = serde_json::from_value(serde_json::json!({
            "url": "https://example.com",
            "format": "compact"
        }))
        .expect("compact must deserialize");
        assert_eq!(params2.format, SnapshotFormatParams::Compact);
    }

    #[test]
    fn snapshot_format_params_unknown_format_rejected() {
        let json = serde_json::json!({
            "url": "https://example.com",
            "format": "unknown"
        });
        let res = serde_json::from_value::<GetAccessibilitySnapshotParams>(json);
        assert!(res.is_err(), "unknown format must be rejected");
    }

    // ========================================================================
    // Issue #980 — persistencemode-5b: backward-compatible wire renames
    // ========================================================================

    /// `ExportFileParams` accepts `format` (legacy) as a serde alias for `content_format`.
    #[test]
    fn export_file_params_format_alias_deserializes() {
        let json = serde_json::json!({
            "output_dir": "/tmp/out",
            "filename": "test",
            "format": "jsonl",
            "content": "hello"
        });
        let params: ExportFileParams = serde_json::from_value(json)
            .expect("`format` alias must deserialize to `content_format`");
        assert_eq!(params.content_format, "jsonl");
    }

    /// `ProcessExportPipelineParams` accepts `export_format` (legacy) as a serde alias for `pipeline_format`.
    #[test]
    fn process_export_pipeline_params_export_format_alias_deserializes() {
        let json = serde_json::json!({
            "url": "https://example.com",
            "export_format": "vector"
        });
        let params: ProcessExportPipelineParams = serde_json::from_value(json)
            .expect("`export_format` alias must deserialize to `pipeline_format`");
        assert_eq!(params.pipeline_format, Some("vector".to_string()));
    }

    /// `ProcessExportPipelineParams` accepts `format` (legacy) as a serde alias for `pipeline_format`.
    #[test]
    fn process_export_pipeline_params_format_alias_deserializes() {
        let json = serde_json::json!({
            "url": "https://example.com",
            "format": "jsonl"
        });
        let params: ProcessExportPipelineParams = serde_json::from_value(json)
            .expect("`format` alias must deserialize to `pipeline_format`");
        assert_eq!(params.pipeline_format, Some("jsonl".to_string()));
    }
}
