//! RAM-aware resource governor for heavyweight downloaders.
//!
//! Uses `sysinfo` to read available system memory and a [`tokio::sync::Semaphore`]
//! to gate concurrent Chrome/headless-browser instances. The formula:
//!
//! ```text
//! max_instances = (available_ram_bytes * 0.6) / CHROME_INSTANCE_COST
//! ```
//!
//! Thresholds:
//! - **80 %** RAM used → warning log, max_instances halved
//! - **90 %** RAM used → all new Chrome permits denied
//!
//! # Admission waits are observable (#1610, OBS-P1-2 / OBS-M2)
//!
//! [`ResourceGovernor::acquire`] is the only admission gate a heavyweight
//! downloader passes through, and it used to be silent: a render queue that
//! serialised behind the permit budget looked exactly like a slow browser.
//! Every acquire now emits one `semaphore_acquire` event carrying the measured
//! wait, the occupancy it waited behind, and how the wait ended.
//!
//! The held permit is an [`tokio::sync::OwnedSemaphorePermit`], not a
//! `MutexGuard`/`RwLockReadGuard`: it is the documented semaphore pattern and
//! it is NOT the "never hold a lock across `.await`" rule that governs mutexes.
//!
//! # Sanitizer coverage
//!
//! - **Miri: inapplicable** — `sysinfo` calls `sysconf(SC_PHYS_PAGES)`, which
//!   Miri cannot execute (unsupported operation). Tests are gated with
//!   `#[cfg(not(miri))]` and document the reason.
//! - **TSan: covered** — the semaphore-based concurrent gating is exercised
//!   under ThreadSanitizer in CI (`sanitizers.yml`), where sysconf works.

use std::sync::Arc;

use sysinfo::System;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::DownloadError;

/// RAM usage percentage that triggers a warning and reduces capacity by half.
///
/// Derived from the budget model's [`RamThresholds`] so the governor and the
/// pure derivation share ONE threshold source of truth (#897 item 4). Infra
/// → domain is the allowed dependency direction.
const WARNING_THRESHOLD: u8 =
    crate::domain::budget::derivation::RamThresholds::DEFAULT_WARNING_PERCENT;

/// RAM usage percentage that denies all new Chrome instances; see
/// [`WARNING_THRESHOLD`] for the single-source-of-truth rationale.
const CRITICAL_THRESHOLD: u8 =
    crate::domain::budget::derivation::RamThresholds::DEFAULT_CRITICAL_PERCENT;

/// `operation` field of every event this governor emits (#1610).
const RESOURCE_GOVERNOR_OPERATION: &str = "browser.resource_governor.acquire";

/// Gates concurrent heavyweight downloader instances based on system RAM.
///
/// Holds a [`CancellationToken`] (#509) so a caller blocked in
/// [`acquire`](Self::acquire) can be woken on engine shutdown instead of
/// hanging on the semaphore forever. [`new`](Self::new) uses a fresh token
/// that never fires (pre-#509 behavior); the crawl engine injects its own
/// token via [`with_cancel_token`](Self::with_cancel_token).
pub struct ResourceGovernor {
    semaphore: Arc<Semaphore>,
    cancel: CancellationToken,
    /// Configured permit budget, kept so the emitted event can report the
    /// occupancy a waiter observed (`available` / `total`) instead of only the
    /// free count, which alone cannot distinguish "full" from "empty" (#1610).
    max_permits: usize,
}

impl ResourceGovernor {
    /// Create a governor calibrated to current system RAM.
    ///
    /// The embedded cancellation token never fires; use
    /// [`with_cancel_token`](Self::with_cancel_token) to wire shutdown.
    pub fn new() -> Self {
        Self::with_cancel_token(CancellationToken::new())
    }

    /// Create a governor calibrated to current system RAM whose
    /// [`acquire`](Self::acquire) aborts when `cancel` fires (#509).
    pub fn with_cancel_token(cancel: CancellationToken) -> Self {
        Self::with_max_instances(Self::compute_max_instances(), cancel)
    }

    /// Create a governor with an explicit permit budget.
    ///
    /// Calibration seam behind [`new`](Self::new) /
    /// [`with_cancel_token`](Self::with_cancel_token): lets constrained
    /// environments (containers) and tests set the budget directly instead
    /// of deriving it from system RAM.
    pub fn with_max_instances(max_permits: usize, cancel: CancellationToken) -> Self {
        let max_permits = max_permits.max(1);
        debug!("ResourceGovernor: max_permits={max_permits}");

        Self {
            semaphore: Arc::new(Semaphore::new(max_permits)),
            cancel,
            max_permits,
        }
    }

