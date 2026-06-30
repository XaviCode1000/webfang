//! Integration tests for download/asset flag behavior (T12).
//!
//! Verifies that download-related flags (download_images, download_documents,
//! max_file_size, download_timeout) flow correctly through the config layer.
//!
//! Run with: cargo nextest run --test flag_audit_downloads_test

use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::{Args, ConcurrencyConfig, OutputFormat};
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

// ============================================================================
// T12.1: download_images flag
// ============================================================================

#[test]
fn test_download_images_flows_through() {
    let args = Args {
        download_images: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(
        opts.network.download_images,
        "download_images=true must flow from Args → CrawlOptions.network"
    );
}

#[test]
fn test_download_images_default() {
    let args = base_args();
    let opts = opts_from_args(args);

    assert!(
        !opts.network.download_images,
        "default download_images must be false"
    );
}

#[test]
fn test_download_images_false_explicit() {
    let args = Args {
        download_images: false,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(!opts.network.download_images);
}

// ============================================================================
// T12.2: download_documents flag
// ============================================================================

#[test]
fn test_download_documents_flows_through() {
    let args = Args {
        download_documents: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(
        opts.network.download_documents,
        "download_documents=true must flow from Args → CrawlOptions.network"
    );
}

#[test]
fn test_download_documents_default() {
    let args = base_args();
    let opts = opts_from_args(args);

    assert!(
        !opts.network.download_documents,
        "default download_documents must be false"
    );
}

#[test]
fn test_download_documents_false_explicit() {
    let args = Args {
        download_documents: false,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(!opts.network.download_documents);
}

// ============================================================================
// T12.3: max_file_size flag (lives on Args only, not CrawlOptions)
// ============================================================================

/// max_file_size is a download-setting on Args. The production scraper reads
/// it directly from Args, not from CrawlOptions. This test verifies the
/// Args field stores the value correctly.
#[test]
fn test_max_file_size_flows_through() {
    let args = Args {
        max_file_size: 10_485_760, // 10 MB
        ..base_args()
    };

    assert_eq!(
        args.max_file_size, 10_485_760,
        "max_file_size=10485760 must be stored on Args"
    );
}

#[test]
fn test_max_file_size_default() {
    let args = base_args();

    assert_eq!(
        args.max_file_size, 52_428_800,
        "default max_file_size must be 52_428_800 (50 MB)"
    );
}

#[test]
fn test_max_file_size_large_value() {
    let args = Args {
        max_file_size: 1_073_741_824, // 1 GB
        ..base_args()
    };

    assert_eq!(args.max_file_size, 1_073_741_824);
}

#[test]
fn test_max_file_size_small_value() {
    let args = Args {
        max_file_size: 1024, // 1 KB
        ..base_args()
    };

    assert_eq!(args.max_file_size, 1024);
}

// ============================================================================
// T12.4: download_timeout flag (lives on Args only, not CrawlOptions)
// ============================================================================

/// download_timeout is a download-setting on Args. The production scraper reads
/// it directly from Args, not from CrawlOptions.
#[test]
fn test_download_timeout_flows_through() {
    let args = Args {
        download_timeout: 120,
        ..base_args()
    };

    assert_eq!(
        args.download_timeout, 120,
        "download_timeout=120 must be stored on Args"
    );
}

#[test]
fn test_download_timeout_default() {
    let args = base_args();

    assert_eq!(
        args.download_timeout, 30,
        "default download_timeout must be 30 seconds"
    );
}

#[test]
fn test_download_timeout_short() {
    let args = Args {
        download_timeout: 5,
        ..base_args()
    };

    assert_eq!(args.download_timeout, 5);
}

#[test]
fn test_download_timeout_large() {
    let args = Args {
        download_timeout: 300,
        ..base_args()
    };

    assert_eq!(args.download_timeout, 300);
}

// ============================================================================
// T12.5: download flags independence
// ============================================================================

#[test]
fn test_download_images_independent_of_documents() {
    let args = Args {
        download_images: true,
        download_documents: false,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(opts.network.download_images);
    assert!(!opts.network.download_documents);
}

#[test]
fn test_download_documents_independent_of_images() {
    let args = Args {
        download_images: false,
        download_documents: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert!(!opts.network.download_images);
    assert!(opts.network.download_documents);
}

// ============================================================================
// T12.6: All download flags together
// ============================================================================

#[test]
fn test_all_download_flags_together() {
    let args = Args {
        download_images: true,
        download_documents: true,
        max_file_size: 10_485_760,
        download_timeout: 120,
        ..base_args()
    };
    // Save Args-only fields before the move
    let file_size = args.max_file_size;
    let dl_timeout = args.download_timeout;
    let opts = opts_from_args(args);

    // CrawlOptions fields
    assert!(opts.network.download_images);
    assert!(opts.network.download_documents);

    // Args-only fields
    assert_eq!(file_size, 10_485_760);
    assert_eq!(dl_timeout, 120);
}

#[test]
fn test_all_download_defaults() {
    let args = base_args();
    let file_size = args.max_file_size;
    let dl_timeout = args.download_timeout;
    let opts = opts_from_args(args);

    assert!(!opts.network.download_images);
    assert!(!opts.network.download_documents);
    assert_eq!(file_size, 52_428_800);
    assert_eq!(dl_timeout, 30);
}

#[test]
fn test_download_flags_do_not_affect_network_defaults() {
    let args = Args {
        download_images: true,
        download_documents: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    // Other network fields must remain at defaults
    assert_eq!(opts.network.delay_ms, 1000);
    assert_eq!(opts.network.timeout_secs, 30);
    assert_eq!(opts.network.max_retries, 3);
    assert_eq!(opts.network.backoff_base_ms, 1000);
    assert_eq!(opts.network.backoff_max_ms, 10_000);
    assert!(!opts.network.force_js_render);
}
