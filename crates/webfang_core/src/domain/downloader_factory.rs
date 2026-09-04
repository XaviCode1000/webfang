//! Downloader factory port — domain-owned seam for building fetch downloaders.
//!
//! ADR-0012 sub-slice 3.B-1b (issue #994). The crawl [`Engine`] used to call the
//! infrastructure free function `build_fetch_router` directly, which forced
//! `application → infrastructure` (absorbed by an allowlist entry). This port
//! inverts that dependency: `application` describes *what* it needs with a
//! [`DownloaderSpec`](crate::domain::downloader_factory::DownloaderSpec) and asks
//! an injected
//! [`DownloaderFactory`](crate::domain::downloader_factory::DownloaderFactory) to
//! build it, while the concrete downloader stack stays in
//! `infrastructure::downloader`.
//!
//! [`Engine`]: crate::application::crawler::engine::Engine
//!
//! # What is a spec field and what is a `build()` parameter
//!
//! [`DownloaderSpec`](crate::domain::downloader_factory::DownloaderSpec) carries
//! *configuration only*. Two collaborators are deliberately
//! [`build`](crate::domain::downloader_factory::DownloaderFactory::build)
//! parameters instead:
//!
//! - `cookie_bridge` is shared mutable state that must outlive the downloader it
//!   is injected into — the engine keeps writing fetched cookies into the same
//!   bridge after the build (see `ingest_cookies` in the crawl task).
//! - `cancel_token` is per-run lifecycle, not configuration: it is created with
//!   the engine and fired on shutdown, so it cannot be captured in a spec that a
//!   caller might reuse across runs.
//!
//! # Third-party types in `domain/` — accepted deliberately
//!
//! This module puts two third-party types into the domain layer:
//! [`wreq::cookie::Jar`] (as `DownloaderSpec::initial_cookie_jar`) and
//! [`tokio_util::sync::CancellationToken`] (as a `build()` parameter). They are
//! the first two *non-`wreq-util`* external types to appear in `domain/` by
//! design (`wreq_util::Profile` already crossed this line via
//! `crate::domain::http_config::HttpClientConfig` and is reused here).
//!
//! This is a known, accepted leak, not an oversight: the ADR-0010 intra-crate
//! direction gate (`scripts/check_intra_crate_direction.sh`) only inspects
//! `crate::<layer>::…` paths, so it cannot see third-party leakage at all. A
//! future `domain`-owned cookie-jar or cancellation newtype would close the gap;
//! until then, do not read a green gate as "the domain layer is framework-free".

use std::fmt;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use wreq::cookie::Jar;
use wreq_util::Profile;

use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::downloader_port::{DownloadError, Downloader};
use crate::domain::JsStrategy;

/// Default binary name for the Hybrid Layer 2 (Obscura) downloader.
///
/// A bare name is resolved from `PATH` at spawn time; a path is invoked as
/// given (#787). The literal lives in `domain` — not in
/// `infrastructure::downloader::obscura_downloader` — because the engine's
/// options default needs it and `application` must not name an infrastructure
/// downloader item. The infrastructure module keeps a `pub use` of this
/// constant so its historical public path still resolves.
pub const DEFAULT_OBSCURA_BINARY: &str = "obscura";

/// Domain-shaped inputs for building a fetch downloader.
///
/// Pure configuration — see the [module docs](self) for why the cookie bridge
/// and the cancellation token are
/// [`build`](crate::domain::downloader_factory::DownloaderFactory::build)
/// parameters instead of fields here.
///
/// `obscura_binary` is an owned `String` on purpose: a borrowed `&str` field
/// would force a lifetime parameter onto [`DownloaderSpec`], and a
/// lifetime-carrying argument makes [`DownloaderFactory::build`] non-
/// dyn-compatible, which is the whole point of the port.
#[derive(Debug, Clone)]
pub struct DownloaderSpec {
    /// Rendering strategy selecting the downloader stack.
    pub strategy: JsStrategy,
    /// Request timeout in seconds (connect timeout is clamped to 10s).
    pub timeout_secs: u64,
    /// TLS/HTTP2 fingerprint profile applied to the wreq layer.
    pub tls_emulation: Profile,
    /// Bypass WAF classification on the hybrid spa-detection path (REQ-WAF-07).
    pub ignore_waf: bool,
    /// Optional pinned User-Agent for the wreq layer (#503). `None` keeps the
    /// emulation-default UA plus 403 pool rotation.
    pub user_agent: Option<String>,
    /// Operator headers (`--header`, #890) applied to the wreq layer after the
    /// emulation profile so user values win.
    pub custom_headers: Vec<(String, String)>,
    /// Optional Accept-Language override (`--accept-language`, #890).
    pub accept_language: Option<String>,
    /// Pre-seeded wreq cookie store (`--cookie`, #890) shared by the Static and
    /// Hybrid-L1 layers.
    pub initial_cookie_jar: Option<Arc<Jar>>,
    /// Maximum number of retry attempts for failed fetches.
    pub max_retries: u32,
    /// Base delay for exponential backoff (ms).
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (ms).
    pub backoff_max_ms: u64,
    /// Hybrid Layer 2 (Obscura) binary name or path (#787).
    pub obscura_binary: String,
}

/// Builds the downloader for a crawl strategy.
///
/// Implemented in `infrastructure::downloader` (see
/// [`DefaultDownloaderFactory`](crate::infrastructure::downloader::fetch_router::DefaultDownloaderFactory));
/// consumed by `application` through `Arc<dyn DownloaderFactory>`. Keeping
/// `build` synchronous matches the pre-existing `build_fetch_router` contract:
/// the only fallible step is wreq client construction, which is sync.
pub trait DownloaderFactory: Send + Sync {
    /// Build the downloader for `spec`.
    ///
    /// `cookie_bridge` is the run's shared bridge, injected into the
    /// Chromiumoxide layer so CDP sessions receive crawled cookies. It uses
    /// `tokio::sync::RwLock` (#1119): the lock is acquired with `.await`
    /// inside async fetch paths, so a contended bridge yields the worker
    /// instead of parking an executor thread, and the async lock cannot be
    /// poisoned — no panic path inside the downloader future.
    /// `cancel_token` is the run's shutdown token, injected into the Full
    /// strategy's resource governor so permit waits abort on shutdown (#509).
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] when the underlying HTTP client
    /// cannot be constructed (invalid TLS profile, malformed header value).
    fn build(
        &self,
        spec: &DownloaderSpec,
        cookie_bridge: Arc<RwLock<CookieBridge>>,
        cancel_token: CancellationToken,
    ) -> Result<Arc<dyn Downloader>, DownloadError>;
}

/// Manual `Debug` for the trait object so `EngineOptions` (which derives
/// `Debug`) can hold an `Arc<dyn DownloaderFactory>`.
///
/// Same shape as the existing `impl fmt::Display for dyn ContentProcessor` in
/// [`crate::domain::content_processor`]: a factory carries no observable state,
/// so there is nothing better to print.
impl fmt::Debug for dyn DownloaderFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dyn DownloaderFactory")
    }
}
