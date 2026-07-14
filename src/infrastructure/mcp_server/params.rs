//! MCP parameter structs — shared request parameter types
//!
//! These structs define the input parameters for MCP tool invocations.

use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ScrapeUrlParams {
    /// URL to scrape (must start with http:// or https://)
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ScrapeWithOptionsParams {
    /// URL to scrape
    pub url: String,
    /// Maximum pages to crawl (default: 1)
    pub max_pages: Option<u32>,
    /// Download images if found (default: false)
    pub download_images: Option<bool>,
    /// Download documents if found (default: false)
    pub download_documents: Option<bool>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ScrapeBatchParams {
    /// List of URLs to scrape
    pub urls: Vec<String>,
    /// Concurrency limit (default: 4)
    pub concurrency: Option<usize>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct CrawlSiteParams {
    /// Base URL to crawl
    pub url: String,
    /// Maximum crawl depth (default: 3)
    pub max_depth: Option<u8>,
    /// Maximum pages to crawl (default: 100)
    pub max_pages: Option<u32>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct CrawlWithSitemapParams {
    /// Base URL of the website
    pub url: String,
    /// Optional explicit sitemap URL
    pub sitemap_url: Option<String>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct DiscoverUrlsParams {
    /// URL to extract links from
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct DetectSpaParams {
    /// URL to check for SPA content
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct CleanHtmlParams {
    /// Raw HTML to clean
    pub html: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct HtmlToMarkdownParams {
    /// HTML to convert
    pub html: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ExtractLinksParams {
    /// HTML to extract links from
    pub html: String,
    /// Base URL for resolving relative links
    pub base_url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct HighlightCodeParams {
    /// Markdown with code blocks
    pub markdown: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ConvertWikiLinksParams {
    /// Markdown content
    pub markdown: String,
    /// Base domain for link conversion
    pub base_domain: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ValidateUrlParams {
    /// URL to validate
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ExtractDomainParams {
    /// URL to extract domain from
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct NormalizeUrlParams {
    /// URL to normalize
    pub url: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct MatchUrlPatternParams {
    /// URL to check
    pub url: String,
    /// Glob pattern to match against
    pub pattern: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct IsInternalLinkParams {
    /// URL to check
    pub url: String,
    /// Seed domain to compare against
    pub seed_domain: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct DetectWafParams {
    /// HTML body to scan for WAF signatures
    pub html: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ExportFileParams {
    /// Output directory path
    pub output_dir: String,
    /// Filename (without extension)
    pub filename: String,
    /// Export format: markdown, text, json, jsonl, vector
    pub format: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct DetectVaultParams {
    /// Explicit vault path (optional)
    pub vault_path: Option<String>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct BuildObsidianUriParams {
    /// Vault name
    pub vault_name: String,
    /// File path within vault
    pub file_path: String,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct SearchObsidianParams {
    /// Search query
    pub query: String,
    /// Optional vault path to search in
    pub vault_path: Option<String>,
    /// Maximum results (default: 10)
    pub limit: Option<usize>,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct DownloadAssetsParams {
    /// HTML containing asset references
    pub html: String,
    /// Base URL for resolving relative asset paths
    pub base_url: String,
    /// Download images (default: true)
    pub images: Option<bool>,
    /// Download documents (default: false)
    pub documents: Option<bool>,
}

// Params for tools that accept free-form JSON input
#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
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

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct GenerateRichMetadataParams {
    /// Document content for analysis
    pub content: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ExportJsonlParams {
    /// Output directory path
    pub output_dir: Option<String>,
    /// Filename (without extension)
    pub filename: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ExportVectorParams {
    /// Output directory path
    pub output_dir: Option<String>,
    /// Filename (without extension)
    pub filename: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct ProcessExportPipelineParams {
    /// URL to scrape and export
    pub url: Option<String>,
    /// Export format
    pub format: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize, JsonSchema, Debug)]
pub(crate) struct VerifyWafIntegrityParams {
    /// HTML body to inspect
    pub html: Option<String>,
}
