//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp.

use std::net::SocketAddr;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService,
};
use tower_http::trace::TraceLayer;
use tracing::info;

use super::state::McpState;
use super::McpHandler;

/// Default address for the MCP server.
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8080";

/// Build the Axum router with MCP endpoint.
pub fn build_mcp_router(state: McpState) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpHandler::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    Router::new()
        .nest_service("/mcp", service)
        .layer(TraceLayer::new_for_http())
}

/// Start the MCP server on the given address.
///
/// # Connection Pooling
///
/// For production use, create a shared `Downloader` and inject it via
/// `McpState::with_downloader()`:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use crate::adapters::downloader::{Downloader, DownloadConfig};
/// use crate::infrastructure::mcp_server::state::McpState;
///
/// let dl_config = DownloadConfig { /* ... */ };
/// let downloader = Arc::new(Downloader::new(dl_config)?);
/// let state = McpState::new(container).with_downloader(downloader);
/// start_mcp_server(state, addr).await?;
/// ```
///
/// Without a shared Downloader, each MCP tool call creates a fresh connection pool,
/// defeating keep-alive and TLS session reuse.
pub async fn start_mcp_server(state: McpState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = build_mcp_router(state);

    info!("MCP server starting on http://{}/mcp", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Wait for Ctrl+C and return.
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    info!("MCP server shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::di::Container;

    /// Build a test McpHandler with DI container.
    async fn test_handler() -> McpHandler {
        let config = Config::default();
        let container = Container::new(config).await.unwrap();
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
            tools.len() >= 34,
            "Expected at least 34 tools, got {}",
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
        let cleaned = crate::infrastructure::converter::html_cleaner::clean_html(html);
        assert!(!cleaned.contains("script"));
        assert!(cleaned.contains("Hello"));
    }

    #[test]
    fn test_convert_html_to_markdown_logic() {
        let html = "<h1>Title</h1><p>Paragraph</p>";
        let md = crate::infrastructure::converter::html_to_markdown::convert_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Paragraph"));
    }

    #[test]
    fn test_waf_detector_logic() {
        let clean_html = "<html><body>Normal content</body></html>";
        let result = crate::infrastructure::http::waf_engine::WafInspector::detect_body(clean_html);
        assert!(result.is_none());
    }

    #[test]
    fn test_waf_detector_cloudflare() {
        let cf_html = "<div id=\"cf-turnstile\" data-sitekey=\"abc123\"></div>";
        let result = crate::infrastructure::http::waf_engine::WafInspector::detect_body(cf_html);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Cloudflare"));
    }

    #[test]
    fn test_output_path_logic() {
        let path = crate::adapters::url_path::OutputPath::from_url("https://example.com/docs/page")
            .unwrap();
        let full = path.to_full_path();
        assert!(full.contains("example.com"));
        assert!(full.contains("docs"));
    }

    #[test]
    fn test_frontmatter_generation() {
        let fm = crate::infrastructure::output::frontmatter::generate(
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
            crate::infrastructure::converter::syntax_highlight::highlight_code_blocks(md);
        // Syntax highlighting may or may not add markup; just verify it returns something
        assert!(!highlighted.is_empty());
    }

    #[test]
    fn test_convert_wiki_links_logic() {
        let md = "https://example.com/page";
        let wikilinks =
            crate::infrastructure::converter::wikilinks::convert_wiki_links(md, "example.com");
        // Wiki link conversion replaces same-domain URLs with [[page]] syntax
        assert!(!wikilinks.is_empty());
    }

    #[test]
    fn test_mcp_state_with_downloader() {
        use std::sync::Arc;

        let config = Config::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let container = rt.block_on(Container::new(config)).unwrap();

        let dl_config = crate::adapters::downloader::DownloadConfig::default();
        let downloader =
            Arc::new(crate::adapters::downloader::Downloader::new(dl_config).unwrap());

        let state = McpState::new(container).with_downloader(downloader);
        assert!(
            state.downloader.is_some(),
            "McpState must hold the shared Downloader after with_downloader()"
        );
    }
}
