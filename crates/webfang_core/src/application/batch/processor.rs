//! Batch processor — concurrent execution of multiple crawl jobs
//!
//! Uses [`tokio::sync::Semaphore`] for job-level concurrency control.
//! Each URL in the batch is a separate `crawl_site()` call.
//!
//! # Usage
//!
//! ```no_run
//! use webfang_core::application::batch::{BatchJob, BatchProcessor};
//! use webfang_core::domain::CrawlerConfig;
//! use url::Url;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = CrawlerConfig::new(Url::parse("https://example.com")?);
//! let job = BatchJob::new(
//!     "batch-1".to_string(),
//!     vec!["https://example.com".to_string()],
//!     config,
//! );
//!
//! let processor = BatchProcessor::new(3).unwrap();
//! let result = processor.process_batch(job).await?;
//!
//! println!("Processed {} URLs, {} succeeded, {} failed",
//!     result.total, result.succeeded, result.failed);
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn, Instrument};

use super::BatchJob;
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::domain::{CrawlError, CrawlErrorCategory, CrawlerConfig};
use crate::error::ScraperError;

/// Result of processing a batch job
///
/// Not `Clone`: `errors` carries [`ScraperError`], which owns non-cloneable
/// sources (`std::io::Error`, boxed trait objects). Callers that need to
/// forward the errors must move them (#537).
#[derive(Debug)]
pub struct BatchResult {
    /// ID of the batch job
    pub job_id: String,
    /// Total number of URLs processed
    pub total: usize,
    /// Number of successfully processed URLs
    pub succeeded: usize,
    /// Number of failed URLs
    pub failed: usize,
    /// List of (url, error) for failed URLs.
    ///
    /// The [`ScraperError`] variant is preserved (not flattened to a string)
    /// so exit-code routing can aggregate severity via
    /// [`ScraperError::classify`] (#537).
    pub errors: Vec<(String, ScraperError)>,
}

/// Batch processor with concurrency control
///
/// Uses [`tokio::sync::Semaphore`] to limit the number of concurrent
/// crawl operations. This prevents resource exhaustion when processing
/// large batches of URLs.
#[derive(Clone)]
pub struct BatchProcessor {
    max_concurrent_jobs: usize,
    semaphore: Arc<Semaphore>,
    /// Optional sink that captures every fetched page body (#631).
    ///
    /// Shared across all concurrent crawls in the batch, so the CLI ends up
    /// with one collection covering every URL in the run.
    content_sink: Option<Arc<dyn CrawlContentSink>>,
}

impl BatchProcessor {
    /// Create a new batch processor with the given concurrency limit
    ///
    /// # Arguments
    ///
    /// * `max_concurrent` - Maximum number of concurrent crawl operations
    ///
    /// # Errors
    ///
    /// Returns [`BatchError::InvalidConcurrency`] if `max_concurrent` is 0.
    pub fn new(max_concurrent: usize) -> Result<Self, BatchError> {
        if max_concurrent == 0 {
            return Err(BatchError::InvalidConcurrency);
        }
        Ok(Self {
            max_concurrent_jobs: max_concurrent,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            content_sink: None,
        })
    }

    /// Capture every fetched page body into `sink` (#631).
    ///
    /// Without a sink the batch crawl discards page content and the CLI has
    /// nothing to export — the root cause of `--batch` writing zero files.
    #[must_use]
    pub fn with_content_sink(mut self, sink: Arc<dyn CrawlContentSink>) -> Self {
        self.content_sink = Some(sink);
        self
    }

