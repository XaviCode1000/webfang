//! Crawl task execution — per-page fetch, pipeline, and link extraction.
//!
//! Extracted from `engine.rs` (strangler fig, issue #439). Holds the free
//! functions spawned per discovered URL by `Engine::run()`:
//! `run_crawl_task` (the per-page worker) and `handle_crawl_result`
//! (task-completion bookkeeping). Both consume `Arc<CrawlTaskCtx>` and carry
//! no `Engine` state.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, span, warn, Level};
use url::Url;

use super::checkpoint::BannedDomain;
use super::collector::CrawlMessage;
use super::crawl_task_ctx::CrawlTaskCtx;
use crate::application::pipeline::{ScrapedItem, StageOutcome};
use crate::application::url_filter::is_allowed;
use crate::domain::{CrawlError, CrawlErrorCategory, DiscoveredUrl};
use crate::infrastructure::crawler::{extract_links, fetch_url, is_internal_link, UrlSource};
use crate::infrastructure::downloader::{DownloadError, Downloader};
use crate::infrastructure::network::session_pool::SessionManager;
use crate::infrastructure::observability::log_scrape_error;

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => {
            let category = CrawlErrorCategory::from(&e);
            warn!("Task error: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[category.index()].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
        Err(e) => {
            warn!("Task panicked: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[CrawlErrorCategory::Panic.index()]
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    }
}

/// Execute a single crawl task using shared context.
///
/// Extracted from the inline async block in `Engine::run()` to reduce
/// the per-spawn clone surface from 18 individual `Arc::clone()` calls
/// to a single `Arc<CrawlTaskCtx>` clone.
pub(crate) async fn run_crawl_task(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
) -> Result<(), CrawlError> {
    // Rate limiting
    ctx.rate_limiter.until_ready().await;

    let url_str = discovered_url.url.as_str().to_string();
    let url_depth = discovered_url.depth;
    let parent_url = discovered_url.url.clone();

    // Per-page correlation (issue #356): share the crawl's trace_id, fresh
    // span_id. Lets a whole crawl be reconstructed by trace_id while each
    // page stays distinguishable by span_id.
    let page_correlation = ctx.correlation_id.child();
    let page_span = span!(
        Level::DEBUG,
        "crawl_page",
        correlation_id = %page_correlation,
        trace_id = %page_correlation.trace_id(),
        url = %url_str,
        depth = url_depth
    );
    let _page_guard = page_span.enter();

    // Session pool: check if domain is healthy before fetching
    let mut session_id = None;
    if let Some(ref pool) = ctx.session_pool {
        let domain = url::Url::parse(&url_str)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        match pool.acquire(&domain) {
            Some(id) => {
                session_id = Some(id);
            },
            None => {
                debug!("Domain {} has no available sessions, skipping", domain);
                return Ok(());
            },
        }
    }

    debug!("Crawling: {} (depth={})", url_str, url_depth);

    // Fetch URL — use fetch_router if available, else static fetch_url()
    let (response, fetched_cookies) = if let Some(ref router) = ctx.fetch_router {
        let parsed_url = url::Url::parse(&url_str)
            .map_err(|e| CrawlError::Internal(format!("invalid URL: {e}")))?;
        match router.fetch(&parsed_url).await {
            Ok(page) => {
                let cookies = page.cookies.clone();
                (page.html, cookies)
            },
            Err(DownloadError::WafChallenge(msg)) => {
                // Ban the domain
                if let Some(domain) = parsed_url.host_str() {
                    let banned = BannedDomain {
                        domain: domain.to_string(),
                        banned_until: None,
                        reason: msg.clone(),
                    };
                    if let Ok(mut domains) = ctx.banned_domains.write() {
                        if !domains.iter().any(|d| d.domain == domain) {
                            domains.push(banned);
                            warn!("Banned domain {} due to WAF: {}", domain, msg);
                        }
                    }
                }
                log_scrape_error(
                    &msg,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "WAF challenge detected",
                );
                return Err(DownloadError::WafChallenge(msg).into());
            },
            Err(e) => {
                log_scrape_error(
                    &e,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "page fetch failed",
                );
                return Err(e.into());
            },
        }
    } else {
        match fetch_url(&url_str, &ctx.config).await {
            Ok(html) => (html, Vec::new()),
            Err(e) => {
                if format!("{e}").contains("WAF") {
                    // Ban the domain
                    if let Ok(parsed) = url::Url::parse(&url_str) {
                        if let Some(domain) = parsed.host_str() {
                            let banned = BannedDomain {
                                domain: domain.to_string(),
                                banned_until: None,
                                reason: e.to_string(),
                            };
                            if let Ok(mut domains) = ctx.banned_domains.write() {
                                if !domains.iter().any(|d| d.domain == domain) {
                                    domains.push(banned);
                                    warn!("Banned domain {} due to WAF: {}", domain, e);
                                }
                            }
                        }
                    }
                }
                log_scrape_error(
                    &e,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "page fetch failed",
                );
                return Err(e);
            },
        }
    };

    // Ingest cookies into the cookie bridge
    if !fetched_cookies.is_empty() {
        if let Ok(mut bridge) = ctx.cookie_bridge.write() {
            for cookie in &fetched_cookies {
                bridge.add(cookie.clone());
            }
        }
    }

    // Report success to session pool
    if let Some(ref pool) = ctx.session_pool {
        if let Some(id) = session_id {
            if let Ok(parsed) = url::Url::parse(&url_str) {
                if let Some(domain) = parsed.host_str() {
                    pool.report_success(domain, id);
                }
            }
        }
    }

    // Track pages crawled
    ctx.pages_crawled
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Pipeline processing: convert to ScrapedItem and run through pipeline
    if let Some(ref pipeline) = ctx.pipeline {
        let item = ScrapedItem {
            url: url_str.clone(),
            raw_html: response.clone(),
            text_content: None,
            metadata: std::collections::HashMap::new(),
            status_code: 200,
            embeddings: None,
        };

        match pipeline.execute(item).await {
            StageOutcome::Continue(processed_item) => {
                // Pass to output stages
                for stage in &ctx.output_stages {
                    if let Err(e) = stage.write(&processed_item).await {
                        warn!("Output stage '{}' failed: {}", stage.name(), e);
                    }
                }
            },
            StageOutcome::Skip => {
                debug!("Pipeline skipped item: {}", url_str);
                return Ok(());
            },
            StageOutcome::Reject(reason) => {
                warn!("Pipeline rejected {}: {}", url_str, reason);
                return Ok(());
            },
        }
    }

    // Add to results via channel (sin lock)
    if let Err(e) = ctx
        .collector
        .send(CrawlMessage::success(discovered_url))
        .await
    {
        debug!("Failed to send result: {}", e);
    }

    // Extract links and add to queue
    if url_depth < ctx.config.max_depth {
        match extract_links(&response, &url_str) {
            Ok(links) => {
                for link in links {
                    // extract_links() already normalizes each link
                    if let Ok(parsed_url) = Url::parse(&link) {
                        if let Some(seed_domain) = ctx.config.seed_url.host_str() {
                            let link_domain = parsed_url.host_str().unwrap_or("");
                            if is_internal_link(&link, seed_domain)
                                && is_allowed(&link, &ctx.config)
                                && (ctx.ignore_robots
                                    || ctx.robots_fetcher.is_allowed(&link, link_domain).await)
                                && ctx.visited.try_insert(&link)
                            {
                                // Record URL string for checkpoint
                                if let Ok(mut urls) = ctx.visited_urls.write() {
                                    urls.push(link.clone());
                                }

                                let new_discovered = DiscoveredUrl::html(
                                    parsed_url,
                                    url_depth + 1,
                                    parent_url.clone(),
                                );
                                ctx.queue
                                    .push_prioritized(new_discovered, UrlSource::Link)
                                    .await;
                            }
                        }
                    }
                }
            },
            Err(e) => {
                warn!("Failed to extract links from {}: {}", url_str, e);
                ctx.error_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ctx.error_breakdown[CrawlErrorCategory::Extraction.index()]
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            },
        }
    }

    Ok(())
}
