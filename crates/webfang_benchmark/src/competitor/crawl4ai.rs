//! Crawl4AI adapter skeleton (slice 2): request-building/validation only.
//!
//! Endpoint layout per documented self-host server (`POST {base}/crawl`,
//! optional gateway bearer token from `CRAWL4AI_API_KEY`). Sizing/pricing
//! constants stay in `crate::cost::config` (AC-4.2).
//!
//! Execution contract: [`run`] prepares the full request, enforces the gate,
//! and then returns a typed deferral error — NO HTTP client exists in this
//! build. When Tier B execution lands it MUST use wreq exclusively (C-3).

use super::{resolve_key_and_gate, CompetitorTarget, PreparedRequest, StartCrawlParams};
use crate::error::{BenchmarkError, Result};

/// Documented default port of the local Crawl4AI docker server.
pub const DEFAULT_SERVER_BASE_URL: &str = "http://127.0.0.1:11235";

/// Crawl4AI self-host connection settings.
#[derive(Debug, Clone)]
pub struct Crawl4AiConfig {
    /// Server origin, e.g. `http://127.0.0.1:11235`.
    pub server_base_url: String,
}

impl Default for Crawl4AiConfig {
    fn default() -> Self {
        Self {
            server_base_url: DEFAULT_SERVER_BASE_URL.to_string(),
        }
    }
}

impl Crawl4AiConfig {
    /// Build and validate the crawl endpoint URL.
    ///
    /// # Errors
    ///
    /// [`BenchmarkError::Engine`] when the configured base URL is unparseable,
    /// has a non-HTTP(S) scheme, or rejects the `/crawl` path.
    pub fn crawl_url(&self) -> Result<::url::Url> {
        let base = ::url::Url::parse(self.server_base_url.trim()).map_err(|error| {
            BenchmarkError::Engine(format!(
                "invalid crawl4ai server base url `{}`: {error}",
                self.server_base_url
            ))
        })?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(BenchmarkError::Engine(format!(
                "crawl4ai server base url must use http(s), got `{}`",
                base.scheme()
            )));
        }
        base.join("/crawl").map_err(|error| {
            BenchmarkError::Engine(format!(
                "cannot join crawl4ai /crawl path onto `{}`: {error}",
                self.server_base_url
            ))
        })
    }
}

/// Build the fully-described crawl request (offline).
///
/// # Errors
///
/// - [`BenchmarkError::LiveDisabled`] unless a non-empty API key AND an
///   explicit opt-in flag are present;
/// - [`BenchmarkError::Engine`] for unparseable endpoint or target URLs.
pub fn prepare_crawl(
    config: &Crawl4AiConfig,
    params: &StartCrawlParams,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<PreparedRequest> {
    let url = config.crawl_url()?;
    let target = ::url::Url::parse(params.target_url.trim()).map_err(|error| {
        BenchmarkError::Engine(format!(
            "invalid crawl target url `{}`: {error}",
            params.target_url
        ))
    })?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err(BenchmarkError::Engine(format!(
            "crawl target url must use http(s), got `{}`",
            target.scheme()
        )));
    }
    let bearer_token = resolve_key_and_gate(CompetitorTarget::Crawl4Ai, api_key, opt_in_flag)?;

    Ok(PreparedRequest {
        method: "POST",
        url,
        bearer_token,
        body_json: serde_json::json!({
            "urls": [target.as_str()],
        }),
    })
}

/// Run-style entry point mirroring the future Tier B execution signature.
///
/// This build performs ZERO network I/O: after full validation and the
/// fail-closed gate it returns a typed deferral error instead of executing.
/// The future implementation will construct a wreq client here (C-3).
///
/// # Errors
///
/// - [`BenchmarkError::LiveDisabled`] when the gate refuses;
/// - [`BenchmarkError::Engine`] for invalid URLs and for the (current)
///   unconditional execution deferral.
pub async fn run(
    config: &Crawl4AiConfig,
    params: &StartCrawlParams,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<PreparedRequest> {
    let prepared = prepare_crawl(config, params, api_key, opt_in_flag)?;
    Err(BenchmarkError::Engine(format!(
        "crawl4ai live execution is not wired in this build (Tier B execution lands in a later slice); \
         POST {} was fully prepared but no HTTP request was sent",
        prepared.url
    )))
}
