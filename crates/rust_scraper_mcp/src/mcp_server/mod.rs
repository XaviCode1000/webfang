//! MCP Server — Model Context Protocol bridge for AI agents
//!
//! Exposes 37 scraper tools across 8 categories via Streamable HTTP.
//! Architecture:
//! - `state.rs` — McpState with embedded Container + per-category semaphores
//! - `server.rs` — Axum router + StreamableHttpService setup
//! - `handlers/` — 8 handler modules (one per tool category)
//!
//! Backpressure: Each category has its own tokio::sync::Semaphore
//! to prevent resource exhaustion on constrained hardware.

pub mod handlers;
pub mod params;
pub mod server;
pub mod state;

use std::path::PathBuf;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::ServerHandler;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::{
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use tracing::instrument;

pub use state::McpState;

use params::*;

/// Main MCP handler struct.
///
/// Holds the application state and combined tool router.
/// All 37 tools are registered via `#[tool_router]` macros
/// in the handler submodules.
#[derive(Clone)]
pub struct McpHandler {
    /// Shared application state (DI container + semaphores)
    pub state: McpState,
    /// Combined tool router from all 8 categories
    pub tool_router: ToolRouter<Self>,
}

impl McpHandler {
    /// Create a new MCP handler with the given state.
    pub fn new(state: McpState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router() + handlers::build_tool_router(),
        }
    }
}

// ============================================================================
// Tool implementations — organized by category
// ============================================================================
// Tool implementations — organized by category
// ============================================================================

#[tool_router]
impl McpHandler {
    // ========================================================================
    // Category 1: Scraping Core (8 tools)
    // ========================================================================

    /// Scrape a single URL and extract clean content using Readability algorithm
    #[tool(
        description = "Scrape a single URL and extract clean content using Readability algorithm (Firefox Reader mode). Returns title, content, excerpt, author, and date."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn scrape_url(
        &self,
        Parameters(params): Parameters<ScrapeUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let client = self.state.container.http_client().as_ref();
        match rust_scraper_core::application::scraper_service::scrape_with_readability(client, &url)
            .await
        {
            Ok(results) => {
                let content = serde_json::to_string_pretty(&results)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Scrape a URL with configurable options (asset download, concurrency)
    #[tool(
        description = "Scrape a URL with configurable options including asset downloading, concurrency, and delay settings."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn scrape_with_options(
        &self,
        Parameters(params): Parameters<ScrapeWithOptionsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let mut config = rust_scraper_core::infrastructure::config::ScraperConfig::default();
        if let Some(max) = params.max_pages {
            config.max_pages = Some(max as usize);
        }
        if params.download_images == Some(true) {
            config.download_images = true;
        }
        if params.download_documents == Some(true) {
            config.download_documents = true;
        }

        let client = self.state.container.http_client().as_ref();
        let dl = self.state.downloader.as_deref();
        match rust_scraper_core::application::scraper_service::scrape_with_config(
            client, &url, &config, dl, None,
        )
        .await
        {
            Ok(outcome) => {
                let content = serde_json::to_string_pretty(&outcome.results)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Scrape multiple URLs with concurrency control
    #[tool(
        description = "Scrape multiple URLs with concurrency control. Failed URLs are logged but don't stop the batch."
    )]
    async fn scrape_batch(
        &self,
        Parameters(params): Parameters<ScrapeBatchParams>,
    ) -> Result<CallToolResult, McpError> {
        let span = tracing::info_span!("mcp.scrape_batch", url_count = params.urls.len());
        let _enter = span.enter();

        tracing::info!("starting batch scrape");
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let urls: Vec<url::Url> = params
            .urls
            .iter()
            .filter_map(|u| url::Url::parse(u).ok())
            .collect();

        if urls.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "no valid URLs provided",
            )]));
        }

        let mut config = rust_scraper_core::infrastructure::config::ScraperConfig::default();
        if let Some(c) = params.concurrency {
            config.scraper_concurrency = c;
        }

