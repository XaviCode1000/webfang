//! Integration tests for Obsidian / vault flag behavior (T10).
//!
//! Verifies that vault detection, obsidian flag flows, and CrawlOptions
//! mapping work correctly using MockVault and config-level assertions.
//!
//! Run with: cargo nextest run --test flag_audit_obsidian_test

mod common;

use common::MockVault;
use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::infrastructure::obsidian::vault_detector::detect_vault;
use rust_scraper::Args;
use std::path::{Path, PathBuf};

// ============================================================================
// Helpers
// ============================================================================

/// Build CrawlOptions from CLI args.
fn opts_from_args(args: Args) -> CrawlOptions {
    CrawlOptions::from(args)
}

/// Minimal Args with obsidian fields at defaults.
fn base_args() -> Args {
    Args {
        subcommand: None,
        url: Some("https://example.com".into()),
        selector: "body".into(),
        output: PathBuf::from("output"),
        format: rust_scraper::OutputFormat::Markdown,
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

// ============================================================================
// Vault Detection
// ============================================================================

/// MockVault creates a valid `.obsidian/` structure — vault_detector should
/// recognize it when pointed at via the explicit CLI path.
#[test]
fn test_vault_detector_recognizes_mock_vault() {
    let vault = MockVault::new();
    let result = detect_vault(Some(vault.path()), None, None);
    assert!(
        result.is_some(),
        "detect_vault must recognize MockVault as a valid vault"
    );
    assert_eq!(
        result.unwrap(),
        vault.path().clone(),
        "detected vault path must match MockVault path"
    );
}

/// When --vault is set, the path flows through Args → CrawlOptions → ExportOptions.
#[test]
fn test_vault_path_flows_through() {
    let vault = MockVault::new();
    let vault_path = vault.path().clone();

    let args = Args {
        vault: Some(vault_path.clone()),
        ..base_args()
    };
    let opts = opts_from_args(args);

    assert_eq!(
        opts.export.obsidian_vault,
        Some(vault_path),
        "vault path must flow from Args to CrawlOptions.export.obsidian_vault"
    );
}

/// Default vault path is None when --vault is not provided.
#[test]
fn test_vault_path_default_is_none() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(
        opts.export.obsidian_vault.is_none(),
        "default obsidian_vault must be None"
    );
}

// ============================================================================
// Wiki Links
// ============================================================================

#[test]
fn test_obsidian_wiki_links() {
    let args = Args {
        obsidian_wiki_links: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.export.obsidian_wiki_links,
        "obsidian_wiki_links must flow through when true"
    );
}

#[test]
fn test_obsidian_wiki_links_default() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(
        !opts.export.obsidian_wiki_links,
        "obsidian_wiki_links must default to false"
    );
}

// ============================================================================
// Tags
// ============================================================================

/// When obsidian_tags is None (CLI flag omitted), the resulting Vec must be
/// empty — never None — so downstream code can always iterate.
#[test]
fn test_obsidian_tags_none() {
    let args = Args {
        obsidian_tags: None,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.export.obsidian_tags.is_empty(),
        "obsidian_tags must be empty Vec when CLI flag is None"
    );
}

#[test]
fn test_obsidian_tags_some() {
    let tags = vec!["dev".to_string(), "rust".to_string()];
    let args = Args {
        obsidian_tags: Some(tags.clone()),
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert_eq!(
        opts.export.obsidian_tags, tags,
        "obsidian_tags must flow through exactly"
    );
}

// ============================================================================
// Relative Assets
// ============================================================================

#[test]
fn test_obsidian_relative_assets() {
    let args = Args {
        obsidian_relative_assets: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.export.obsidian_relative_assets,
        "obsidian_relative_assets must flow through when true"
    );
}

#[test]
fn test_obsidian_relative_assets_default() {
    let args = base_args();
    let opts = opts_from_args(args);
    assert!(
        !opts.export.obsidian_relative_assets,
        "obsidian_relative_assets must default to false"
    );
}

// ============================================================================
// Rich Metadata
// ============================================================================

#[test]
fn test_obsidian_rich_metadata() {
    let args = Args {
        obsidian_rich_metadata: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.export.obsidian_rich_metadata,
        "obsidian_rich_metadata must flow through when true"
    );
}

// ============================================================================
// Quick Save
// ============================================================================

#[test]
fn test_obsidian_quick_save() {
    let args = Args {
        quick_save: true,
        ..base_args()
    };
    let opts = opts_from_args(args);
    assert!(
        opts.export.quick_save,
        "quick_save must flow through when true"
    );
}

// ============================================================================
// All Obsidian Flags Together
// ============================================================================

#[test]
fn test_all_obsidian_flags_together() {
    let vault = MockVault::new();
    let vault_path = vault.path().clone();

    let args = Args {
        vault: Some(vault_path.clone()),
        obsidian_wiki_links: true,
        obsidian_tags: Some(vec!["dev".into(), "rust".into()]),
        obsidian_relative_assets: true,
        obsidian_rich_metadata: true,
        quick_save: true,
        ..base_args()
    };
    let opts = opts_from_args(args);

    // Vault path
    assert_eq!(opts.export.obsidian_vault, Some(vault_path));

    // Wiki links
    assert!(opts.export.obsidian_wiki_links);

    // Tags
    assert_eq!(
        opts.export.obsidian_tags,
        vec!["dev".to_string(), "rust".to_string()]
    );

    // Relative assets
    assert!(opts.export.obsidian_relative_assets);

    // Rich metadata
    assert!(opts.export.obsidian_rich_metadata);

    // Quick save
    assert!(opts.export.quick_save);
}

// ============================================================================
// Vault Detection Priority Hierarchy
// ============================================================================

/// When --vault is set, it takes priority over env var and config.
#[test]
fn test_vault_cli_over_env_and_config() {
    let vault = MockVault::new();

    // Create a second vault to serve as "env" and "config" targets
    let other_vault = MockVault::new();

    // Set env var pointing to the OTHER vault
    std::env::set_var("RUST_SCRAPER_T10_TEST_VAULT", other_vault.path());

    let result = detect_vault(
        Some(vault.path()),
        Some("RUST_SCRAPER_T10_TEST_VAULT"),
        Some(other_vault.path().to_str().unwrap()),
    );

    assert!(result.is_some());
    assert_eq!(
        result.unwrap(),
        vault.path().clone(),
        "CLI path must take priority over env var and config"
    );

    std::env::remove_var("RUST_SCRAPER_T10_TEST_VAULT");
}

/// detect_vault returns None when no vault is found at any priority level.
#[test]
fn test_vault_not_found_returns_none() {
    let result = detect_vault(
        Some(Path::new("/nonexistent/vault/path")),
        Some("NONEXISTENT_ENV_VAR_T10"),
        None,
    );
    assert!(result.is_none(), "nonexistent path must return None");
}

/// MockVault's is_recognized_as_vault() returns true for a valid vault.
#[test]
fn test_mock_vault_is_recognized() {
    let vault = MockVault::new();
    assert!(
        vault.is_recognized_as_vault(),
        "MockVault must pass its own recognition check"
    );
}

/// MockVault's vault_json() returns the path to .obsidian/obsidian.json.
#[test]
fn test_mock_vault_json_path() {
    let vault = MockVault::new();
    let json_path = vault.vault_json();
    assert!(
        json_path.exists(),
        "vault_json() must point to an existing file"
    );
    assert!(
        json_path.to_string_lossy().contains("obsidian.json"),
        "vault_json() filename must be obsidian.json"
    );
}