    /// Check whether resources are available and return a permit count, or an
    /// error if the system is under too much memory pressure.
    ///
    /// The returned `usize` represents how many permits *could* be acquired
    /// right now (0 when denied).
    pub fn check_resources(&self) -> Result<usize, ResourceError> {
        let usage = Self::ram_usage_percent();

        if usage >= CRITICAL_THRESHOLD {
            warn!("RAM usage {usage}% >= {CRITICAL_THRESHOLD}%: new Chrome instances denied");
            return Err(ResourceError::RamTooHigh(usage));
        }

        let available = self.semaphore.available_permits();

        if usage >= WARNING_THRESHOLD {
            let reduced = available / 2;
            warn!(
                "RAM usage {usage}% >= {WARNING_THRESHOLD}%: available permits reduced {available} -> {reduced}"
            );
            return Ok(reduced);
        }

        Ok(available)
    }

    /// Acquire an owned semaphore permit, returning an error when resources
    /// are exhausted.
    ///
    /// Returns a [`tokio::sync::OwnedSemaphorePermit`] (`'static`) so callers can hold
    /// it across async boundaries without tying it to the governor's lifetime.
    ///
    /// The wait aborts with [`DownloadError::Cancelled`] if the governor's
    /// cancellation token fires while blocked (#509). Dropping the pending
    /// acquire consumes no permit, so a cancelled wait cannot leak one.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Cancelled`] on cancellation and
    /// [`DownloadError::Internal`] if the semaphore was closed.
    pub async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, DownloadError> {
        let arc = Arc::clone(&self.semaphore);
        // Occupancy BEFORE the wait: what the waiter had to queue behind.
        let available_before = arc.available_permits();
        let started = std::time::Instant::now();

        let outcome = tokio::select! {
            result = arc.acquire_owned() => {
                // LCOV_EXCL_LINE defensive: semaphore-closed — acquire_owned fails only when the governor is shut down
                result.map_err(|_| {
                    DownloadError::Internal("resource governor semaphore closed".to_string())
                })
            },
            () = self.cancel.cancelled() => Err(DownloadError::Cancelled),
        };

        // #1610 (OBS-M2): the wait is the admission decision, so it emits
        // whether or not it blocked — a zero-cost acquire is what makes a
        // NON-saturated governor provable. DEBUG keeps the console quiet;
        // --trace-file always captures it.
        debug!(
            operation = RESOURCE_GOVERNOR_OPERATION,
            waited_ms = started.elapsed().as_millis() as u64,
            available_permits = available_before,
            total_permits = self.max_permits,
            outcome = match &outcome {
                Ok(_) => "granted",
                Err(DownloadError::Cancelled) => "cancelled",
                Err(_) => "closed",
            },
            "semaphore acquire"
        );

        outcome
    }

    /// Current number of available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Compute `max_instances` from current system RAM.
    ///
    /// Delegates to the budget model's pure derivation (task 2.7): one source
    /// of truth for the RAM formula. At zero usage pressure the decision can
    /// never be `Deny`, so the legacy floor (`max(1)`) is preserved via the
    /// derivation's own minimum.
    fn compute_max_instances() -> usize {
        let total = Self::total_ram_bytes();
        match crate::domain::budget::derivation::derive_max_instances(
            total,
            0,
            crate::domain::budget::derivation::RamThresholds::default(),
        ) {
            crate::domain::budget::derivation::MaxChromeDecision::Allow(instances) => {
                instances.get()
            },
            // Unreachable at zero pressure; keep the legacy floor defensively.
            crate::domain::budget::derivation::MaxChromeDecision::Deny => 1,
        }
    }

    /// Total system RAM in bytes.
    fn total_ram_bytes() -> u64 {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory()
    }

    /// RAM usage as a percentage (0–100).
    pub fn ram_usage_percent() -> u8 {
        let mut sys = System::new();
        sys.refresh_memory();
        let total = sys.total_memory();
        if total == 0 {
            return 0;
        }
        let used = sys.used_memory();
        ((used * 100) / total) as u8
    }
}

impl Default for ResourceGovernor {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors raised when system resources cannot support a new instance.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    /// RAM usage exceeded the critical threshold.
    #[error("RAM usage too high ({0}%): new Chrome instances denied")]
    RamTooHigh(u8),

