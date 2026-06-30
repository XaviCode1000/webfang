//! Integration tests for sitemap and resume/state flag behavior (T8 + T9).
//!
//! Verifies that sitemap discovery flags and resume/state tracking flags
//! flow correctly through CrawlOptions and that the resume filtering logic
//! behaves as expected.
//!
//! Run with: cargo nextest run --test flag_audit_sitemap_resume_test

use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::cli::scrape_flow::apply_resume_mode;
use rust_scraper::domain::ExportState;
use rust_scraper::infrastructure::export::state_store::StateStore;
use rust_scraper::{Args, ConcurrencyConfig, ExportFormat, OutputFormat};
use std::path::PathBuf;
use url::Url;

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
        export_format: ExportFormat::Jsonl,
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

// ============================================================================
// T8: Sitemap Flags
// ============================================================================

// ── T8.1: use_sitemap=true flows through ──────────────────────────────────

#[test]
fn test_sitemap_use_sitemap_true() {
    let args = Args {
        use_sitemap: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.crawl.use_sitemap,
        "use_sitemap=true must flow to CrawlLimits"
    );
}

// ── T8.2: use_sitemap defaults to false ───────────────────────────────────

#[test]
fn test_sitemap_use_sitemap_default_false() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(!opts.crawl.use_sitemap, "use_sitemap must default to false");
}

// ── T8.3: sitemap_url flows through ──────────────────────────────────────

#[test]
fn test_sitemap_url_flows_through() {
    let args = Args {
        sitemap_url: Some("https://example.com/sitemap.xml".into()),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.crawl.sitemap_url.as_deref(),
        Some("https://example.com/sitemap.xml"),
        "sitemap_url must flow to CrawlLimits"
    );
}

// ── T8.4: sitemap_url default is None ────────────────────────────────────

#[test]
fn test_sitemap_url_default_is_none() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(
        opts.crawl.sitemap_url.is_none(),
        "sitemap_url must default to None"
    );
}

// ── T8.5: sitemap_depth is on Args (CrawlLimits has no sitemap_depth) ────
//
// sitemap_depth lives on Args but is NOT mapped into CrawlLimits.
// This test documents the current contract: Args carries it, CrawlLimits
// does not. If the design changes, update CrawlLimits + From<Args>.

#[test]
fn test_sitemap_depth_on_args_not_in_crawl_limits() {
    let args = Args {
        sitemap_depth: 5,
        ..base_args()
    };
    // Verify Args carries sitemap_depth
    assert_eq!(args.sitemap_depth, 5);
    // Verify CrawlLimits does NOT have sitemap_depth
    let opts = opts_from_args(args);
    // max_depth is a different field — confirm it's unaffected
    assert_eq!(opts.crawl.max_depth, 2);
}

// ── T8.6: sitemap defaults on CrawlLimits ────────────────────────────────

#[test]
fn test_sitemap_defaults_on_crawl_limits() {
    let opts = CrawlOptions::default();
    assert!(!opts.crawl.use_sitemap, "use_sitemap must default false");
    assert!(
        opts.crawl.sitemap_url.is_none(),
        "sitemap_url must default None"
    );
    assert_eq!(opts.crawl.max_depth, 2, "max_depth must default 2");
}

// ============================================================================
// T9: Resume/State Flags
// ============================================================================

// ── T9.1: resume defaults to false ───────────────────────────────────────

#[test]
fn test_resume_default_false() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(!opts.crawl.resume, "resume must default to false");
}

// ── T9.2: resume=true flows through ─────────────────────────────────────

#[test]
fn test_resume_true_flows_through() {
    let args = Args {
        resume: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(opts.crawl.resume, "resume=true must flow to CrawlLimits");
}

// ── T9.3: state_dir flows through ───────────────────────────────────────

#[test]
fn test_state_dir_flows_through() {
    let custom_dir = PathBuf::from("/tmp/my-custom-state");
    let args = Args {
        state_dir: Some(custom_dir.clone()),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.crawl.state_dir.as_ref(),
        Some(&custom_dir),
        "state_dir must flow to CrawlLimits"
    );
}

// ── T9.4: state_dir default is None ──────────────────────────────────────

#[test]
fn test_state_dir_default_derived() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(
        opts.crawl.state_dir.is_none(),
        "state_dir must default to None (derived at runtime)"
    );
}

