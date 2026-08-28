//! AXTree port — domain trait for accessibility-tree snapshots.

use std::future::Future;
use std::pin::Pin;

use url::Url;

use crate::error::ScraperError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Domain port for fetching raw accessibility trees.
///
/// `#[cfg(feature = "chromium")]` impl spawns headless Chromium via CDP;
/// non-chromium stub returns an error.
pub trait AxTreePort: Send + Sync {
    /// Fetch the raw AXTree JSON for a URL.
    fn fetch_raw_axtree<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<String, ScraperError>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ScraperError;
    use std::sync::Arc;
    use url::Url;

    struct FakeAx;

    impl AxTreePort for FakeAx {
        fn fetch_raw_axtree<'a>(
            &'a self,
            url: &'a Url,
        ) -> BoxFuture<'a, Result<String, ScraperError>> {
            let u = url.clone();
            Box::pin(async move { Ok(format!("axtree:{u}")) })
        }
    }

    #[tokio::test]
    async fn fetch_raw_axtree_returns_string() {
        let port = FakeAx;
        let url = Url::parse("https://example.com").unwrap();
        let out = port.fetch_raw_axtree(&url).await.unwrap();
        assert!(out.contains("example.com"));

        let url2 = Url::parse("https://rust-lang.org").unwrap();
        let out2 = port.fetch_raw_axtree(&url2).await.unwrap();
        assert!(out2.contains("rust-lang.org"));
    }

    #[test]
    fn axtree_port_is_object_safe() {
        fn assert_dyn(_: &dyn AxTreePort) {}
        let p = FakeAx;
        assert_dyn(&p);
        let _: Arc<dyn AxTreePort> = Arc::new(FakeAx);
    }
}
