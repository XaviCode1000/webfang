//! Three-layer hybrid downloader with SPA-aware escalation.
//!
//! Fetch strategy:
//!
//! 1. **Layer 1 — wreq**: fast static HTTP fetch
//! 2. **SPA detection**: analyse the HTML for SPA mount points / WAF markers
//! 3. **Layer 2 — Obscura**: subprocess markdown extraction (if SPA detected)
//! 4. **Layer 3 — Chromiumoxide**: full CDP rendering (if Obscura insufficient)
//!
//! WAF detection at **any** layer short-circuits with an error — escalation is
//! never attempted against a WAF challenge.
//!
//! Uses generics over the `Downloader` trait because native `async fn` in traits
//! is not dyn-compatible. Each layer is a separate type parameter.

use tracing::{debug, instrument, warn};
use url::Url;

use super::resource_governor::ResourceGovernor;
use super::spa_detector::{detect_spa, SpaSignal};
use super::{DownloadError, Downloader, FetchedPage};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    DOWNLOADER_ESCALATIONS, DOWNLOADER_LAYER_LATENCY, DOWNLOADER_WAF_BLOCKS,
};
#[cfg(feature = "otel-metrics")]
use std::time::Instant;

/// Three-layer hybrid downloader.
///
/// Type parameters correspond to the three fetch layers:
/// - `L1`: static HTTP (typically [`WreqDownloader`](super::wreq_downloader::WreqDownloader))
/// - `L2`: subprocess fallback (typically [`ObscuraDownloader`](super::obscura_downloader::ObscuraDownloader))
/// - `L3`: headless browser (typically [`ChromiumoxideDownloader`](super::chromiumoxide_downloader::ChromiumoxideDownloader))
pub struct HybridRouter<L1: Downloader, L2: Downloader, L3: Downloader> {
    layer1: L1,
    layer2: L2,
    layer3: L3,
    governor: ResourceGovernor,
}

impl<L1: Downloader, L2: Downloader, L3: Downloader> HybridRouter<L1, L2, L3> {
    pub fn new(layer1: L1, layer2: L2, layer3: L3) -> Self {
        Self {
            layer1,
            layer2,
            layer3,
            governor: ResourceGovernor::new(),
        }
    }

    /// Inspect a [`FetchedPage`] and decide whether escalation is needed.
    ///
    /// Returns:
    /// - `Ok(page)` if the page has usable static content
    /// - `Err(DownloadError)` for WAF or unrecoverable errors
    /// - `None` when SPA detected (caller should try next layer)
    fn evaluate_fetch(&self, page: FetchedPage) -> Result<Option<FetchedPage>, DownloadError> {
        let signal = detect_spa(&page.html);

        match signal {
            SpaSignal::StaticContent => {
                debug!("SPA check: static content — no escalation needed");
                Ok(Some(page))
            },
            SpaSignal::WafBlocked => {
                warn!("WAF detected at fetch time — aborting escalation");
                Err(DownloadError::WafChallenge(
                    "WAF challenge detected in response".to_string(),
                ))
            },
            SpaSignal::SpaDetected(reason) => {
                debug!("SPA detected ({reason:?}) — escalation warranted");
                Ok(None)
            },
        }
    }
}

