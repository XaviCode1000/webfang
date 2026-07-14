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

use std::pin::Pin;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::http_client::{HttpError, HttpResponse};
    use std::collections::HashMap;

    // --- Mock implementations for testing port traits ---

    struct MockHttpClientPort {
        responses: HashMap<String, crate::application::http_client::HttpResult<HttpResponse>>,
    }

    impl MockHttpClientPort {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
            }
        }

        fn with_response(
            mut self,
            url: &str,
            result: crate::application::http_client::HttpResult<HttpResponse>,
        ) -> Self {
            self.responses.insert(url.to_string(), result);
            self
        }
    }

    impl HttpClientPort for MockHttpClientPort {
        fn get(
            &self,
            url: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::application::http_client::HttpResult<HttpResponse>,
                    > + Send
                    + '_,
            >,
        > {
            let result = self
                .responses
                .get(url)
                .cloned()
                .unwrap_or(Err(HttpError::ClientError(404)));
            Box::pin(async move { result })
        }
    }

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
        let mock: Box<dyn HttpClientPort> = Box::new(MockHttpClientPort::new().with_response(
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
            MockHttpClientPort::new()
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
