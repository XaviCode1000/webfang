//! Firecrawl adapter skeleton (slice 2): request-building/validation only.
//!
//! Endpoint layout per public API docs (`POST {base}/{version}/crawl`,
//! `Authorization: Bearer <FIRECRAWL_API_KEY>`). Pricing constants stay in
//! `crate::cost::config` (AC-4.2); this module owns interface facts only.
//!
//! Execution contract: [`run`] prepares the full request, enforces the gate,
//! and then returns a typed deferral error — NO HTTP client exists in this
//! build. When Tier B execution lands it MUST use wreq exclusively (C-3).

use super::{resolve_key_and_gate, CompetitorTarget, PreparedRequest, StartCrawlParams};
use crate::error::{BenchmarkError, Result};
use ::url::Url;

/// Documented public API origin.
pub const DEFAULT_API_BASE_URL: &str = "https://api.firecrawl.dev";
/// Current documented API version prefix.
pub const DEFAULT_API_VERSION_PREFIX: &str = "v2";

/// Firecrawl connection settings (interface facts, not pricing).
#[derive(Debug, Clone)]
pub struct FirecrawlConfig {
    /// API origin, e.g. `https://api.firecrawl.dev`.
    pub api_base_url: String,
    /// Version path segment appended before `/crawl`.
    pub api_version_prefix: String,
}

impl Default for FirecrawlConfig {
    fn default() -> Self {
        Self {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            api_version_prefix: DEFAULT_API_VERSION_PREFIX.to_string(),
        }
    }
}

impl FirecrawlConfig {
    /// Build and validate the start-crawl endpoint URL.
    ///
    /// # Errors
    ///
    /// [`BenchmarkError::Engine`] when the configured base URL is unparseable,
    /// has a non-HTTP(S) scheme, or rejects the versioned `/crawl` path.
    pub fn start_crawl_url(&self) -> Result<::url::Url> {
        let base = ::url::Url::parse(self.api_base_url.trim()).map_err(|error| {
            BenchmarkError::Engine(format!(
                "invalid firecrawl api base url `{}`: {error}",
                self.api_base_url
            ))
        })?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(BenchmarkError::Engine(format!(
                "firecrawl api base url must use http(s), got `{}`",
                base.scheme()
            )));
        }
        let path = format!(
            "/{}/crawl",
            self.api_version_prefix.trim_matches('/')
        );
        let mut url = base;
        url.set_path(&path);
        Ok(url)
    }
}

/// Build the fully-described start-crawl request (offline).
///
/// # Errors
///
/// - [`BenchmarkError::LiveDisabled`] unless a non-empty API key AND an
///   explicit opt-in flag are present;
/// - [`BenchmarkError::Engine`] for unparseable endpoint or target URLs.
pub fn prepare_start_crawl(
    config: &FirecrawlConfig,
    params: &StartCrawlParams,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<PreparedRequest> {
    let url = config.start_crawl_url()?;
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
    let bearer_token = resolve_key_and_gate(CompetitorTarget::Firecrawl, api_key, opt_in_flag)?;

    Ok(PreparedRequest {
        method: "POST",
        url,
        bearer_token,
        body_json: serde_json::json!({
            "url": target.as_str(),
            "limit": params.page_limit,
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
    config: &FirecrawlConfig,
    params: &StartCrawlParams,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<PreparedRequest> {
    let prepared = prepare_start_crawl(config, params, api_key, opt_in_flag)?;
    Err(BenchmarkError::Engine(format!(
        "firecrawl live execution is not wired in this build (Tier B execution lands in a later slice); \
         POST {} was fully prepared but no HTTP request was sent",
        prepared.url
    )))
}
