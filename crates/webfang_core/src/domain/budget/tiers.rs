//! Budget tier newtypes — Global→Domain→Operation→Asset concurrency values.
//!
//! Every tier is a NonZero-guarded newtype: a zero-semaphore, zero-burst or
//! zero-buffer configuration cannot be represented. Constructors are
//! fallible and reject 0 with a
//! [`BudgetValidationError`](tiers::BudgetValidationError) — they never
//! silently substitute a default.
//!
//! Newtype → enforcement-mechanism map:
//!
//! | Newtype | Enforcement mechanism (adapter keeps the mechanism) |
//! |---|---|
//! | [`GlobalConcurrency`] | whole-process ceiling feeding every other tier |
//! | [`DomainSlots`] | `DomainSessionPool` slots per domain |
//! | [`CrawlConcurrency`] | crawler JoinSet gating / scrape `buffer_unordered` |
//! | [`BatchConcurrency`] | batch processor `Semaphore` |
//! | [`InferenceWorkers`] | adaptive-engine inference `Semaphore` / worker pool |
//! | [`ElasticPermits`] | elastic byte-weighted `Semaphore::acquire_many` |
//! | [`DownloadConcurrency`] | asset-download `buffer_unordered` (Asset tier) |
//! | [`BurstPermits`] | rate-limiter token-bucket burst (independent, Q1) |
//! | [`MaxChromeInstances`] | ResourceGovernor RAM `Semaphore` permits |

use std::convert::TryFrom;
use std::num::{NonZeroU32, NonZeroUsize};

/// Validation error raised when a budget tier value is constructed with 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BudgetValidationError {
    /// Zero cannot be represented in a budget tier; the minimum is 1.
    #[error("el valor de presupuesto no puede ser 0 (debe ser mayor que 0)")]
    Zero,
}

/// Whole-process concurrency ceiling (Global tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalConcurrency(NonZeroUsize);

/// Per-domain session-pool slot count (Domain tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DomainSlots(NonZeroUsize);

/// Crawl-path concurrency (Operation tier): JoinSet gating / scrape fan-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CrawlConcurrency(NonZeroUsize);

/// Batch-processing concurrency (Operation tier): batch `Semaphore` bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BatchConcurrency(NonZeroUsize);

/// Inference worker count (Operation tier): adaptive-engine `Semaphore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InferenceWorkers(NonZeroUsize);

/// Elastic-ingestion permit count (Operation tier): byte-weighted `Semaphore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElasticPermits(NonZeroUsize);

/// Asset-download concurrency (Asset tier): download `buffer_unordered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DownloadConcurrency(NonZeroUsize);

/// Rate-limiter burst permits (governor token bucket; independent of crawl
/// concurrency per decision Q1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BurstPermits(NonZeroU32);

/// Chrome-instance cap derived from RAM (ResourceGovernor permits).
///
/// The governor's legacy `max(1)` floor is folded into the *derivation*
/// (`derive_max_instances`), which never hands 0 to this type; the type
/// itself still rejects 0 at construction so the invariant is total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaxChromeInstances(NonZeroUsize);

macro_rules! nonzero_usize_tier {
    ($($name:ident),* $(,)?) => {
        $(
            impl $name {
                /// Construct from a raw value, rejecting 0 with a validation
                /// error (never a silent default).
                ///
                /// # Errors
                ///
                /// Returns [`BudgetValidationError::Zero`] when `value == 0`.
                pub fn new(value: usize) -> Result<Self, BudgetValidationError> {
                    NonZeroUsize::new(value)
                        .map(Self)
                        .ok_or(BudgetValidationError::Zero)
                }

                /// Raw value.
                #[must_use]
                pub const fn get(self) -> usize {
                    self.0.get()
                }
            }

            impl From<NonZeroUsize> for $name {
                fn from(value: NonZeroUsize) -> Self {
                    Self(value)
                }
            }

            impl TryFrom<usize> for $name {
                type Error = BudgetValidationError;

                fn try_from(value: usize) -> Result<Self, Self::Error> {
                    Self::new(value)
                }
            }
        )*
    };
}

nonzero_usize_tier!(
    GlobalConcurrency,
    DomainSlots,
    CrawlConcurrency,
    BatchConcurrency,
    InferenceWorkers,
    ElasticPermits,
    DownloadConcurrency,
    MaxChromeInstances,
);

