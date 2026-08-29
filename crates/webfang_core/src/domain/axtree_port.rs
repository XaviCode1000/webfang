//! AXTree port — domain trait for accessibility-tree snapshots.
//!
//! Owns the pure DTOs (`SnapshotFormat`, `CompactNode`, `CompactSnapshot`)
//! and the `AxTreePort` trait. The I/O implementations
//! (chromiumoxide CDP fetcher, playwright serializer) live in
//! `infrastructure::axtree` and impl this trait; `application::som_capture`
//! consumes the trait through container DI.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
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

/// Snapshot serialization formats (spec R3).
///
/// Only [`Compact`](Self::Compact) is implemented in this slice; `playwright-mcp`
/// returns an honest unsupported error (ai.rs precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SnapshotFormat {
    /// Interactive-only `@eN`-referenced node list with a `token_estimate`.
    #[default]
    Compact,
    /// Playwright MCP AXSnapshot format — deferred to a follow-up change.
    PlaywrightMcp,
}

/// A single compact node: `@eN` ref, accessible name, and role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactNode {
    /// Snapshot-scoped reference (`@e1`, `@e2`, …) — valid ONLY within the
    /// snapshot that created it (RDD causal invariant).
    #[serde(rename = "ref")]
    pub r#ref: String,
    /// Accessible name.
    pub name: String,
    /// Accessible role.
    pub role: String,
}

/// Compact accessibility snapshot — interactive nodes plus a token estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactSnapshot {
    /// Emitted nodes, each with a snapshot-scoped `@eN` ref.
    pub nodes: Vec<CompactNode>,
    /// `Σ(2 + name_chars/4 + role_chars/4)` over the emitted nodes.
    pub token_estimate: usize,
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
