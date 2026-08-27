//! Scraping flow logic extracted from orchestrator.

use std::path::PathBuf;

use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::build_fetch_router;
use crate::application::export_factory;
use crate::application::progress_observer::ProgressObserver;
use crate::application::resume::{filter_committed, record_store_bridge};
use crate::application::scrape_single_url_for_tui;
use crate::cli::error::CliExit;
use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::persistence::PersistenceMode;
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

/// Resolve the default state directory (XDG_CACHE_HOME or `~/.cache/webfang/state`).
///
/// Pure helper extracted for `PersistenceMode::from_limits` callers.
#[must_use]
pub fn resolve_default_state_dir() -> PathBuf {
    let cache_base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache")
        });
    cache_base.join("webfang").join("state")
}

/// Resolve the resume state directory, defaulting to the XDG cache path.
#[allow(dead_code)]
fn resolve_state_dir(opts: &CrawlOptions) -> PathBuf {
    opts.crawl
        .state_dir
        .clone()
        .unwrap_or_else(resolve_default_state_dir)
}

/// Emit `warn!` when `--state-dir` is set without `--resume` (soft-degrade, not error).
///
/// Called from the orchestrator before constructing `PersistenceMode`; the domain
/// resolver also emits the same `warn!` so unit tests capture it via `tracing_test`.
pub(crate) fn warn_if_state_dir_without_resume(opts: &CrawlOptions) {
    if opts.crawl.state_dir.is_some() && !opts.crawl.resume {
        warn!(
            state_dir = ?opts.crawl.state_dir,
            "ignoring --state-dir without --resume"
        );
    }
}

/// Apply resume mode filtering via `PersistenceMode`.
///
/// `mode` is the unified control-plane — exhaustive `match` on
/// `Disabled|Resume|Checkpoint|Full`. Only `Resume` and `Full` create a
/// `StateStore` and filter committed URLs; `Checkpoint` and `Disabled` pass
/// all URLs through.
///
/// # Errors
///
/// Returns `CliExit::IoError` when resume is active and the state store
/// cannot be created.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    mode: &PersistenceMode,
    target_url: &str,
    _root_correlation: &CorrelationId,
) -> Result<(Vec<Url>, Option<StateStore>), CliExit> {
    let state_store: Option<StateStore> = match mode {
        PersistenceMode::Disabled | PersistenceMode::Checkpoint { .. } => None,
        PersistenceMode::Resume { dir }
        | PersistenceMode::Full {
            resume_dir: dir, ..
        } => {
            info!("Resume mode enabled - tracking processed URLs");
            let domain = export_factory::domain_from_url(target_url);
            info!("State store domain: {}", domain);
            match export_factory::create_state_store(dir.clone(), &domain) {
                Ok(store) => Some(store),
                // LCOV_EXCL_START defensive: state-store-creation
                Err(e) => {
                    tracing::error!(error = %e, "state store creation failed with --resume active");
                    return Err(CliExit::IoError(format!(
                        "No se pudo crear el almacén de estado para --resume: {e}"
                    )));
                },
                // LCOV_EXCL_STOP
            }
        },
    };

    let filtered = match (&state_store, mode.is_resume()) {
        (Some(store), true) => {
            let record_store = record_store_bridge(store);
            filter_committed(urls_to_scrape, &record_store).0
        },
        _ => urls_to_scrape,
    };

    crate::cli::crash_points::hit(crate::cli::crash_points::PRE_FIRST_PERSIST);

    Ok((filtered, state_store))
}

