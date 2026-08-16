//! Scraping flow logic extracted from orchestrator.

use std::path::PathBuf;

use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::build_fetch_router;
use crate::application::export_factory;
use crate::application::progress_observer::ProgressObserver;
use crate::application::scrape_single_url_for_tui;
use crate::cli::error::CliExit;
use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::{CorrelationId, ScrapedContent};
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::downloader::Downloader;
use crate::infrastructure::export::state_store::StateStore;
use crate::infrastructure::observability::log_scrape_error;
use crate::HttpClientConfig;
use crate::ScraperConfig;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

/// Apply resume mode filtering.
///
/// # Errors
///
/// Returns `CliExit::IoError` when `--resume` is active and the state store
/// cannot be created. Without a working store the crawl would silently
/// re-scrape every URL, defeating the purpose of resume mode.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    opts: &CrawlOptions,
    target_url: &str,
    root_correlation: &CorrelationId,
) -> Result<(Vec<Url>, Option<StateStore>), CliExit> {
    let state_store: Option<StateStore> = if opts.crawl.resume {
        info!("Resume mode enabled - tracking processed URLs");
        let state_dir = resolve_state_dir(opts);

        let domain = export_factory::domain_from_url(target_url);
        info!("State store domain: {}", domain);
        match export_factory::create_state_store(state_dir, &domain) {
            Ok(store) => Some(store),
            // LCOV_EXCL_START defensive: state-store-creation — store creation failure with --resume is an environment invariant break
            Err(e) => {
                tracing::error!(error = %e, "state store creation failed with --resume active");
                return Err(CliExit::IoError(format!(
                    "No se pudo crear el almacén de estado para --resume: {e}"
                )));
            },
            // LCOV_EXCL_STOP
        }
    } else {
        None
    };

    let filtered = if opts.crawl.resume {
        match state_store.as_ref() {
            Some(store) => filter_processed_urls(urls_to_scrape, store, root_correlation),
            None => urls_to_scrape,
        }
    } else {
        urls_to_scrape
    };

    Ok((filtered, state_store))
}

/// Resolve the resume state directory, defaulting to the XDG cache path.
fn resolve_state_dir(opts: &CrawlOptions) -> PathBuf {
    opts.crawl.state_dir.clone().unwrap_or_else(|| {
        let cache_base = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".cache")
            });
        cache_base.join("webfang").join("state")
    })
}

/// Drop URLs already processed by a prior run, degrading gracefully on error.
fn filter_processed_urls(
    urls_to_scrape: Vec<Url>,
    store: &StateStore,
    root_correlation: &CorrelationId,
) -> Vec<Url> {
    match store.load_or_default() {
        Ok(state) => {
            let original_count = urls_to_scrape.len();
            let filtered: Vec<_> = urls_to_scrape
                .into_iter()
                .filter(|url| {
                    let should_skip = store.is_processed(&state, url.as_str());
                    if should_skip {
                        info!("Skipping already processed: {}", url);
                    }
                    !should_skip
                })
                .collect();

            let skipped_count = original_count - filtered.len();
            info!(
                "Resume mode: {} URLs already processed, {} new URLs to scrape",
                skipped_count,
                filtered.len()
            );

            filtered
        },
        Err(e) => {
            log_scrape_error(
                &e,
                "",
                "state-load",
                Some(root_correlation),
                "could not load checkpoint state",
            );
            urls_to_scrape
        },
    }
}

/// Scrape all URLs, reporting progress via the provided observer.
///
/// Returns `(results, failures, blocked)` where `blocked` counts URLs skipped
/// because robots.txt disallowed them (#705) — distinct from both successful
/// results and failures, and from shutdown-skipped URLs.
///
/// Correlation contract (#501): `root_correlation` is the run-root identity
/// owned by the orchestrator; each page derives `.child()` from it — one
/// shared `trace_id` for the whole run, a fresh `span_id` per page.
///
/// The observer handles quiet/channel logic internally — callers pass
/// `&NoopObserver` for dry-run or `&LiveProgressObserver` for live output.
///
/// # Errors
///
/// Returns [`crate::error::ScraperError`] if the configured H2/TLS profile name
/// (`opts.network.h2_profile`) is not recognized, or if the fetch router's HTTP
/// client cannot be built. Both are setup failures that abort the whole batch
/// before any URL is scraped.
// The parameter list is the scrape phase's full dependency set (config,
// observer, downloader, adaptive engine, correlation root, shutdown token).
// Bundling them into a struct would only move the same wiring one level up.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_urls(
    urls: &[Url],
    scraper_config: &ScraperConfig,
    opts: &CrawlOptions,
    observer: &dyn ProgressObserver,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    root_correlation: &CorrelationId,
    cancel: &CancellationToken,
) -> Result<
    (
        Vec<ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
    ),
    crate::error::ScraperError,
