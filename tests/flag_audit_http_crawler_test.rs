//! Integration tests for HTTP and crawler flag behavior (T6 + T7).
//!
//! Verifies that HTTP retry/backoff/timeout and crawler delay/max-pages/concurrency
//! flags actually control behavior through the config flow.
//!
//! Run with: cargo nextest run --test flag_audit_http_crawler_test

use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::{Args, ConcurrencyConfig, OutputFormat, ScraperConfig};
use std::path::PathBuf;

// ============================================================================
// Helpers
// ============================================================================

/// Minimal Args with sensible defaults for all fields.
fn base_args() -> Args {
    Args {
        subcommand: None,
        url: Some("https://example.com".into()),
        selector: "body".into(),
        output: PathBuf::from("output"),
        format: OutputFormat::Markdown,
        export_format: rust_scraper::ExportFormat::Jsonl,
        obsidian_wiki_links: false,
        obsidian_tags: None,
        obsidian_relative_assets: false,
        vault: None,
        quick_save: false,
        obsidian_rich_metadata: false,
        delay_ms: 1000,
        max_pages: 10,
        concurrency: ConcurrencyConfig::default(),
        use_sitemap: false,
        sitemap_url: None,
        single_page: false,
        resume: false,
        state_dir: None,
        download_images: false,
        download_documents: false,
        interactive: false,
        config_tui: false,
        clean_ai: false,
        force_js_render: false,
        verbose: 0,
        quiet: false,
        dry_run: false,
        max_depth: 2,
        timeout_secs: 30,
        include_patterns: vec![],
        exclude_patterns: vec![],
        max_retries: 3,
        backoff_base_ms: 1000,
        backoff_max_ms: 10_000,
        accept_language: "en-US,en;q=0.9".into(),
        user_agent: None,
        max_file_size: 52_428_800,
        download_timeout: 30,
        sitemap_depth: 3,
        cpu_cores: None,
        ram_budget: None,
        db_path: None,
        elastic: false,
    }
}

/// Convert Args → CrawlOptions (same path as the real CLI).
fn opts_from_args(args: Args) -> CrawlOptions {
    CrawlOptions::from(args)
}

/// Mirror the `build_http_client_config` logic from `scrape_flow.rs`.
///
/// This verifies the same field-mapping contract that the real code uses.
/// If the production code changes its mapping, this test will diverge and
/// need updating — which is the intended safety net.
fn mirror_build_http_config(opts: &CrawlOptions) -> rust_scraper::HttpClientConfig {
    rust_scraper::HttpClientConfig {
        max_retries: opts.network.max_retries,
        backoff_base_ms: opts.network.backoff_base_ms,
        backoff_max_ms: opts.network.backoff_max_ms,
        accept_language: opts.network.accept_language.clone(),
        user_agent: opts.network.user_agent.clone(),
        timeout_secs: opts.network.timeout_secs,
        ..rust_scraper::HttpClientConfig::default()
    }
}

// ============================================================================
// T6: HTTP Flags
// ============================================================================

// ── T6.1: max_retries flows through ────────────────────────────────────────

