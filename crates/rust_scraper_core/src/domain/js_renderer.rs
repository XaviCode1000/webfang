//! JavaScript renderer trait — Forward-compatible stub for SPA support
//!
//! This trait defines the interface for JavaScript rendering of web pages.
//! Currently no implementation exists — this is a forward-compatible stub
//! for Phase 2 (full JS rendering with headless browser).
//!
//! # Architecture
//!
//! Following Clean Architecture: this trait lives in the Domain layer because
//! it defines a business capability (rendering JS-dependent pages), not a
//! specific implementation. Infrastructure will provide the actual renderer
//! (e.g., headless Chrome via `headless_chrome` or `fantoccini`).
//!
//! # Planned Implementation (v1.4)
//!
//! ```text
//! Domain:       JsRenderer trait (this file)
//! Infrastructure: HeadlessChromeRenderer (future)
//! Application:  ScraperService selects renderer based on content detection
//! ```
//!
//! See: <https://github.com/XaviCode1000/rust-scraper/issues/16>

use thiserror::Error;

/// Error type for JavaScript rendering failures.
///
/// Covers all expected failure modes for headless browser rendering:
/// - Browser launch/communication failures
/// - Page load timeouts
/// - Navigation errors
#[derive(Error, Debug)]
pub enum JsRenderError {
    /// Browser failed to launch or communicate
    #[error("browser error: {0}")]
    Browser(String),

    /// Page load timed out
    #[error("timeout loading {url} after {timeout_ms}ms")]
    Timeout {
        /// URL that timed out
        url: String,
        /// Timeout duration in milliseconds
        timeout_ms: u64,
    },

    /// Navigation to URL failed
    #[error("navigation failed: {0}")]
    Navigation(String),

    /// Content extraction after rendering failed
    #[error("content extraction failed: {0}")]
    Extraction(String),
}

/// Trait for JavaScript rendering of web pages.
///
/// Implementations use a headless browser to execute JavaScript and return
/// the fully rendered HTML. This is needed for Single Page Applications (SPAs)
/// that render content client-side.
///
/// Uses native async fn in trait (Rust 1.88+), no `async-trait` crate needed.
///
/// # Example (future implementation)
///
/// ```ignore
/// use rust_scraper::domain::JsRenderer;
/// use url::Url;
///
/// // Future: HeadlessChromeRenderer implements JsRenderer
/// let renderer = HeadlessChromeRenderer::new().await?;
/// let html = renderer.render(&Url::parse("https://example.com/spa")?).await?;
/// ```
pub trait JsRenderer: Send + Sync {
    /// Render a URL by executing JavaScript in a headless browser.
    ///
    /// # Arguments
    ///
    /// * `url` - The URL to render
    ///
    /// # Returns
    ///
    /// The fully rendered HTML content as a string.
    ///
    /// # Errors
    ///
    /// Returns `JsRenderError` if rendering fails for any reason.
    fn render(
        &self,
        url: &url::Url,
    ) -> impl std::future::Future<Output = Result<String, JsRenderError>> + Send;
}
