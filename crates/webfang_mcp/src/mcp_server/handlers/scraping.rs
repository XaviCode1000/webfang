//! Scraping Core tools — 8 tools for URL scraping and crawling
//!
//! Tools: scrape_url, scrape_with_options, scrape_batch, crawl_site,
//! crawl_with_sitemap, discover_urls, discover_sitemap, detect_spa

use super::McpHandler;
use crate::mcp_server::metrics::{domain_of, Outcome, ScrapeEvent};
use crate::mcp_server::params::*;
use crate::mcp_server::selector_service;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use std::time::Instant;
use tracing::instrument;

#[tool_router(router = tool_router_scraping, vis = "pub")]
impl McpHandler {
    /// Scrape a single URL and extract clean content using Readability algorithm
    #[tool(
        description = "Scrape a single URL and extract clean content using Readability algorithm (Firefox Reader mode). Returns title, content, excerpt, author, and date."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn scrape_url(
        &self,
        Parameters(params): Parameters<ScrapeUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, scraping);

        let url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let start = Instant::now();
        let client = self.state.container.http_client().as_ref();
        match webfang_core::application::scraper_service::scrape_with_readability(client, &url)
            .await
        {
            Ok(results) => {
                let count = results.len();
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_url",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                let content = serde_json::to_string_pretty(&results)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_url",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
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
        let _permit = acquire_semaphore!(self, scraping);

        let url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let mut config = webfang_core::infrastructure::config::ScraperConfig::default();
        if let Some(max) = params.max_pages {
            config.max_pages = Some(max as usize);
        }
        if params.download_images == Some(true) {
            config.download_images = true;
        }
        if params.download_documents == Some(true) {
            config.download_documents = true;
        }
        // Wire CSS selector (defaults to "body" when not provided)
        if let Some(ref sel) = params.selector {
            config.selector = sel.clone();
        }

        let start = Instant::now();
        let client = self.state.container.http_client().as_ref();
        let dl = self
            .state
            .downloader
            .as_deref()
            .map(|d| d as &dyn webfang_core::domain::ports::AssetDownloaderPort);
        let inspector = self.state.inspector.as_deref();
        // An MCP tool call IS an operation (#501): mint the run-root identity
        // at the handler entry; the use case derives the page child from it.
        let root_correlation = webfang_core::domain::CorrelationId::new();
        match webfang_core::application::scraper_service::scrape_with_config(
            client,
            &url,
            &config,
            dl,
            inspector,
            None,
            &root_correlation,
        )
        .await
        {
            Ok(outcome) => {
                let count = outcome.results.len();
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_with_options",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                let response = selector_service::build_scrape_response(
                    outcome.results,
                    &outcome.extract_result,
                    &params.selector,
                );
                let content = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_with_options",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
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
        let _permit = acquire_semaphore!(self, scraping);

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

        let start = Instant::now();
        let domain = urls
            .first()
            .and_then(|u| u.host_str())
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string());
        let count = urls.len();

        let mut config = webfang_core::infrastructure::config::ScraperConfig::default();
        if let Some(c) = params.concurrency {
            config.scraper_concurrency = c;
        }

        let client = self.state.container.http_client().as_ref();
        let dl = self
            .state
            .downloader
            .as_deref()
            .map(|d| d as &dyn webfang_core::domain::ports::AssetDownloaderPort);
        match webfang_core::application::scraper_service::scrape_multiple_with_limit(
            client, &urls, &config, dl,
        )
        .await
        {
            Ok(results) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_batch",
                    domain,
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                tracing::info!("batch scrape complete: {} pages", results.len());
                let content = serde_json::to_string_pretty(&results)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "scrape_batch",
                    domain,
                    outcome: Outcome::Error,
                    count,
                    duration: start.elapsed(),
                });
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
    // serde_json::to_string cannot fail for a serde_json::Value.
    #[allow(clippy::expect_used)]
    async fn crawl_site(
        &self,
        Parameters(params): Parameters<CrawlSiteParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, scraping);

        let seed_url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        let start = Instant::now();
        let crawler_config = webfang_core::domain::CrawlerConfig::builder(seed_url)
            .max_depth(params.max_depth.unwrap_or(3))
            .max_pages(params.max_pages.unwrap_or(100) as usize)
            .build();

        match webfang_core::application::crawler::crawl_site(crawler_config).await {
            Ok(result) => {
                let count = result.total_pages;
                self.state.record_scrape(ScrapeEvent {
                    tool: "crawl_site",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                let urls: Vec<String> = result.urls.iter().map(|u| u.url.to_string()).collect();
                let json = serde_json::json!({
                    "urls": urls,
                    "total_pages": result.total_pages,
                    "errors": result.errors,
                    "error_breakdown": result.error_breakdown,
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&json)
                        .expect("serializing JSON to a string cannot fail"),
                )]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "crawl_site",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
        }
    }

    /// Discover and crawl URLs from a sitemap
    #[tool(
        description = "Discover URLs from a website's sitemap and crawl them. Auto-discovers sitemap from robots.txt if not provided."
    )]
    // serde_json::to_string cannot fail for a serde_json::Value.
    #[allow(clippy::expect_used)]
    async fn crawl_with_sitemap(
        &self,
        Parameters(params): Parameters<CrawlWithSitemapParams>,
    ) -> Result<CallToolResult, McpError> {
        let span = tracing::info_span!("mcp.crawl_with_sitemap", url = %params.url);
        let _enter = span.enter();

        tracing::info!("starting sitemap crawl");
        let _permit = acquire_semaphore!(self, scraping);

        let seed_url = url::Url::parse(&params.url).map_err(|e| {
            tracing::error!("invalid URL: {}", e);
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;
        let start = Instant::now();
        let config = webfang_core::domain::CrawlerConfig::new(seed_url);
        match webfang_core::application::crawler::crawl_with_sitemap(
            &params.url,
            params.sitemap_url.as_deref(),
            &config,
        )
        .await
        {
            Ok(urls) => {
                let count = urls.len();
                self.state.record_scrape(ScrapeEvent {
                    tool: "crawl_with_sitemap",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                tracing::info!("sitemap crawl complete: {} urls found", urls.len());
                let url_strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&url_strings)
                        .expect("serializing JSON to a string cannot fail"),
                )]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "crawl_with_sitemap",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
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
        let _permit = acquire_semaphore!(self, scraping);

        let start = Instant::now();
        let port = self.state.container.http_client();
        match port.get(&params.url).await {
            Ok(resp) => {
                let html = resp.body;
                match webfang_core::infrastructure::crawler::extract_links(&html, &params.url) {
                    Ok(links) => {
                        let count = links.len();
                        self.state.record_scrape(ScrapeEvent {
                            tool: "discover_urls",
                            domain: domain_of(&params.url),
                            outcome: Outcome::Success,
                            count,
                            duration: start.elapsed(),
                        });
                        let content = serde_json::to_string_pretty(&links)
                            .unwrap_or_else(|_| "failed to serialize".into());
                        Ok(CallToolResult::success(vec![Content::text(content)]))
                    },
                    Err(e) => {
                        self.state.record_scrape(ScrapeEvent {
                            tool: "discover_urls",
                            domain: domain_of(&params.url),
                            outcome: Outcome::Error,
                            count: 0,
                            duration: start.elapsed(),
                        });
                        Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
                    },
                }
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "discover_urls",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "HTTP error: {e}"
                ))]))
            },
        }
    }

    /// Auto-discover sitemap URL from robots.txt or common locations
    #[tool(
        description = "Auto-discover a website's sitemap URL by checking robots.txt and common locations (/sitemap.xml, /sitemap_index.xml, etc.)."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn discover_sitemap(
        &self,
        Parameters(params): Parameters<DiscoverUrlsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, scraping);

        let seed = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;
        let start = Instant::now();
        let crawler_config = webfang_core::domain::CrawlerConfig::new(seed);

        match webfang_core::application::crawler::crawl_with_sitemap(
            &params.url,
            None,
            &crawler_config,
        )
        .await
        {
            Ok(discovered) => {
                let count = discovered.len();
                self.state.record_scrape(ScrapeEvent {
                    tool: "discover_sitemap",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count,
                    duration: start.elapsed(),
                });
                let urls: Vec<String> = discovered.into_iter().map(|d| d.url.to_string()).collect();
                let content = serde_json::to_string_pretty(&urls)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "discover_sitemap",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            },
        }
    }

    /// Detect if a URL requires JavaScript rendering (SPA)
    #[tool(
        description = "Detect if a page requires JavaScript rendering (Single Page Application). Checks for minimal content and SPA markers like <div id=\"root\"> or <div id=\"app\">."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    // serde_json::to_string cannot fail for a serde_json::Value.
    #[allow(clippy::expect_used)]
    async fn detect_spa(
        &self,
        Parameters(params): Parameters<DetectSpaParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, scraping);

        let start = Instant::now();
        let port = self.state.container.http_client();
        match port.get(&params.url).await {
            Ok(resp) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "detect_spa",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Success,
                    count: 1,
                    duration: start.elapsed(),
                });
                let html = resp.body;
                let text = webfang_core::infrastructure::scraper::fallback::extract_text(&html);
                match webfang_core::application::scraper_service::detect_spa_content(
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
                            serde_json::to_string_pretty(&json)
                                .expect("serializing JSON to a string cannot fail"),
                        )]))
                    },
                    None => Ok(CallToolResult::success(vec![Content::text(
                        "not an SPA - sufficient content found",
                    )])),
                }
            },
            Err(e) => {
                self.state.record_scrape(ScrapeEvent {
                    tool: "detect_spa",
                    domain: domain_of(&params.url),
                    outcome: Outcome::Error,
                    count: 0,
                    duration: start.elapsed(),
                });
                Ok(CallToolResult::error(vec![Content::text(format!(
                    "HTTP error: {e}"
                ))]))
            },
        }
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_scraping()
}
