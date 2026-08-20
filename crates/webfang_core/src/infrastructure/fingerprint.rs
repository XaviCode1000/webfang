//! No-op fingerprint repository (#792 Slice B).
//!
//! Zero-dependency [`FingerprintRepository`] implementation for builds without
//! the `persistence` feature (the lightweight core binary ships without
//! bundled libsqlite3). Records nothing and always reports a failure count of
//! `0`, so hint generation degrades gracefully instead of erroring.

use std::future::Future;
use std::pin::Pin;

use crate::domain::extraction_quality::FingerprintRecord;
use crate::domain::fingerprint_repository::FingerprintRepository;
use crate::error::ScraperError;

/// Fingerprint repository that discards every record.
///
/// Used when the `persistence` feature is disabled or when the caller opts
/// out of fingerprint recording (e.g. `--extraction-fingerprint` absent).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopFingerprintRepository;

impl FingerprintRepository for NoopFingerprintRepository {
    fn record_failure<'a>(
        &'a self,
        _record: &'a FingerprintRecord,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(0) })
    }

    fn get_failure_count<'a>(
        &'a self,
        _site_base_url: &'a str,
        _selector_signature: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u32, ScraperError>> + Send + 'a>> {
        Box::pin(async move { Ok(0) })
    }
}
