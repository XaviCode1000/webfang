//! Fetch router — strategy-based dispatch to the appropriate downloader.
//!
//! [`FetchRouter`] maps a [`JsStrategy`] to a concrete downloader stack and
//! implements [`Downloader`] so it can be passed as a trait object for runtime
//! dispatch and test mocking. [`build_fetch_router`] is the single source of
//! truth for the strategy → router mapping.

use std::sync::{Arc, RwLock};

use url::Url;
use wreq_util::Profile;

use futures::future::BoxFuture;

use crate::domain::JsStrategy;
use crate::infrastructure::downloader::chromiumoxide_downloader::ChromiumoxideDownloader;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::downloader::hybrid_router::HybridRouter;
use crate::infrastructure::downloader::obscura_downloader::ObscuraDownloader;
use crate::infrastructure::downloader::resource_governor::ResourceGovernor;
use crate::infrastructure::downloader::wreq_downloader::WreqDownloader;
use crate::infrastructure::downloader::{DownloadError, Downloader, FetchedPage};

/// Type-erased fetch router that dispatches to the appropriate downloader
/// based on the configured [`JsStrategy`].
///
/// Implements [`Downloader`] so it can be passed as `&dyn Downloader` for
/// runtime dispatch and test mocking. Inner types are `Arc`-wrapped so the
/// router can be cheaply cloned into spawned tasks.
#[derive(Clone)]
pub enum FetchRouter {
    /// Static HTTP only (wreq). Default.
    Static(Arc<WreqDownloader>),
    /// Hybrid 3-layer: wreq → Obscura → Chromiumoxide.
    Hybrid(Arc<HybridRouter<WreqDownloader, ObscuraDownloader, ChromiumoxideDownloader>>),
    /// Chrome-direct rendering with RAM-aware concurrency gating.
    ///
    /// Bypasses the wreq → Obscura escalation and renders every page in
    /// Chromiumoxide. A dedicated [`ResourceGovernor`] checks system memory
    /// before each fetch and holds a semaphore permit for its duration,
    /// preventing OOM on large crawls. Both fields are `Arc`-wrapped so the
    /// router stays cheaply cloneable into spawned tasks.
    Full(Arc<ChromiumoxideDownloader>, Arc<ResourceGovernor>),
}

/// Build the [`FetchRouter`] for a given JavaScript rendering strategy.
///
/// Single source of truth for the strategy → router mapping, shared by the
/// crawl [`Engine`](super::engine::Engine) and the CLI scrape path.
/// `timeout_secs` drives the wreq request timeout (connect timeout is clamped
/// to 10s); `tls_emulation` is the TLS/HTTP2 fingerprint profile applied to the
/// wreq layer; `cookie_bridge` is shared with the Chromiumoxide layer for
/// cookie injection; `ignore_waf` bypasses WAF classification on the hybrid
/// spa-detection path (REQ-WAF-07); `user_agent` pins the User-Agent on the
/// wreq layer (Static and Hybrid L1) so `--user-agent` reaches the wire —
/// `None` keeps the emulation-default + 403-rotation behavior (#503).
///
/// # Errors
///
/// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
pub fn build_fetch_router(
    strategy: &JsStrategy,
    timeout_secs: u64,
    tls_emulation: Profile,
    cookie_bridge: Arc<RwLock<CookieBridge>>,
    ignore_waf: bool,
    user_agent: Option<String>,
) -> Result<FetchRouter, DownloadError> {
    let connect_timeout = timeout_secs.min(10);
    Ok(match strategy {
        JsStrategy::Static => FetchRouter::Static(Arc::new(WreqDownloader::new(
            timeout_secs,
            connect_timeout,
            tls_emulation,
            user_agent,
        )?)),
        JsStrategy::Hybrid => {
            let l1 = WreqDownloader::new(timeout_secs, connect_timeout, tls_emulation, user_agent)?;
            let l2 = ObscuraDownloader::new(timeout_secs);
            let l3 = ChromiumoxideDownloader::new(cookie_bridge);
            FetchRouter::Hybrid(Arc::new(HybridRouter::new(l1, l2, l3, ignore_waf)))
        },
        // Full renders every page directly in Chrome (no wreq → Obscura
        // escalation) and gates concurrency on system RAM via its own
        // ResourceGovernor to prevent OOM on large crawls.
        JsStrategy::Full => {
            let dl = ChromiumoxideDownloader::new(cookie_bridge);
            let governor = ResourceGovernor::new();
            FetchRouter::Full(Arc::new(dl), Arc::new(governor))
        },
    })
}

impl Downloader for FetchRouter {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        match self {
            Self::Static(dl) => dl.fetch(url),
            Self::Hybrid(dl) => dl.fetch(url),
            Self::Full(dl, governor) => Box::pin(async move {
                governor.check_resources().map_err(DownloadError::from)?;
                let _permit = governor.acquire().await?;
                dl.fetch(url).await
            }),
        }
    }

    fn supports_interactions(&self) -> bool {
        match self {
            Self::Static(dl) => dl.supports_interactions(),
            Self::Hybrid(dl) => dl.supports_interactions(),
            Self::Full(dl, _) => dl.supports_interactions(),
        }
    }

    fn memory_cost(&self) -> usize {
        match self {
            Self::Static(dl) => dl.memory_cost(),
            Self::Hybrid(dl) => dl.memory_cost(),
            Self::Full(dl, _) => dl.memory_cost(),
        }
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod router_tests {
    use super::*;
    use crate::domain::JsStrategy;
    use std::sync::RwLock;

    fn test_cookie_bridge() -> Arc<RwLock<CookieBridge>> {
        Arc::new(RwLock::new(CookieBridge::new()))
    }

    #[test]
    fn build_fetch_router_static_returns_static_variant() {
        let router = build_fetch_router(
            &JsStrategy::Static,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
            None,
        )
        .expect("static router must build");
        assert!(
            matches!(router, FetchRouter::Static(_)),
            "Static strategy must produce FetchRouter::Static"
        );
    }

    #[test]
    fn build_fetch_router_hybrid_returns_hybrid_variant() {
        let router = build_fetch_router(
            &JsStrategy::Hybrid,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
            None,
        )
        .expect("hybrid router must build");
        assert!(
            matches!(router, FetchRouter::Hybrid(_)),
            "Hybrid strategy must produce FetchRouter::Hybrid"
        );
    }

    #[test]
    fn build_fetch_router_full_returns_full_variant() {
        let router = build_fetch_router(
            &JsStrategy::Full,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
            None,
        )
        .expect("full router must build");
        assert!(
            matches!(router, FetchRouter::Full(..)),
            "Full strategy must produce FetchRouter::Full (chrome-direct), not Hybrid"
        );
    }
}
