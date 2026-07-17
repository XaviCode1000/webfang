//! Repository interfaces for domain data persistence
//!
//! Defines contracts for storing and retrieving domain entities.
//! Infrastructure layer implements these traits.

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
        };
        let result = repo.save(&content);
        assert!(result.is_ok());
    }
}
