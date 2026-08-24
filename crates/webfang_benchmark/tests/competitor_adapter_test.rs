//! Slice-2 competitor adapter skeletons: request-building/validation logic
//! only. These tests exercise URL construction, auth-key presence checks, the
//! fail-closed gate, and the deferred-execution behavior of `run()` — all
//! fully offline (no socket is ever created).
//!
//! Run: cargo nextest run -p webfang_benchmark --test competitor_adapter_test

use webfang_benchmark::competitor::{self, Crawl4AiConfig, FirecrawlConfig, StartCrawlParams};
use webfang_benchmark::BenchmarkError;

fn params(url: &str, limit: u32) -> StartCrawlParams {
    StartCrawlParams {
        target_url: url.to_string(),
        page_limit: limit,
    }
}

/// Default endpoint derives from documented public API layout.
#[test]
fn firecrawl_default_start_crawl_url() {
    let config = FirecrawlConfig::default();
    let url = config.start_crawl_url().expect("default url builds");
    assert_eq!(url.as_str(), "https://api.firecrawl.dev/v2/crawl");
}

/// A custom base URL is honored end-to-end.
#[test]
fn firecrawl_custom_base_url_is_honored() {
    let config = FirecrawlConfig {
        api_base_url: "https://mirror.example.internal".to_string(),
        ..FirecrawlConfig::default()
    };
    let url = config.start_crawl_url().expect("custom url builds");
    assert_eq!(url.as_str(), "https://mirror.example.internal/v2/crawl");
}

/// Unparseable base URLs fail loudly with a typed error.
#[test]
fn firecrawl_invalid_base_url_is_typed_error() {
    let config = FirecrawlConfig {
        api_base_url: "not a url".to_string(),
        ..FirecrawlConfig::default()
    };
    let err = config.start_crawl_url().expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Engine(_)));
}

/// Unparseable crawl target URLs are rejected before any request is built.
#[test]
fn firecrawl_invalid_target_url_is_typed_error() {
    let err = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("not a url", 10),
        Some("key-123"),
        true,
    )
    .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Engine(_)));
}

/// Missing API key: typed refusal naming the provider env var.
#[test]
fn firecrawl_missing_key_yields_live_disabled() {
    let err = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("https://example.com", 5),
        None,
        true,
    )
    .expect_err("must refuse without key");
    match err {
        BenchmarkError::LiveDisabled { env_var, .. } => {
            assert_eq!(env_var, "FIRECRAWL_API_KEY")
        },
        other => panic!("expected LiveDisabled, got {other:?}"),
    }
}

/// Blank/whitespace keys count as absent.
#[test]
fn firecrawl_blank_key_yields_live_disabled() {
    let err = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("https://example.com", 5),
        Some("   "),
        true,
    )
    .expect_err("must refuse blank key");
    assert!(matches!(err, BenchmarkError::LiveDisabled { .. }));
}

/// Even with a valid key, execution without explicit opt-in is refused.
#[test]
fn firecrawl_without_opt_in_yields_live_disabled() {
    let err = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("https://example.com", 5),
        Some("key-123"),
        false,
    )
    .expect_err("must refuse without opt-in");
    assert!(matches!(err, BenchmarkError::LiveDisabled { .. }));
}

/// Gate open: the fully-built request description comes back — method, URL,
/// bearer token, JSON body — with nothing sent anywhere.
#[test]
fn firecrawl_prepared_request_fully_described_offline() {
    let prepared = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("https://example.com/docs", 42),
        Some("fc-secret"),
        true,
    )
    .expect("prepared request");
    assert_eq!(prepared.method, "POST");
    assert_eq!(prepared.url.as_str(), "https://api.firecrawl.dev/v2/crawl");
    assert_eq!(prepared.bearer_token, "fc-secret");
    assert_eq!(prepared.body_json["url"], "https://example.com/docs");
    assert_eq!(prepared.body_json["limit"], 42);
}

/// Crawl4AI defaults to its documented local server port.
#[test]
fn crawl4ai_default_server_url() {
    let config = Crawl4AiConfig::default();
    let url = config.crawl_url().expect("default url builds");
    assert_eq!(url.as_str(), "http://127.0.0.1:11235/crawl");
}

/// Secrets never leak through Debug formatting of a prepared request:
/// both single-line and pretty `{:#?}` render the bearer token as
/// `[REDACTED]` (Tier B Step 0 prerequisite — never log secrets).
#[test]
fn prepared_request_debug_redacts_bearer_token() {
    let token = "super-secret-token-value";
    let prepared = competitor::firecrawl::prepare_start_crawl(
        &FirecrawlConfig::default(),
        &params("https://example.com/docs", 42),
        Some(token),
        true,
    )
    .expect("prepared request");
    let debug = format!("{prepared:?}");
    let debug_pretty = format!("{prepared:#?}");
    assert!(
        !debug.contains(token),
        "single-line Debug leaked the token: {debug}"
    );
    assert!(
        !debug_pretty.contains(token),
        "pretty Debug leaked the token: {debug_pretty}"
    );
    assert!(
        debug.contains("[REDACTED]"),
        "redaction marker missing: {debug}"
    );
    assert!(debug_pretty.contains("[REDACTED]"));
}

/// Crawl4AI enforces the same gate contract as Firecrawl.
#[test]
fn crawl4ai_gate_contract_matches_firecrawl() {
    let config = Crawl4AiConfig::default();
    let p = params("https://example.com", 3);
    assert!(matches!(
        competitor::crawl4ai::prepare_crawl(&config, &p, None, true),
        Err(BenchmarkError::LiveDisabled { .. })
    ));
    assert!(matches!(
        competitor::crawl4ai::prepare_crawl(&config, &p, Some("c4a-key"), false),
        Err(BenchmarkError::LiveDisabled { .. })
    ));
    let prepared = competitor::crawl4ai::prepare_crawl(&config, &p, Some("c4a-key"), true)
        .expect("prepared request");
    assert_eq!(prepared.url.as_str(), "http://127.0.0.1:11235/crawl");
    assert_eq!(prepared.bearer_token, "c4a-key");
}

/// `run()` never performs I/O in this build: even with the gate fully open it
/// returns a typed deferral error instead of executing the prepared request.
#[test]
fn run_defers_execution_even_when_gate_opens() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let outcome = rt.block_on(competitor::firecrawl::run(
        &FirecrawlConfig::default(),
        &params("https://example.com", 5),
        Some("fc-secret"),
        true,
    ));
    match outcome {
        Err(BenchmarkError::Engine(detail)) => {
            assert!(
                detail.contains("no HTTP request was sent"),
                "deferral message must state nothing was sent, got: {detail}"
            );
        },
        other => panic!("expected typed deferral error, got {other:?}"),
    }
}
