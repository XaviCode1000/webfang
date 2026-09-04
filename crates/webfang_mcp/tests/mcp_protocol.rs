//! MCP protocol integration tests.
//!
//! Tests handler construction, state initialization, semaphore backpressure,
//! and parameter deserialization without requiring a running server.

use std::num::NonZeroUsize;

use webfang_core::application::container::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;
use webfang_mcp::mcp_server::state::{CategoryLimits, CategorySemaphores, McpState};
use webfang_mcp::mcp_server::McpHandler;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Non-zero literal helper — `CategoryLimits` fields are `NonZeroUsize`
/// since #1132, so raw `0` cannot even be named at a call site.
fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).expect("test literal is non-zero")
}

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

    assert!(state.limits.ai.get() >= 1);
    assert!(state.limits.scraping.get() >= 1);
}

#[tokio::test]
async fn mcp_state_construction_with_custom_limits() {
    let container = make_container().await;
    let limits = CategoryLimits {
        ai: nz(1),
        scraping: nz(4),
        export: nz(2),
        obsidian: nz(1),
        content: nz(3),
        url_utils: nz(8),
        security: nz(4),
        assets: nz(2),
    };
    let state = McpState::with_limits(container, limits);

    assert_eq!(state.limits.ai.get(), 1);
    assert_eq!(state.limits.scraping.get(), 4);
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

    assert!(limits.ai.get() >= 1);
    assert!(limits.scraping.get() >= 1);
    assert!(limits.export.get() >= 1);
    assert!(limits.obsidian.get() >= 1);
    assert!(limits.content.get() >= 1);
    assert!(limits.url_utils.get() >= 1);
    assert!(limits.security.get() >= 1);
    assert!(limits.assets.get() >= 1);
}

/// #1132 reproduction: this test used to assert that
/// `CategoryLimits { ai: 0, .. }` was CONSTRUCTABLE and that
/// `from_limits` silently clamped the zero-permit semaphore to 1 —
/// masking the misconfiguration that would otherwise deadlock the tool
/// category. The clamp is gone because the invalid state is gone: every
/// field is a `NonZeroUsize`, so `ai: 0` no longer compiles (the old
/// literal would now be a type error at this very call site).
#[test]
fn issue_1132_zero_limits_unrepresentable_and_mapping_is_one_to_one() {
    // The only way to name a limit is through the non-zero type; 0 is
    // rejected there — no clamp downstream can ever see it.
    assert!(
        NonZeroUsize::new(0).is_none(),
        "0 permits must be unnameable"
    );

    let limits = CategoryLimits {
        ai: nz(1),
        scraping: nz(2),
        export: nz(3),
        ..CategoryLimits::default()
    };
    let semaphores = CategorySemaphores::from_limits(&limits);

    // 1:1 mapping — what you name is exactly what the semaphore gets.
    assert_eq!(semaphores.ai.available_permits(), 1);
    assert_eq!(semaphores.scraping.available_permits(), 2);
    assert_eq!(semaphores.export.available_permits(), 3);
}

#[test]
fn semaphores_reflect_configured_limits() {
    let limits = CategoryLimits {
        ai: nz(3),
        scraping: nz(12),
        export: nz(6),
        obsidian: nz(4),
        content: nz(8),
        url_utils: nz(24),
        security: nz(12),
        assets: nz(6),
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
        ai: nz(1),
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
    assert_eq!(params.url.as_str(), "https://example.com/");
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
    assert_eq!(params.url.as_str(), "https://example.com/");
}

#[test]
fn discover_urls_params_deserialize() {
    let json = r#"{"url": "https://example.com"}"#;
    let params: webfang_mcp::mcp_server::params::DiscoverUrlsParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.url.as_str(), "https://example.com/");
}

#[test]
fn export_file_params_deserialize() {
    let json =
        r#"{"output_dir": "/tmp", "filename": "test", "format": "jsonl", "content": "hello"}"#;
    let params: webfang_mcp::mcp_server::params::ExportFileParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.content_format, "jsonl");
    assert_eq!(params.content, "hello");
}

#[test]
fn detect_vault_params_optional_path() {
    let json = r#"{}"#;
    let params: webfang_mcp::mcp_server::params::DetectVaultParams =
        serde_json::from_str(json).unwrap();
    assert_eq!(params.vault_path, None);
}
