//! Scraping flow logic extracted from orchestrator.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::application::container;
use crate::application::container::Container;
use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::content_sink::CapturedPage;
use crate::application::export_factory;
use crate::application::progress_observer::ProgressObserver;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::application::resume::filter_committed;
use crate::application::scrape_single_url;
use crate::cli::error::CliExit;
use crate::domain::config::ScraperConfig;
use crate::domain::crawler_port::{RobotsDecision, RobotsPort};
use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::persistence::PersistenceMode;
use crate::domain::persistence::RecordStoreError;
use crate::domain::persistence::StateStorePort;
use crate::domain::{CorrelationId, ScrapedContent};
use crate::infrastructure::downloader::Downloader;
use crate::infrastructure::export::RecordStore;
use crate::infrastructure::observability::log_scrape_error;
use crate::HttpClientConfig;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

/// Resolve the default state directory (XDG_CACHE_HOME or `~/.cache/webfang/state`).
///
/// Pure helper extracted for `PersistenceMode::from_config` callers.
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

/// Apply resume mode filtering via `PersistenceMode`.
///
/// `mode` is the unified control-plane — exhaustive `match` on
/// `Disabled|Resume|Checkpoint|Full`. Only `Resume` and `Full` create a
/// state store and filter committed URLs; `Checkpoint` and `Disabled` pass
/// all URLs through.
///
/// # Errors
///
/// Currently never fails: state-store creation is lazy and infallible, so
/// the `Ok` arm always carries the outcome. The `Result` is kept so callers
/// (and a future eager factory) keep the `CliExit::IoError` route.
pub async fn apply_resume_mode(
    urls_to_scrape: Vec<Url>,
    mode: &PersistenceMode,
    target_url: &str,
    _root_correlation: &CorrelationId,
) -> Result<(Vec<Url>, Option<Arc<dyn StateStorePort>>), CliExit> {
    let state_store: Option<Arc<dyn StateStorePort>> = match mode {
        PersistenceMode::Disabled | PersistenceMode::Checkpoint { .. } => None,
        PersistenceMode::Resume { dir }
        | PersistenceMode::Full {
            resume_dir: dir, ..
        } => {
            info!("Resume mode enabled - tracking processed URLs");
            let domain = export_factory::domain_from_url(target_url);
            info!("State store domain: {}", domain);
            // Lazy and infallible: `build_state_store` performs no I/O — the
            // state directory is created later, on `save`. The former
            // `CliExit::IoError` creation-failure branch was unreachable dead
            // code (pinned by
            // `test_build_state_store_returns_ok_even_when_state_dir_is_a_file`).
            Some(container::build_state_store(dir.clone(), &domain))
        },
    };

    let filtered = match (state_store.as_ref(), mode.is_resume()) {
        (Some(store), true) => {
            let record_store = record_store_bridge(store.as_ref());
            warn_unreadable_state(&record_store);
            filter_committed(urls_to_scrape, &record_store).0
        },
        _ => urls_to_scrape,
    };

    crate::cli::crash_points::hit(crate::cli::crash_points::PRE_FIRST_PERSIST);

    Ok((filtered, state_store))
}

/// P8-3 — tell the user, in Spanish, when the resume state file cannot be
/// read, BEFORE the resume gate treats it as empty.
///
/// The fresh-start policy itself is deliberate and pinned by
/// `resume_test::corrupt_state_falls_back_to_full_scrape`; what was missing is
/// that the only signal was an English `tracing::WARN`, so a user watched the
/// tool silently discard their persisted state and re-do all the work. AGENTS.md
/// reserves English for internal logs: user-facing text is Spanish.
///
/// Never fails and never changes the run's outcome; the original bytes stay on
/// disk (`load_or_init` preserves them) and the message names the path so the
/// user can inspect them.
fn warn_unreadable_state(store: &RecordStore) {
    let problem = match store.load() {
        Ok(_) => return,
        Err(RecordStoreError::Corrupt { path }) => format!(
            "el archivo de estado {} está corrupto o no es JSON válido",
            path.display(),
        ),
        Err(RecordStoreError::UnsupportedVersion { path, found }) => format!(
            "el archivo de estado {} pertenece a una versión no soportada ({found})",
            path.display(),
        ),
        Err(err @ (RecordStoreError::Io { .. } | RecordStoreError::Backup { .. })) => {
            // Internal detail stays in the log; the user gets the plain fact.
            warn!(error = %err, "resume state file unreadable");
            format!(
                "no se pudo leer el archivo de estado {}",
                store.state_path().display(),
            )
        },
        // Writer-side rejection: not reachable from a read, matched so a new
        // error variant can never fall through into silence (fail-closed).
        Err(err @ RecordStoreError::InvalidRecord { .. }) => {
            warn!(error = %err, "resume state file rejected");
            format!(
                "el archivo de estado {} fue rechazado por inválido",
                store.state_path().display(),
            )
        },
    };
    eprintln!("Advertencia: {problem}. Se reanuda desde cero; el archivo original se conserva para inspección.");
}