impl<L1: Downloader, L2: Downloader, L3: Downloader> Downloader for HybridRouter<L1, L2, L3> {
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        // --- Layer 1: fast static HTTP ---
        debug!("Layer 1 (wreq): fetching {url}");
        #[cfg(feature = "otel-metrics")]
        let l1_start = Instant::now();
        let page = match self.layer1.fetch(url).await {
            Ok(p) => p,
            Err(DownloadError::WafChallenge(msg)) => {
                #[cfg(feature = "otel-metrics")]
                {
                    DOWNLOADER_WAF_BLOCKS.add(1, &[opentelemetry::KeyValue::new("layer", "1")]);
                    DOWNLOADER_LAYER_LATENCY.record(
                        l1_start.elapsed().as_secs_f64(),
                        &[opentelemetry::KeyValue::new("layer", "1")],
                    );
                }
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                debug!("Layer 1 failed ({e}) — aborting");
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l1_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "1")],
                );
                return Err(e);
            },
        };

        #[cfg(feature = "otel-metrics")]
        DOWNLOADER_LAYER_LATENCY.record(
            l1_start.elapsed().as_secs_f64(),
            &[opentelemetry::KeyValue::new("layer", "1")],
        );

        // SPA detected — continue escalation; static content — return early
        if let Some(page) = self.evaluate_fetch(page)? {
            return Ok(page);
        }

        // --- Layer 2: Obscura subprocess ---
        debug!("Layer 2 (Obscura): attempting fetch for {url}");
        #[cfg(feature = "otel-metrics")]
        DOWNLOADER_ESCALATIONS.add(
            1,
            &[
                opentelemetry::KeyValue::new("from", "1"),
                opentelemetry::KeyValue::new("to", "2"),
            ],
        );

        // Check resources before spawning a subprocess
        if let Err(e) = self.governor.check_resources() {
            warn!("ResourceGovernor denied Layer 2: {e}");
            return Err(DownloadError::Internal(format!(
                "resource governor denied obscura: {e}"
            )));
        }

        #[cfg(feature = "otel-metrics")]
        let l2_start = Instant::now();
        match self.layer2.fetch(url).await {
            Ok(page) if !page.html.is_empty() => {
                debug!("Layer 2 returned {} bytes", page.html.len());
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l2_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "2")],
                );
                return Ok(page);
            },
            Ok(_) => {
                debug!("Layer 2 returned empty content — will try Layer 3");
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l2_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "2")],
                );
            },
            Err(DownloadError::WafChallenge(msg)) => {
                #[cfg(feature = "otel-metrics")]
                {
                    DOWNLOADER_WAF_BLOCKS.add(1, &[opentelemetry::KeyValue::new("layer", "2")]);
                    DOWNLOADER_LAYER_LATENCY.record(
                        l2_start.elapsed().as_secs_f64(),
                        &[opentelemetry::KeyValue::new("layer", "2")],
                    );
                }
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                debug!("Layer 2 failed ({e}) — will try Layer 3");
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l2_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "2")],
                );
            },
        }

        // --- Layer 3: Chromiumoxide CDP ---
        debug!("Layer 3 (Chromiumoxide): attempting fetch for {url}");
        #[cfg(feature = "otel-metrics")]
        DOWNLOADER_ESCALATIONS.add(
            1,
            &[
                opentelemetry::KeyValue::new("from", "2"),
                opentelemetry::KeyValue::new("to", "3"),
            ],
        );

        if let Err(e) = self.governor.check_resources() {
            warn!("ResourceGovernor denied Layer 3: {e}");
            return Err(DownloadError::Internal(format!(
                "resource governor denied chromiumoxide: {e}"
            )));
        }

        #[cfg(feature = "otel-metrics")]
        let l3_start = Instant::now();
        match self.layer3.fetch(url).await {
            Ok(page) => {
                debug!("Layer 3 returned {} bytes", page.html.len());
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l3_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "3")],
                );
                return Ok(page);
            },
            Err(DownloadError::WafChallenge(msg)) => {
                #[cfg(feature = "otel-metrics")]
                {
                    DOWNLOADER_WAF_BLOCKS.add(1, &[opentelemetry::KeyValue::new("layer", "3")]);
                    DOWNLOADER_LAYER_LATENCY.record(
                        l3_start.elapsed().as_secs_f64(),
                        &[opentelemetry::KeyValue::new("layer", "3")],
                    );
                }
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                warn!("All layers exhausted for {url}: {e}");
                #[cfg(feature = "otel-metrics")]
                DOWNLOADER_LAYER_LATENCY.record(
                    l3_start.elapsed().as_secs_f64(),
                    &[opentelemetry::KeyValue::new("layer", "3")],
                );
                return Err(e);
            },
        }
    }

    fn supports_interactions(&self) -> bool {
        self.layer3.supports_interactions()
    }

    fn memory_cost(&self) -> usize {
        self.layer1.memory_cost() + self.layer2.memory_cost() + self.layer3.memory_cost()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test doubles --------------------------------------------------

    struct StubDownloader {
        html: String,
        cost: usize,
        interactions: bool,
    }

    impl StubDownloader {
        fn static_page() -> Self {
            Self {
                html: "<html><body><article><h1>Hello</h1><p>Enough content here to pass the threshold check and avoid SPA detection.</p></article></body></html>".into(),
                cost: 1_000_000,
                interactions: false,
            }
        }

        fn spa_page() -> Self {
            Self {
                html: r#"<!DOCTYPE html><html><body><div id="root"></div></body></html>"#.into(),
                cost: 1_000_000,
                interactions: false,
            }
        }

        fn empty_page() -> Self {
            Self {
                html: String::new(),
                cost: 1_000_000,
                interactions: false,
            }
        }

        fn waf_page() -> Self {
            Self {
                html: r#"<!DOCTYPE html><html><body><div id="challenge-running">Checking your browser</div></body></html>"#.into(),
                cost: 1_000_000,
                interactions: false,
            }
        }

        fn with_cost(mut self, cost: usize) -> Self {
            self.cost = cost;
            self
        }

        fn with_interactions(mut self, v: bool) -> Self {
            self.interactions = v;
            self
        }
    }

    impl Downloader for StubDownloader {
        async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
            Ok(FetchedPage {
                url: url.clone(),
                html: self.html.clone(),
                status: 200,
                cookies: vec![],
            })
        }

        fn supports_interactions(&self) -> bool {
            self.interactions
        }

        fn memory_cost(&self) -> usize {
            self.cost
        }
    }

    struct FailingDownloader {
        message: String,
    }

    impl Downloader for FailingDownloader {
        async fn fetch(&self, _url: &Url) -> Result<FetchedPage, DownloadError> {
            Err(DownloadError::Network(Box::new(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                self.message.clone(),
            ))))
        }

        fn supports_interactions(&self) -> bool {
            false
        }

        fn memory_cost(&self) -> usize {
            0
        }
    }

    // ---- Tests ---------------------------------------------------------

    #[tokio::test]
    async fn test_layer1_sufficient_no_escalation() {
        let router = HybridRouter::new(
            StubDownloader::static_page(),
            StubDownloader::spa_page(),
            StubDownloader::static_page().with_interactions(true),
        );
        let url: Url = "https://example.com".parse().unwrap();
        let page = router.fetch(&url).await.unwrap();
        assert!(page.html.contains("Hello"));
    }

    #[tokio::test]
    async fn test_spa_detected_escalates_to_layer2() {
        let router = HybridRouter::new(
            StubDownloader::spa_page(),
            StubDownloader::static_page().with_cost(30_000_000),
            StubDownloader::static_page().with_interactions(true),
        );
        let url: Url = "https://spa.example.com".parse().unwrap();
        let page = router.fetch(&url).await.unwrap();
        assert!(page.html.contains("Enough content"));
    }

    #[tokio::test]
    async fn test_waf_at_layer1_aborts() {
        let router = HybridRouter::new(
            StubDownloader::waf_page(),
            StubDownloader::static_page(),
            StubDownloader::static_page(),
        );
        let url: Url = "https://waf.example.com".parse().unwrap();
        let err = router.fetch(&url).await.unwrap_err();
        assert!(matches!(err, DownloadError::WafChallenge(_)));
    }

    #[tokio::test]
    async fn test_layer2_empty_escalates_to_layer3() {
        let router = HybridRouter::new(
            StubDownloader::spa_page(),
            StubDownloader::empty_page(),
            StubDownloader::static_page().with_interactions(true),
        );
        let url: Url = "https://spa.example.com".parse().unwrap();
        let page = router.fetch(&url).await.unwrap();
        assert!(page.html.contains("Enough content"));
    }

    #[tokio::test]
    async fn test_layer1_failure_propagates() {
        let router = HybridRouter::new(
            FailingDownloader {
                message: "dns failed".into(),
            },
            StubDownloader::static_page(),
            StubDownloader::static_page(),
        );
        let url: Url = "https://down.example.com".parse().unwrap();
        let err = router.fetch(&url).await.unwrap_err();
        assert!(matches!(err, DownloadError::Network(_)));
    }

    #[test]
    fn test_hybrid_router_memory_cost_sums() {
        let router = HybridRouter::new(
            StubDownloader::static_page().with_cost(1_000_000),
            StubDownloader::static_page().with_cost(30_000_000),
            StubDownloader::static_page()
                .with_cost(200_000_000)
                .with_interactions(true),
        );
        assert_eq!(router.memory_cost(), 231_000_000);
    }

    #[test]
    fn test_hybrid_router_supports_interactions_from_layer3() {
        let router = HybridRouter::new(
            StubDownloader::static_page(),
            StubDownloader::static_page(),
            StubDownloader::static_page().with_interactions(true),
        );
        assert!(router.supports_interactions());

        let router = HybridRouter::new(
            StubDownloader::static_page(),
            StubDownloader::static_page(),
            StubDownloader::static_page(),
        );
        assert!(!router.supports_interactions());
    }
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    #[test]
    fn test_hybrid_router_instruments_init() {
        let _ = &*super::DOWNLOADER_ESCALATIONS;
        let _ = &*super::DOWNLOADER_LAYER_LATENCY;
        let _ = &*super::DOWNLOADER_WAF_BLOCKS;
    }
}