// ── T9.5: apply_resume_mode with resume=false returns all URLs ───────────

#[tokio::test]
async fn test_apply_resume_mode_without_state() {
    let args = Args {
        resume: false,
        ..base_args()
    };
    let opts = opts_from_args(args);

    let urls: Vec<Url> = vec![
        Url::parse("https://example.com/page1").unwrap(),
        Url::parse("https://example.com/page2").unwrap(),
        Url::parse("https://example.com/page3").unwrap(),
    ];
    let original_count = urls.len();

    let (filtered, state_store) = apply_resume_mode(urls, &opts, "https://example.com").await;

    assert!(
        state_store.is_none(),
        "resume=false must not create a StateStore"
    );
    assert_eq!(
        filtered.len(),
        original_count,
        "resume=false must return all URLs unfiltered"
    );
}

// ── T9.6: apply_resume_mode with resume=true creates a StateStore ────────

#[tokio::test]
async fn test_apply_resume_mode_not_resume() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let state_dir = tmp_dir.path().to_path_buf();

    let args = Args {
        resume: true,
        state_dir: Some(state_dir),
        ..base_args()
    };
    let opts = opts_from_args(args);

    let urls: Vec<Url> = vec![
        Url::parse("https://example.com/page1").unwrap(),
        Url::parse("https://example.com/page2").unwrap(),
    ];

    let (filtered, state_store) =
        apply_resume_mode(urls.clone(), &opts, "https://example.com").await;

    // With an empty state store (no previously processed URLs), all URLs pass through
    assert!(
        state_store.is_some(),
        "resume=true must create a StateStore"
    );
    assert_eq!(
        filtered.len(),
        2,
        "empty state store must not filter any URLs"
    );
}

// ── T9.7: apply_resume_mode filters previously processed URLs ────────────

#[tokio::test]
async fn test_apply_resume_mode_filters_processed_urls() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let state_dir = tmp_dir.path().to_path_buf();

    // Pre-populate state store with one already-processed URL
    let mut store = StateStore::new("example.com");
    store.set_cache_dir(state_dir.clone());
    let mut state = ExportState::new("example.com");
    state.mark_processed("https://example.com/page1");
    store.save(&state).unwrap();

    let args = Args {
        resume: true,
        state_dir: Some(state_dir),
        ..base_args()
    };
    let opts = opts_from_args(args);

    let urls: Vec<Url> = vec![
        Url::parse("https://example.com/page1").unwrap(),
        Url::parse("https://example.com/page2").unwrap(),
    ];

    let (filtered, state_store) = apply_resume_mode(urls, &opts, "https://example.com").await;

    assert!(
        state_store.is_some(),
        "resume=true must create a StateStore"
    );
    assert_eq!(
        filtered.len(),
        1,
        "previously processed URL must be filtered out"
    );
    assert_eq!(
        filtered[0].as_str(),
        "https://example.com/page2",
        "only the unprocessed URL must remain"
    );
}

// ── T9.8: apply_resume_mode with custom state_dir uses that path ────────

#[tokio::test]
async fn test_apply_resume_mode_custom_state_dir() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let state_dir = tmp_dir.path().join("my_custom_state");
    std::fs::create_dir_all(&state_dir).unwrap();

    let args = Args {
        resume: true,
        state_dir: Some(state_dir.clone()),
        ..base_args()
    };
    let opts = opts_from_args(args);

    let urls: Vec<Url> = vec![Url::parse("https://example.com/page1").unwrap()];

    let (_, state_store) = apply_resume_mode(urls, &opts, "https://example.com").await;

    let store = state_store.expect("must create StateStore");
    let state_path = store.get_state_path();
    assert!(
        state_path.starts_with(&state_dir),
        "StateStore path must be under custom state_dir: got {}",
        state_path.display()
    );
}