/// Seed operator `--cookie` values (#890) into a shared wreq jar so the
/// Static and Hybrid-L1 layers carry them from the first fetch. Returns
/// [`None`] when no cookies were requested or no target URL exists. The
/// Chromiumoxide L3 layer keeps consuming the CookieBridge seeded in the
/// caller.
///
/// # Observability
///
/// Emits a `debug!` event with cookie count and host scope only — cookie
/// names and values are credentials and never reach traces.
fn seed_operator_cookie_jar(
    opts: &CrawlOptions,
    urls: &[Url],
) -> Option<std::sync::Arc<wreq::cookie::Jar>> {
    if opts.network.initial_cookies.is_empty() {
        return None;
    }
    urls.first().map(|first_url| {
        let jar = wreq::cookie::Jar::default();
        for (name, value) in &opts.network.initial_cookies {
            // RFC 6265 host-scoped cookie: no Domain attribute, so it
            // matches only the target host.
            jar.add(format!("{name}={value}").as_str(), first_url.as_str());
        }
        debug!(
        seeded_cookie_count = opts.network.initial_cookies.len(),
        host = %first_url.host_str().unwrap_or(""),
        "seeding operator cookies into the scrape client jar"
        );
        std::sync::Arc::new(jar)
    })
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
    let initial_cookie_jar = seed_operator_cookie_jar(opts, urls);
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
        // #890: operator headers, Accept-Language, and seeded cookies reach
        // the wreq layer instead of being silently dropped. Accept-Language
        // mirrors the retry-client semantics: the configured value applies
        // unconditionally (NetworkOptions carries the profile-matching
        // default when the operator did not override it).
        opts.network.custom_headers.clone(),
        Some(opts.network.accept_language.clone()),
        initial_cookie_jar,
        // #653: the run's shutdown token, so Full-strategy governor waits abort
        // on SIGINT/SIGTERM instead of hanging until their own timeout.
        cancel.clone(),
        http_config.max_retries,
        http_config.backoff_base_ms,
        http_config.backoff_max_ms,
        // #787: propagate --obscura-binary into the Hybrid Layer 2 downloader
        // instead of always resolving the bare `obscura` name from PATH.
        &opts.network.obscura_binary,
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
        fingerprint_repo: build_fingerprint_repo(opts).await,
    };

    // Concurrency bound (#653): the previous sequential loop made concurrency a
    // no-op on the default scrape path. `buffer_unordered` keeps at most
    // `concurrency` fetches in flight; the enumerated index restores the
    // original URL order afterwards so output stays deterministic. The bound
    // derives from the budget model's Operation.crawl tier (task 2.5a) — the
    // NonZero tier type guarantees ≥ 1, so no `.max(1)` guard is needed.
    let concurrency = scrape_concurrency(opts, &crate::domain::budget::detector::SystemDetector);
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
    /// Extraction failure fingerprint sink (#792). `None` when
    /// `--extraction-fingerprint` is off — recording is opt-in.
    fingerprint_repo:
        Option<std::sync::Arc<dyn crate::domain::fingerprint_repository::FingerprintRepository>>,
}

