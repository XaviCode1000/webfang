//! Operational error classification.
//!
//! [`ErrorClass`] lives in the domain layer per the Error Classification
//! Matrix (`docs/error-classification-matrix.md`, contract ID
//! `261bdb66-197e-420f-a73b-66c0e889102d`): classification is decided where
//! the error is born and derived upward through existing `From` conversions.

/// Operational classification of errors for observability and retry logic.
///
/// Partitions error variants by severity and recoverability.
///
/// Serializes as its snake_case variant name so persisted records
/// (`RawRecord.last_error.class`) stay stable across releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Transient errors that should be retried immediately (e.g., connection reset, 5xx)
    TransientRetriable,
    /// Transient errors that require backoff before retry (e.g., rate limit, slowloris)
    TransientBackoff,
    /// Permanent errors that cannot be recovered by retry (e.g., 4xx, invalid URL, WAF)
    PermanentFatal,
    /// Internal errors that indicate a bug (e.g., integer overflow, semaphore exhaustion)
    InternalFatal,
    /// Domain-level error against a single item — fall back and continue the job.
    ///
    /// Unlike `PermanentFatal` (which means "abort everything"), this means
    /// "this one page failed but the pipeline is healthy, so use raw content
    /// and keep crawling". Example: a chunk exceeding the user's `--max-tokens`
    /// limit during AI semantic cleaning.
    DomainRecoverable,
}
