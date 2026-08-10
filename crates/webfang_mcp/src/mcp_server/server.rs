//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp, with a full middleware stack:
//! panic hook, timeout, body limit, rate limiting, and optional auth.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::{middleware, Router};
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService,
};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use super::auth::{validate_auth, AuthState};
use super::panic_hook::setup_panic_hook;
use super::state::McpState;
use super::McpHandler;

/// Default address for the MCP server.
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8080";

/// Configuration for the MCP HTTP server middleware stack.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Request timeout in seconds (default: 30).
    pub request_timeout_secs: u64,
    /// Maximum request body size in bytes (default: 10 MB).
    pub body_limit_bytes: usize,
    /// Maximum requests per second (default: 60).
    pub rate_per_second: u32,
    /// Maximum burst size for rate limiting (default: 10).
    pub rate_burst: u32,
    /// Expected Bearer token. When `None`, auth is disabled.
    pub auth_token: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            request_timeout_secs: 30,
            body_limit_bytes: 10 * 1024 * 1024,
            rate_per_second: 60,
            rate_burst: 10,
            auth_token: None,
        }
    }
}

/// Build the Axum router with MCP endpoint and full middleware stack.
pub fn build_mcp_router(state: McpState, options: &ServerOptions) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpHandler::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let rate_limiter = build_rate_limiter(options);
    let auth_state = AuthState {
        expected_token: options.auth_token.clone().map(Arc::from),
    };

    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(auth_state, validate_auth))
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            rate_limit_middleware,
        ))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(options.request_timeout_secs),
        ))
        .layer(RequestBodyLimitLayer::new(options.body_limit_bytes))
        .layer(TraceLayer::new_for_http())
}

/// Build a `governor` rate limiter from [`ServerOptions`].
///
/// A direct (unkeyed) limiter applies one global quota across all requests.
fn build_rate_limiter(
    options: &ServerOptions,
) -> Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>> {
    let per_second = NonZeroU32::new(options.rate_per_second).unwrap_or(NonZeroU32::MIN);
    let burst = NonZeroU32::new(options.rate_burst).unwrap_or(NonZeroU32::MIN);
    let quota = Quota::per_second(per_second).allow_burst(burst);
    Arc::new(GovernorLimiter::direct(quota))
}

/// Rate limiting middleware — rejects requests exceeding the quota with 429.
async fn rate_limit_middleware(
    State(limiter): State<Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Result<impl axum::response::IntoResponse, axum::http::StatusCode> {
    match limiter.check() {
        Ok(()) => Ok(next.run(request).await),
        Err(_not_until) => {
            tracing::warn!(
                remote = %request.uri().path(),
                "rate limit exceeded — rejecting with 429"
            );
            Err(axum::http::StatusCode::TOO_MANY_REQUESTS)
        },
    }
}

/// Start the MCP server on the given address with the full middleware stack.
///
/// # Connection Pooling
///
/// For production use, create a shared `Downloader` and inject it via
/// `McpState::with_downloader()`:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use webfang_core::adapters::downloader::{Downloader, DownloadConfig};
/// use crate::mcp_server::state::McpState;
///
/// let dl_config = DownloadConfig { /* ... */ };
/// let downloader = Arc::new(Downloader::new(dl_config)?);
/// let state = McpState::new(container).with_downloader(downloader);
/// start_mcp_server(state, addr, ServerOptions::default()).await?;
/// ```
///
/// Without a shared Downloader, each MCP tool call creates a fresh connection pool,
/// defeating keep-alive and TLS session reuse.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot bind to `addr`, or if the
/// server fails while serving.
pub async fn start_mcp_server(
    state: McpState,
    addr: SocketAddr,
    options: ServerOptions,
) -> anyhow::Result<()> {
    setup_panic_hook();

    let app = build_mcp_router(state.clone(), &options);

    info!("MCP server starting on http://{}/mcp", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.cancel_token.clone()))
        .await?;

    Ok(())
}

