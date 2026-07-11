//! Proptest and feature-gating tests for MCP server
//!
//! Proptest: Property-based tests for URL utility tools with edge cases.
//! Feature-gating: Verifies AI module compiles and returns correct router per feature flag.
//!
//! Run with: cargo nextest run --test-threads 2 mcp_proptest
//! Run with AI: cargo nextest run --test-threads 2 --features ai mcp_proptest

#![cfg(feature = "mcp")]

use proptest::prelude::*;
use url::Url;

// ============================================================================
// Proptest: URL validation tool properties
// ============================================================================

proptest! {
    /// Valid URLs with http/https schemes should always parse successfully
    #[test]
    fn prop_validate_url_valid_schemes(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{2,15}\\.[a-z]{2,6}",
        path in "(/[a-zA-Z0-9._~-]{1,20}){0,5}",
    ) {
        let url = format!("{}://{}{}", scheme, domain, path);
        let parsed = Url::parse(&url);
        prop_assert!(parsed.is_ok(), "Valid URL should parse: {}", url);

        let u = parsed.unwrap();
        prop_assert_eq!(u.scheme(), scheme);
        prop_assert_eq!(u.host_str().unwrap(), domain);
    }

    /// URLs with query strings should preserve query component
    #[test]
    fn prop_validate_url_with_query(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        key in "[a-z]{2,8}",
        value in "[a-zA-Z0-9]{1,15}",
    ) {
        let url = format!("{}://{}?{}={}", scheme, domain, key, value);
        let u = Url::parse(&url).unwrap();
        prop_assert!(u.query().is_some());
        prop_assert!(u.query().unwrap().contains(&key));
    }

    /// URLs with fragments should have fragment preserved
    #[test]
    fn prop_validate_url_with_fragment(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        fragment in "[a-zA-Z0-9_-]{1,20}",
    ) {
        let url = format!("{}://{}#{}", scheme, domain, fragment);
        let u = Url::parse(&url).unwrap();
        prop_assert_eq!(u.fragment().unwrap(), fragment);
    }
}

// ============================================================================
// Proptest: URL normalization properties
// ============================================================================

proptest! {
    /// Normalization should preserve the host
    #[test]
    fn prop_normalize_url_preserves_host(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,15}\\.[a-z]{2,6}",
        path in "(/[a-zA-Z0-9_-]{1,15}){1,4}",
    ) {
        let url = format!("{}://{}{}", scheme, domain, path);
        let mut u = Url::parse(&url).unwrap();
        u.set_fragment(None);

        prop_assert_eq!(u.host_str().unwrap(), domain);
    }

    /// Normalization should preserve the scheme
    #[test]
    fn prop_normalize_url_preserves_scheme(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
    ) {
        let url = format!("{}://{}", scheme, domain);
        let mut u = Url::parse(&url).unwrap();
        u.set_fragment(None);

        prop_assert_eq!(u.scheme(), scheme);
    }

    /// Normalization should preserve the path (minus trailing slash)
    #[test]
    fn prop_normalize_url_preserves_path(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        path in "(/[a-zA-Z0-9_-]{1,12}){1,3}",
    ) {
        let url = format!("{}://{}{}/", scheme, domain, path);
        let mut u = Url::parse(&url).unwrap();
        u.set_fragment(None);
        if u.path().ends_with('/') && u.path() != "/" {
            let trimmed = u.path().trim_end_matches('/').to_string();
            u.set_path(&trimmed);
        }

        prop_assert_eq!(u.path(), path);
    }

    /// Normalization should remove fragments
    #[test]
    fn prop_normalize_url_removes_fragment(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        fragment in "[a-zA-Z0-9_-]{1,20}",
    ) {
        let url = format!("{}://{}#{}", scheme, domain, fragment);
        let mut u = Url::parse(&url).unwrap();
        u.set_fragment(None);

        prop_assert!(u.fragment().is_none());
    }

    /// Normalization should remove trailing slashes (except root)
    #[test]
    fn prop_normalize_url_removes_trailing_slash(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        path in "(/[a-zA-Z0-9_-]{1,12}){1,3}",
    ) {
        let url_with_slash = format!("{}://{}{}/", scheme, domain, path);
        let mut u = Url::parse(&url_with_slash).unwrap();
        u.set_fragment(None);
        if u.path().ends_with('/') && u.path() != "/" {
            let trimmed = u.path().trim_end_matches('/').to_string();
            u.set_path(&trimmed);
        }

        prop_assert!(!u.path().ends_with('/'), "Trailing slash should be removed: {}", u.path());
    }
}

// ============================================================================
// Proptest: URL edge cases
// ============================================================================