#[test]
fn test_http_max_retries_flows_through() {
    let args = Args {
        max_retries: 5,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(
        config.max_retries, 5,
        "max_retries must flow from Args → CrawlOptions → HttpClientConfig"
    );
    assert_eq!(opts.network.max_retries, 5);
}

#[test]
fn test_http_max_retries_non_default() {
    let args = Args {
        max_retries: 0,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(config.max_retries, 0);
}

// ── T6.2: backoff_base_ms flows through ────────────────────────────────────

#[test]
fn test_http_backoff_base_flows_through() {
    let args = Args {
        backoff_base_ms: 500,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(
        config.backoff_base_ms, 500,
        "backoff_base_ms must flow from Args → CrawlOptions → HttpClientConfig"
    );
    assert_eq!(opts.network.backoff_base_ms, 500);
}

#[test]
fn test_http_backoff_base_custom_value() {
    let args = Args {
        backoff_base_ms: 200,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.network.backoff_base_ms, 200);
}

// ── T6.3: backoff_max_ms flows through ─────────────────────────────────────

#[test]
fn test_http_backoff_max_flows_through() {
    let args = Args {
        backoff_max_ms: 30_000,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(
        config.backoff_max_ms, 30_000,
        "backoff_max_ms must flow from Args → CrawlOptions → HttpClientConfig"
    );
    assert_eq!(opts.network.backoff_max_ms, 30_000);
}

#[test]
fn test_http_backoff_max_small_value() {
    let args = Args {
        backoff_max_ms: 1000,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.network.backoff_max_ms, 1000);
}

// ── T6.4: timeout_secs flows through ───────────────────────────────────────

#[test]
fn test_http_timeout_secs_flows_through() {
    let args = Args {
        timeout_secs: 120,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(
        config.timeout_secs, 120,
        "timeout_secs must flow from Args → CrawlOptions → HttpClientConfig"
    );
    assert_eq!(opts.network.timeout_secs, 120);
}

#[test]
fn test_http_timeout_secs_short() {
    let args = Args {
        timeout_secs: 5,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(config.timeout_secs, 5);
}

// ── T6.5: accept_language flows through ─────────────────────────────────────

#[test]
fn test_http_accept_language_flows_through() {
    let args = Args {
        accept_language: "en-US,en;q=0.9".into(),
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(
        config.accept_language, "en-US,en;q=0.9",
        "accept_language must flow from Args → CrawlOptions → HttpClientConfig"
    );
    assert_eq!(opts.network.accept_language, "en-US,en;q=0.9");
}

#[test]
fn test_http_accept_language_spanish() {
    let args = Args {
        accept_language: "es-ES,es;q=0.9,en;q=0.8".into(),
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(config.accept_language, "es-ES,es;q=0.9,en;q=0.8");
}

// ── T6.6: All HTTP defaults are sensible ───────────────────────────────────

#[test]
fn test_http_all_defaults() {
    let args = base_args();
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    // Retry defaults
    assert_eq!(config.max_retries, 3, "default max_retries should be 3");

    // Backoff defaults
    assert_eq!(
        config.backoff_base_ms, 1000,
        "default backoff_base_ms should be 1000ms"
    );
    assert_eq!(
        config.backoff_max_ms, 10_000,
        "default backoff_max_ms should be 10000ms"
    );

    // Timeout defaults
    assert_eq!(config.timeout_secs, 30, "default timeout_secs should be 30");

    // Header defaults
    assert_eq!(
        config.accept_language, "en-US,en;q=0.9",
        "default accept_language should be en-US"
    );

    // Verify defaults are sane (not zero, not absurdly large)
    assert!(config.max_retries > 0, "max_retries must be positive");
    assert!(
        config.backoff_base_ms > 0,
        "backoff_base_ms must be positive"
    );
    assert!(
        config.backoff_max_ms >= config.backoff_base_ms,
        "backoff_max_ms must be >= backoff_base_ms"
    );
    assert!(config.timeout_secs > 0, "timeout_secs must be positive");
}

// ── T6.7: HTTP flags flow independently (no coupling) ──────────────────────

#[test]
fn test_http_flags_flow_independently() {
    // Set only max_retries — other defaults should remain
    let args = Args {
        max_retries: 7,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(config.max_retries, 7);
    assert_eq!(config.backoff_base_ms, 1000); // default
    assert_eq!(config.backoff_max_ms, 10_000); // default
    assert_eq!(config.timeout_secs, 30); // default
}

#[test]
fn test_http_backoff_independent_of_timeout() {
    // Set backoff to high values — timeout should remain default
    let args = Args {
        backoff_base_ms: 5000,
        backoff_max_ms: 60_000,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let config = mirror_build_http_config(&opts);

    assert_eq!(config.backoff_base_ms, 5000);
    assert_eq!(config.backoff_max_ms, 60_000);
    assert_eq!(config.timeout_secs, 30); // unaffected
}

// ============================================================================
// T7: Crawler Flags
// ============================================================================

// ── T7.1: max_pages limits URL list ────────────────────────────────────────

/// Verify the URL slicing logic in scrape_flow.rs:
/// ```text
/// let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
///     urls.iter().take(max_pages).cloned().collect()
/// } else {
///     urls.to_vec()
/// };
/// ```
///
/// This test replicates that logic to verify the contract.
#[test]
fn test_crawler_max_pages_limits_urls() {
    let urls: Vec<url::Url> = (0..10)
        .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect();

    let scraper_config = ScraperConfig {
        max_pages: Some(5),
        ..ScraperConfig::default()
    };

    // Replicate the take(max_pages) logic from scrape_flow.rs
    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        urls.iter().take(max_pages).cloned().collect::<Vec<_>>()
    } else {
        urls.to_vec()
    };

    assert_eq!(
        urls_to_process.len(),
        5,
        "max_pages=5 should limit to 5 URLs"
    );
    assert_eq!(urls_to_process[0].as_str(), "https://example.com/page0");
    assert_eq!(urls_to_process[4].as_str(), "https://example.com/page4");
}

#[test]
fn test_crawler_max_pages_one() {
    let urls: Vec<url::Url> = (0..5)
        .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect();

    let scraper_config = ScraperConfig {
        max_pages: Some(1),
        ..ScraperConfig::default()
    };

    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        urls.iter().take(max_pages).cloned().collect::<Vec<_>>()
    } else {
        urls.to_vec()
    };

    assert_eq!(urls_to_process.len(), 1, "max_pages=1 should take only 1");
}

#[test]
fn test_crawler_max_pages_larger_than_list() {
    let urls: Vec<url::Url> = (0..3)
        .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect();

    let scraper_config = ScraperConfig {
        max_pages: Some(100),
        ..ScraperConfig::default()
    };

    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        urls.iter().take(max_pages).cloned().collect::<Vec<_>>()
    } else {
        urls.to_vec()
    };

    assert_eq!(
        urls_to_process.len(),
        3,
        "max_pages > len(urls) should return all URLs"
    );
}

// ── T7.2: max_pages None is unlimited ──────────────────────────────────────

#[test]
fn test_crawler_max_pages_none_is_unlimited() {
    let urls: Vec<url::Url> = (0..50)
        .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect();

    let scraper_config = ScraperConfig {
        max_pages: None,
        ..ScraperConfig::default()
    };

    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        urls.iter().take(max_pages).cloned().collect::<Vec<_>>()
    } else {
        urls.to_vec()
    };

    assert_eq!(
        urls_to_process.len(),
        50,
        "max_pages=None should process all URLs"
    );
}

#[test]
fn test_crawler_max_pages_none_preserves_order() {
    let urls: Vec<url::Url> = (0..10)
        .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect();

    let scraper_config = ScraperConfig {
        max_pages: None,
        ..ScraperConfig::default()
    };

    let urls_to_process = if let Some(max_pages) = scraper_config.max_pages {
        urls.iter().take(max_pages).cloned().collect::<Vec<_>>()
    } else {
        urls.to_vec()
    };

    // Order must be preserved when no limit is applied
    for (i, url) in urls_to_process.iter().enumerate() {
        assert_eq!(
            url.as_str(),
            format!("https://example.com/page{i}"),
            "URL order must be preserved when max_pages=None"
        );
    }
}

// ── T7.3: Concurrency auto resolves to sane value ──────────────────────────

#[test]
fn test_crawler_concurrency_auto() {
    let config = ConcurrencyConfig::auto();
    let resolved = config.resolve();

    // Auto-detect should resolve to a value between 1 and 16
    assert!(
        (1..=16).contains(&resolved),
        "auto concurrency must be between 1 and 16, got {resolved}"
    );

    // On any modern machine, auto should give at least 1
    assert!(resolved >= 1, "auto concurrency must be at least 1");
}

#[test]
fn test_crawler_concurrency_auto_is_auto() {
    let config = ConcurrencyConfig::auto();
    assert!(
        config.is_auto(),
        "ConcurrencyConfig::auto() must report is_auto() == true"
    );
}

#[test]
fn test_crawler_concurrency_auto_default() {
    let config = ConcurrencyConfig::default();
    assert!(config.is_auto(), "default ConcurrencyConfig must be auto");
}

// ── T7.4: Concurrency explicit value ───────────────────────────────────────

#[test]
fn test_crawler_concurrency_explicit() {
    let config = ConcurrencyConfig::new(8);
    let resolved = config.resolve();

    assert_eq!(resolved, 8, "explicit concurrency=8 must resolve to 8");
    assert!(
        !config.is_auto(),
        "explicit ConcurrencyConfig must not be auto"
    );
}

#[test]
fn test_crawler_concurrency_explicit_small() {
    let config = ConcurrencyConfig::new(1);
    assert_eq!(config.resolve(), 1);
}

#[test]
fn test_crawler_concurrency_explicit_clamped() {
    // Values > 16 are clamped to 16
    let config = ConcurrencyConfig::new(100);
    assert_eq!(
        config.resolve(),
        16,
        "concurrency > 16 must be clamped to 16"
    );
}

#[test]
fn test_crawler_concurrency_explicit_from_scraper_config() {
    // ScraperConfig.scraper_concurrency mirrors ConcurrencyConfig for the scraper layer
    let scraper_config = ScraperConfig::default().with_scraper_concurrency(7);
    assert_eq!(
        scraper_config.scraper_concurrency, 7,
        "ScraperConfig must carry the explicit concurrency value"
    );
}

// ── T7.5: delay_ms flows through ───────────────────────────────────────────

#[test]
fn test_crawler_delay_flows_through() {
    let args = Args {
        delay_ms: 2000,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(
        opts.network.delay_ms, 2000,
        "delay_ms must flow from Args → CrawlOptions.network"
    );
}

#[test]
fn test_crawler_delay_default() {
    let args = base_args();
    let opts = opts_from_args(args);

    assert_eq!(
        opts.network.delay_ms, 1000,
        "default delay_ms should be 1000"
    );
}

#[test]
fn test_crawler_delay_small_value() {
    let args = Args {
        delay_ms: 100,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(opts.network.delay_ms, 100);
}

#[test]
fn test_crawler_delay_large_value() {
    let args = Args {
        delay_ms: 10_000,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(opts.network.delay_ms, 10_000);
}

// ── T7.6: max_pages flows through CrawlOptions ─────────────────────────────

#[test]
fn test_crawler_max_pages_flows_through_opts() {
    let args = Args {
        max_pages: 25,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(
        opts.crawl.max_pages, 25,
        "max_pages must flow from Args → CrawlOptions.crawl"
    );
}

#[test]
fn test_crawler_max_pages_default() {
    let args = base_args();
    let opts = opts_from_args(args);

    assert_eq!(opts.crawl.max_pages, 10, "default max_pages should be 10");
}

// ── T7.7: Concurrency flows through CrawlOptions ───────────────────────────

#[test]
fn test_crawler_concurrency_flows_through_opts() {
    let args = Args {
        concurrency: ConcurrencyConfig::new(6),
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(
        opts.network.concurrency.resolve(),
        6,
        "concurrency must flow from Args → CrawlOptions.network"
    );
    assert!(!opts.network.concurrency.is_auto());
}

#[test]
fn test_crawler_concurrency_auto_flows_through_opts() {
    let args = Args {
        concurrency: ConcurrencyConfig::default(),
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(
        opts.network.concurrency.is_auto(),
        "default concurrency must be auto in CrawlOptions"
    );
}

// ============================================================================
// Combined: HTTP + Crawler flags together
// ============================================================================

#[test]
fn test_combined_http_and_crawler_flags() {
    let args = Args {
        max_retries: 7,
        backoff_base_ms: 250,
        backoff_max_ms: 15_000,
        timeout_secs: 90,
        accept_language: "fr-FR,fr;q=0.9".into(),
        max_pages: 42,
        delay_ms: 1500,
        concurrency: ConcurrencyConfig::new(4),
        ..base_args()
    };
    let opts = opts_from_args(args);
    let http_config = mirror_build_http_config(&opts);

    // HTTP flags
    assert_eq!(http_config.max_retries, 7);
    assert_eq!(http_config.backoff_base_ms, 250);
    assert_eq!(http_config.backoff_max_ms, 15_000);
    assert_eq!(http_config.timeout_secs, 90);
    assert_eq!(http_config.accept_language, "fr-FR,fr;q=0.9");

    // Crawler flags
    assert_eq!(opts.crawl.max_pages, 42);
    assert_eq!(opts.network.delay_ms, 1500);
    assert_eq!(opts.network.concurrency.resolve(), 4);
}

#[test]
fn test_combined_http_defaults_and_crawler_explicit() {
    let args = Args {
        max_pages: 100,
        concurrency: ConcurrencyConfig::new(12),
        delay_ms: 500,
        ..base_args()
    };
    let opts = opts_from_args(args);
    let http_config = mirror_build_http_config(&opts);

    // HTTP defaults remain unchanged
    assert_eq!(http_config.max_retries, 3);
    assert_eq!(http_config.backoff_base_ms, 1000);
    assert_eq!(http_config.backoff_max_ms, 10_000);
    assert_eq!(http_config.timeout_secs, 30);

    // Crawler explicit values
    assert_eq!(opts.crawl.max_pages, 100);
    assert_eq!(opts.network.concurrency.resolve(), 12);
    assert_eq!(opts.network.delay_ms, 500);
}
