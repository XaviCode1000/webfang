//! Repository interfaces for domain data persistence
//!
//! Defines contracts for storing and retrieving domain entities.
//! Infrastructure layer implements these traits.

use std::future::Future;
use std::pin::Pin;

use crate::domain::{CrawlError, ScrapedContent};

/// Repository interface for crawl results
///
/// Defines the contract for persisting and retrieving crawl data.
/// Implementations can use files, databases, or other storage backends.
pub trait CrawlResultRepository: Send + Sync {
    /// Save scraped content
    ///
    /// # Arguments
    ///
    /// * `content` - The scraped content to persist
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Success
    /// * `Err(CrawlError)` - Persistence error
    fn save(&self, content: &ScrapedContent) -> Result<(), CrawlError>;

    /// Find scraped content by URL
    ///
    /// # Arguments
    ///
    /// * `url` - The URL to search for
    ///
    /// # Returns
    ///
    /// * `Ok(Some(content))` - Found content
    /// * `Ok(None)` - Not found
    /// * `Err(CrawlError)` - Query error
    fn find_by_url(&self, url: &str) -> Result<Option<ScrapedContent>, CrawlError>;

    /// Get all crawled URLs
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - List of crawled URLs
    /// * `Err(CrawlError)` - Query error
    fn get_all_urls(&self) -> Result<Vec<String>, CrawlError>;

    /// Load all persisted content in bulk.
    ///
    /// Returns every saved [`ScrapedContent`] item. This is the bulk
    /// alternative to the `get_all_urls` → `find_by_url` loop, avoiding an
    /// N+1 query pattern for consumers that need the whole result set
    /// (e.g. the MCP export tools).
    ///
    /// The default implementation iterates `get_all_urls` and resolves each
    /// URL via `find_by_url`. Implementations with direct storage access
    /// SHOULD override this with a single sequential scan for efficiency.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<ScrapedContent>)` - All persisted content
    /// * `Err(CrawlError)` - Query error
    fn load_all(&self) -> Result<Vec<ScrapedContent>, CrawlError> {
        self.get_all_urls()?
            .into_iter()
            .filter_map(|url| self.find_by_url(&url).transpose())
            .collect()
    }

    /// Gracefully shut down any background persistence resources.
    ///
    /// Default: no-op — implementations without a background writer have
    /// nothing to drain. Implementations that buffer writes off-thread MUST
    /// close the send side, drain pending records, and join the writer so
    /// the durability claimed by `save` actually holds when the caller
    /// proceeds (#1121).
    ///
    /// Dyn-compatible `BoxFuture` shape follows `VectorRepository`: the
    /// join is awaited, never blocked on a worker thread.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - background writer drained and joined cleanly
    /// * `Err(CrawlError)` - the writer died or reported I/O errors
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = Result<(), CrawlError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::ValidUrl;
    use crate::domain::ScrapedContent;
    use url::Url;

    /// Minimal mock that implements the new save(&ScrapedContent) signature.
    /// This test verifies the trait accepts ScrapedContent, NOT CrawlResult.
    struct MockRepo;

    impl CrawlResultRepository for MockRepo {
        fn save(&self, _content: &ScrapedContent) -> Result<(), CrawlError> {
            Ok(())
        }

        fn find_by_url(&self, _url: &str) -> Result<Option<ScrapedContent>, CrawlError> {
            Ok(None)
        }

        fn get_all_urls(&self) -> Result<Vec<String>, CrawlError> {
            Ok(vec![])
        }
    }

    #[test]
    fn test_repository_trait_save_accepts_scraped_content() {
        let repo = MockRepo;
        let url = Url::parse("https://example.com").unwrap();
        let valid_url = ValidUrl::new(url);
        let content = ScrapedContent {
            url: valid_url,
            title: "Test".to_string(),
            content: "Hello".to_string(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };
        let result = repo.save(&content);
        assert!(result.is_ok());
    }
}