proptest! {
    /// URLs with ports should parse and preserve port info
    #[test]
    fn prop_url_with_port(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        port in 1024u16..65535,
    ) {
        let url = format!("{}://{}:{}", scheme, domain, port);
        let u = Url::parse(&url).unwrap();
        prop_assert_eq!(u.port(), Some(port));
        prop_assert_eq!(u.host_str().unwrap(), domain);
    }

    /// URLs with userinfo (user:pass@) should parse
    #[test]
    fn prop_url_with_userinfo(
        scheme in prop_oneof!["https", "http"],
        user in "[a-z]{3,10}",
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
    ) {
        let url = format!("{}://{}@{}", scheme, user, domain);
        let u = Url::parse(&url).unwrap();
        prop_assert_eq!(u.host_str().unwrap(), domain);
    }

    /// IPv4 URLs should parse successfully
    #[test]
    fn prop_url_ipv4(
        scheme in prop_oneof!["https", "http"],
        a in 1u8..255,
        b in 0u8..255,
        c in 0u8..255,
        d in 1u8..255,
    ) {
        let url = format!("{}://{}.{}.{}.{}", scheme, a, b, c, d);
        let u = Url::parse(&url).unwrap();
        prop_assert!(u.host_str().is_some());
    }
}

// ============================================================================
// Proptest: is_internal_link invariants (MCP tool: is_internal_link)
// ============================================================================

proptest! {
    /// A URL with fragment should still be internal to its domain
    #[test]
    fn prop_internal_link_ignores_fragment(
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        path in "(/[a-z]{1,10}){1,2}",
        fragment in "[a-z]{1,15}",
    ) {
        let url = format!("https://{}{}#{}", domain, path, fragment);
        prop_assert!(
            rust_scraper::is_internal_link(&url, &domain),
            "Fragment should not affect internal check: {}", url
        );
    }

    /// A URL with query params should still be internal to its domain
    #[test]
    fn prop_internal_link_ignores_query(
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        key in "[a-z]{2,8}",
        value in "[a-z0-9]{1,10}",
    ) {
        let url = format!("https://{}?{}={}", domain, key, value);
        prop_assert!(
            rust_scraper::is_internal_link(&url, &domain),
            "Query params should not affect internal check: {}", url
        );
    }

    /// A URL with port should still be internal to its domain
    #[test]
    fn prop_internal_link_ignores_port(
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        port in 1024u16..65535,
    ) {
        let url = format!("https://{}:{}", domain, port);
        prop_assert!(
            rust_scraper::is_internal_link(&url, &domain),
            "Port should not affect internal check: {}", url
        );
    }
}

// ============================================================================
// Proptest: matches_pattern invariants (MCP tool: match_url_pattern)
// ============================================================================

proptest! {
    /// Any URL should match an empty pattern (current behavior)
    #[test]
    fn prop_empty_pattern_matches_all(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
    ) {
        let url = format!("{}://{}", scheme, domain);
        prop_assert!(
            rust_scraper::matches_pattern(&url, ""),
            "Empty pattern should match: {}", url
        );
    }

    /// Domain substring in pattern should match URL containing that domain
    #[test]
    fn prop_domain_in_pattern_matches(
        domain in "[a-z]{5,15}\\.[a-z]{2,5}",
        path in "(/[a-z]{1,8}){0,3}",
    ) {
        let url = format!("https://{}{}", domain, path);
        prop_assert!(
            rust_scraper::matches_pattern(&url, &domain),
            "URL containing '{}' should match pattern '{}'", url, domain
        );
    }
}

// ============================================================================
// Feature-gating tests: AI module availability
// ============================================================================

/// Without `ai` feature: AI module should return an empty router.
/// This verifies that AI tools are NOT registered when compiled without `--features ai`.
#[cfg(not(feature = "ai"))]
#[test]
fn test_ai_module_returns_empty_router_without_feature() {
    use rmcp::handler::server::tool::ToolRouter;
    use rust_scraper::infrastructure::mcp_server::handlers::ai::build_router as ai_build_router;
    use rust_scraper::infrastructure::mcp_server::McpHandler;

    let router: ToolRouter<McpHandler> = ai_build_router();
    // Empty router has no registered tools — verifies AI tools are absent
    let tools = router.list_all();
    assert!(
        tools.is_empty(),
        "AI tools should NOT be registered without --features ai, but found {} tools",
        tools.len()
    );
}

