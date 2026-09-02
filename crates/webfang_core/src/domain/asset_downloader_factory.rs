//! Asset downloader factory — domain-owned seam for building the batch asset
//! downloader (ADR-0012-B cheap wins, issue #994; follow-up to 3.B-1b).
//!
//! # Why this port exists
//!
//! `application::asset_download::download_asset_urls` accepts an *optional*
//! shared [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort).
//! When the caller has one (the crawl path, the MCP server when a downloader
//! was injected), it is used directly. When it has none — `McpState::default()`
//! ships `downloader: None`, so this is a live production path, not only a test
//! convenience — the function had to build one itself, and the only way to do
//! that was to name `crate::adapters::downloader::Downloader` from
//! `application`. That is an outward `application -> adapters` edge, absorbed
//! for a long time by an allowlist entry.
//!
//! This port inverts it: `application` describes what it needs in terms of
//! [`ScraperConfig`](crate::domain::config::ScraperConfig) and asks an injected
//! factory for a ready-to-use trait object, while the concrete client stack
//! stays in `adapters::downloader`.
//!
//! # Why this is NOT [`DownloaderFactory`]
//!
//! The ADR-0012-B roadmap first recorded this site as "rewrite the call to
//! `DownloaderFactory::build`". That was wrong, and the two ports are not
//! interchangeable:
//!
//! - [`DownloaderFactory`] builds the **page-fetch** downloader
//!   ([`downloader_port::Downloader`](crate::domain::downloader_port::Downloader):
//!   `fetch`, `supports_interactions`, `memory_cost`). The asset path needs
//!   [`AssetDownloaderPort::download_batch`], which the fetch router does not
//!   implement.
//! - `DownloaderFactory::build` takes a run-scoped
//!   [`CookieBridge`](crate::domain::cookie_bridge::CookieBridge) and a
//!   [`CancellationToken`](tokio_util::sync::CancellationToken). Neither exists
//!   at this call site: `download_asset_urls` is a free function handed only a
//!   config.
//! - [`DownloaderSpec`](crate::domain::downloader_factory::DownloaderSpec) is
//!   fetch configuration (JS strategy, TLS profile). Asset configuration is a
//!   different shape entirely (output directory, include/exclude globs, naming
//!   strategy, cache bound).
//!
//! # Seams and bounds
//!
//! `build` is synchronous, matching the pre-existing `Downloader::new` contract:
//! the only fallible step is `wreq` client construction, which is sync. The
//! trait is object-safe (no generics, no `Self` in return) so `application` can
//! hold `Arc<dyn AssetDownloaderFactory>`. It is deliberately **not** sealed,
//! matching [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort)
//! itself, which is also unsealed so tests can implement it.
//!
//! No third-party type crosses into `domain` here: the signature speaks only
//! `ScraperConfig`, `Arc`, and the crate's own error and port types.

use std::fmt;
use std::sync::Arc;

use crate::domain::config::ScraperConfig;
use crate::domain::ports::AssetDownloaderPort;
use crate::error::Result;

/// Builds an asset downloader for a scrape configuration.
///
/// Implemented in `adapters::downloader` (see
/// [`DefaultAssetDownloaderFactory`]); consumed by `application` through
/// `Arc<dyn AssetDownloaderFactory>`.
pub trait AssetDownloaderFactory: Send + Sync {
    /// Build the batch asset downloader described by `config`.
    ///
    /// # Errors
    ///
    /// Propagates the underlying HTTP-client construction failure (invalid TLS
    /// profile, malformed header value) unchanged, so the caller's error text
    /// stays identical to the historical inline `Downloader::new` path.
    fn build(&self, config: &ScraperConfig) -> Result<Arc<dyn AssetDownloaderPort>>;
}

/// Manual `Debug` for the trait object so any `Debug`-deriving struct can hold
/// an `Arc<dyn AssetDownloaderFactory>`.
///
/// Same shape as `impl fmt::Debug for dyn DownloaderFactory` in
/// [`crate::domain::downloader_factory`]: a factory carries no observable
/// state, so there is nothing better to print.
impl fmt::Debug for dyn AssetDownloaderFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dyn AssetDownloaderFactory")
    }
}

/// Domain-owned default factory; its [`AssetDownloaderFactory`] impl lives in
/// `adapters::downloader`.
///
/// The type is declared here and the behaviour is supplied by the adapter
/// layer, which is the same split as
/// [`DefaultSsrfGuard`](crate::domain::ssrf_guard::DefaultSsrfGuard) (trait impl
/// in `infrastructure::ssrf`). Both are legal because they point inward:
/// `adapters -> domain` is the allowed direction, so `application` never has to
/// name an adapter concrete.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultAssetDownloaderFactory;

/// The process default factory.
///
/// `application` calls this instead of constructing an adapter type. A
/// composition root that wants a different implementation injects its own
/// `Arc<dyn AssetDownloaderFactory>` at the call site rather than arming a
/// global, which keeps this seam free of init-order coupling.
#[must_use]
pub fn default_factory() -> Arc<dyn AssetDownloaderFactory> {
    Arc::new(DefaultAssetDownloaderFactory)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile test for object safety: if `AssetDownloaderFactory` ever grows a
    /// generic method or returns `Self`, this `Arc<dyn ...>` stops type-checking
    /// and every consumer fails at once instead of at runtime.
    #[test]
    fn factory_port_is_object_safe() {
        let factory: Arc<dyn AssetDownloaderFactory> = default_factory();
        assert_eq!(format!("{factory:?}"), "dyn AssetDownloaderFactory");
    }

    /// The default factory must actually produce a working asset downloader from
    /// a plain config. This is the behaviour the `application` fallback used to
    /// get by naming the adapter concrete directly.
    #[test]
    fn default_factory_builds_an_asset_downloader() {
        let factory = DefaultAssetDownloaderFactory;
        let downloader: Arc<dyn AssetDownloaderPort> =
            factory.build(&ScraperConfig::default()).expect("build");
        // A batch of zero URLs is the deterministic assertion: it must return
        // an empty Ok without touching the network.
        let urls: Vec<String> = Vec::new();
        let assets = futures::executor::block_on(downloader.download_batch(&urls));
        assert!(assets.expect("empty batch must succeed").is_empty());
    }
}