> {
    // Build the fetch router from the configured JS strategy.
    let http_config = build_http_client_config(opts)?;
    let mut cookie_bridge = CookieBridge::new();
    if let Some(first_url) = urls.first() {
        let domain = first_url.host_str().unwrap_or("").to_string();
        if !domain.is_empty() {
            for (name, value) in &opts.network.initial_cookies {
                cookie_bridge.seed(name, value, &domain);
            }
        }
    }
    let cookie_bridge = std::sync::Arc::new(std::sync::RwLock::new(cookie_bridge));
    let router = build_fetch_router(
        &opts.network.js_strategy,
        http_config.timeout_secs,
        http_config.tls_emulation,
        cookie_bridge,
        opts.crawl.ignore_waf,
        // #503: move the operator's --user-agent into the wreq layer instead
        // of dropping it on the floor. `http_config` stays usable below —
        // only this field moves out.
        http_config.user_agent,
        // #653: the run's shutdown token, so Full-strategy governor waits abort
        // on SIGINT/SIGTERM instead of hanging until their own timeout.
        cancel.clone(),
        http_config.max_retries,
        http_config.backoff_base_ms,
        http_config.backoff_max_ms,
    )?;

    let _total_urls = urls.len();

    // Robots.txt fetcher — shares the batch's TLS fingerprint so the robots.txt
    // request is indistinguishable from a page fetch (#337). Shared across all
    // URLs in this batch.
    let robots_fetcher = RobotsFetcher::new(http_config.tls_emulation, http_config.timeout_secs)?;

    // Apply max_pages limit if configured
    let urls_to_process = apply_max_pages_limit(urls, scraper_config);

    let processing_count = urls_to_process.len();
    let mut results = Vec::with_capacity(processing_count);
    let mut failures: Vec<(String, crate::error::ScraperError)> = Vec::new();

    let ctx = ScrapeContext {
        router: &router,
        scraper_config,
        downloader,
        engine,
        robots_fetcher: &robots_fetcher,
    };

    // Honor `--concurrency` (#653): the previous sequential loop made the flag
    // a no-op on the default scrape path. `buffer_unordered` keeps at most
    // `concurrency` fetches in flight; the enumerated index restores the
    // original URL order afterwards so output stays deterministic.
    let concurrency = opts.network.concurrency.resolve().max(1);
    info!(
        concurrency,
        urls = processing_count,
        "scraping with bounded concurrency"
    );

    let mut ordered: Vec<(usize, Option<ScrapeOutcome>)> =
        futures::stream::iter(urls_to_process.into_iter().enumerate())
            .map(|(index, url)| {
                let ctx = &ctx;
                async move {
                    // Shutdown (#653): stop starting new pages, but let the ones
                    // already in flight finish so their content still reaches
                    // the export phase.
                    if cancel.is_cancelled() {
                        return (index, None);
                    }
                    // Per-page identity: child of the run root — shared trace_id, fresh
                    // span_id (#501).
                    let page_correlation = root_correlation.child();
                    let outcome =
                        scrape_one_url(&url, ctx, opts, observer, &page_correlation).await;
                    (index, Some((url, outcome)))
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

    ordered.sort_by_key(|(index, _)| *index);

    let mut skipped = 0usize;
    let mut blocked = 0usize;
    for (_, slot) in ordered {
        let Some((url, outcome)) = slot else {
            skipped += 1;
            continue;
        };
        match outcome {
            Ok(Some(content)) => results.push(content),
            // Robots.txt blocked — skipped, not a failure. Counted separately
            // (#705) so an all-blocked run can exit 77 instead of a misleading
            // "no pages scraped" network error.
            Ok(None) => blocked += 1,
            Err(e) => failures.push((url.as_str().to_string(), e)),
        }
    }

    if skipped > 0 {
        warn!(skipped, "shutdown requested — URLs left unscraped");
    }

    let total_successful = results.len();
    let total_failed = failures.len();
    observer
        .on_finished(processing_count, total_successful, total_failed)
        .await;

    Ok((results, failures, blocked))
}

/// Result of one page scrape: the URL plus its outcome (content, robots-skip,
/// or failure). Kept as an alias so the concurrent pipeline's element type
/// stays readable.
type ScrapeOutcome = (
    Url,
    Result<Option<ScrapedContent>, crate::error::ScraperError>,
);

/// Shared per-URL dependencies for a single scrape, bundled to keep the
/// per-page helper's signature small.
struct ScrapeContext<'a> {
    router: &'a dyn Downloader,
    scraper_config: &'a ScraperConfig,
    downloader: Option<&'a dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&'a AdaptiveSelectorEngine>,
    robots_fetcher: &'a RobotsFetcher,
}

/// Apply the `max_pages` cap to the URL list when configured.
fn apply_max_pages_limit(urls: &[Url], scraper_config: &ScraperConfig) -> Vec<Url> {
    if let Some(max_pages) = scraper_config.max_pages {
        let limited: Vec<_> = urls.iter().take(max_pages).cloned().collect();
        if limited.len() < urls.len() {
            tracing::info!(
                "Limiting to {} pages (max_pages={}), skipping {} URLs",
                limited.len(),
                max_pages,
                urls.len() - limited.len()
            );
        }
        limited
    } else {
        urls.to_vec()
    }
}

/// Scrape a single URL, reporting progress and enforcing robots.txt.
///
/// Returns `Ok(Some(content))` on success, `Ok(None)` when the URL is blocked
/// by robots.txt (skipped, not a failure), and `Err(e)` on a scrape failure.
async fn scrape_one_url(
    url: &Url,
    ctx: &ScrapeContext<'_>,
    opts: &CrawlOptions,
    observer: &dyn ProgressObserver,
    page_correlation: &CorrelationId,
) -> Result<Option<ScrapedContent>, crate::error::ScraperError> {
    let url_str = url.as_str();
    let _url_host = url.host_str().unwrap_or("unknown").to_string();

    observer.on_page_started(url_str).await;

    // Robots.txt enforcement — skip disallowed URLs unless --ignore-robots
    if !opts.crawl.ignore_robots {
        let domain = url.host_str().unwrap_or("unknown");
        if !ctx.robots_fetcher.is_allowed(url_str, domain).await {
            info!("Blocked by robots.txt: {}", url_str);
            observer.on_robots_blocked(url_str).await;
            return Ok(None);
        }
    }

    observer
        .on_status_changed(url_str, ScrapeStatus::Fetching)
        .await;

    match scrape_single_url_for_tui(
        ctx.router,
        url,
        ctx.scraper_config,
        ctx.downloader,
        ctx.engine,
        None,
        page_correlation,
    )
    .await
    {
        Ok(content) => {
            observer
                .on_status_changed(url_str, ScrapeStatus::Extracting)
                .await;
            let chars = content.content.chars().count();
            observer.on_page_completed(url_str, chars).await;
            Ok(Some(content))
        },
        Err(e) => {
            let url_str = url.as_str().to_string();
            log_scrape_error(
                &e,
                &url_str,
                "scrape",
                Some(page_correlation),
                "page scrape failed",
            );
            // ScraperError doesn't impl Clone, so we format for the observer
            // and keep the original for the failures vec (needed for error chain display).
            let scrape_err = ScrapeError::Other(format!("{e}"));
            observer.on_page_failed(&url_str, &scrape_err).await;
            Err(e)
        },
    }
}

fn build_http_client_config(
    opts: &CrawlOptions,
) -> Result<HttpClientConfig, crate::domain::UnknownProfileError> {
    Ok(HttpClientConfig {
        max_retries: opts.network.max_retries,
        backoff_base_ms: opts.network.backoff_base_ms,
        backoff_max_ms: opts.network.backoff_max_ms,
        accept_language: opts.network.accept_language.clone(),
        user_agent: opts.network.user_agent.clone(),
        timeout_secs: opts.network.timeout_secs,
        tls_emulation: HttpClientConfig::profile_from_name(&opts.network.h2_profile)?,
        ignore_waf: opts.crawl.ignore_waf,
        custom_headers: opts.network.custom_headers.clone(),
        ..HttpClientConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::{apply_resume_mode, build_http_client_config, scrape_urls, RobotsFetcher};
    use crate::application::crawl_options::CrawlOptions;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    // ===== shutdown tests (#653) =====

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn a_cancelled_run_scrapes_nothing() {
        // Regression for #653: a shutdown signal must stop new page fetches.
        // The URLs point at a closed port — if any were actually fetched they
        // would land in `failures` instead of being silently skipped.
        let urls: Vec<Url> = (0..4)
            .map(|i| {
                Url::parse(&format!("http://127.0.0.1:1/{i}")).expect("loopback URL must parse")
            })
            .collect();
        let opts = CrawlOptions {
            crawl: crate::application::crawl_options::CrawlLimits {
                ignore_robots: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        cancel.cancel();

        let (results, failures, blocked) = scrape_urls(
            &urls,
            &crate::ScraperConfig::default(),
            &opts,
            &crate::application::progress_observer::NoopObserver,
            None,
            None,
            &crate::domain::CorrelationId::new(),
            &cancel,
        )
        .await
        .expect("setup must succeed even when cancelled");

        assert!(results.is_empty(), "no page may be scraped after shutdown");
        assert!(
            failures.is_empty(),
            "skipped URLs are not failures, got: {failures:?}"
        );
        assert_eq!(blocked, 0, "shutdown skips are not robots-blocks");
    }

    // ===== robots-blocked counting tests (#705) =====

    /// A URL disallowed by robots.txt is neither a result nor a failure: it
    /// lands in the dedicated blocked counter so the orchestrator can route
    /// all-blocked runs to exit 77 (#705).
    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn robots_blocked_urls_are_counted_not_failed() {
        let server = wiremock::MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /\n"),
            )
            .mount(&server)
            .await;

        let urls = vec![Url::parse(&format!("{}/page", server.uri())).expect("valid URL")];
        let opts = CrawlOptions::default(); // ignore_robots: false
        let cancel = CancellationToken::new();

        let (results, failures, blocked) = scrape_urls(
            &urls,
            &crate::ScraperConfig::default(),
            &opts,
            &crate::application::progress_observer::NoopObserver,
            None,
            None,
            &crate::domain::CorrelationId::new(),
            &cancel,
        )
        .await
        .expect("setup must succeed");

        assert!(results.is_empty(), "blocked URL must not be scraped");
        assert!(
            failures.is_empty(),
            "robots-blocked URLs are not failures, got: {failures:?}"
        );
        assert_eq!(blocked, 1, "blocked URL must be counted");
    }

    // ===== build_http_client_config tests =====

    #[test]
    fn build_http_client_config_uses_opts_timeout_secs() {
        let mut opts = CrawlOptions::default();
        opts.network.timeout_secs = 7;

        let config = build_http_client_config(&opts).unwrap();

        assert_eq!(config.timeout_secs, 7);
        assert_eq!(config.max_retries, opts.network.max_retries);
        assert_eq!(config.backoff_base_ms, opts.network.backoff_base_ms);
        assert_eq!(config.backoff_max_ms, opts.network.backoff_max_ms);
        assert_eq!(config.accept_language, opts.network.accept_language);
    }

    #[test]
    fn build_http_client_config_preserves_default_timeout_when_unset() {
        let opts = CrawlOptions::default();

        let config = build_http_client_config(&opts).unwrap();

        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn build_http_client_config_propagates_ignore_waf() {
        // REQ-WAF-07: the bypass flag flows CrawlOptions -> HttpClientConfig so
        // the HTTP client builds InspectionContext with ignore_waf set.
        let mut opts = CrawlOptions::default();
        opts.crawl.ignore_waf = true;

        let config = build_http_client_config(&opts).unwrap();

        assert!(config.ignore_waf);
    }

    #[test]
    fn build_http_client_config_maps_h2_profile_to_tls_emulation() {
        let mut opts = CrawlOptions::default();
        opts.network.h2_profile = "Chrome131".to_owned();

        let config = build_http_client_config(&opts).unwrap();

        assert_eq!(config.tls_emulation, wreq_util::Profile::Chrome131);
    }

    #[test]
    fn build_http_client_config_rejects_unknown_profile() {
        let mut opts = CrawlOptions::default();
        opts.network.h2_profile = "Firefox".to_owned();

        let err = build_http_client_config(&opts).unwrap_err();

        assert_eq!(err.name, "Firefox");
    }

    // ===== robots tests =====

    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn robots_cache_allows_public_urls() {
        let fetcher = RobotsFetcher::new(wreq_util::Profile::Chrome145, 30).unwrap();
        // No robots.txt for localhost → fail-open → allowed
        assert!(
            fetcher
                .is_allowed("http://localhost:18080/page", "localhost")
                .await
        );
    }

    #[test]
    fn ignore_robots_flag_defaults_to_false() {
        let opts = CrawlOptions::default();
        assert!(!opts.crawl.ignore_robots);
    }

    // ===== apply_resume_mode tests =====

    #[tokio::test]
    async fn apply_resume_mode_disabled_returns_all_urls() {
        let root = crate::domain::CorrelationId::new();
        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
        ];
        let opts = CrawlOptions {
            crawl: crate::application::crawl_options::CrawlLimits {
                resume: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let (filtered, state_store) =
            apply_resume_mode(urls.clone(), &opts, "https://example.com", &root)
                .await
                .expect("resume disabled should not fail");

        assert_eq!(filtered.len(), 2);
        assert!(state_store.is_none());
    }

    #[tokio::test]
    async fn apply_resume_mode_skips_previously_scraped_urls() {
        let root = crate::domain::CorrelationId::new();
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();

        // Pre-populate state with one processed URL
        let state_file = state_dir.join("example.com.json");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            &state_file,
            r#"{"domain":"example.com","processed_urls":["https://example.com/a"],"last_export":null,"total_exported":1}"#,
        ).unwrap();

        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
            Url::parse("https://example.com/c").unwrap(),
        ];
        let opts = CrawlOptions {
            crawl: crate::application::crawl_options::CrawlLimits {
                resume: true,
                state_dir: Some(state_dir),
                ..Default::default()
            },
            ..Default::default()
        };

        let (filtered, state_store) = apply_resume_mode(urls, &opts, "https://example.com", &root)
            .await
            .expect("valid state dir should not fail");

        // URL "a" was already processed, should be skipped
        assert_eq!(filtered.len(), 2, "should skip 1 already-processed URL");
        assert!(
            !filtered
                .iter()
                .any(|u| u.as_str() == "https://example.com/a"),
            "processed URL should be filtered out"
        );
        assert!(
            state_store.is_some(),
            "should create state store when resume enabled"
        );
    }

    #[tokio::test]
    async fn apply_resume_mode_with_corrupted_state_returns_all_urls() {
        let root = crate::domain::CorrelationId::new();
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();

        // Write corrupted state file
        let state_file = state_dir.join("example.com.json");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(&state_file, "not valid json!!!").unwrap();

        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
        ];
        let opts = CrawlOptions {
            crawl: crate::application::crawl_options::CrawlLimits {
                resume: true,
                state_dir: Some(state_dir),
                ..Default::default()
            },
            ..Default::default()
        };

        let (filtered, state_store) =
            apply_resume_mode(urls.clone(), &opts, "https://example.com", &root)
                .await
                .expect("corrupted state file should not prevent store creation");

        // Corrupted state → fallback to all URLs (graceful degradation)
        assert_eq!(
            filtered.len(),
            2,
            "should return all URLs on corrupted state"
        );
        assert!(state_store.is_some());
    }

    #[tokio::test]
    async fn apply_resume_mode_with_custom_state_dir() {
        let root = crate::domain::CorrelationId::new();
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().join("custom_state");
        std::fs::create_dir_all(&state_dir).unwrap();

        let urls = vec![Url::parse("https://example.com/a").unwrap()];
        let opts = CrawlOptions {
            crawl: crate::application::crawl_options::CrawlLimits {
                resume: true,
                state_dir: Some(state_dir.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        let (filtered, state_store) = apply_resume_mode(urls, &opts, "https://example.com", &root)
            .await
            .expect("custom state dir should not fail");

        assert_eq!(filtered.len(), 1);
        assert!(
            state_store.is_some(),
            "should create state store with custom dir"
        );
        // Verify state store uses custom dir
        let store = state_store.unwrap();
        let state_path = store.get_state_path();
        assert!(
            state_path.starts_with(&state_dir),
            "state path should be under custom state_dir: {state_path:?}"
        );
    }
}