/// Apply the `max_pages` cap to the URL list when configured.
/// Scrape-path `buffer_unordered` bound.
///
/// Derives from the budget model's Operation.crawl tier built from the run's
/// operator overrides plus the given hardware detector (task 2.5a); the
/// enforcement mechanism (`buffer_unordered`) is unchanged.
fn scrape_concurrency(
    opts: &CrawlOptions,
    detector: &dyn crate::domain::budget::detector::HardwareDetector,
) -> usize {
    crate::domain::budget::BudgetModel::build(opts.budget_overrides, detector)
        .crawl()
        .get()
}

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
        Ok(mut content) => {
            observer
                .on_status_changed(url_str, ScrapeStatus::Extracting)
                .await;
            // Extraction failure fingerprinting (#792 Slice B): a low-quality
            // extraction that produced an honest hint is recorded against its
            // site/selector pair, and the accumulated failure count is attached
            // back to the hint. Recording failures never fail the scrape —
            // persistence is best-effort observability, not a data path.
            record_extraction_fingerprint(
                url,
                ctx.fingerprint_repo.as_deref(),
                &ctx.scraper_config.selector,
                &mut content,
            )
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

/// Build the fingerprint repository for this run (#792 Slice B).
///
/// Returns `None` unless `--extraction-fingerprint` is set — recording is
/// opt-in. With the `persistence` feature the sink is the shared SQLite DB
/// (`~/.webfang/crawl.db`, overridable via `--db-path`/`WEBFANG_DB_PATH`);
/// without it the flag degrades to a no-op sink with a one-time warning.
/// A pool/schema failure also degrades to no-op: fingerprinting must never
/// abort a scrape run.
async fn build_fingerprint_repo(
    opts: &CrawlOptions,
) -> Option<std::sync::Arc<dyn crate::domain::fingerprint_repository::FingerprintRepository>> {
    if !opts.extraction_fingerprint {
        return None;
    }

    #[cfg(feature = "persistence")]
    {
        use crate::infrastructure::autotuning::{env_db_path, resolve_db_path};
        use crate::infrastructure::persistence::{create_pool, SqliteFingerprintRepository};

        let db_path = resolve_db_path(opts.elastic.db_path.as_deref(), env_db_path());
        match create_pool(&db_path, 1) {
            Ok(pool) => {
                let repo = SqliteFingerprintRepository::new(pool);
                match repo.setup_schema().await {
                    Ok(()) => {
                        tracing::info!(
                        db_path = %db_path.display(),
                        "extraction_fingerprint_sink_wired"
                        );
                        return Some(std::sync::Arc::new(repo));
                    },
                    Err(e) => {
                        tracing::warn!(
                        error = %e,
                        "extraction fingerprint schema init failed — degrading to no-op"
                        );
                    },
                }
            },
            Err(e) => {
                tracing::warn!(
                error = %e,
                "extraction fingerprint pool creation failed — degrading to no-op"
                );
            },
        }
        Some(std::sync::Arc::new(
            crate::infrastructure::fingerprint::NoopFingerprintRepository,
        ))
    }

    #[cfg(not(feature = "persistence"))]
    {
        tracing::warn!(
            "--extraction-fingerprint requires the `persistence` feature — degrading to no-op"
        );
        Some(std::sync::Arc::new(
            crate::infrastructure::fingerprint::NoopFingerprintRepository,
        ))
    }
}

/// Record an extraction failure fingerprint when a low-quality extraction
/// produced an honest hint, and attach the accumulated failure count back to
/// the hint (#792 Slice B).
///
/// Best-effort: a persistence error is logged and swallowed — fingerprinting
/// is observability, never a data-path failure.
async fn record_extraction_fingerprint(
    url: &Url,
    repo: Option<&dyn crate::domain::fingerprint_repository::FingerprintRepository>,
    selector: &str,
    content: &mut ScrapedContent,
) {
    let Some(repo) = repo else {
        return;
    };
    let Some(hint) = content.quality_hint.as_mut() else {
        return;
    };

    let site_base_url = url.origin().ascii_serialization();
    let selector_signature = selector.to_owned();
    let record = crate::domain::extraction_quality::FingerprintRecord {
        site_base_url: site_base_url.clone(),
        selector_signature: selector_signature.clone(),
        score_at_failure: hint.score.total,
        failure_count: 1,
        last_seen: chrono::Utc::now().timestamp(),
        last_note: Some(hint.message_es.clone()),
    };

    match repo.record_failure(&record).await {
        Ok(count) => {
            tracing::info!(
            site = %site_base_url,
            selector = %selector_signature,
            score = hint.score.total,
            failure_count = count,
            "extraction_fingerprint_recorded"
            );
            let mut recorded = record;
            recorded.failure_count = count;
            hint.fingerprint = Some(recorded);
        },
        Err(e) => {
            tracing::warn!(
            error = %e,
            site = %site_base_url,
            "extraction fingerprint recording failed — continuing without it"
            );
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
    use std::num::NonZeroUsize;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    // ===== scrape-path concurrency derives from the budget model (task 2.5a) =====

    fn fixed_cores(n: usize) -> crate::domain::budget::detector::FixedDetector {
        crate::domain::budget::detector::FixedDetector::with_detection(
            NonZeroUsize::new(n).expect("test core counts are non-zero"),
            None,
        )
    }

    /// The scrape bound must follow the INJECTED detector's crawl tier —
    /// never the host's `available_parallelism` and never the raw CLI flag.
    #[test]
    fn scrape_concurrency_follows_injected_detector_crawl_tier() {
        let opts = CrawlOptions::default();

        // Auto table: 6 cores ⇒ 5, ≥9 cores ⇒ min(cores−1, 8) = 8.
        assert_eq!(super::scrape_concurrency(&opts, &fixed_cores(6)), 5);
        assert_eq!(super::scrape_concurrency(&opts, &fixed_cores(9)), 8);
    }

    // ===== extraction fingerprint wiring tests (#792 Slice B) =====

    mod fingerprint_wiring {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Mutex;

        use crate::application::crawl_options::CrawlOptions;
        use crate::domain::extraction_quality::{
            ExtractionQualityHint, FingerprintRecord, StructuralScore,
        };
        use crate::domain::fingerprint_repository::FingerprintRepository;
        use crate::domain::{ScrapedContent, ValidUrl};
        use crate::error::ScraperError;
        use url::Url;

        /// Mock repository capturing every recorded fingerprint.
        #[derive(Default)]
        struct CapturingRepo {
            recorded: Mutex<Vec<FingerprintRecord>>,
            next_count: Mutex<u32>,
        }

        impl FingerprintRepository for CapturingRepo {
            fn record_failure<'a>(
                &'a self,
                record: &'a FingerprintRecord,
            ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
                let record = record.clone();
                Box::pin(async move {
                    self.recorded.lock().unwrap().push(record);
                    let mut count = self.next_count.lock().unwrap();
                    *count += 1;
                    Ok(*count)
                })
            }

            fn get_failure_count<'a>(
                &'a self,
                _site: &'a str,
                _signature: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
                Box::pin(async move { Ok(0) })
            }
        }

        fn hint_with_score(total: f64) -> ExtractionQualityHint {
            ExtractionQualityHint {
                score: StructuralScore {
                    semantic_drift: 0.3,
                    context_collapse: 0.3,
                    result_size: 0.3,
                    total,
                    active_factors: 3,
                },
                message_es: format!("baja calidad ({total}/100)"),
                fingerprint: None,
            }
        }

        fn scraped_with_hint(hint: ExtractionQualityHint) -> ScrapedContent {
            let url = Url::parse("https://example.com/article").unwrap();
            ScrapedContent {
                title: "t".into(),
                content: "c".into(),
                url: ValidUrl::new(url),
                excerpt: None,
                author: None,
                date: None,
                html: None,
                assets: vec![],
                correlation_id: None,
                quality_hint: Some(hint),
            }
        }

        /// A hinted extraction is recorded and the count attaches to the hint.
        #[tokio::test]
        async fn hinted_extraction_records_fingerprint_and_attaches_count() {
            let repo = CapturingRepo::default();
            let url = Url::parse("https://example.com/article").unwrap();
            let mut content = scraped_with_hint(hint_with_score(35.0));

            super::super::record_extraction_fingerprint(
                &url,
                Some(&repo),
                "article|.body",
                &mut content,
            )
            .await;

            let recorded = repo.recorded.lock().unwrap();
            assert_eq!(recorded.len(), 1, "hinted extraction must be recorded");
            assert_eq!(recorded[0].site_base_url, "https://example.com");
            assert_eq!(recorded[0].selector_signature, "article|.body");
            assert_eq!(recorded[0].score_at_failure, 35.0);

            let hint = content.quality_hint.as_ref().expect("hint must survive");
            let fp = hint
                .fingerprint
                .as_ref()
                .expect("count must attach to hint");
            assert_eq!(fp.failure_count, 1);
        }

        /// A clean extraction (no hint) records nothing.
        #[tokio::test]
        async fn clean_extraction_records_nothing() {
            let repo = CapturingRepo::default();
            let url = Url::parse("https://example.com/article").unwrap();
            let mut content = scraped_with_hint(hint_with_score(35.0));
            content.quality_hint = None;

            super::super::record_extraction_fingerprint(
                &url,
                Some(&repo),
                "article|.body",
                &mut content,
            )
            .await;

            assert!(repo.recorded.lock().unwrap().is_empty());
        }

        /// No repository wired (flag off) → no-op, hint untouched.
        #[tokio::test]
        async fn missing_repo_is_a_noop() {
            let url = Url::parse("https://example.com/article").unwrap();
            let mut content = scraped_with_hint(hint_with_score(35.0));

            super::super::record_extraction_fingerprint(&url, None, "article|.body", &mut content)
                .await;

            assert!(
                content.quality_hint.as_ref().unwrap().fingerprint.is_none(),
                "no repo → no fingerprint attached"
            );
        }

        /// Flag off → no repository is built.
        #[tokio::test]
        async fn flag_off_builds_no_repo() {
            let opts = CrawlOptions::default();
            assert!(!opts.extraction_fingerprint);
            assert!(super::super::build_fingerprint_repo(&opts).await.is_none());
        }

        /// Flag on → a repository is always produced (SQLite or degraded no-op).
        #[tokio::test]
        async fn flag_on_builds_a_repo() {
            // Point the DB at a temp dir so the test never touches ~/.webfang.
            let tmp = tempfile::TempDir::new().unwrap();
            let opts = CrawlOptions {
                extraction_fingerprint: true,
                elastic: crate::application::crawl_options::IngestionTuning {
                    db_path: Some(tmp.path().join("fp.db")),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(super::super::build_fingerprint_repo(&opts).await.is_some());
        }
    }

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

    // ===== apply_resume_mode tests (via PersistenceMode) =====

    #[tokio::test]
    async fn apply_resume_mode_disabled_returns_all_urls() {
        let root = crate::domain::CorrelationId::new();
        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
        ];
        let mode = crate::domain::persistence::PersistenceMode::Disabled;

        let (filtered, state_store) =
            apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
                .await
                .expect("resume disabled should not fail");

        assert_eq!(filtered.len(), 2);
        assert!(state_store.is_none());
    }

    #[tokio::test]
    async fn apply_resume_mode_checkpoint_returns_all_urls() {
        let root = crate::domain::CorrelationId::new();
        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
        ];
        let mode = crate::domain::persistence::PersistenceMode::Checkpoint {
            cfg: crate::domain::persistence::CheckpointCfg {
                dir: std::path::PathBuf::from("/tmp/chk"),
                interval: 100,
            },
        };

        let (filtered, state_store) =
            apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
                .await
                .expect("checkpoint only should not fail");

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
        let mode = crate::domain::persistence::PersistenceMode::Resume {
            dir: state_dir.clone(),
        };

        let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
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
    async fn apply_resume_mode_full_skips_previously_scraped_urls() {
        let root = crate::domain::CorrelationId::new();
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().to_path_buf();

        let state_file = state_dir.join("example.com.json");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            &state_file,
            r#"{"domain":"example.com","processed_urls":["https://example.com/a"],"last_export":null,"total_exported":1}"#,
        ).unwrap();

        let urls = vec![
            Url::parse("https://example.com/a").unwrap(),
            Url::parse("https://example.com/b").unwrap(),
        ];
        let mode = crate::domain::persistence::PersistenceMode::Full {
            resume_dir: state_dir.clone(),
            checkpoint: crate::domain::persistence::CheckpointCfg {
                dir: state_dir.clone(),
                interval: 50,
            },
        };

        let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
            .await
            .expect("Full mode should not fail");

        assert_eq!(filtered.len(), 1);
        assert!(state_store.is_some());
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
        let mode = crate::domain::persistence::PersistenceMode::Resume {
            dir: state_dir.clone(),
        };

        let (filtered, state_store) =
            apply_resume_mode(urls.clone(), &mode, "https://example.com", &root)
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
        let mode = crate::domain::persistence::PersistenceMode::Resume {
            dir: state_dir.clone(),
        };

        let (filtered, state_store) = apply_resume_mode(urls, &mode, "https://example.com", &root)
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

    #[test]
    fn resolve_default_state_dir_contains_webfang_state() {
        let dir = super::resolve_default_state_dir();
        assert!(
            dir.to_string_lossy().contains("webfang/state"),
            "default state dir should contain webfang/state, got: {dir:?}"
        );
    }
}
