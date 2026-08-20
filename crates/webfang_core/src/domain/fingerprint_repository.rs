//! Fingerprint repository port — per-site extraction failure tracking (#792 Slice B).
//!
//! Defines the contract for persisting [`FingerprintRecord`]s so repeated
//! low-score extractions on the same site/selector pair can be detected and
//! surfaced as honest error hints instead of silent degradation.
//!
//! Following the frozen decision #1 pattern from [`crate::domain::repository`],
//! methods are desugared to `Pin<Box<dyn Future<…> + Send + '_>>` so the trait
//! is dyn-compatible without the `async_trait` crate. All database failures
//! surface as [`ScraperError::Persistence`] (frozen decision #4: no separate
//! storage error enum).

use std::future::Future;
use std::pin::Pin;

use crate::domain::extraction_quality::FingerprintRecord;
use crate::error::ScraperError;

/// Repository interface for extraction failure fingerprints.
///
/// Implementations live in the infrastructure layer
/// (`infrastructure::persistence::fingerprint`): a SQLite-backed repository
/// (feature `persistence`) and a no-op repository for builds without
/// persistence.
pub trait FingerprintRepository: Send + Sync {
    /// Record an extraction failure fingerprint.
    ///
    /// Upserts on the `(site_base_url, selector_signature)` pair: a new pair
    /// inserts with `failure_count = 1`; an existing pair increments the count
    /// and refreshes `score_at_failure`, `last_seen`, and `last_note`.
    ///
    /// # Returns
    ///
    /// The failure count for this pair **after** the upsert (1 on first
    /// occurrence). A no-op implementation returns `0` — nothing was recorded.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn record_failure<'a>(
        &'a self,
        record: &'a FingerprintRecord,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>>;

    /// Get the recorded failure count for a site/selector pair.
    ///
    /// Returns `0` when the pair has never been recorded.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn get_failure_count<'a>(
        &'a self,
        site_base_url: &'a str,
        selector_signature: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>>;
}