    /// Generic resource exhaustion.
    #[error("insufficient resources: {0}")]
    Insufficient(String),
}

impl From<ResourceError> for DownloadError {
    fn from(err: ResourceError) -> Self {
        DownloadError::ResourceExhausted(err.to_string())
    }
}

#[cfg(test)]
#[cfg(not(miri))] // sysinfo uses sysconf (unsupported by Miri — but TSan covers the semaphore)
mod tests {
    /// Task 2.7: the governor's live budget must equal the budget model's
    /// pure derivation for the same injected RAM values (single source of
    /// truth), including the legacy floor on tiny machines.
    #[test]
    fn compute_max_instances_matches_budget_model_derivation() {
        use crate::domain::budget::{detector::FixedDetector, BudgetModel, BudgetOverrides};
        for total_gib in [1_u64, 2, 4, 8, 16, 32, 64] {
            let total = total_gib * 1024 * 1024 * 1024;
            let detector = FixedDetector::with_detection(
                std::num::NonZeroUsize::new(8).expect("8 is non-zero"),
                Some(total),
            );
            let model = BudgetModel::build(BudgetOverrides::default(), &detector);
            let model_max = model
                .max_chrome_instances()
                .map(crate::domain::budget::tiers::MaxChromeInstances::get)
                .unwrap_or(1);
            // Legacy inline formula (pre-delegation reference):
            let legacy = (((total as f64 * 0.6) as u64) / 200_000_000).max(1) as usize;
            assert_eq!(
                model_max, legacy,
                "model must reproduce legacy at {total_gib}GiB"
            );
            assert_eq!(
                ResourceGovernor::with_max_instances(model_max, CancellationToken::new())
                    .available_permits(),
                model_max,
                "governor accepts the model-derived ceiling at {total_gib}GiB"
            );
        }
    }

    use super::*;

    use std::time::Duration;

    // The sysinfo-backed tests below read /proc via `sysconf`, which Miri
    // cannot execute — gate them there. The semaphore/cancellation tests use
    // the explicit-budget seam (`with_max_instances`) and DO run under Miri.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_governor_creation() {
        let gov = ResourceGovernor::new();
        // On any real machine we should have at least 1 permit
        assert!(gov.available_permits() >= 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_check_resources_returns_ok() {
        let gov = ResourceGovernor::new();
        // On CI or dev machines RAM usage is typically well below 80%
        let result = gov.check_resources();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_total_ram_nonzero() {
        let bytes = ResourceGovernor::total_ram_bytes();
        assert!(bytes > 0, "total RAM should be > 0 on any running system");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_usage_percent_range() {
        let pct = ResourceGovernor::ram_usage_percent();
        assert!(pct <= 100);
    }

    #[test]
    fn test_resource_error_display() {
        let err = ResourceError::RamTooHigh(92);
        assert!(err.to_string().contains("92%"));
    }

    #[test]
    fn test_resource_error_into_download_error() {
        let res_err = ResourceError::Insufficient("test".into());
        let dl_err: DownloadError = res_err.into();
        assert!(matches!(dl_err, DownloadError::ResourceExhausted(_)));
    }

    // ========================================================================
    // RAII permit tests (#509) — prove no semaphore permit leaks
    // ========================================================================

    #[tokio::test]
    async fn acquire_releases_permit_on_drop() {
        let gov = ResourceGovernor::with_max_instances(2, CancellationToken::new());
        assert_eq!(gov.available_permits(), 2);

        let permit = gov.acquire().await.expect("first acquire must succeed");
        assert_eq!(gov.available_permits(), 1);

        drop(permit);
        assert_eq!(
            gov.available_permits(),
            2,
            "dropping the permit must return it to the pool"
        );
    }

    #[tokio::test]
    async fn acquire_grants_permit_when_token_idle() {
        let gov = ResourceGovernor::with_max_instances(1, CancellationToken::new());

        let permit = tokio::time::timeout(Duration::from_secs(1), gov.acquire())
            .await
            .expect("acquire must not hang when permits are available");
        assert!(permit.is_ok());
    }

    // ========================================================================
    // Admission-wait observability (#1610, OBS-M2)
    // ========================================================================

    /// `tracing` caches the per-callsite `Interest` process-wide: whichever
    /// test reaches the `semaphore acquire` callsite first WITHOUT a subscriber
    /// freezes it at `never` and the assertions below would pass vacuously. A
    /// global sink subscriber installed once keeps every callsite interested;
    /// the per-test `FileTraceLayer` is what collects.
    static GLOBAL_SUBSCRIBER: std::sync::Once = std::sync::Once::new();

    /// Run `body` under a real `FileTraceLayer` and return the records whose
    /// `message` equals `message`.
    ///
    /// The production layer is the assertion surface — what this proves is
    /// that the event survives the same path `--trace-file` uses. The body
    /// drives its own runtime so every event it emits stays inside the
    /// `with_default` scope.
    fn capture<F, Fut>(message: &'static str, body: F) -> Vec<serde_json::Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        use crate::infrastructure::observability::FileTraceLayer;
        use tracing_subscriber::layer::SubscriberExt;

        GLOBAL_SUBSCRIBER.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).expect("trace layer");
        let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()));

        std::fs::read_to_string(&path)
            .expect("trace file")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
            .filter(|v| v["message"] == message)
            .collect()
    }