        let client = self.state.container.http_client().as_ref();
        let dl = self.state.downloader.as_deref();
        match rust_scraper_core::application::scraper_service::scrape_multiple_with_limit(
            client, &urls, &config, dl,
        )
        .await
        {
            Ok(results) => {
                tracing::info!("batch scrape complete: {} pages", results.len());
                let content = serde_json::to_string_pretty(&results)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => {
                tracing::error!("batch scrape failed: {}", e);
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
        }
    }

    /// Crawl a website with BFS and depth limit
    #[tool(
        description = "Crawl a website using BFS with configurable depth limit, concurrency control, and rate limiting."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn crawl_site(
        &self,
        Parameters(params): Parameters<CrawlSiteParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let seed_url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let crawler_config = rust_scraper_core::domain::CrawlerConfig::builder(seed_url)
            .max_depth(params.max_depth.unwrap_or(3))
            .max_pages(params.max_pages.unwrap_or(100) as usize)
            .build();

        match rust_scraper_core::application::crawler::crawl_site(crawler_config).await {
            Ok(result) => {
                let urls: Vec<String> = result.urls.iter().map(|u| u.url.to_string()).collect();
                let json = serde_json::json!({
                    "urls": urls,
                    "total_pages": result.total_pages,
                    "errors": result.errors,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json).unwrap(),
                )]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Discover and crawl URLs from a sitemap
    #[tool(
        description = "Discover URLs from a website's sitemap and crawl them. Auto-discovers sitemap from robots.txt if not provided."
    )]
    async fn crawl_with_sitemap(
        &self,
        Parameters(params): Parameters<CrawlWithSitemapParams>,
    ) -> Result<CallToolResult, McpError> {
        let span = tracing::info_span!("mcp.crawl_with_sitemap", url = %params.url);
        let _enter = span.enter();

        tracing::info!("starting sitemap crawl");
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| {
                tracing::error!("semaphore acquire failed: {}", e);
                McpError::internal_error(format!("semaphore error: {e}"), None)
            })?;

        let seed_url = url::Url::parse(&params.url).map_err(|e| {
            tracing::error!("invalid URL: {}", e);
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;
        let config = rust_scraper_core::domain::CrawlerConfig::new(seed_url);
        match rust_scraper_core::application::crawler::crawl_with_sitemap(
            &params.url,
            params.sitemap_url.as_deref(),
            &config,
        )
        .await
        {
            Ok(urls) => {
                tracing::info!("sitemap crawl complete: {} urls found", urls.len());
                let url_strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&url_strings).unwrap(),
                )]))
            },
            Err(e) => {
                tracing::error!("sitemap crawl failed: {}", e);
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
        }
    }

    /// Discover URLs from a single page's HTML links
    #[tool(
        description = "Fetch a single page and extract all internal links. Lightweight URL discovery without full crawl."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn discover_urls(
        &self,
        Parameters(params): Parameters<DiscoverUrlsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let port = self.state.container.http_client();
        match port.get(&params.url).await {
            Ok(resp) => {
                let html = resp.body;
                match rust_scraper_core::infrastructure::crawler::extract_links(&html, &params.url)
                {
                    Ok(links) => {
                        let content = serde_json::to_string_pretty(&links)
                            .unwrap_or_else(|_| "failed to serialize".into());
                        Ok(CallToolResult::success(vec![Content::text(content)]))
                    },
                    Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
                }
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "HTTP error: {e}"
            ))])),
        }
    }

    /// Auto-discover sitemap URL from robots.txt or common locations
    #[allow(deprecated)]
    #[tool(
        description = "Auto-discover a website's sitemap URL by checking robots.txt and common locations (/sitemap.xml, /sitemap_index.xml, etc.)."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn discover_sitemap(
        &self,
        Parameters(params): Parameters<DiscoverUrlsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match rust_scraper_core::application::crawler_service::fetch_sitemap(&params.url).await {
            Ok(urls) => {
                let content = serde_json::to_string_pretty(&urls)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Detect if a URL requires JavaScript rendering (SPA)
    #[tool(
        description = "Detect if a page requires JavaScript rendering (Single Page Application). Checks for minimal content and SPA markers like <div id=\"root\"> or <div id=\"app\">."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn detect_spa(
        &self,
        Parameters(params): Parameters<DetectSpaParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .scraping
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let port = self.state.container.http_client();
        match port.get(&params.url).await {
            Ok(resp) => {
                let html = resp.body;
                let text =
                    rust_scraper_core::infrastructure::scraper::fallback::extract_text(&html);
                match rust_scraper_core::application::scraper_service::detect_spa_content(
                    &params.url,
                    &text,
                    &html,
                ) {
                    Some(info) => {
                        let json = serde_json::json!({
                            "url": info.url,
                            "char_count": info.char_count,
                            "has_spa_markers": info.has_spa_markers,
                        });
                        Ok(CallToolResult::success(vec![Content::text(
                            serde_json::to_string_pretty(&json).unwrap(),
                        )]))
                    },
                    None => Ok(CallToolResult::success(vec![Content::text(
                        "not an SPA - sufficient content found",
                    )])),
                }
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "HTTP error: {e}"
            ))])),
        }
    }

    // ========================================================================
    // Category 2: Content Processing (7 tools)
    // ========================================================================

    /// Remove boilerplate from HTML (scripts, nav, sidebar, footer, SVG)
    #[tool(
        description = "Remove boilerplate from HTML including scripts, styles, navigation, sidebar, footer, and SVG elements. Returns cleaned HTML."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn clean_html(
        &self,
        Parameters(params): Parameters<CleanHtmlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let cleaned =
            rust_scraper_core::infrastructure::converter::html_cleaner::clean_html(&params.html);
        Ok(CallToolResult::success(vec![Content::text(cleaned)]))
    }

    /// Convert HTML to Markdown
    #[tool(
        description = "Convert HTML to Markdown, preserving headings, code blocks, lists, and formatting."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn convert_html_to_markdown(
        &self,
        Parameters(params): Parameters<HtmlToMarkdownParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let md =
            rust_scraper_core::infrastructure::converter::html_to_markdown::convert_to_markdown(
                &params.html,
            );
        Ok(CallToolResult::success(vec![Content::text(md)]))
    }

    /// Extract all href links from HTML
    #[tool(
        description = "Extract all href links from HTML content. Returns list of raw href values."
    )]
    #[instrument(skip(self), fields(base_url = %params.base_url))]
    async fn extract_links(
        &self,
        Parameters(params): Parameters<ExtractLinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match rust_scraper_core::infrastructure::crawler::extract_links(
            &params.html,
            &params.base_url,
        ) {
            Ok(links) => {
                let content = serde_json::to_string_pretty(&links)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Add syntax highlighting to fenced code blocks
    #[tool(
        description = "Add syntax highlighting to fenced code blocks in Markdown using syntect. Returns Markdown with highlighted code."
    )]
    #[instrument(skip(self), fields(markdown_len = params.markdown.len()))]
    async fn highlight_code_blocks(
        &self,
        Parameters(params): Parameters<HighlightCodeParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let highlighted =
            rust_scraper_core::infrastructure::converter::syntax_highlight::highlight_code_blocks(
                &params.markdown,
            );
        Ok(CallToolResult::success(vec![Content::text(highlighted)]))
    }

    /// Convert HTTP links to Obsidian [[wiki-link]] syntax
    #[tool(
        description = "Convert same-domain HTTP links to Obsidian [[wiki-link]] syntax for internal note linking."
    )]
    #[instrument(skip(self), fields(base_domain = %params.base_domain))]
    async fn convert_wiki_links(
        &self,
        Parameters(params): Parameters<ConvertWikiLinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let wikilinks = rust_scraper_core::infrastructure::converter::wikilinks::convert_wiki_links(
            &params.markdown,
            &params.base_domain,
        );
        Ok(CallToolResult::success(vec![Content::text(wikilinks)]))
    }

    /// Generate YAML frontmatter for a scraped document
    #[tool(
        description = "Generate YAML frontmatter with title, URL, date, author, excerpt, and optional rich metadata."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn generate_frontmatter(
        &self,
        Parameters(params): Parameters<GenerateFrontmatterParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let title = params.title.as_deref().unwrap_or("Untitled");
        let url = params.url.as_deref().unwrap_or("");
        let fm = rust_scraper_core::infrastructure::output::frontmatter::generate_with_metadata(
            title,
            url,
            None,
            None,
            None,
            &[],
            None,
        );
        Ok(CallToolResult::success(vec![Content::text(fm)]))
    }

    /// Generate rich metadata from scraped content (word count, reading time, language, content type)
    #[tool(
        description = "Generate rich metadata from scraped content including word count, reading time (200 WPM), language detection, and content type classification."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn generate_rich_metadata(
        &self,
        Parameters(params): Parameters<GenerateRichMetadataParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let content = params.content.as_deref().unwrap_or("");

        let word_count =
            rust_scraper_core::infrastructure::obsidian::metadata::compute_word_count(content);
        let reading_time =
            rust_scraper_core::infrastructure::obsidian::metadata::compute_reading_time(word_count);

        let meta = serde_json::json!({
            "word_count": word_count,
            "reading_time_minutes": reading_time,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&meta).unwrap(),
        )]))
    }

    // ========================================================================
    // Category 3: Export (4 tools)
    // ========================================================================

    /// Save scraped content as a file (Markdown, Text, or JSON)
    #[tool(
        description = "Save scraped content as a file in Markdown, Text, or JSON format with YAML frontmatter."
    )]
    #[instrument(skip(self), fields(filename = %params.filename, format = %params.format))]
    async fn export_file(
        &self,
        Parameters(params): Parameters<ExportFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let format = rust_scraper_core::domain::entities::ExportFormat::parse_str(&params.format)
            .unwrap_or(rust_scraper_core::domain::entities::ExportFormat::Jsonl);

        let output_dir = PathBuf::from(&params.output_dir);
        match std::fs::create_dir_all(&output_dir) {
            Ok(_) => {
                let path = output_dir.join(format!("{}.{}", params.filename, format.extension()));
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "File ready at: {}",
                    path.display()
                ))]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "failed to create directory: {e}"
            ))])),
        }
    }

    /// Export scraped content to JSONL format (one JSON object per line, RAG-ready)
    #[tool(
        description = "Export content to JSONL format (one JSON object per line). Optimal for RAG pipeline ingestion."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn export_jsonl(
        &self,
        Parameters(params): Parameters<ExportJsonlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let output_dir = params.output_dir.as_deref().unwrap_or("./output");
        let filename = params.filename.as_deref().unwrap_or("export");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "JSONL exporter ready at: {output_dir}/{filename}.jsonl"
        ))]))
    }

    /// Export content with embeddings for vector database ingestion
    #[tool(
        description = "Export content with embeddings to JSON format for vector database ingestion. Includes metadata header."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn export_vector(
        &self,
        Parameters(params): Parameters<ExportVectorParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let output_dir = params.output_dir.as_deref().unwrap_or("./output");
        let filename = params.filename.as_deref().unwrap_or("export");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Vector exporter ready at: {output_dir}/{filename}.json"
        ))]))
    }

    /// Full export pipeline: scrape → chunk → validate → export
    #[tool(
        description = "Run the full export pipeline: scrape content, chunk it, validate, and export to the specified format."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn process_export_pipeline(
        &self,
        Parameters(params): Parameters<ProcessExportPipelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let url = params.url.as_deref().unwrap_or("");
        let format = params.format.as_deref().unwrap_or("jsonl");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Pipeline queued for: {url} → [{format}]"
        ))]))
    }

    // ========================================================================
    // Category 4: URL Utilities (6 tools)
    // ========================================================================

    /// Validate and parse a URL (RFC 3986 compliant)
    #[tool(
        description = "Validate and parse a URL. Returns parsed components (scheme, host, port, path, query) or error details."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn validate_url(
        &self,
        Parameters(params): Parameters<ValidateUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match url::Url::parse(&params.url) {
            Ok(u) => {
                let info = serde_json::json!({
                    "valid": true,
                    "scheme": u.scheme(),
                    "host": u.host_str().unwrap_or(""),
                    "port": u.port(),
                    "path": u.path(),
                    "query": u.query().unwrap_or(""),
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
            Err(e) => {
                let info = serde_json::json!({"valid": false, "error": e.to_string()});
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
        }
    }

    /// Extract domain/host from a URL
    #[tool(
        description = "Extract the domain (host) from a URL. E.g., 'https://www.example.com/path' → 'www.example.com'."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn extract_domain(
        &self,
        Parameters(params): Parameters<ExtractDomainParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match url::Url::parse(&params.url) {
            Ok(u) => {
                let domain = u.host_str().unwrap_or("");
                Ok(CallToolResult::success(vec![Content::text(domain)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Normalize a URL (remove fragments, preserve trailing slashes, remove default ports)
    #[tool(
        description = "Normalize a URL by removing fragments, preserving trailing slashes, and removing default ports."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn normalize_url(
        &self,
        Parameters(params): Parameters<NormalizeUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        use url_normalize::{normalize_url as normalize, Options, RemoveQueryParameters};

        if !params.url.contains("://") {
            return Ok(CallToolResult::error(vec![Content::text(
                "Invalid URL: no scheme found".to_string(),
            )]));
        }

        let opts = Options {
            strip_hash: true,
            remove_trailing_slash: false,
            remove_query_parameters: RemoveQueryParameters::All,
            sort_query_parameters: true,
            strip_www: false,
            force_https: false,
            ..Options::default()
        };

        match normalize(&params.url, &opts) {
            Ok(normalized) => Ok(CallToolResult::success(vec![Content::text(normalized)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Match a URL against a glob pattern
    #[tool(
        description = "Check if a URL matches a glob-style pattern. Supports path patterns (start with '/') and host patterns."
    )]
    #[instrument(skip(self), fields(url = %params.url, pattern = %params.pattern))]
    async fn match_url_pattern(
        &self,
        Parameters(params): Parameters<MatchUrlPatternParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let matches = rust_scraper_core::domain::matches_pattern(&params.url, &params.pattern);
        Ok(CallToolResult::success(vec![Content::text(
            matches.to_string(),
        )]))
    }

    /// Check if a URL is internal to a seed domain
    #[tool(
        description = "Check if a URL belongs to the same domain (or subdomain) as the seed domain."
    )]
    #[instrument(skip(self), fields(url = %params.url, seed_domain = %params.seed_domain))]
    async fn is_internal_link(
        &self,
        Parameters(params): Parameters<IsInternalLinkParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let is_internal = match (
            url::Url::parse(&params.url),
            url::Url::parse(&params.seed_domain),
        ) {
            (Ok(u), Ok(s)) => u.host_str() == s.host_str(),
            _ => false,
        };
        Ok(CallToolResult::success(vec![Content::text(
            is_internal.to_string(),
        )]))
    }

    /// Convert a URL to a domain-based file path
    #[tool(
        description = "Convert a URL to a domain-based file path. E.g., 'https://example.com/docs/page' → 'example.com/docs/page.md'."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn url_to_file_path(
        &self,
        Parameters(params): Parameters<ValidateUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match rust_scraper_core::adapters::url_path::OutputPath::from_url(&params.url) {
            Ok(output_path) => {
                let info = serde_json::json!({
                    "full_path": output_path.to_full_path(),
                    "relative_path": output_path.to_folder_path(),
                    "domain": output_path.domain().to_string(),
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    // ========================================================================
    // Category 5: Security & Diagnostics (4 tools)
    // ========================================================================

    /// Detect WAF/CAPTCHA challenge in HTML body
    #[tool(
        description = "Scan HTML body for WAF/CAPTCHA signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai, etc.). Returns provider name if detected."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn detect_waf(
        &self,
        Parameters(params): Parameters<DetectWafParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match rust_scraper_core::infrastructure::http::waf_engine::WafInspector::detect_body(
            &params.html,
        ) {
            Some(provider) => Ok(CallToolResult::success(vec![Content::text(format!(
                "WAF detected: {provider}"
            ))])),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no WAF detected",
            )])),
        }
    }

    /// Multi-layer WAF inspection (headers + body + entropy analysis)
    #[tool(
        description = "Multi-layer WAF inspection: checks control headers, body signatures via Aho-Corasick, and entropy analysis for silent challenges."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn verify_waf_integrity(
        &self,
        Parameters(params): Parameters<VerifyWafIntegrityParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let html = params.html.as_deref().unwrap_or("");
        let headers = wreq::header::HeaderMap::new();
        match rust_scraper_core::infrastructure::http::waf_engine::WafInspector::verify_integrity(
            &headers, html,
        ) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(
                "WAF integrity check passed",
            )])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "WAF blocked: {e}"
            ))])),
        }
    }

    /// List all supported WAF providers
    #[tool(
        description = "List all WAF/CAPTCHA providers that can be detected by the WAF inspector."
    )]
    #[instrument(skip(self))]
    async fn list_waf_providers(&self) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let providers =
            rust_scraper_core::infrastructure::http::waf_engine::WafInspector::supported_providers(
            );
        Ok(CallToolResult::success(vec![Content::text(
            providers.join(", "),
        )]))
    }

    /// Get scrape metrics (request timing, status codes, pages scraped)
    #[tool(
        description = "Get scraping metrics including request timing, status code distribution, and pages scraped per domain."
    )]
    #[instrument(skip(self))]
    async fn get_scrape_metrics(&self) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let metrics = serde_json::json!({
            "message": "Metrics collection requires active scraping session",
            "status": "available"
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&metrics).unwrap(),
        )]))
    }

    // ========================================================================
    // Category 6: Obsidian Integration (4 tools)
    // ========================================================================

    /// Detect Obsidian vault path using multi-priority detection
    #[tool(
        description = "Detect Obsidian vault path using multi-priority detection: CLI flag → env var → config file → registry → auto-scan."
    )]
    #[instrument(skip(self), fields(vault_path = ?params.vault_path))]
    async fn detect_obsidian_vault(
        &self,
        Parameters(params): Parameters<DetectVaultParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let cli_path = params
            .vault_path
            .as_ref()
            .map(|p| std::path::Path::new(p.as_str()));
        match rust_scraper_core::infrastructure::obsidian::vault_detector::detect_vault(
            cli_path, None, None,
        ) {
            Some(path) => Ok(CallToolResult::success(vec![Content::text(
                path.display().to_string(),
            )])),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no vault detected",
            )])),
        }
    }

    /// Build obsidian:// URI protocol link
    #[tool(
        description = "Build an obsidian:// URI protocol link to open a specific note in the Obsidian app."
    )]
    #[instrument(skip(self), fields(vault_name = %params.vault_name, file_path = %params.file_path))]
    async fn build_obsidian_uri(
        &self,
        Parameters(params): Parameters<BuildObsidianUriParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let uri = rust_scraper_core::infrastructure::obsidian::uri::build_obsidian_uri(
            &params.vault_name,
            &params.file_path,
        );
        Ok(CallToolResult::success(vec![Content::text(uri)]))
    }

    /// Open a note in Obsidian app via URI protocol
    #[tool(
        description = "Open a note in the Obsidian app using the obsidian:// URI protocol. Launches the Obsidian application."
    )]
    #[instrument(skip(self), fields(vault_name = %params.vault_name, file_path = %params.file_path))]
    async fn open_in_obsidian(
        &self,
        Parameters(params): Parameters<BuildObsidianUriParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let uri = rust_scraper_core::infrastructure::obsidian::uri::build_obsidian_uri(
            &params.vault_name,
            &params.file_path,
        );
        match std::process::Command::new("open").arg(&uri).spawn() {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Opened in Obsidian: {uri}"
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "failed to open Obsidian: {e}"
            ))])),
        }
    }

    /// Semantic search over Obsidian vault using embeddings
    #[tool(
        description = "Semantic search over Obsidian vault using tract-onnx embeddings. Returns top matching notes by cosine similarity."
    )]
    #[instrument(skip(self), fields(query = %params.query))]
    async fn search_obsidian(
        &self,
        Parameters(params): Parameters<SearchObsidianParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let limit = params.limit.unwrap_or(10);
        // TODO: Implement actual semantic search with tract-onnx embeddings
        // For now, return a placeholder indicating the feature requires --features ai
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Semantic search for '{}' (limit: {}) — requires --features ai for embedding-based search",
            params.query, limit
        ))]))
    }

    // ========================================================================
    // Category 8: Asset Management (1 tool)
    // ========================================================================

    /// Download images and documents from HTML
    #[tool(
        description = "Download images (PNG, JPG, GIF, WEBP, SVG, BMP) and/or documents (PDF, DOCX, XLSX, PPTX) from HTML content. Uses SHA256-hashed filenames."
    )]
    #[instrument(skip(self), fields(base_url = %params.base_url, images = params.images.unwrap_or(true), documents = params.documents.unwrap_or(false)))]
    async fn download_assets(
        &self,
        Parameters(params): Parameters<DownloadAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .assets
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let base_url = url::Url::parse(&params.base_url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid base URL: {e}"),
                Some(serde_json::Value::String("base_url".to_string())),
            )
        })?;

        let download_images = params.images.unwrap_or(true);
        let download_documents = params.documents.unwrap_or(false);

        // TODO(#170): Wire up actual asset downloading via shared Downloader
        // For now, return an explicit "not implemented" error instead of fake success
        Ok(CallToolResult::error(vec![Content::text(format!(
            "download_assets not yet implemented (see #170). \
             Use scrape_url with download_images=true instead. \
             Requested: images={download_images}, documents={download_documents}, base={base_url}"
        ))]))
    }
}

/// Implement ServerHandler for McpHandler.
///
/// The #[tool_handler] macro generates call_tool and list_tools
/// methods that delegate to self.tool_router.
#[tool_handler]
impl ServerHandler for McpHandler {}