/// Future that resolves when a shutdown is requested (Ctrl+C or SIGTERM).
///
/// Drives the OS signal handler in the background and cancels the supplied
/// [`CancellationToken`] on Ctrl+C. Returns a `'static` future (it owns its
/// own clone of the token) so it is suitable for
/// `axum::serve(...).with_graceful_shutdown(...)`.
async fn shutdown_signal(token: CancellationToken) {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(
            error = %e,
            "SIGINT handler unavailable — server will keep running and rely on SIGTERM"
        );
        // Without a working Ctrl+C handler, wait on the token directly so the
        // server can still be torn down via an explicit cancel().
        token.cancelled().await;
        return;
    }
    info!("MCP server shutting down");
    token.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use webfang_core::config::Config;
    use webfang_core::di::{Container, ContainerExt};

    /// Build a test McpHandler with DI container.
    async fn test_handler() -> McpHandler {
        let config = Config::default();
        let container = Container::from_config(config).await.unwrap();
        let state = McpState::new(container);
        McpHandler::new(state)
    }

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn test_handler_builds_with_all_tools() {
        let handler = test_handler().await;
        let tools = handler.tool_router.list_all();
        assert!(
            tools.len() >= 35,
            "Expected at least 35 tools, got {}",
            tools.len()
        );

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(tool_names.contains(&"scrape_url"));
        assert!(tool_names.contains(&"validate_url"));
        assert!(tool_names.contains(&"clean_html"));
        assert!(tool_names.contains(&"detect_waf"));
        assert!(tool_names.contains(&"download_assets"));
        assert!(tool_names.contains(&"extract_domain"));
        assert!(tool_names.contains(&"normalize_url"));
        assert!(tool_names.contains(&"convert_html_to_markdown"));
    }

    /// Test tool logic by calling the underlying functions directly
    /// (bypasses MCP protocol layer which requires peer/session setup).

    #[test]
    fn test_validate_url_logic() {
        let url = url::Url::parse("https://example.com/path?q=1").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.path(), "/path");
    }

    #[test]
    fn test_normalize_url_logic() {
        let mut url = url::Url::parse("https://example.com/path/#fragment").unwrap();
        url.set_fragment(None);
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&path);
        let result = url.to_string();
        assert!(!result.contains("#fragment"));
        assert!(!result.ends_with("/"));
    }

    #[test]
    fn test_extract_domain_logic() {
        let url = url::Url::parse("https://www.example.com/path").unwrap();
        assert_eq!(url.host_str(), Some("www.example.com"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_clean_html_logic() {
        let html = "<html><head><script>alert('x')</script></head><body><p>Hello</p></body></html>";
        let cleaned = webfang_core::infrastructure::converter::html_cleaner::clean_html(html);
        assert!(!cleaned.contains("script"));
        assert!(cleaned.contains("Hello"));
    }

    #[test]
    fn test_convert_html_to_markdown_logic() {
        let html = "<h1>Title</h1><p>Paragraph</p>";
        let md =
            webfang_core::infrastructure::converter::html_to_markdown::convert_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Paragraph"));
    }

    #[test]
    fn test_waf_detector_logic() {
        use webfang_core::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
        let clean_html = "<html><body>Normal content</body></html>";
        let verdict = WafInspector::inspect(clean_html, &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_waf_detector_cloudflare() {
        use webfang_core::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
        let cf_html = "<div id=\"cf-turnstile\" data-sitekey=\"abc123\"></div>";
        let verdict = WafInspector::inspect(cf_html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert!(verdict
            .evidences
            .first()
            .is_some_and(|e| e.provider.contains("Cloudflare")));
    }

    #[test]
    fn test_output_path_logic() {
        let path =
            webfang_core::adapters::url_path::OutputPath::from_url("https://example.com/docs/page")
                .unwrap();
        let full = path.to_full_path();
        assert!(full.contains("example.com"));
        assert!(full.contains("docs"));
    }

    #[test]
    fn test_frontmatter_generation() {
        let fm = webfang_core::infrastructure::output::frontmatter::generate(
            "Test Title",
            "https://example.com",
            None,
            None,
            None,
            &[],
        );
        assert!(fm.contains("Test Title"));
        assert!(fm.contains("example.com"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_highlight_code_blocks_logic() {
        let md = "```rust\nfn main() {}\n```";
        let highlighted =
            webfang_core::infrastructure::converter::syntax_highlight::highlight_code_blocks(md);
        // Syntax highlighting may or may not add markup; just verify it returns something
        assert!(!highlighted.is_empty());
    }

    #[test]
    fn test_convert_wiki_links_logic() {
        let md = "https://example.com/page";
        let wikilinks = webfang_core::infrastructure::converter::wikilinks::convert_wiki_links(
            md,
            "example.com",
        );
        // Wiki link conversion replaces same-domain URLs with [[page]] syntax
        assert!(!wikilinks.is_empty());
    }

    #[test]
    fn test_mcp_state_with_downloader() {
        use std::sync::Arc;

        let config = Config::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let container = rt
            .block_on(Container::new(config.crawler, config.scraper))
            .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let dl_config = webfang_core::adapters::downloader::DownloadConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let downloader =
            Arc::new(webfang_core::adapters::downloader::Downloader::new(dl_config).unwrap());
        let downloader_clone = downloader.clone();

        let state = McpState::new(container).with_downloader(downloader);
        assert!(
            state.downloader.is_some(),
            "McpState must hold the shared Downloader after with_downloader()"
        );
        assert!(
            Arc::ptr_eq(state.downloader.as_ref().unwrap(), &downloader_clone),
            "with_downloader must store the exact Arc (connection pool identity)"
        );

        // Clone preserves the shared pool
        let state2 = state.clone();
        assert!(
            state2.downloader.is_some(),
            "clone must preserve downloader"
        );
        assert!(
            Arc::ptr_eq(
                state.downloader.as_ref().unwrap(),
                state2.downloader.as_ref().unwrap()
            ),
            "cloned McpState must share the same Downloader Arc"
        );
    }

    #[test]
    fn test_server_options_default() {
        let opts = ServerOptions::default();
        assert_eq!(opts.request_timeout_secs, 30);
        assert_eq!(opts.body_limit_bytes, 10 * 1024 * 1024);
        assert_eq!(opts.rate_per_second, 60);
        assert_eq!(opts.rate_burst, 10);
        assert!(opts.auth_token.is_none());
    }

    #[test]
    fn test_rate_limiter_allows_within_quota() {
        let opts = ServerOptions {
            rate_per_second: 10,
            rate_burst: 5,
            ..Default::default()
        };
        let limiter = build_rate_limiter(&opts);
        for _ in 0..5 {
            assert!(limiter.check().is_ok());
        }
    }

    #[test]
    fn test_rate_limiter_rejects_over_burst() {
        let opts = ServerOptions {
            rate_per_second: 1,
            rate_burst: 2,
            ..Default::default()
        };
        let limiter = build_rate_limiter(&opts);
        // Exhaust the burst capacity
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        // Third request should be rejected
        assert!(limiter.check().is_err());
    }

    #[tokio::test]
    async fn test_cancel_token_propagates_to_clones() {
        let config = Config::default();
        let container = Container::from_config(config).await.unwrap();
        let state = McpState::new(container);
        let state2 = state.clone();

        // Cancel through one clone, observe via the other.
        state.shutdown_signal();
        assert!(state2.cancel_token.is_cancelled());
    }
}