    /// A CONTENDED acquire emits the wait it paid: `available_permits = 0`
    /// (the occupancy it queued behind) and a measurable cost. This is what
    /// turns "the render queue serialised us" from an inference drawn from
    /// slowness into a queryable fact.
    #[test]
    fn contended_acquire_emits_measured_wait() {
        let events = capture("semaphore acquire", || async {
            let gov = std::sync::Arc::new(ResourceGovernor::with_max_instances(
                1,
                CancellationToken::new(),
            ));
            let held = gov.acquire().await.expect("first permit");

            // A second waiter cannot proceed until `held` is released, so the
            // wait is real rather than a scheduling accident.
            let waiter = {
                let gov = std::sync::Arc::clone(&gov);
                tokio::spawn(async move { gov.acquire().await.map(drop).is_ok() })
            };
            tokio::time::sleep(Duration::from_millis(60)).await;
            drop(held);
            assert!(
                waiter.await.expect("waiter task"),
                "the queued waiter must be granted once the permit frees"
            );
        });

        assert_eq!(events.len(), 2, "both acquires emit");
        let queued = events
            .iter()
            .find(|e| e["fields"]["available_permits"] == serde_json::json!(0))
            .expect("the queued acquire must report zero free permits");
        let fields = queued["fields"].as_object().expect("fields");
        assert_eq!(
            fields["operation"].as_str(),
            Some("browser.resource_governor.acquire")
        );
        assert_eq!(fields["outcome"].as_str(), Some("granted"));
        assert_eq!(fields["total_permits"].as_u64(), Some(1));
        assert!(
            fields["waited_ms"].as_u64().is_some_and(|ms| ms >= 40),
            "a queued acquire must report a measurable wait, got {:?}",
            fields["waited_ms"]
        );
    }

    /// #1610: a wait released by shutdown reports `outcome = "cancelled"`, not
    /// a generic failure — an admission skip and an admission denial are
    /// different operational facts.
    #[test]
    fn cancelled_acquire_emits_cancelled_outcome() {
        let events = capture("semaphore acquire", || async {
            let cancel = CancellationToken::new();
            let gov = ResourceGovernor::with_max_instances(1, cancel.clone());
            let held = gov.acquire().await.expect("single permit");
            cancel.cancel();
            let result = gov.acquire().await;
            assert!(matches!(result, Err(DownloadError::Cancelled)));
            drop(held);
        });

        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["fields"]["outcome"].as_str(), Some("cancelled"));
        assert_eq!(
            events[1]["fields"]["available_permits"].as_u64(),
            Some(0),
            "the cancelled waiter queued behind a held permit"
        );
    }

    // ========================================================================
    // Cancellation tests (#509)
    // ========================================================================

    #[tokio::test]
    async fn acquire_returns_cancelled_when_blocked_and_token_fires() {
        let cancel = CancellationToken::new();
        let gov = std::sync::Arc::new(ResourceGovernor::with_max_instances(1, cancel.clone()));

        // Take the single permit so the next acquire must block.
        let held = gov.acquire().await.expect("single permit must be granted");
        assert_eq!(gov.available_permits(), 0);

        let waiter = {
            let gov = std::sync::Arc::clone(&gov);
            tokio::spawn(async move { gov.acquire().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("cancelled acquire must return within the bound");
        assert!(matches!(result, Ok(Err(DownloadError::Cancelled))));

        // The cancelled wait must not have consumed a permit.
        assert_eq!(gov.available_permits(), 0);

        // RAII: releasing the held permit makes it visible again.
        drop(held);
        assert_eq!(gov.available_permits(), 1);
    }
}
