//! MCP protocol integration tests.
//!
//! Tests handler construction, state initialization, semaphore backpressure,
//! and parameter deserialization without requiring a running server.

use webfang_core::application::container::Container;
use webfang_core::domain::CrawlerConfig;
use webfang_core::infrastructure::config::ScraperConfig;
use webfang_mcp::mcp_server::state::{CategoryLimits, CategorySemaphores, McpState};
use webfang_mcp::mcp_server::McpHandler;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_container() -> Container {
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://example.com").expect("valid seed URL"));
    Container::new(crawler_config, ScraperConfig::default())
        .await
        .expect("Container::new should succeed in tests")
}

// ===========================================================================
// McpState Construction Tests
// ===========================================================================

#[tokio::test]
async fn mcp_state_construction_with_default_limits() {
    let container = make_container().await;
    let state = McpState::new(container);

    assert!(state.limits.ai >= 1);
    assert!(state.limits.scraping >= 1);
}

#[tokio::test]
async fn mcp_state_construction_with_custom_limits() {
    let container = make_container().await;
    let limits = CategoryLimits {
        ai: 1,
        scraping: 4,
        export: 2,
        obsidian: 1,
        content: 3,
        url_utils: 8,
        security: 4,
        assets: 2,
    };
    let state = McpState::with_limits(container, limits);

    assert_eq!(state.limits.ai, 1);
    assert_eq!(state.limits.scraping, 4);
}

#[tokio::test]
async fn mcp_state_clone_shares_arc_internals() {
    let container = make_container().await;
    let state = McpState::new(container);
    let cloned = state.clone();

    assert_eq!(cloned.limits.ai, state.limits.ai);
}

// ===========================================================================
// CategoryLimits Tests
// ===========================================================================

#[test]
fn default_limits_have_reasonable_values() {
    let limits = CategoryLimits::default();

    assert!(limits.ai <= limits.scraping, "AI should be <= scraping");
    assert!(limits.ai <= limits.url_utils, "AI should be <= url_utils");

    assert!(limits.ai >= 1);
    assert!(limits.scraping >= 1);
    assert!(limits.export >= 1);
    assert!(limits.obsidian >= 1);
    assert!(limits.content >= 1);
    assert!(limits.url_utils >= 1);
    assert!(limits.security >= 1);
    assert!(limits.assets >= 1);
}

#[test]
fn category_semaphores_clamp_zero_to_one() {
    let limits = CategoryLimits {
        ai: 0,
        scraping: 0,
        export: 0,
        obsidian: 0,
        content: 0,
        url_utils: 0,
        security: 0,
        assets: 0,
    };
    let semaphores = CategorySemaphores::from_limits(&limits);

    assert_eq!(semaphores.ai.available_permits(), 1);
    assert_eq!(semaphores.scraping.available_permits(), 1);
    assert_eq!(semaphores.export.available_permits(), 1);
}

#[test]
fn semaphores_reflect_configured_limits() {
    let limits = CategoryLimits {
        ai: 3,
        scraping: 12,
        export: 6,
        obsidian: 4,
        content: 8,
        url_utils: 24,
        security: 12,
        assets: 6,
    };
    let semaphores = CategorySemaphores::from_limits(&limits);

    assert_eq!(semaphores.ai.available_permits(), 3);
    assert_eq!(semaphores.scraping.available_permits(), 12);
    assert_eq!(semaphores.url_utils.available_permits(), 24);
}

// ===========================================================================
// Handler Construction Tests
// ===========================================================================

#[tokio::test]
async fn mcp_handler_construction_succeeds() {
    let container = make_container().await;
    let state = McpState::new(container);
    let handler = McpHandler::new(state);

    let _ = handler.tool_router;
}

#[tokio::test]
async fn mcp_handler_semaphores_limit_concurrency() {
    let limits = CategoryLimits {
        ai: 1,
        ..CategoryLimits::default()
    };
    let state = McpState::with_limits(make_container().await, limits);

    let permit = state
        .semaphores
        .ai
        .try_acquire()
        .expect("first acquire should succeed");
    assert!(
        state.semaphores.ai.try_acquire().is_err(),
        "second acquire should fail with 1 permit"
    );
    drop(permit);

    assert!(state.semaphores.ai.try_acquire().is_ok());
}

// ===========================================================================
// Parameter Deserialization Tests (JSON-RPC input validation)
// ===========================================================================

#[test]
fn scrape_url_params_deserialize_valid() {
    let json = r#"{"url": "https://example.com"}"#;
    let params: webfang_mcp::mcp_server::params::ScrapeUrlParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.url, "https://example.com");
}

#[test]
fn scrape_url_params_rejects_missing_url() {
    let json = r#"{}"#;
    let result = serde_json::from_str::<webfang_mcp::mcp_server::params::ScrapeUrlParams>(json);
    assert!(result.is_err(), "should reject missing url field");
}

#[test]
fn crawl_site_params_deserialize() {
    let json = r#"{"url": "https://example.com", "max_depth": 5, "max_pages": 50}"#;
    let params: webfang_mcp::mcp_server::params::CrawlSiteParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.max_depth, Some(5));
    assert_eq!(params.max_pages, Some(50));
}

#[test]
fn clean_html_params_deserialize() {
    let json = r#"{"html": "<p>Hello</p>"}"#;
    let params: webfang_mcp::mcp_server::params::CleanHtmlParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.html, "<p>Hello</p>");
}

#[test]
fn crawl_with_sitemap_params_optional_fields() {
    let json = r#"{"url": "https://example.com"}"#;
    let params: webfang_mcp::mcp_server::params::CrawlWithSitemapParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.sitemap_url, None);
}

#[test]
fn scrape_batch_params_deserialize() {
    let json = r#"{"urls": ["https://a.com", "https://b.com"], "concurrency": 2}"#;
    let params: webfang_mcp::mcp_server::params::ScrapeBatchParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.urls.len(), 2);
    assert_eq!(params.concurrency, Some(2));
}

#[test]
fn detect_spa_params_deserialize() {
    let json = r#"{"url": "https://example.com"}"#;
    let params: webfang_mcp::mcp_server::params::DetectSpaParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.url, "https://example.com");
}

#[test]
fn discover_urls_params_deserialize() {
    let json = r#"{"url": "https://example.com"}"#;
    let params: webfang_mcp::mcp_server::params::DiscoverUrlsParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.url, "https://example.com");
}

#[test]
fn export_file_params_deserialize() {
    let json = r#"{"output_dir": "/tmp", "filename": "test", "format": "markdown"}"#;
    let params: webfang_mcp::mcp_server::params::ExportFileParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.format, "markdown");
}

#[test]
fn detect_vault_params_optional_path() {
    let json = r#"{}"#;
    let params: webfang_mcp::mcp_server::params::DetectVaultParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.vault_path, None);
}
