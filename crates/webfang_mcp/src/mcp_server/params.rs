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

use crate::mcp_server::validation::{
    require_http_url, require_max_len, require_max_value_u16, require_max_value_u64,
    require_non_empty, require_one_of, require_range_u64, require_safe_domain,
    require_safe_filename, require_safe_name, require_safe_path, require_safe_path_allow_absolute,
    require_safe_seed, MAX_BLOB_LEN,
};

const EXPORT_FORMATS: &[&str] = &["jsonl", "vector", "auto"];

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
}

impl ScrapeWithOptionsParams {
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

/// Parameters for the `scrape_batch` tool.
#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScrapeBatchParams {
    /// List of URLs to scrape
    pub urls: Vec<String>,
    /// Concurrency limit (default: 4)
    pub concurrency: Option<usize>,
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
    /// Maximum crawl depth (default: 3)
    pub max_depth: Option<u8>,
    /// Maximum pages to crawl (default: 100)
    pub max_pages: Option<u32>,
}

impl CrawlSiteParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` is not a valid http(s) URL,
    /// `max_depth` exceeds 10, `max_pages` exceeds 100_000, or `max_pages` is
    /// less than 1.
    pub fn validate(&self) -> Result<(), McpError> {
        require_http_url("url", &self.url)?;
        if let Some(d) = self.max_depth {
            require_max_value_u64("max_depth", u64::from(d), 10)?;
        }
        if let Some(p) = self.max_pages {
            require_range_u64("max_pages", u64::from(p), 1, 100_000)?;
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
    pub format: String,
    /// Content to export (written to the output file)
    pub content: String,
}

impl ExportFileParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `output_dir` is not a safe
    /// relative path, `filename` is empty or too long, `format` is not one of
    /// the allowed formats, or `content` exceeds the maximum blob size.
    pub fn validate(&self) -> Result<(), McpError> {
        require_safe_path("output_dir", &self.output_dir)?;
        require_safe_filename("filename", &self.filename)?;
        require_one_of("format", &self.format, EXPORT_FORMATS)?;
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
    /// bytes, `vault_path` (when present) is not a safe relative path, or
    /// `limit` exceeds 1000.
    pub fn validate(&self) -> Result<(), McpError> {
        require_non_empty("query", &self.query)?;
        require_max_len("query", &self.query, 1024)?;
        if let Some(p) = &self.vault_path {
            require_safe_path("vault_path", p)?;
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
    /// present) is not a safe relative path.
    pub fn validate(&self) -> Result<(), McpError> {
        require_max_len("html", &self.html, MAX_BLOB_LEN)?;
        require_http_url("base_url", &self.base_url)?;
        if let Some(d) = &self.output_dir {
            require_safe_path("output_dir", d)?;
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
    /// not a safe relative path or `filename` (when present) is not a single
    /// flat component (no `..`, `/`, or subdirectory — issue #601).
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(d) = &self.output_dir {
            require_safe_path("output_dir", d)?;
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
    /// not a safe relative path or `filename` (when present) is not a single
    /// flat component (no `..`, `/`, or subdirectory — issue #601).
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(d) = &self.output_dir {
            require_safe_path("output_dir", d)?;
        }
        if let Some(f) = &self.filename {
            require_safe_filename("filename", f)?;
        }
        Ok(())
    }
}

#[derive(Deserialize, JsonSchema, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessExportPipelineParams {
    /// URL to scrape and export
    pub url: Option<String>,
    /// Export format
    pub format: Option<String>,
}

impl ProcessExportPipelineParams {
    /// # Errors
    /// Returns `McpError::invalid_params` if `url` (when present) is not a
    /// valid http(s) URL or `format` (when present) is not one of the allowed
    /// formats.
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(u) = &self.url {
            require_http_url("url", u)?;
        }
        if let Some(f) = &self.format {
            require_one_of("format", f, EXPORT_FORMATS)?;
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
    /// maximum blob size or `status` (when present) exceeds 599.
    pub fn validate(&self) -> Result<(), McpError> {
        if let Some(h) = &self.html {
            require_max_len("html", h, MAX_BLOB_LEN)?;
        }
        if let Some(s) = self.status {
            require_max_value_u16("status", s, 599)?;
        }
        Ok(())
    }
}
