//! Resource-download port — domain seam for byte-weighted resource fetching
//! (ADR-0012-B unit 5).
//!
//! Extracted per the ADR-0010 §2 mapping (ResourceDownloader port slice →
//! `domain::downloader_port` vocabulary): `application::elastic_ingestion`
//! consumed the concrete
//! `infrastructure::crawler::resource_downloader::ResourceDownloader` for a
//! single surface — [`ResourceDownloadPort::download`]. The concrete (wreq
//! client, byte-weighted semaphore, `PermitGuard` RAII) stays in
//! infrastructure per ADR-0012-B §2.1 and is constructed only at the
//! composition root (`application::container`), which passes it behind this
//! trait object.
//!
//! The signature mirrors the concrete's contract byte-for-byte (`&str` URL,
//! `Result<Vec<u8>, ScraperError>`) so error text and CLI exit-code mapping
//! are unchanged; the method is dyn-compatible via manual `BoxFuture`
//! desugaring, matching the repo's frozen desugaring decision (see
//! [`crate::domain::crawler_port::sitemap`]).

use futures::future::BoxFuture;

use crate::error::ScraperError;

/// Download resource bodies (documents, assets) over HTTP.
///
/// Implemented in `infrastructure::crawler::resource_downloader` by
/// [`ResourceDownloader`](crate::infrastructure::crawler::ResourceDownloader);
/// `application::elastic_ingestion` consumes it as
/// `Arc<dyn ResourceDownloadPort>` wired through the composition root.
pub trait ResourceDownloadPort: Send + Sync {
    /// Download the body at `url` under the byte-weighted semaphore budget.
    ///
    /// # Errors
    ///
    /// Returns the same [`ScraperError`] variants the concrete produced
    /// pre-port (`GlobalTimeout`, `SlowlorisTimeout`, `PayloadTooLarge`,
    /// `Network`, `SemaphoreInanition`), so caller error handling and exit
    /// codes stay byte-identical.
    fn download<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Vec<u8>, ScraperError>>;
}
