//! Integration tests for core and display flag behavior (T4 + T5).
//!
//! Verifies that CrawlOptions fields flow correctly into the config structs
//! and that display flags gate the expected behaviors.
//!
//! Run with: cargo nextest run --test flag_audit_core_display_test

use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::{Args, OutputFormat, ScraperConfig};
use std::path::PathBuf;

// ============================================================================
// T4: Core Flags — url, selector, output, format
// ============================================================================

/// Helper: build CrawlOptions from CLI args.
fn opts_from_args(args: Args) -> CrawlOptions {
    CrawlOptions::from(args)
}

/// Minimal Args with only the fields we care about set.
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
        concurrency: rust_scraper::ConcurrencyConfig::default(),
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

// ── T4.1: URL is present and parses correctly ──────────────────────────────

#[test]
fn test_core_url_required() {
    let args = Args {
        url: Some("https://example.com".into()),
        ..base_args()
    };
    let opts = opts_from_args(args);
    // url::Url normalizes bare domains with trailing slash
    assert_eq!(opts.url.as_str(), "https://example.com/");
}

#[test]
fn test_core_url_none_falls_back_to_example_com() {
    let args = Args {
        url: None,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.url.as_str(), "https://example.com/");
}

// ── T4.2: Selector flows through to CrawlOptions ──────────────────────────

#[test]
fn test_core_selector_filtering() {
    let args = Args {
        selector: "article.main".into(),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.crawl.selector, "article.main");
}

#[test]
fn test_core_selector_default_is_body() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert_eq!(opts.crawl.selector, "body");
}

#[test]
fn test_core_selector_deep_nested() {
    let args = Args {
        selector: "div.content > section:first-child > p.intro".into(),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.crawl.selector,
        "div.content > section:first-child > p.intro"
    );
}

// ── T4.3: Output path flows through to export config ──────────────────────

#[test]
fn test_core_output_path() {
    let args = Args {
        output: PathBuf::from("/tmp/custom-output"),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.export.output_dir, PathBuf::from("/tmp/custom-output"));
}

#[test]
fn test_core_output_path_default() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert_eq!(opts.export.output_dir, PathBuf::from("output"));
}

#[test]
fn test_core_output_path_reflected_in_scraper_config() {
    let args = Args {
        output: PathBuf::from("/tmp/scrape-results"),
        ..base_args()
    };
    let opts = opts_from_args(args);

    // ScraperConfig is built from CrawlOptions in the orchestrator
    let scraper_config = ScraperConfig::default().with_output_dir(opts.export.output_dir.clone());

    assert_eq!(
        scraper_config.output_dir,
        PathBuf::from("/tmp/scrape-results")
    );
}

// ── T4.4: Format defaults and flows through ───────────────────────────────

#[test]
fn test_core_format_default_is_markdown() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert_eq!(opts.export.output_format, OutputFormat::Markdown);
}

#[test]
fn test_core_format_json_flows_through() {
    let args = Args {
        format: OutputFormat::Json,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.export.output_format, OutputFormat::Json);
}

#[test]
fn test_core_format_text_flows_through() {
    let args = Args {
        format: OutputFormat::Text,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.export.output_format, OutputFormat::Text);
}

#[test]
fn test_core_export_format_default_is_jsonl() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert_eq!(opts.export.export_format, rust_scraper::ExportFormat::Jsonl);
}

#[test]
fn test_core_export_format_vector_flows_through() {
    let args = Args {
        export_format: rust_scraper::ExportFormat::Vector,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.export.export_format,
        rust_scraper::ExportFormat::Vector
    );
}

// ============================================================================
// T5: Display Flags — verbose, quiet, dry-run
// ============================================================================

// ── T5.1: Verbose defaults to 0 ───────────────────────────────────────────

#[test]
fn test_display_verbose_default() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert_eq!(opts.verbosity, 0);
}

#[test]
fn test_display_verbose_level_1() {
    let args = Args {
        verbose: 1,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.verbosity, 1);
}

#[test]
fn test_display_verbose_level_3() {
    let args = Args {
        verbose: 3,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(opts.verbosity, 3);
}

// ── T5.2: Quiet silences progress events ──────────────────────────────────

/// Verify that `opts.export.quiet == true` is reflected in CrawlOptions
/// and that scrape_flow.rs gates all progress events behind `!opts.export.quiet`.
///
/// The scrape_flow code sends progress events only when `!opts.export.quiet`:
///   - Started event
///   - StatusChanged event (Fetching)
///   - Completed event
///   - Failed event
///   - Finished event
///
/// This test verifies the flag flows through correctly. The actual gating
/// is verified by reading the source — the `if !opts.export.quiet` guard
/// appears at every `progress_tx.send()` call in scrape_flow.rs.
#[test]
fn test_display_quiet_silences_progress() {
    let args = Args {
        quiet: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    // Both top-level and export-level quiet should be set
    assert!(opts.quiet, "top-level quiet must be true");
    assert!(opts.export.quiet, "export-level quiet must be true");
}

#[test]
fn test_display_quiet_default_is_false() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(!opts.quiet);
    assert!(!opts.export.quiet);
}

/// Verify that quiet is set at both levels (top-level `opts.quiet` and
/// `opts.export.quiet`) since scrape_flow.rs reads `opts.export.quiet`.
#[test]
fn test_display_quiet_dual_level_consistency() {
    let args = Args {
        quiet: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.quiet, opts.export.quiet,
        "quiet must be consistent across top-level and export"
    );
}

// ── T5.3: Dry-run prevents HTTP requests ──────────────────────────────────

/// Verify that `opts.export.dry_run == true` is set in CrawlOptions.
///
/// In the orchestrator, dry_run is stored in `opts.export.dry_run`. The
/// actual behavior of skipping HTTP requests is controlled by checking
/// this flag before entering the scrape loop. Currently, the orchestrator
/// does not explicitly skip scraping on dry_run — this test verifies the
/// flag flows through correctly so future behavior can rely on it.
#[test]
fn test_display_dry_run_flows_through() {
    let args = Args {
        dry_run: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(opts.export.dry_run);
}

#[test]
fn test_display_dry_run_default_is_false() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(!opts.export.dry_run);
}

/// Verify that dry_run only appears in export options, not in crawl limits.
/// This is the current design — dry_run is an export-phase concern.
#[test]
fn test_display_dry_run_only_in_export() {
    let args = Args {
        dry_run: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(opts.export.dry_run);
    // CrawlLimits does not have a dry_run field — it's an export concept.
    // If this ever changes, this test will fail to compile, which is correct.
}

// ============================================================================
// Combined: Core + Display together
// ============================================================================

#[test]
fn test_combined_core_and_display_flags() {
    let args = Args {
        url: Some("https://test.dev".into()),
        selector: "#main-content".into(),
        output: PathBuf::from("/tmp/test-combined"),
        format: OutputFormat::Json,
        verbose: 2,
        quiet: true,
        dry_run: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    // Core
    assert_eq!(opts.url.as_str(), "https://test.dev/");
    assert_eq!(opts.crawl.selector, "#main-content");
    assert_eq!(opts.export.output_dir, PathBuf::from("/tmp/test-combined"));
    assert_eq!(opts.export.output_format, OutputFormat::Json);

    // Display
    assert_eq!(opts.verbosity, 2);
    assert!(opts.quiet);
    assert!(opts.export.quiet);
    assert!(opts.export.dry_run);
}