/// Bridge a state-store port handle onto the v2 `RecordStore` seam:
/// same directory + domain, so a legacy v1 state file migrates in place
/// on first load (Gate 2 policy lives inside `RecordStore`).
///
/// Lives in `cli` (ADR-0010): the composition edge constructs infrastructure
/// concretes; `application` consumes them through `RecordStorePort` only.
/// Shared with `export_flow` so both resume paths derive the same bridge.
pub(crate) fn record_store_bridge(state_store: &dyn StateStorePort) -> RecordStore {
    let path = state_store.get_state_path();
    let dir = path.parent().map_or_else(
        || std::path::PathBuf::from("."),
        std::path::Path::to_path_buf,
    );
    let domain = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("unknown")
        .to_string();
    RecordStore::new(domain).with_state_dir(dir)
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
/// `captured` carries discovery-captured bodies (F-05, #1229): URLs with a
/// cached body skip the HTTP fetch and extract from the capture instead —
/// one request per page. Misses fall back to a normal fetch. Pass `&[]`
/// when discovery ran without a sink (dry-run, sitemap, single-page).
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
    captured: &[CapturedPage],
) -> Result<
    (
        Vec<ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
    ),
    crate::error::ScraperError,
> {
    // Single wiring graph (#1149): the fetch downloader, cookie set, and
    // robots fetcher are built through the `Container` ephemeral factories —
    // the same production factory the crawl Engine uses. Wire values are
    // unchanged: UA #503, headers/lang/cookies/jar #890, obscura binary #787,
    // shutdown token #653, robots-per-batch TLS fingerprint #337.
    let http_config = build_http_client_config(opts)?;
    let (cookie_bridge, initial_cookie_jar) = Container::fresh_scrape_cookie_set(opts, urls);
    let spec = Container::scrape_downloader_spec(opts, &http_config, initial_cookie_jar);
    let router = Container::build_scrape_downloader(&spec, cookie_bridge, cancel.clone())?;

    let _total_urls = urls.len();

    // Robots.txt fetcher — shares the batch's TLS fingerprint so the robots.txt
    // request is indistinguishable from a page fetch (#337). Shared across all
    // URLs in this batch. Erased to the domain seam; the concrete is the same
    // `RobotsFetcher` as before, now named only inside the Container.
    let robots_fetcher =
        container::build_robots_fetcher(http_config.tls_emulation, http_config.timeout_secs)?;

    // F-05 (#1229 slice 2): index discovery-captured bodies by URL so
    // cache hits skip the HTTP fetch — one request per page. First
    // occurrence wins; the engine dedups, so dupes only arise from
    // overlapping runs sharing a sink.
    let mut captured_bodies: HashMap<String, String> = HashMap::with_capacity(captured.len());
    for page in captured {
        captured_bodies
            .entry(page.url.clone())
            .or_insert_with(|| page.html.clone());
    }

    // Apply max_pages limit if configured
    let urls_to_process = apply_max_pages_limit(urls, scraper_config);

    let processing_count = urls_to_process.len();
    let mut results = Vec::with_capacity(processing_count);
    let mut failures: Vec<(String, crate::error::ScraperError)> = Vec::new();

    let ctx = ScrapeContext {
        router: router.as_ref(),
        scraper_config,
        downloader,
        engine,
        robots_fetcher: robots_fetcher.as_ref(),
        fingerprint_repo: build_fingerprint_repo(opts).await,
        captured_bodies: &captured_bodies,
        rate_limiter: build_scrape_rate_limiter(opts),
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
                    // #P4-4: take a token BEFORE any socket opens for this
                    // URL. Governor consumes the permit at grant time, so
                    // waiting after the fetch would space nothing. A wait
                    // abandoned by shutdown is a skip, not a failure (#509).
                    if let Some(limiter) = ctx.rate_limiter.as_ref() {
                        if limiter.until_ready_or_cancel(cancel).await.is_err() {
                            return (index, None);
                        }
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
    robots_fetcher: &'a dyn RobotsPort,
    /// Discovery-captured bodies by URL (F-05, #1229): hits skip the
    /// fetch and extract from the capture instead.
    captured_bodies: &'a HashMap<String, String>,
    /// Extraction failure fingerprint sink (#792). `None` when
    /// `--extraction-fingerprint` is off — recording is opt-in.
    fingerprint_repo:
        Option<std::sync::Arc<dyn crate::domain::fingerprint_repository::FingerprintRepository>>,
    /// Token bucket gating every scrape-phase fetch (#P4-4).
    ///
    /// `None` means "no `--delay-ms` was asked for": no bucket is built and
    /// the per-URL path performs no await at all, so the unthrottled run
    /// keeps its exact pre-fix cost and cadence.
    rate_limiter: Option<SharedRateLimiter>,
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
    // Single budget point (#1149): the CLI scrape bound reads the same
    // Operation.crawl tier the Engine tiers derive from.
    Container::scrape_concurrency(opts, detector)
}

/// Build the token bucket that gates the scrape phase (#P4-4).
///
/// `--delay-ms` reached the crawl Engine's discovery limiter but never the
/// scrape path, so a direct scrape ignored it entirely and a crawl's
/// scrape-phase re-fetches ran free. The bucket is built from the SAME two
/// inputs `Engine::run` uses — `delay_ms` as the refill period and the
/// budget model's independent burst tier (`rate_limiter_config`,
/// engine.rs:152) — so discovery and scrape share one cadence policy.
///
/// `delay_ms == 0` returns `None`: no bucket is allocated and the per-URL
/// path performs no await, which keeps an unthrottled run identical to the
/// pre-fix behavior (zero overhead, zero drift in the existing suites).
///
/// A construction failure degrades to `None` with a WARN, mirroring the
/// `Container` precedent (`container.rs:608`). It is unreachable in
/// practice: `SharedRateLimiter::new` rejects only a zero period (clamped
/// to 1 ms) or zero burst (`BurstPermits` is `NonZeroU32`).
fn build_scrape_rate_limiter(opts: &CrawlOptions) -> Option<SharedRateLimiter> {
    if opts.network.delay_ms == 0 {
        return None;
    }
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let burst = budget.burst().get();
    match SharedRateLimiter::new(&RateLimiterConfig::new(opts.network.delay_ms, burst)) {
        Ok(limiter) => {
            info!(
                delay_ms = opts.network.delay_ms,
                burst, "scrape rate limiter wired"
            );
            Some(limiter)
        },
        Err(e) => {
            warn!(
            error = %e,
            delay_ms = opts.network.delay_ms,
            "scrape rate limiter unavailable — continuing unthrottled"
            );
            None
        },
    }
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
    // SSRF entry guard (F-06 + F-32, #1217): reject literal-IP seeds with a
    // typed Spanish error BEFORE robots.txt or page fetches open any
    // socket. Same shared choke-point check the downloader and MCP use.
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(url) {
        return Err(crate::error::ScraperError::invalid_url(
            rejection.to_string(),
        ));
    }
    let url_str = url.as_str();
    let _url_host = url.host_str().unwrap_or("unknown").to_string();

    observer.on_page_started(url_str).await;

    // Robots.txt enforcement — skip disallowed URLs unless --ignore-robots.
    // #1329: the typed verdict names the cause; the entry guard above already
    // rejected literal-IP seeds with the CLI's `invalid_url` error, so a
    // `PolicyRefused` here is defensive — surface the guard's real cause
    // instead of a phantom robots skip.
    if !opts.crawl.ignore_robots {
        let domain = url.host_str().unwrap_or("unknown");
        match ctx.robots_fetcher.is_allowed(url_str, domain).await {
            RobotsDecision::Allowed => {},
            RobotsDecision::RulesDenied => {
                info!("Blocked by robots.txt: {}", url_str);
                observer.on_robots_blocked(url_str).await;
                return Ok(None);
            },
            RobotsDecision::PolicyRefused(forbidden) => {
                return Err(crate::error::ScraperError::Network(Box::new(forbidden)));
            },
        }
    }

    observer
        .on_status_changed(url_str, ScrapeStatus::Fetching)
        .await;

    // F-05 (#1229 slice 2): reuse the discovery-captured body when
    // present — one HTTP request per page. SSRF and robots above still
    // apply; a miss (cap tripped, uncaptured URL) falls back to a normal
    // fetch through the same post-processing below. No `PageSource` seam:
    // the branch is a plain cache lookup at the call site.
    let outcome = if let Some(html) = ctx.captured_bodies.get(url.as_str()) {
        crate::application::crawler::extract_content(
            html,
            url,
            ctx.scraper_config,
            ctx.downloader,
            ctx.engine,
            page_correlation,
        )
        .await
    } else {
        scrape_single_url(
            ctx.router,
            url,
            ctx.scraper_config,
            ctx.downloader,
            ctx.engine,
            None,
            page_correlation,
        )
        .await
    };
    match outcome {
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
    use super::{
        apply_resume_mode, build_http_client_config, build_scrape_rate_limiter, scrape_urls,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
    use std::num::NonZeroUsize;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, Request, Respond, ResponseTemplate};

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
            &crate::domain::config::ScraperConfig::default(),
            &opts,
            &crate::application::progress_observer::NoopObserver,
            None,
            None,
            &crate::domain::CorrelationId::new(),
            &cancel,
            &[],
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
        // Entry-guard allowance (F-06 + F-32, #1217): the seed below is a
        // wiremock loopback literal, which production now rejects at entry.
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        )]);
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
            &crate::domain::config::ScraperConfig::default(),
            &opts,
            &crate::application::progress_observer::NoopObserver,
            None,
            None,
            &crate::domain::CorrelationId::new(),
            &cancel,
            &[],
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

    // ===== scrape-phase rate limiting (#P4-4) =====

    /// Server-side arrival recorder.
    ///
    /// `wiremock::Request` (0.6.5) carries no timestamp, so `received_requests()`
    /// cannot answer "when did this arrive?". `Respond::respond` runs inside the
    /// mock server's request handler BEFORE the template's `set_delay` is awaited
    /// (`mock_server/hyper.rs:34-51`), so the instant recorded here IS the
    /// server-side arrival time the spacing assertion needs.
    #[derive(Clone)]
    struct Arrivals {
        template: ResponseTemplate,
        seen: Arc<Mutex<Vec<Instant>>>,
    }

    impl Respond for Arrivals {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Instant::now());
            self.template.clone()
        }
    }

    /// Body rich enough for the extractor to return content rather than an
    /// `ExtractionFailed` error, so the assertions measure cadence only.
    const RATE_LIMIT_PAGE_HTML: &str = "<html><head><title>Rate limit probe</title></head>\
        <body><main><article><h1>Rate limit probe</h1>\
        <p>Substantive article text, long enough for the content extractor to\
        consider this a real page rather than an empty shell document.</p>\
        <p>A second paragraph of substantive prose keeps the quality score above\
        the extraction floor used by the pipeline.</p>\
        </article></main></body></html>";

    /// #P4-4: `--delay-ms` must gate the SCRAPE path, not only discovery.
    ///
    /// Before the fix the flag reached the crawl Engine's token bucket and
    /// nothing else: every scrape-phase fetch ran back-to-back. The two seeds
    /// below therefore arrive ~50 ms apart (mock latency only) when the wiring
    /// regresses and ~400 ms apart when it holds.
    #[cfg_attr(miri, ignore)] // btls/wreq FFI (BoringSSL TLS_method) not supported by Miri
    #[tokio::test]
    async fn scrape_phase_refetch_respects_rate_limit() {
        // The seeds are wiremock loopback literals, which the SSRF entry guard
        // rejects in production — same allowance the robots-blocked test uses.
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        )]);

        // 400 ms token period against a 50 ms mock latency: the two numbers are
        // deliberately far apart so a spacing inside the mock's own cost proves
        // the limiter never gated the fetch.
        const DELAY_MS: u64 = 400;
        const MOCK_LATENCY_MS: u64 = 50;

        let server = wiremock::MockServer::start().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        Mock::given(method("GET"))
            .respond_with(Arrivals {
                template: ResponseTemplate::new(200)
                    .set_body_string(RATE_LIMIT_PAGE_HTML)
                    .set_delay(Duration::from_millis(MOCK_LATENCY_MS)),
                seen: seen.clone(),
            })
            // Exactly two arrivals: one per seed, nothing else.
            .expect(2)
            .mount(&server)
            .await;

        let urls: Vec<Url> = (0..2)
            .map(|i| Url::parse(&format!("{}/page/{i}", server.uri())).expect("valid URL"))
            .collect();

        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = DELAY_MS;
        // Burst 1 is what makes the wait observable: the derived default (≈ 8 on
        // this host) grants both seeds immediately and masks the period entirely.
        opts.budget_overrides.rate_burst =
            Some(crate::domain::budget::BurstPermits::new(1).expect("burst 1 is valid"));
        // robots.txt would add a request per domain; ignoring it keeps the mock
        // at exactly two arrivals so the measurement is page-fetch only.
        opts.crawl.ignore_robots = true;

        let (results, failures, blocked) = scrape_urls(
            &urls,
            &crate::domain::config::ScraperConfig::default(),
            &opts,
            &crate::application::progress_observer::NoopObserver,
            None,
            None,
            &crate::domain::CorrelationId::new(),
            &CancellationToken::new(),
            &[],
        )
        .await
        .expect("setup must succeed");

        assert_eq!(results.len(), 2, "both seeds must be scraped");
        assert!(failures.is_empty(), "no seed may fail, got: {failures:?}");
        assert_eq!(blocked, 0, "nothing may be robots-blocked here");

        let arrivals = seen.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(arrivals.len(), 2, "one arrival per seed");
        let spacing = arrivals[1] - arrivals[0];

        // The brief's literal bound: the gap must exceed twice the mock's own
        // latency, so it cannot be explained by the response delay.
        assert!(
            spacing >= Duration::from_millis(2 * MOCK_LATENCY_MS),
            "arrival spacing {spacing:?} is within the mock's own latency — the limiter did not gate the scrape path"
        );
        // The real invariant: a burst-1 bucket refills one permit per DELAY_MS,
        // so consecutive arrivals sit at least three quarters of a period apart.
        // Not the full period: a shared CI runner can absorb that much scheduling
        // slack between the grant and the arrival, and a false red here would be
        // worse than a slightly looser bound.
        assert!(
            spacing >= Duration::from_millis(DELAY_MS * 3 / 4),
            "arrival spacing {spacing:?} must be >= 0.75x the {DELAY_MS}ms token period"
        );
    }

    /// `delay_ms == 0` must build NO bucket at all — not a bucket with a 1 ms
    /// floor. That is what keeps an unthrottled run free of any added await.
    #[test]
    fn scrape_rate_limiter_is_built_only_for_a_positive_delay() {
        let opts = CrawlOptions::default();
        assert!(
            build_scrape_rate_limiter(&opts).is_some(),
            "the default positive --delay-ms must gate the scrape path"
        );

        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 0;
        assert!(
            build_scrape_rate_limiter(&opts).is_none(),
            "--delay-ms 0 must build no bucket and add no await"
        );
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
        assert!(fetcher
            .is_allowed("http://localhost:18080/page", "localhost")
            .await
            .allows());
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