impl BurstPermits {
    /// Construct from a raw value, rejecting 0 with a validation error
    /// (never a silent default).
    ///
    /// # Errors
    ///
    /// Returns [`BudgetValidationError::Zero`] when `value == 0`.
    pub fn new(value: u32) -> Result<Self, BudgetValidationError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(BudgetValidationError::Zero)
    }

    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl From<NonZeroU32> for BurstPermits {
    fn from(value: NonZeroU32) -> Self {
        Self(value)
    }
}

impl TryFrom<u32> for BurstPermits {
    type Error = BudgetValidationError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Aggregate of every operation-tier concurrency (crawl/batch/inference/elastic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationTier {
    /// Crawl/scrape-path concurrency.
    pub crawl: CrawlConcurrency,
    /// Batch-processing concurrency.
    pub batch: BatchConcurrency,
    /// Inference worker count.
    pub inference: InferenceWorkers,
    /// Elastic-ingestion permit count.
    pub elastic: ElasticPermits,
}

/// Aggregate of all budget tiers derived by the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetTiers {
    /// Whole-process ceiling.
    pub global: GlobalConcurrency,
    /// Per-host session-pool slots.
    pub domain: DomainSlots,
    /// Operation-tier work units.
    pub operation: OperationTier,
    /// Asset-tier downloads.
    pub asset: DownloadConcurrency,
    /// Rate-limiter burst (independent knob, Q1).
    pub burst: BurstPermits,
    /// Governor RAM-derived Chrome-instance cap; `None` when system RAM is
    /// undetectable and the adapter must fall back to its legacy path.
    pub max_chrome_instances: Option<MaxChromeInstances>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_rejects_zero() {
        assert_eq!(GlobalConcurrency::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn domain_slots_reject_zero() {
        assert_eq!(DomainSlots::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn crawl_rejects_zero() {
        assert_eq!(CrawlConcurrency::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn batch_rejects_zero() {
        assert_eq!(BatchConcurrency::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn inference_rejects_zero() {
        assert_eq!(InferenceWorkers::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn elastic_rejects_zero() {
        assert_eq!(ElasticPermits::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn download_rejects_zero() {
        assert_eq!(
            DownloadConcurrency::new(0),
            Err(BudgetValidationError::Zero)
        );
    }

    #[test]
    fn burst_rejects_zero() {
        assert_eq!(BurstPermits::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn max_chrome_instances_rejects_zero() {
        assert_eq!(MaxChromeInstances::new(0), Err(BudgetValidationError::Zero));
    }

    #[test]
    fn valid_values_round_trip() {
        assert_eq!(GlobalConcurrency::new(16).expect("valid").get(), 16);
        assert_eq!(DomainSlots::new(8).expect("valid").get(), 8);
        assert_eq!(CrawlConcurrency::new(5).expect("valid").get(), 5);
        assert_eq!(BatchConcurrency::new(4).expect("valid").get(), 4);
        assert_eq!(InferenceWorkers::new(7).expect("valid").get(), 7);
        assert_eq!(ElasticPermits::new(4).expect("valid").get(), 4);
        assert_eq!(DownloadConcurrency::new(3).expect("valid").get(), 3);
        assert_eq!(BurstPermits::new(5).expect("valid").get(), 5);
        assert_eq!(MaxChromeInstances::new(2).expect("valid").get(), 2);
    }

    #[test]
    fn try_from_round_trips() {
        assert_eq!(CrawlConcurrency::try_from(6).expect("valid").get(), 6);
        assert_eq!(BurstPermits::try_from(9_u32).expect("valid").get(), 9);
        assert_eq!(
            CrawlConcurrency::try_from(0),
            Err(BudgetValidationError::Zero)
        );
    }

    #[test]
    fn from_nonzero_never_fails() {
        let nz = NonZeroUsize::new(3).expect("3 is non-zero");
        assert_eq!(CrawlConcurrency::from(nz).get(), 3);
        let nz32 = NonZeroU32::new(2).expect("2 is non-zero");
        assert_eq!(BurstPermits::from(nz32).get(), 2);
    }

    #[test]
    fn zero_error_message_is_spanish() {
        let msg = BudgetValidationError::Zero.to_string();
        assert!(msg.contains("no puede ser 0"), "unexpected message: {msg}");
    }
}
