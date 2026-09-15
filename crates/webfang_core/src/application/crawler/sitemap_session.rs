//! Session-owned `crawl_with_sitemap` run entry (sitemap-crawl-run-parity D2).
//!
//! The MCP `crawl_with_sitemap` tool discovers sitemap URLs and then needs a
//! stored crawl run — exactly as `crawl_site` does — so that a subsequent
//! `export_jsonl` / `export_vector` serves the sitemap run instead of a stale
//! previous run. This module is the single public seam for that: owned
//! [`DiscoveredUrl`]s + [`CrawlerConfig`] + [`EngineOptions`] +
//! [`CorrelationId`] in, [`CrawlResult`] out. The handler never names session
//! types (`CrawlSessionBuilder` / `CrawlIdentity` / `Engine::from_session` are
//! all `pub(crate)`), preserving the `mcp → core` direction.

#![deny(clippy::await_holding_lock)]

use tracing::{info, instrument};

use super::engine::{Engine, EngineOptions};
use super::session::{CrawlIdentity, CrawlPorts, CrawlSession, TransportPolicy};
use crate::domain::persistence::PersistenceMode;
use crate::domain::{CorrelationId, CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl};
use crate::infrastructure::observability::log_scrape_error;

/// Crawl the seed plus sitemap-discovered URLs as one session-owned run.
///
/// BFS-expands links from the seed AND injects `extra_seeds` as additional
/// seeds (drained with [`UrlSource::Sitemap`](crate::domain::crawler_port::UrlSource)
/// right after the scheduler seed — enqueue-time dedup absorbs seed/sitemap
/// overlap, so the seed page is crawled exactly once even when absent from the
/// sitemap). Checkpointing is fixed to [`PersistenceMode::Disabled`], as
/// `crawl_site` without `checkpoint_dir`.
///
/// # Errors
///
/// Returns [`CrawlError`] when the session description fails validation or the
/// crawl itself fails — the same stratification as
/// [`crawl_site_with_options`](super::engine::crawl_site_with_options).
pub async fn crawl_with_sitemap_session(
    config: CrawlerConfig,
    extra_seeds: Vec<DiscoveredUrl>,
    options: EngineOptions,
    correlation: CorrelationId,
) -> Result<CrawlResult, CrawlError> {
    crawl_with_sitemap_session_inner(config, extra_seeds, options, correlation).await
}

/// Inner implementation of [`crawl_with_sitemap_session`].
///
/// Mirrors `crawl_site_with_options_inner`: the `#[instrument]` span declares
/// the run-root identity (`correlation_id`, `trace_id`) AT CREATION time
/// (#501); no `enter()` guard crosses an `.await` (#519).
#[instrument(
    name = "crawl_with_sitemap_session",
    skip(config, extra_seeds, options, correlation),
    fields(
        correlation_id = %correlation,
        trace_id = %correlation.trace_id(),
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        extra_seeds = extra_seeds.len(),
        session_pool = options.session_pool_enabled,
        ignore_robots = options.ignore_robots,
        capture_enabled = options.content_sink.is_some(),
        shared_limiter = options.rate_limiter.is_some()
    )
)]
async fn crawl_with_sitemap_session_inner(
    config: CrawlerConfig,
    extra_seeds: Vec<DiscoveredUrl>,
    options: EngineOptions,
    correlation: CorrelationId,
) -> Result<CrawlResult, CrawlError> {
    info!(
        "Starting sitemap crawl from {} with max_depth={} max_pages={} extra_seeds={} (session_pool={}, ignore_robots={})",
        config.seed_url,
        config.max_depth,
        config.max_pages,
        extra_seeds.len(),
        options.session_pool_enabled,
        options.ignore_robots
    );

    let seed_url = config.seed_url.as_str().to_string();
    let run_label = config.seed_url.host_str().unwrap_or("seed").to_string();
    // D2: same run-object construction as `crawl_site_with_options_inner`,
    // with checkpointing fixed to Disabled and the sitemap URLs carried as
    // additional seeds. A build failure fails the run before any worker
    // spawns — no silent fallback (matrix rows 31/33).
    let mut session = CrawlSession::builder()
        .config(config)
        .persistence(PersistenceMode::Disabled)
        .transport(TransportPolicy::from(&options))
        .ports(CrawlPorts {
            session_pool: None,
            downloader_factory: options.downloader_factory.clone(),
            content_sink: options.content_sink.clone(),
            pipeline: None,
            output_stages: Vec::new(),
        })
        .identity(CrawlIdentity {
            root: correlation.clone(),
            run_label,
        })
        .extra_seeds(extra_seeds)
        .build()
        .map_err(|err| {
            log_scrape_error(
                &err,
                &seed_url,
                "session",
                Some(&correlation),
                "session build failed — refusing to run (no legacy fallback)",
            );
            CrawlError::from(err)
        })?;

    session.begin();
    let mut engine = Engine::from_session(session)?;
    // #1428 RUN-scope limiter: same swap as `crawl_site_with_options_inner` —
    // a shared bucket carried on the options replaces the per-engine bucket
    // before `run()` spawns any task. `None` keeps per-engine behavior.
    if let Some(limiter) = options.rate_limiter.clone() {
        engine.set_shared_limiter(limiter);
    }
    let result = engine.run().await;
    engine.shutdown().await;
    result
}