    /// Get the maximum concurrency limit
    #[must_use]
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent_jobs
    }

    /// Process a batch job, crawling all URLs concurrently
    ///
    /// Returns a [`BatchResult`] with success/failure counts and error details.
    /// All tasks complete before returning (graceful shutdown).
    ///
    /// # Errors
    ///
    /// Returns an error if the batch job itself is malformed (e.g., empty URLs).
    #[instrument(name = "process_batch", skip(self, job), fields(job_id = %job.id, url_count = job.urls.len()))]
    pub async fn process_batch(&self, job: BatchJob) -> Result<BatchResult, BatchError> {
        self.process_batch_cancellable(job, &CancellationToken::new())
            .await
    }

    /// Process a batch job, stopping early when `cancel` is fired.
    ///
    /// On cancellation no further URL is dispatched, but every already-spawned
    /// task is drained to completion so its captured content still reaches the
    /// sink — an abrupt drop would lose the pages already fetched (#653).
    ///
    /// # Errors
    ///
    /// Same as [`process_batch`](Self::process_batch).
    #[instrument(name = "process_batch_cancellable", skip(self, job, cancel), fields(job_id = %job.id, url_count = job.urls.len()))]
    pub async fn process_batch_cancellable(
        &self,
        mut job: BatchJob,
        cancel: &CancellationToken,
    ) -> Result<BatchResult, BatchError> {
        if job.urls.is_empty() {
            return Err(BatchError::EmptyBatch);
        }

        info!(
            "Starting batch job {} with {} URLs (concurrency: {})",
            job.id,
            job.urls.len(),
            self.max_concurrent_jobs
        );

        job.start();
        let progress = job.progress.clone();
        let job_id = job.id.clone();
        let base_config = job.config.clone();

        let mut join_set = JoinSet::new();
        let mut errors: Vec<(String, ScraperError)> = Vec::new();

        let mut cancelled = false;
        for url_str in &job.urls {
            if cancel.is_cancelled() {
                cancelled = true;
                warn!(
                    job_id = %job.id,
                    "shutdown requested — no further URLs will be dispatched"
                );
                break;
            }
            let url = url_str.clone();
            let config = base_config.clone();
            let sink = self.content_sink.clone();
            let permit = self
                .semaphore
                .clone()
                .acquire_owned()
                .await
                // LCOV_EXCL_LINE defensive: semaphore-closed — acquire_owned fails only when the batch governor is shut down
                .map_err(|_| BatchError::SemaphoreClosed)?;

            progress.start_one();

            join_set.spawn(
                async move {
                    let _permit = permit; // Hold permit for duration of task
                    let result = process_single_url(&url, config, sink).await;
                    (url, result)
                }
                .in_current_span(),
            );
        }

        // Collect results as tasks complete
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok((url, Ok(_))) => {
                    progress.complete_one();
                    info!("Completed crawl for {url}");
                },
                Ok((url, Err(e))) => {
                    progress.fail_one();
                    // Preserve the full variant through the CrawlError ->
                    // ScraperError conversion (#537): severity routing needs
                    // classify(), which a flattened string cannot provide.
                    let scraper_err = ScraperError::from(e);
                    warn!(error = %scraper_err, "Failed to crawl {url}");
                    errors.push((url, scraper_err));
                },
                Err(e) => {
                    progress.fail_one();
                    error!("Task panicked: {e}");
                    errors.push((
                        "unknown".to_string(),
                        ScraperError::Internal(format!("task-panic: {e}")),
                    ));
                },
            }
        }

        let succeeded = progress.completed();
        let failed = progress.failed();
        let total = progress.total();

        if cancelled {
            job.fail("cancelled by shutdown signal".to_string());
        } else {
            job.complete();
        }

        info!(
            "Batch job {} completed: {succeeded}/{total} succeeded, {failed} failed",
            job.id
        );

        Ok(BatchResult {
            job_id,
            total,
            succeeded,
            failed,
            errors,
        })
    }
}

/// Build the per-URL [`CrawlerConfig`] used by [`process_single_url`].
///
/// Copies the crawl-relevant settings from the base config but uses the given
/// URL as the seed.
fn build_per_url_config(
    url: &str,
    base_config: &CrawlerConfig,
) -> Result<CrawlerConfig, CrawlError> {
    let parsed_url =
        url::Url::parse(url).map_err(|e| CrawlError::InvalidUrl(format!("{url}: {e}")))?;

    Ok(CrawlerConfig::builder(parsed_url)
        .max_depth(base_config.max_depth)
        .max_pages(base_config.max_pages)
        .concurrency(base_config.concurrency)
        .delay_ms(base_config.delay_ms)
        .timeout_secs(base_config.timeout_secs)
        .ignore_robots(base_config.ignore_robots)
        .tls_emulation(base_config.tls_emulation)
        .exclude_patterns(base_config.exclude_patterns.clone())
        .include_patterns(base_config.include_patterns.clone())
        // Bug R2-1: operator budget overrides staged on the base config
        // must survive the per-URL rebuild or the Engine re-derives auto
        // tiers, silently ignoring --concurrency / --rate-limit-burst.
        .budget_overrides(base_config.budget_overrides)
        .build())
}

