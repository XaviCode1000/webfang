//! Port traits — Domain-level abstractions for infrastructure services.
//!
//! Following Hexagonal Architecture: ports define what the application needs,
//! adapters provide the real implementations. The domain layer owns these
//! trait definitions with ZERO infrastructure dependencies.
//!
//! # Port Types
//!
//! - [`HttpClientPort`] — HTTP fetching abstraction (owned by the domain layer)
//! - [`ScraperPort`] — Content extraction abstraction
//! - [`PersistencePort`] — Data persistence abstraction
//!
//! The `HttpClientPort` trait and its `HttpResponse` DTO are defined in
//! `domain::http_port` (the domain layer owns the contract). The production
//! `wreq`-backed impl and mock tests live in `application::http_client`.
//! We re-export the trait here so downstream code can import from the
//! domain layer without reaching into the application layer.

pub use crate::domain::http_port::HttpClientPort;

use std::future::Future;
use std::pin::Pin;

use crate::domain::entities::progress::{ScrapeError, ScrapeStatus};
use crate::domain::entities::ScrapedContent;
use crate::domain::error::DomainError;

/// Port trait for content extraction (scraping).
///
/// Abstracts the Readability/fallback extraction pipeline so that
/// application services don't depend on specific HTML parsers.
pub trait ScraperPort: Send + Sync {
    /// Scrape a single URL and return extracted content.
    ///
    /// # Errors
    ///
    /// Returns `DomainError` on extraction failure.
    fn scrape(
        &self,
        url: &str,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<Vec<ScrapedContent>, DomainError>> + Send + '_>,
    >;
}

/// Port trait for data persistence (save/load crawled results).
pub trait PersistencePort: Send + Sync {
    /// Save scraped content to persistent storage.
    ///
    /// # Errors
    ///
    /// Returns `DomainError` on persistence failure.
    fn save(
        &self,
        content: &ScrapedContent,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), DomainError>> + Send + '_>>;

    /// Load scraped content by URL.
    ///
    /// # Errors
    ///
    /// Returns `DomainError` on query failure.
    fn load_by_url(
        &self,
        url: &str,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Option<ScrapedContent>, DomainError>>
                + Send
                + '_,
        >,
    >;
}

/// Port trait for downloading assets (images, documents).
///
/// Abstracts asset downloading so application services don't depend on
/// specific download implementations. The production adapter streams
/// to disk with ~8KB RAM; tests inject mocks that count calls.
pub trait AssetDownloaderPort: Send + Sync {
    /// Download a batch of assets from URLs.
    ///
    /// Returns partial results — individual failures don't abort the batch.
    fn download_batch(
        &self,
        urls: &[String],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<Vec<crate::domain::entities::DownloadedAsset>>,
                > + Send
                + '_,
        >,
    >;
}

/// Port trait for writing binary payloads (PDFs, images, archives) to disk.
///
/// Abstracts filesystem persistence so application use cases never call
/// `std::fs` directly — Clean Architecture keeps the application layer free of
/// filesystem specifics (issue #442). The production adapter
/// [`FsBinaryWriter`](crate::infrastructure::crawler::FsBinaryWriter) creates
/// the parent directory tree and writes the bytes; tests inject a temp-dir or
/// in-memory implementation.
pub trait BinaryWriterPort: Send + Sync {
    /// Write `bytes` to `path`, creating parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Io`](crate::error::ScraperError::Io) when parent
    /// directory creation or the write itself fails.
    fn write_bytes(&self, path: &std::path::Path, bytes: &[u8]) -> crate::error::Result<()>;
}

/// Port trait for real-time progress reporting during scraping.
///
/// Implementations receive structured progress events as the scraper
/// processes each URL. The trait is `Send + Sync` so observers can be
/// shared across async tasks.
pub trait ProgressObserver: Send + Sync {
    /// Called when scraping starts for a URL.
    fn on_page_started<'a>(&'a self, url: &'a str)
        -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when the status changes for a URL (Fetching, Extracting, etc.).
    fn on_status_changed<'a>(
        &'a self,
        url: &'a str,
        status: ScrapeStatus,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when a URL is successfully scraped.
    fn on_page_completed<'a>(
        &'a self,
        url: &'a str,
        chars: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when scraping a URL fails.
    fn on_page_failed<'a>(
        &'a self,
        url: &'a str,
        error: &'a ScrapeError,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called when a URL is blocked by robots.txt.
    fn on_robots_blocked<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

    /// Called after all URLs have been processed.
    fn on_finished<'a>(
        &'a self,
        total: usize,
        successful: usize,
        failed: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_error::HttpError;
    use crate::domain::http_port::HttpResponse;
    use crate::test_fixtures::MockHttpClient;
    use std::collections::HashMap;

    // --- Mock implementations for testing port traits ---

    struct MockPersistencePort {
        store: std::sync::Arc<std::sync::Mutex<HashMap<String, ScrapedContent>>>,
    }

    impl MockPersistencePort {
        fn new() -> Self {
            Self {
                store: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
            }
        }
    }

    impl PersistencePort for MockPersistencePort {
        fn save(
            &self,
            content: &ScrapedContent,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<(), DomainError>> + Send + '_>>
        {
            let url = content.url.as_str().to_string();
            let content = content.clone();
            let store = std::sync::Arc::clone(&self.store);
            Box::pin(async move {
                store.lock().unwrap().insert(url, content);
                Ok(())
            })
        }

        fn load_by_url(
            &self,
            url: &str,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<Option<ScrapedContent>, DomainError>>
                    + Send
                    + '_,
            >,
        > {
            let url = url.to_string();
            let store = std::sync::Arc::clone(&self.store);
            Box::pin(async move { Ok(store.lock().unwrap().get(&url).cloned()) })
        }
    }

    // --- Test: HttpClientPort trait is object-safe ---

    #[tokio::test]
    async fn test_http_client_port_object_safe() {
        let mock: Box<dyn HttpClientPort> = Box::new(MockHttpClient::new().with_response(
            "https://example.com",
            Ok(HttpResponse {
                status: 200,
                body: "<p>Hello</p>".into(),
                headers: HashMap::new(),
            }),
        ));

        let resp = mock.get("https://example.com").await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "<p>Hello</p>");
    }

    #[tokio::test]
    async fn test_http_client_port_error_propagation() {
        let mock: Box<dyn HttpClientPort> = Box::new(
            MockHttpClient::new()
                .with_response("https://fail.com", Err(HttpError::ClientError(500))),
        );

        let result = mock.get("https://fail.com").await;
        assert!(result.is_err());
    }

    // --- Test: PersistencePort trait is object-safe ---

    #[tokio::test]
    async fn test_persistence_port_round_trip() {
        let mock: Box<dyn PersistencePort> = Box::new(MockPersistencePort::new());
        let url = url::Url::parse("https://example.com").unwrap();
        let content = crate::domain::ScrapedContent {
            url: crate::domain::ValidUrl::new(url),
            title: "Test".into(),
            content: "Hello".into(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
        };

        mock.save(&content).await.unwrap();
        let loaded = mock.load_by_url("https://example.com/").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().title, "Test");
    }

    #[tokio::test]
    async fn test_persistence_port_missing_url_returns_none() {
        let mock: Box<dyn PersistencePort> = Box::new(MockPersistencePort::new());
        let result = mock.load_by_url("https://nonexistent.com").await.unwrap();
        assert!(result.is_none());
    }
}