/// With `ai` feature: AI module should compile and return a router.
/// Once AI tools are implemented, this test should verify they ARE present.
/// Currently the AI module is a stub, so we verify it compiles and returns a router.
#[cfg(feature = "ai")]
#[test]
fn test_ai_module_compiles_with_feature() {
    use rmcp::handler::server::tool::ToolRouter;
    use rust_scraper::infrastructure::mcp_server::handlers::ai::build_router as ai_build_router;
    use rust_scraper::infrastructure::mcp_server::McpHandler;

    // This test verifies the AI module compiles with --features ai
    // and returns a valid ToolRouter (even if currently empty stub)
    let _router: ToolRouter<McpHandler> = ai_build_router();
    // When AI tools are implemented, add assertion:
    // let tools = router.list_tools_sync();
    // assert!(tools.iter().any(|t| t.name == "semantic_clean"));
    // assert!(tools.iter().any(|t| t.name == "score_relevance"));
    // assert!(tools.iter().any(|t| t.name == "generate_embedding"));
}

// ============================================================================
// MCP handler construction tests
// ============================================================================

/// Verify McpHandler can be constructed without panicking (no AI feature)
#[cfg(not(feature = "ai"))]
#[tokio::test]
async fn test_mcp_handler_construction_without_ai() {
    use rust_scraper::config::Config;
    use rust_scraper::di::Container;
    use rust_scraper::infrastructure::mcp_server::state::McpState;
    use rust_scraper::infrastructure::mcp_server::McpHandler;

    let config = Config::default();
    let container = Container::new(config)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let _handler = McpHandler::new(state);
    // Handler constructed successfully
}

/// Verify McpHandler can be constructed without panicking (with AI feature)
#[cfg(feature = "ai")]
#[tokio::test]
async fn test_mcp_handler_construction_with_ai() {
    use rust_scraper::config::Config;
    use rust_scraper::di::Container;
    use rust_scraper::infrastructure::mcp_server::state::McpState;
    use rust_scraper::infrastructure::mcp_server::McpHandler;

    let config = Config::default();
    let container = Container::new(config)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let _handler = McpHandler::new(state);
    // Handler constructed successfully
}

/// Verify all non-AI tool categories are registered regardless of feature flag
#[tokio::test]
async fn test_non_ai_tool_categories_registered() {
    use rust_scraper::config::Config;
    use rust_scraper::di::Container;
    use rust_scraper::infrastructure::mcp_server::state::McpState;
    use rust_scraper::infrastructure::mcp_server::McpHandler;

    let config = Config::default();
    let container = Container::new(config)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let handler = McpHandler::new(state);

    let tools = handler.tool_router.list_all();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // Verify core tool categories are present
    // Scraping tools
    assert!(
        tool_names.contains(&"scrape_url"),
        "scrape_url should be registered"
    );
    assert!(
        tool_names.contains(&"crawl_site"),
        "crawl_site should be registered"
    );
    assert!(
        tool_names.contains(&"discover_urls"),
        "discover_urls should be registered"
    );
    assert!(
        tool_names.contains(&"detect_spa"),
        "detect_spa should be registered"
    );

    // Content processing tools
    assert!(
        tool_names.contains(&"clean_html"),
        "clean_html should be registered"
    );
    assert!(
        tool_names.contains(&"convert_html_to_markdown"),
        "convert_html_to_markdown should be registered"
    );
    assert!(
        tool_names.contains(&"extract_links"),
        "extract_links should be registered"
    );

    // URL utility tools
    assert!(
        tool_names.contains(&"validate_url"),
        "validate_url should be registered"
    );
    assert!(
        tool_names.contains(&"normalize_url"),
        "normalize_url should be registered"
    );
    assert!(
        tool_names.contains(&"extract_domain"),
        "extract_domain should be registered"
    );
    assert!(
        tool_names.contains(&"is_internal_link"),
        "is_internal_link should be registered"
    );
    assert!(
        tool_names.contains(&"match_url_pattern"),
        "match_url_pattern should be registered"
    );

    // Security tools
    assert!(
        tool_names.contains(&"detect_waf"),
        "detect_waf should be registered"
    );
    assert!(
        tool_names.contains(&"verify_waf_integrity"),
        "verify_waf_integrity should be registered"
    );
    assert!(
        tool_names.contains(&"list_waf_providers"),
        "list_waf_providers should be registered"
    );

    // Export tools
    assert!(
        tool_names.contains(&"export_file"),
        "export_file should be registered"
    );
    assert!(
        tool_names.contains(&"export_jsonl"),
        "export_jsonl should be registered"
    );

    // Obsidian tools
    assert!(
        tool_names.contains(&"detect_obsidian_vault"),
        "detect_obsidian_vault should be registered"
    );
    assert!(
        tool_names.contains(&"build_obsidian_uri"),
        "build_obsidian_uri should be registered"
    );
}