/// Process a single URL by creating a CrawlerConfig and calling crawl_site
///
/// Creates a new `CrawlerConfig` for the given URL, copying settings from
/// the base config but using the specific URL as the seed.
///
/// Returns `Err(CrawlError)` if the crawl result has any errors (e.g., timeouts),
/// ensuring the batch processor correctly counts failed URLs.
async fn process_single_url(
    url: &str,
    base_config: CrawlerConfig,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
) -> Result<crate::domain::CrawlResult, CrawlError> {
    let config = build_per_url_config(url, &base_config)?;

    let result = match content_sink {
        Some(sink) => {
            crate::application::crawler::engine::crawl_site_capturing(config, sink).await?
        },
        None => crate::application::crawler::engine::crawl_site(config).await?,
    };

    // Treat any crawl errors (timeouts, etc.) as failures for batch processing,
    // but preserve severity (#537): the engine already partitioned them into
    // `error_breakdown` (issue #374). A genuinely-internal category (storage,
    // checkpoint, parse, panic) is a real bug and must stay `Internal` so the
    // run exits 3 (issue #537, phase 1). A purely transient/external failure
    // set (timeout, network, http, rate-limit, waf) must surface as a transient
    // `CrawlError` so it classifies `TransientRetriable`/`Backoff`/`Permanent`
    // and exits 69 — NOT as a bug (exit 3).
    if result.errors > 0 {
        let breakdown = &result.error_breakdown;

        // Defensive: errors reported without a category breakdown are treated
        // as internal failures (fail-safe → exit 3), matching the classify()
        // safety net for genuinely unknown errors.
        if breakdown.is_empty() {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // A genuinely-internal category means a real bug. Mixed batches are
        // dominated by the worst severity, so any such category escalates to
        // exit 3 (matches `scraper_failure_for_internal_fatal` in the CLI).
        let has_internal_bug = [
            CrawlErrorCategory::Internal,
            CrawlErrorCategory::Extraction,
            CrawlErrorCategory::Panic,
        ]
        .iter()
        .any(|c| breakdown.get(c).copied().unwrap_or(0) > 0);
        if has_internal_bug {
            return Err(CrawlError::Internal(format!(
                "crawl completed with {} error(s)",
                result.errors
            )));
        }

        // Purely transient/external: surface as a transient error → exit 69.
        // A timeout is the most common batch failure here.
        if breakdown
            .get(&CrawlErrorCategory::Timeout)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(CrawlError::Timeout);
        }
        return Err(CrawlError::Connection(format!(
            "crawl completed with {} transient error(s)",
            result.errors
        )));
    }

    Ok(result)
}