#[cfg(test)]
#[cfg(not(miri))] // wiremock + wreq use boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;
    use url::Url;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// sitemap-crawl-run-parity 1.5 (TRIANGULATE): the new entry crawls the
    /// seed (BFS, including the linked page) PLUS the sitemap-only unlinked
    /// page, with seed/sitemap overlap deduped — the seed appears exactly
    /// once even when `extra_seeds` carries a seed duplicate, and the seed
    /// is included although absent from the sitemap set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sitemap_entry_covers_seed_bfs_and_sitemap_only_page_once() {
        let server = MockServer::start().await;
        let port = server.address().port();
        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("seed");
        let linked = Url::parse(&format!("http://127.0.0.1:{port}/x")).expect("linked");
        let only = Url::parse(&format!("http://127.0.0.1:{port}/y")).expect("sitemap-only");

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><a href=\"{linked}\">next</a></body></html>"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/x"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>linked</body></html>"),
            )
            .mount(&server)
            .await;
        Mock::given(path("/y"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html><body>sitemap-only</body></html>"),
            )
            .mount(&server)
            .await;

        let config = CrawlerConfig::builder(seed.clone())
            .max_depth(2)
            .max_pages(10)
            .ignore_robots(true)
            .build();
        // Sitemap set: a seed duplicate (overlap) + the unlinked page at the
        // depth `build_discovered_urls` assigns non-seed sitemap URLs (1).
        // The seed itself is absent from the sitemap set as a sitemap row.
        let extra_seeds = vec![
            DiscoveredUrl::html(seed.clone(), 0, seed.clone()),
            DiscoveredUrl::html(only.clone(), 1, seed.clone()),
        ];
        let options = EngineOptions {
            ignore_robots: true,
            ..Default::default()
        };

        let result = crawl_with_sitemap_session(config, extra_seeds, options, CorrelationId::new())
            .await
            .expect("sitemap session run must succeed");

        let urls: Vec<String> = result.urls.iter().map(|u| u.url.to_string()).collect();
        assert!(
            urls.contains(&seed.to_string()),
            "seed must be crawled although absent from the sitemap set: {urls:?}"
        );
        assert!(
            urls.contains(&linked.to_string()),
            "BFS-linked page must be crawled: {urls:?}"
        );
        assert!(
            urls.contains(&only.to_string()),
            "sitemap-only unlinked page must be crawled: {urls:?}"
        );
        assert_eq!(
            urls.iter().filter(|u| *u == &seed.to_string()).count(),
            1,
            "seed/sitemap overlap must dedup to exactly one seed fetch: {urls:?}"
        );
        assert_eq!(result.total_pages, 3, "exactly the three pages: {urls:?}");
    }

    /// sitemap-crawl-run-parity 1.5 (TRIANGULATE, alternate case): empty
    /// `extra_seeds` keeps today's single-seed behaviour — only the seed and
    /// its BFS-linked page are crawled, so every existing call site is
    /// unaffected by the additive hook.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sitemap_entry_with_empty_extra_seeds_keeps_single_seed_behaviour() {
        let server = MockServer::start().await;
        let port = server.address().port();
        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("seed");
        let linked = Url::parse(&format!("http://127.0.0.1:{port}/x")).expect("linked");

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><a href=\"{linked}\">next</a></body></html>"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/x"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>linked</body></html>"),
            )
            .mount(&server)
            .await;

        let config = CrawlerConfig::builder(seed.clone())
            .max_depth(2)
            .max_pages(10)
            .ignore_robots(true)
            .build();
        let options = EngineOptions {
            ignore_robots: true,
            ..Default::default()
        };

        let result = crawl_with_sitemap_session(config, Vec::new(), options, CorrelationId::new())
            .await
            .expect("empty-seed run must succeed");

        let urls: Vec<String> = result.urls.iter().map(|u| u.url.to_string()).collect();
        assert!(urls.contains(&seed.to_string()), "seed crawled: {urls:?}");
        assert!(
            urls.contains(&linked.to_string()),
            "BFS link crawled: {urls:?}"
        );
        assert_eq!(result.total_pages, 2, "no phantom third page: {urls:?}");
    }

    /// sitemap-crawl-run-parity 4.2 (RED): the NAIVE unbounded reading —
    /// `max_pages = 3` with 8 sitemap seeds crawls all 9 pages. This MUST
    /// fail (the bound truncates the run); the GREEN pin below asserts the
    /// documented in-flight-drain bound instead.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sitemap_entry_max_pages_truncates_many_url_fixture() {
        let result = many_url_sitemap_run(3, 2).await;
        let urls: Vec<String> = result.urls.iter().map(|u| u.url.to_string()).collect();
        assert_eq!(
            result.total_pages, 9,
            "NAIVE unbounded reading: all 9 pages crawled: {urls:?}"
        );
    }

    /// Many-URL sitemap fixture for the 4.2 overshoot pin: a link-free
    /// seed plus 8 sitemap-only leaf pages (rich-enough bodies to clear
    /// the extraction pipeline), driven through the new session entry
    /// with an explicit `max_pages` / `concurrency` pair.
    async fn many_url_sitemap_run(max_pages: usize, concurrency: usize) -> CrawlResult {
        use std::num::NonZeroUsize;

        const LEAVES: usize = 8;
        let server = MockServer::start().await;
        let port = server.address().port();
        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("seed");

        Mock::given(path("/"))
                .respond_with(ResponseTemplate::new(200).set_body_string(
                    "<html><head><title>Seed</title></head><body><h1>Overshoot seed</h1><p>Leaf-free seed page with enough ordinary text for the readability pipeline to accept it as main content without tripping the minimum content guard.</p></body></html>".to_string(),
                ))
                .mount(&server)
                .await;
        let mut extra_seeds = Vec::with_capacity(LEAVES);
        for i in 0..LEAVES {
            let leaf = Url::parse(&format!("http://127.0.0.1:{port}/p{i}")).expect("leaf");
            Mock::given(path(format!("/p{i}")))
                    .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                        "<html><head><title>Leaf {i}</title></head><body><h1>Overshoot leaf {i}</h1><p>Leaf page number {i} carrying enough ordinary text for the readability pipeline to accept it as main content without tripping the minimum content guard.</p></body></html>"
                    )))
                    .mount(&server)
                    .await;
            extra_seeds.push(DiscoveredUrl::html(leaf, 1, seed.clone()));
        }

        let config = CrawlerConfig::builder(seed.clone())
            .max_depth(5)
            .max_pages(max_pages)
            .concurrency(NonZeroUsize::new(concurrency).expect("non-zero"))
            .ignore_robots(true)
            .build();
        let options = EngineOptions {
            ignore_robots: true,
            ..Default::default()
        };

        crawl_with_sitemap_session(config, extra_seeds, options, CorrelationId::new())
            .await
            .expect("many-URL sitemap run must succeed")
    }
}