/// Errors that can occur during batch processing
#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    /// Batch contains no URLs
    #[error("batch contains no URLs")]
    EmptyBatch,

    /// Concurrency limit must be greater than zero
    #[error("max_concurrent must be > 0")]
    InvalidConcurrency,

    /// Semaphore was closed unexpectedly
    #[error("concurrency semaphore was closed")]
    SemaphoreClosed,

    /// Crawl operation failed
    #[error("crawl failed for {url}: {error}")]
    CrawlFailed {
        /// URL that failed to crawl
        url: String,
        /// The underlying crawl error
        error: CrawlError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::batch::{BatchJob, BatchJobStatus, BatchProgress};
    use crate::domain::CrawlerConfig;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use url::Url;

    #[test]
    fn test_batch_processor_creation() {
        let processor = BatchProcessor::new(5).unwrap();
        assert_eq!(processor.max_concurrent(), 5);
    }

    #[test]
    fn test_batch_processor_zero_concurrency_returns_error() {
        let result = BatchProcessor::new(0);
        assert!(result.is_err(), "zero concurrency should return Err");
        let err = result.err().unwrap();
        assert!(
            matches!(err, BatchError::InvalidConcurrency),
            "expected InvalidConcurrency, got: {err}"
        );
    }

    #[tokio::test]
    async fn cancelled_batch_dispatches_no_urls() {
        // Regression for #653: a shutdown signal must stop the batch from
        // dispatching further URLs instead of running to completion.
        let processor = BatchProcessor::new(2).unwrap();
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let job = BatchJob::new(
            "cancelled".to_string(),
            (0..8).map(|i| format!("https://example.com/{i}")).collect(),
            config,
        );

        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = processor
            .process_batch_cancellable(job, &cancel)
            .await
            .expect("a cancelled batch still reports its (empty) result");

        assert_eq!(result.succeeded, 0, "no URL may be crawled after shutdown");
        assert_eq!(result.failed, 0, "skipped URLs are not failures");
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_process_empty_batch() {
        let processor = BatchProcessor::new(3).unwrap();
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let job = BatchJob::new("test-1".to_string(), vec![], config);

        let result = processor.process_batch(job).await;
        assert!(matches!(result, Err(BatchError::EmptyBatch)));
    }

    #[tokio::test]
    async fn test_batch_progress_concurrent_updates() {
        let progress = BatchProgress::new(100);
        let mut join_set = JoinSet::new();

        // Deterministic outcomes: first 50 succeed, last 50 fail
        for i in 0..100 {
            let p = progress.clone();
            join_set.spawn(async move {
                p.start_one();
                tokio::task::yield_now().await;
                if i < 50 {
                    p.complete_one();
                    true
                } else {
                    p.fail_one();
                    false
                }
            });
        }

        let mut successes = 0;
        let mut failures = 0;
        while let Some(result) = join_set.join_next().await {
            if result.unwrap() {
                successes += 1;
            } else {
                failures += 1;
            }
        }

        assert_eq!(successes + failures, 100);
        assert_eq!(progress.completed(), 50);
        assert_eq!(progress.failed(), 50);
        assert!(progress.is_complete());
    }

    #[test]
    fn test_batch_result_display() {
        let result = BatchResult {
            job_id: "test-1".to_string(),
            total: 10,
            succeeded: 8,
            failed: 2,
            errors: vec![
                (
                    "https://example.com/404".to_string(),
                    ScraperError::http(404, "https://example.com/404"),
                ),
                (
                    "https://example.com/timeout".to_string(),
                    ScraperError::Network(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Timeout",
                    ))),
                ),
            ],
        };

        assert_eq!(result.total, 10);
        assert_eq!(result.succeeded, 8);
        assert_eq!(result.failed, 2);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_batch_progress_clone() {
        let progress = BatchProgress::new(5);
        progress.complete_one();
        progress.complete_one();

        let cloned = progress.clone();
        assert_eq!(cloned.total(), 5);
        assert_eq!(cloned.completed(), 2);
    }

    // =====================================================================
    // BatchResult structure tests
    // =====================================================================

    #[test]
    fn test_batch_result_all_succeeded() {
        let result = BatchResult {
            job_id: "job-ok".to_string(),
            total: 3,
            succeeded: 3,
            failed: 0,
            errors: vec![],
        };
        assert_eq!(result.total, result.succeeded);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_batch_result_all_failed() {
        let result = BatchResult {
            job_id: "job-fail".to_string(),
            total: 2,
            succeeded: 0,
            failed: 2,
            errors: vec![
                (
                    "https://a.com".to_string(),
                    ScraperError::Internal("error a".to_string()),
                ),
                (
                    "https://b.com".to_string(),
                    ScraperError::Internal("error b".to_string()),
                ),
            ],
        };
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, result.total);
        assert_eq!(result.errors.len(), 2);
    }

    #[test]
    fn test_batch_result_counts_consistent() {
        let errors: Vec<(String, ScraperError)> = (0..5)
            .map(|i| {
                (
                    format!("url-{i}"),
                    ScraperError::Internal(format!("err-{i}")),
                )
            })
            .collect();
        let result = BatchResult {
            job_id: "job-mixed".to_string(),
            total: 10,
            succeeded: 5,
            failed: 5,
            errors,
        };
        assert_eq!(result.succeeded + result.failed, result.total);
        assert_eq!(result.errors.len(), result.failed);
    }

    // =====================================================================
    // BatchProcessor concurrency tests
    // =====================================================================

    #[test]
    fn test_batch_processor_various_concurrency_values() {
        for n in [1, 2, 4, 8, 16] {
            let processor = BatchProcessor::new(n).unwrap();
            assert_eq!(processor.max_concurrent(), n);
        }
    }

    #[test]
    fn test_batch_processor_single_concurrency() {
        let processor = BatchProcessor::new(1).unwrap();
        assert_eq!(processor.max_concurrent(), 1);
    }

    // =====================================================================
    // BatchJob status transitions
    // =====================================================================

    #[test]
    fn test_batch_job_lifecycle() {
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new(
            "lifecycle".to_string(),
            vec!["https://example.com".to_string()],
            config,
        );

        assert_eq!(job.status, BatchJobStatus::Pending);

        job.start();
        assert_eq!(job.status, BatchJobStatus::Running);

        job.complete();
        assert_eq!(job.status, BatchJobStatus::Completed);
    }

    #[test]
    fn test_batch_job_failure_state() {
        let config = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new("fail-job".to_string(), vec![], config);

        job.fail("network error".to_string());
        assert_eq!(
            job.status,
            BatchJobStatus::Failed("network error".to_string())
        );
    }

    #[test]
    fn test_batch_job_status_display() {
        assert_eq!(BatchJobStatus::Pending.to_string(), "Pending");
        assert_eq!(BatchJobStatus::Running.to_string(), "Running");
        assert_eq!(BatchJobStatus::Completed.to_string(), "Completed");
        assert_eq!(
            BatchJobStatus::Failed("oops".to_string()).to_string(),
            "Failed: oops"
        );
    }

    // =====================================================================
    // BatchProgress edge cases
    // =====================================================================

    #[test]
    fn test_batch_progress_percent_partial() {
        let progress = BatchProgress::new(4);
        progress.start_one();
        progress.complete_one();
        progress.start_one();
        progress.fail_one();

        assert!((progress.percent() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_batch_progress_in_progress_count() {
        let progress = BatchProgress::new(5);
        assert_eq!(progress.in_progress(), 0);

        progress.start_one();
        assert_eq!(progress.in_progress(), 1);

        progress.start_one();
        assert_eq!(progress.in_progress(), 2);

        progress.complete_one();
        assert_eq!(progress.in_progress(), 1);

        progress.fail_one();
        assert_eq!(progress.in_progress(), 0);
    }

    // =====================================================================
    // BatchError display tests
    // =====================================================================

    #[test]
    fn test_batch_error_empty_batch_display() {
        let err = BatchError::EmptyBatch;
        assert!(err.to_string().contains("no URLs"));
    }

    #[test]
    fn test_batch_error_crawl_failed_display() {
        let err = BatchError::CrawlFailed {
            url: "https://example.com".to_string(),
            error: CrawlError::InvalidUrl("bad url".to_string()),
        };
        let msg = err.to_string();
        assert!(msg.contains("example.com"));
        assert!(msg.contains("crawl failed"));
    }

    #[test]
    fn test_batch_error_semaphore_closed_display() {
        let err = BatchError::SemaphoreClosed;
        assert!(err.to_string().contains("semaphore"));
    }

    // =====================================================================
    // Bug R2-1: BudgetOverrides must survive the per-URL config rebuild
    // =====================================================================

    #[test]
    fn per_url_config_carries_budget_overrides() {
        // The batch path rebuilds a fresh CrawlerConfig for every URL; the
        // operator overrides staged on the base config must ride along or the
        // Engine silently re-derives the auto tiers.
        let overrides = crate::domain::budget::BudgetOverrides {
            crawl: crate::domain::budget::tiers::CrawlConcurrency::new(3).ok(),
            rate_burst: crate::domain::budget::tiers::BurstPermits::new(7).ok(),
            ..crate::domain::budget::BudgetOverrides::default()
        };
        let base = CrawlerConfig::builder(Url::parse("https://example.com").unwrap())
            .budget_overrides(overrides)
            .build();

        let per_url =
            build_per_url_config("https://example.org/seed", &base).expect("valid per-URL seed");

        assert_eq!(
            per_url.budget_overrides.crawl.map(|c| c.get()),
            Some(3),
            "explicit --concurrency must survive the per-URL rebuild"
        );
        assert_eq!(
            per_url.budget_overrides.rate_burst.map(|b| b.get()),
            Some(7),
            "explicit --rate-limit-burst must survive the per-URL rebuild"
        );
    }

    #[test]
    fn per_url_config_rejects_invalid_url() {
        let base = CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let err = build_per_url_config("not a url", &base);
        assert!(matches!(err, Err(CrawlError::InvalidUrl(_))));
    }

    /// Shared in-flight gauge + six-node star topology (seed + 5 leaves) for
    /// the R2-1 diagnostics: counts every request and the high-water mark of
    /// concurrent responses so the scheduler bound derived from the operator
    /// override is observable end to end.
    struct SixNodeGauge {
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
        total_requests: Arc<AtomicUsize>,
        seed_uri: String,
    }

    impl Clone for SixNodeGauge {
        fn clone(&self) -> Self {
            Self {
                inflight: Arc::clone(&self.inflight),
                max_inflight: Arc::clone(&self.max_inflight),
                total_requests: Arc::clone(&self.total_requests),
                seed_uri: self.seed_uri.clone(),
            }
        }
    }

    impl wiremock::Respond for SixNodeGauge {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let current = self.inflight.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.max_inflight.fetch_max(current, AtomicOrdering::SeqCst);
            self.total_requests.fetch_add(1, AtomicOrdering::SeqCst);
            // Force overlap when the scheduler bound allows parallel fetches,
            // so an over-broad bound is actually observed by the high-water mark.
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.inflight.fetch_sub(1, AtomicOrdering::SeqCst);
            if request.url.path() == "/" {
                let links: String = (0..5)
                    .map(|i| format!(r#"<a href="{}/p{i}">n{i}</a>"#, self.seed_uri))
                    .collect();
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>{links}</body></html>"))
            } else {
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("<html><body>leaf</body></html>")
            }
        }
    }

    /// Six-node diagnostic (bug R2-1): with `crawl = 1` staged as an operator
    /// override on the batch base config, the Engine reached through
    /// `BatchProcessor.process_single_url -> crawl_site` must fetch the six
    /// nodes strictly one at a time — the explicit `--concurrency` wins over
    /// both the configured value and the auto tier table.
    #[cfg(not(miri))] // wiremock + wreq use boring-sys2 FFI (unsupported by Miri)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn batch_mode_enforces_concurrency_override_six_node_diagnostic() {
        use crate::domain::budget::{
            tiers::{BurstPermits, CrawlConcurrency},
            BudgetOverrides,
        };

        let server = wiremock::MockServer::start().await;
        let gauge = SixNodeGauge {
            inflight: Arc::new(AtomicUsize::new(0)),
            max_inflight: Arc::new(AtomicUsize::new(0)),
            total_requests: Arc::new(AtomicUsize::new(0)),
            seed_uri: server.uri(),
        };
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(gauge.clone())
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("{}/", server.uri())).unwrap();
        let base_config = CrawlerConfig::builder(seed)
            .max_depth(1)
            .max_pages(10)
            .concurrency(16) // configured value must be beaten by the override
            .timeout_secs(5)
            .ignore_robots(true)
            .budget_overrides(BudgetOverrides {
                crawl: CrawlConcurrency::new(1).ok(),
                rate_burst: BurstPermits::new(4).ok(),
                ..BudgetOverrides::default()
            })
            .build();

        let job = BatchJob::new(
            "r2-1-six-node".to_string(),
            vec![format!("{}/", server.uri())],
            base_config,
        );
        let processor = BatchProcessor::new(2).unwrap();
        let result = processor.process_batch(job).await.unwrap();

        assert_eq!(result.succeeded, 1, "the single batch URL must succeed");
        assert_eq!(
            gauge.total_requests.load(AtomicOrdering::SeqCst),
            6,
            "seed + 5 discovered leaves must all be crawled"
        );
        assert_eq!(
            gauge.max_inflight.load(AtomicOrdering::SeqCst),
            1,
            "override crawl=1 must cap concurrent fetches at 1 through batch mode"
        );
    }
}
