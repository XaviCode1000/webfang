use clap::Parser;
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use webfang_core::cli::args::{AiArgs, Args, CrawlerArgs, ExportArgs, ObsidianArgs, TuiArgs};
use webfang_core::infrastructure::autotuning::ElasticOverrides;

/// Remove poisoned env vars once before any arg-parsing test runs.
static ENV_GUARD: std::sync::Once = std::sync::Once::new();
fn clean_env() {
    ENV_GUARD.call_once(|| {
        let poisoned: Vec<String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
            .map(|(k, _)| k)
            .collect();
        for key in poisoned {
            std::env::remove_var(&key);
        }
    });
}

#[test]
fn test_elastic_flags_parsed_from_cli() {
    clean_env();
    let args = Args::try_parse_from([
        "webfang",
        "--cpu-cores",
        "4",
        "--ram-budget",
        "8GB",
        "--db-path",
        "/tmp/elastic.db",
    ])
    .expect("flags must parse");
    assert_eq!(args.export.cpu_cores, Some(4));
    assert_eq!(args.export.ram_budget, Some(8 * 1024 * 1024 * 1024));
    assert_eq!(
        args.export.db_path.as_deref(),
        Some(Path::new("/tmp/elastic.db"))
    );

    let overrides = args.elastic_overrides();
    assert_eq!(overrides.cpu_cores, Some(4));
    assert_eq!(overrides.ram_budget_bytes, Some(8 * 1024 * 1024 * 1024));
    assert_eq!(overrides.db_path, Some(PathBuf::from("/tmp/elastic.db")));
}

#[test]
fn test_elastic_flags_default_to_none() {
    clean_env();
    let args = Args::try_parse_from(["webfang"]).expect("minimal parse must succeed");
    assert_eq!(args.export.cpu_cores, None);
    assert_eq!(args.export.ram_budget, None);
    assert_eq!(args.export.db_path, None);
    // No overrides → equals the all-None default.
    assert_eq!(args.elastic_overrides(), ElasticOverrides::default());
}

#[test]
fn test_ram_budget_accepts_plain_bytes_and_suffixes() {
    clean_env();
    let args = Args::try_parse_from(["webfang", "--ram-budget", "2048MB"])
        .expect("suffixed ram-budget must parse");
    assert_eq!(
        args.elastic_overrides().ram_budget_bytes,
        Some(2048 * 1024 * 1024)
    );
}

#[test]
fn test_cpu_cores_and_reject_invalid_values() {
    clean_env();

    let cases: &[(&str, &str, &str)] = &[
        ("--cpu-cores", "0", "cpu-cores debe ser > 0"),
        ("--ram-budget", "0", "ram-budget debe ser > 0"),
        (
            "--ram-budget",
            "banana",
            "no es un tamaño de memoria válido",
        ),
    ];

    for (flag, value, expected_msg) in cases {
        let err = Args::try_parse_from(["webfang", flag, value])
            .expect_err(&format!("{flag} {value} must be rejected"));
        assert!(
            err.to_string().contains(expected_msg),
            "unexpected error for {flag} {value}: {err}"
        );
    }
}

// ========================================================================
// Args → CrawlOptions full parity test
// ========================================================================

/// Build a minimal `Args` with **every** field set to a non-default,
/// identifiable value so we can assert 1:1 mapping into `CrawlOptions`.
fn args_with_all_fields_set() -> Args {
    Args {
        subcommand: None,
        positional_url: None,

        crawler: CrawlerArgs {
            url: Some("https://example.com/test".into()),
            selector: "article.main".into(),
            delay_ms: 500,
            max_pages: 25,
            concurrency: webfang_core::ConcurrencyConfig::new(8),
            use_sitemap: true,
            sitemap_url: Some("https://example.com/sitemap.xml".into()),
            single_page: true,
            resume: true,
            state_dir: Some(std::path::PathBuf::from("/tmp/state")),
            download_images: true,
            download_documents: true,
            clean_ai: true,
            verbose: 3,
            quiet: true,
            dry_run: true,
            max_depth: 5,
            timeout_secs: 60,
            include_patterns: vec!["/blog/**".into(), "/docs/**".into()],
            exclude_patterns: vec!["/admin/**".into()],
            max_retries: 7,
            backoff_base_ms: 2000,
            backoff_max_ms: 30_000,
            accept_language: "es-ES,es;q=0.9".into(),
            user_agent: Some("TestAgent/1.0".into()),
            max_file_size: 100_000_000,
            download_timeout: 120,
            sitemap_depth: 4,
            checkpoint_interval: 50,
            no_checkpoint: true,
            ignore_robots: true,
            no_session_health: true,
            autoscale: true,
            h2_profile: "Chrome131".into(),
            js_strategy: webfang_core::domain::JsStrategy::Hybrid,
            obscura_binary: "/usr/local/bin/obscura".into(),
            asset_naming: "slug".into(),
            download_concurrency: Some(5),
            ..Default::default()
        },

        export: ExportArgs {
            output: std::path::PathBuf::from("/tmp/test-output"),
            format: webfang_core::OutputFormat::Json,
            export_format: webfang_core::ExportFormat::Vector,
            cpu_cores: Some(6),
            ram_budget: Some(4 * 1024 * 1024 * 1024),
            db_path: Some(std::path::PathBuf::from("/tmp/test.db")),
            elastic: true,
            output_vectors: None,
            batch: true,
            batch_file: Some(std::path::PathBuf::from("/tmp/urls.txt")),
            batch_concurrency: Some(8),
            pipeline: true,
            pipeline_output: webfang_core::domain::config::PipelineOutputFormat::None,
        },

        obsidian: ObsidianArgs {
            obsidian_wiki_links: true,
            obsidian_tags: Some(vec!["tag-a".into(), "tag-b".into()]),
            obsidian_relative_assets: true,
            vault: Some(std::path::PathBuf::from("/tmp/vault")),
            quick_save: true,
            obsidian_rich_metadata: true,
        },

        ai: AiArgs::default(),

        tui: TuiArgs::default(),
    }
}

fn assert_full_parity_top_level(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.url.as_str(), "https://example.com/test");
    assert_eq!(opts.verbosity, 3);
    assert!(opts.quiet);
}

fn assert_full_parity_crawl_limits(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.crawl.selector, "article.main");
    assert_eq!(opts.crawl.max_depth, 5);
    assert_eq!(opts.crawl.max_pages, 25);
    assert!(opts.crawl.single_page);
    assert_eq!(
        opts.crawl.include_patterns,
        vec!["/blog/**".to_owned(), "/docs/**".to_owned()]
    );
    assert_eq!(opts.crawl.exclude_patterns, vec!["/admin/**".to_owned()]);
    assert!(!opts.crawl.interactive);
    assert!(opts.crawl.resume);
    assert_eq!(
        opts.crawl.state_dir,
        Some(std::path::PathBuf::from("/tmp/state"))
    );
    assert!(opts.crawl.use_sitemap);
    assert_eq!(
        opts.crawl.sitemap_url.as_deref(),
        Some("https://example.com/sitemap.xml")
    );
    assert_eq!(opts.crawl.checkpoint_interval, 50);
    assert!(opts.crawl.no_checkpoint);
    assert!(opts.crawl.ignore_robots);
    assert!(opts.crawl.no_session_health);
    assert!(opts.crawl.autoscale_enabled);
}

fn assert_full_parity_network(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.network.user_agent.as_deref(), Some("TestAgent/1.0"));
    assert_eq!(opts.network.accept_language, "es-ES,es;q=0.9");
    assert!(!opts.network.concurrency.is_auto());
    assert_eq!(opts.network.concurrency.get(), Some(8));
    assert_eq!(opts.network.delay_ms, 500);
    assert_eq!(opts.network.timeout_secs, 60);
    assert_eq!(opts.network.max_retries, 7);
    assert_eq!(opts.network.backoff_base_ms, 2000);
    assert_eq!(opts.network.backoff_max_ms, 30_000);
    assert!(opts.network.download_images);
    assert!(opts.network.download_documents);
    assert_eq!(opts.network.h2_profile, "Chrome131");
    assert_eq!(
        opts.network.js_strategy,
        webfang_core::domain::JsStrategy::Hybrid
    );
    assert_eq!(opts.network.obscura_binary, "/usr/local/bin/obscura");
}

fn assert_full_parity_export(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.export.output_format, webfang_core::OutputFormat::Json);
    assert_eq!(
        opts.export.export_format,
        webfang_core::ExportFormat::Vector
    );
    assert_eq!(
        opts.export.output_dir,
        std::path::PathBuf::from("/tmp/test-output")
    );
    assert!(opts.export.dry_run);
    assert!(opts.export.quiet);
    assert_eq!(
        opts.export.obsidian_vault,
        Some(std::path::PathBuf::from("/tmp/vault"))
    );
    assert!(opts.export.obsidian_rich_metadata);
    assert_eq!(
        opts.export.obsidian_tags,
        vec!["tag-a".to_owned(), "tag-b".to_owned()]
    );
    assert!(opts.export.obsidian_wiki_links);
    assert!(opts.export.obsidian_relative_assets);
    assert!(opts.export.quick_save);
}

fn assert_full_parity_ingestion_tuning(
    opts: &webfang_core::application::crawl_options::CrawlOptions,
) {
    assert!(opts.elastic.enabled);
    assert_eq!(opts.elastic.cpu_cores, Some(6));
    assert_eq!(opts.elastic.ram_budget_bytes, Some(4 * 1024 * 1024 * 1024));
    assert_eq!(
        opts.elastic.db_path,
        Some(std::path::PathBuf::from("/tmp/test.db"))
    );
}

fn assert_full_parity_item_pipeline(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert!(opts.pipeline_enabled);
    assert_eq!(
        opts.pipeline_output_format,
        webfang_core::domain::config::PipelineOutputFormat::None
    );
}

fn assert_full_parity_asset_naming(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.asset_naming, "slug");
    assert_eq!(opts.download_concurrency, Some(5));
}

#[test]
fn test_args_to_crawl_options_full_parity() {
    clean_env();
    let args = args_with_all_fields_set();
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    // ── Top-level ──────────────────────────────────────────────────────
    assert_full_parity_top_level(&opts);

    // ── CrawlLimits ────────────────────────────────────────────────────
    assert_full_parity_crawl_limits(&opts);

    // ── NetworkOptions ─────────────────────────────────────────────────
    assert_full_parity_network(&opts);

    // ── ExportOptions ──────────────────────────────────────────────────
    assert_full_parity_export(&opts);

    // ── IngestionTuning ────────────────────────────────────────────────
    assert_full_parity_ingestion_tuning(&opts);

    // ── Item Pipeline ─────────────────────────────────────────────────
    assert_full_parity_item_pipeline(&opts);

    // ── Asset naming ─────────────────────────────────────────────────
    assert_full_parity_asset_naming(&opts);

    // ── AiConfig (defaults when AI flags not set) ─────────────────────
    // When feature="ai" is OFF, ai_config should be Default (0.3/32768/false/"")
    // When feature="ai" is ON, ai_config should reflect the AI flag values
    // (tested separately in test_ai_config_parity_* below)
}

// ========================================================================
// AiConfig parity tests (Scenario 2.3.S1, 2.3.S3)
// ========================================================================

#[cfg(feature = "ai")]
#[test]
fn test_ai_config_parity_with_flags() {
    clean_env();
    use webfang_core::application::crawl_options::AiConfig;

    let args = Args::try_parse_from([
        "webfang",
        "--url",
        "https://example.com",
        "--threshold",
        "0.5",
        "--max-tokens",
        "1024",
        "--offline",
        "--ai-model",
        "granite-311m",
    ])
    .expect("flags must parse");

    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    assert_eq!(
        opts.ai_config,
        AiConfig {
            threshold: 0.5,
            max_tokens: 1024,
            offline: true,
            model: "granite-311m".to_string(),
        }
    );
}

#[cfg(feature = "ai")]
#[test]
fn test_ai_config_parity_no_flags() {
    clean_env();
    use webfang_core::application::crawl_options::AiConfig;

    let _guard = webfang_test_utils::EnvGuard::clean(&[
        "WEBFANG_THRESHOLD",
        "WEBFANG_MAX_TOKENS",
        "WEBFANG_OFFLINE",
        "AI_MODEL_ID",
    ]);

    let args = Args::try_parse_from(["webfang"]).expect("minimal parse must succeed");
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    // Default values must reproduce the prior hardcoded behavior (Scenario 2.3.S3)
    assert_eq!(
        opts.ai_config,
        AiConfig {
            threshold: 0.3,
            max_tokens: 32768,
            offline: false,
            model: String::new(),
        }
    );
}

#[cfg(not(feature = "ai"))]
#[test]
fn test_ai_config_defaults_without_ai_feature() {
    clean_env();
    use webfang_core::application::crawl_options::AiConfig;

    let args = Args::try_parse_from(["webfang"]).expect("minimal parse must succeed");
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    // Without AI feature, ai_config should always be Default
    assert_eq!(opts.ai_config, AiConfig::default());
}

// ========================================================================
// #827 — poisoned AI_MODEL_ID / WEBFANG_AI_MODEL_ID must not break
// unrelated CLI paths (slice 5b / #980). The canary must cover BOTH the
// canonical (read by clap via the spec env-fallback) and the legacy
// (read by `webfang_ai::infrastructure_ai::compat::read_ai_model_id`).
// ========================================================================

/// A poisoned canonical `WEBFANG_AI_MODEL_ID` (read by clap via the spec
/// env-fallback on `--ai-model`) must not make every invocation fail
/// with `InvalidValue`: model validation is deferred to the AI init path
/// (#827, #980 slice 5b).
#[cfg(feature = "ai")]
#[test]
fn test_poisoned_webfang_ai_model_id_env_parses_valid_scrape_command() {
    clean_env();
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_AI_MODEL_ID", "not-a-model")]);

    let result = Args::try_parse_from(["webfang", "--url", "https://example.com"]);

    assert!(
        result.is_ok(),
        "poisoned WEBFANG_AI_MODEL_ID must not break arg parsing: {:?}",
        result.err().map(|e| e.to_string())
    );
}

/// Legacy `AI_MODEL_ID` is no longer read by clap, but
/// `webfang_ai::infrastructure_ai::compat::read_ai_model_id` still honors
/// it. A poisoned legacy value must not break arg parsing either (#827,
/// #980 slice 5b).
#[cfg(feature = "ai")]
#[test]
fn test_poisoned_ai_model_id_legacy_env_parses_valid_scrape_command() {
    clean_env();
    let _guard = webfang_test_utils::EnvGuard::with(&[("AI_MODEL_ID", "not-a-model")]);

    let result = Args::try_parse_from(["webfang", "--url", "https://example.com"]);

    assert!(
        result.is_ok(),
        "poisoned AI_MODEL_ID (legacy) must not break arg parsing: {:?}",
        result.err().map(|e| e.to_string())
    );
}

/// An explicitly invalid `--ai-model` value must be accepted as a raw
/// string by clap and rejected later, inside the AI initialization path,
/// where the error surfaces as a Spanish usage message (#827).
#[cfg(feature = "ai")]
#[test]
fn test_invalid_ai_model_value_defers_validation_to_ai_init_path() {
    clean_env();

    let args = Args::try_parse_from(["webfang", "--ai-model", "bogus"])
        .expect("raw --ai-model must parse; validation belongs to the AI init path");

    assert_eq!(args.ai.ai_model.as_deref(), Some("bogus"));
}

// ========================================================================
// #791 — --dom-preprune flag tests
// ========================================================================

#[test]
fn test_dom_preprune_defaults_to_true() {
    // WEBFANG_AI_MODEL_ID must be unset too: clap reads it via the spec
    // env-fallback on --ai-model, so a poisoned value (CI bug-discovery)
    // fails every parse with InvalidValue before the flag under test is
    // even evaluated. Legacy AI_MODEL_ID is cleared for symmetry even
    // though clap no longer reads it (#980 slice 5b).
    let _guard = webfang_test_utils::EnvGuard::clean(&[
        "WEBFANG_DOM_PREPRUNE",
        "WEBFANG_AI_MODEL_ID",
        "AI_MODEL_ID",
    ]);
    // Default value from clap is true
    let args = Args::try_parse_from(["webfang", "-u", "https://example.com"]).expect("valid args");
    assert!(args.crawler.dom_preprune, "dom_preprune defaults to true");
}

#[test]
fn test_dom_preprune_flag_enables() {
    clean_env();
    // With default_value="true", --dom-preprune alone uses the default
    let args = Args::try_parse_from(["webfang", "-u", "https://example.com", "--dom-preprune"])
        .expect("valid args");
    assert!(
        args.crawler.dom_preprune,
        "--dom-preprune enables dom_preprune (uses default_value)"
    );
}

#[test]
fn test_dom_preprune_false() {
    clean_env();
    let args = Args::try_parse_from([
        "webfang",
        "-u",
        "https://example.com",
        "--dom-preprune=false",
    ])
    .expect("valid args");
    assert!(
        !args.crawler.dom_preprune,
        "--dom-preprune=false disables dom_preprune"
    );
}

#[test]
fn test_dom_preprune_env_var_true() {
    // clean_env() unsets WEBFANG_AI_MODEL_ID (clap env fallback on
    // --ai-model would reject a poisoned value) AND AI_MODEL_ID (legacy
    // path exercised by `webfang_ai::infrastructure_ai::compat`); see
    // #980 slice 5b. EnvGuard::with then sets only the flag under test.
    // Two EnvGuards cannot be nested (ENV_LOCK deadlock).
    clean_env();
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_DOM_PREPRUNE", "true")]);
    let args = Args::try_parse_from(["webfang", "-u", "https://example.com"]).expect("valid args");
    assert!(
        args.crawler.dom_preprune,
        "WEBFANG_DOM_PREPRUNE=true enables dom_preprune"
    );
}

#[test]
fn test_dom_preprune_env_var_false() {
    // Same hermeticity rationale as test_dom_preprune_env_var_true.
    clean_env();
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_DOM_PREPRUNE", "false")]);
    let args = Args::try_parse_from(["webfang", "-u", "https://example.com"]).expect("valid args");
    assert!(
        !args.crawler.dom_preprune,
        "WEBFANG_DOM_PREPRUNE=false disables dom_preprune"
    );
}

fn assert_defaults_top_level(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    // url defaults to example.com when None
    assert_eq!(opts.url.as_str(), "https://example.com/");
    assert_eq!(opts.verbosity, 0);
    assert!(!opts.quiet);
}

fn assert_defaults_crawl(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(opts.crawl.selector, "body");
    assert_eq!(opts.crawl.max_depth, 2);
    assert_eq!(opts.crawl.max_pages, 10);
    assert!(!opts.crawl.single_page);
    assert!(opts.crawl.include_patterns.is_empty());
    assert!(opts.crawl.exclude_patterns.is_empty());
    assert!(!opts.crawl.interactive);
    assert!(!opts.crawl.resume);
    assert!(opts.crawl.state_dir.is_none());
    assert!(!opts.crawl.use_sitemap);
    assert!(opts.crawl.sitemap_url.is_none());
    // dom_preprune: true is set in CrawlLimits::default() and in ScraperConfig::default()
    assert!(
        opts.crawl.dom_preprune,
        "dom_preprune defaults to true in CrawlLimits"
    );
}

fn assert_defaults_network(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert!(opts.network.user_agent.is_none());
    assert_eq!(opts.network.accept_language, "en-US,en;q=0.9");
    assert!(opts.network.concurrency.is_auto());
    assert_eq!(opts.network.delay_ms, 1000);
    assert_eq!(opts.network.timeout_secs, 30);
    assert_eq!(opts.network.max_retries, 3);
    assert_eq!(opts.network.backoff_base_ms, 1000);
    assert_eq!(opts.network.backoff_max_ms, 10_000);
    assert!(!opts.network.download_images);
    assert!(!opts.network.download_documents);
}

fn assert_defaults_export(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert_eq!(
        opts.export.output_format,
        webfang_core::OutputFormat::Markdown
    );
    assert_eq!(opts.export.export_format, webfang_core::ExportFormat::Jsonl);
    assert_eq!(opts.export.output_dir, std::path::PathBuf::from("output"));
    assert!(!opts.export.dry_run);
    assert!(!opts.export.quiet);
    assert!(opts.export.obsidian_vault.is_none());
    assert!(!opts.export.obsidian_rich_metadata);
    assert!(opts.export.obsidian_tags.is_empty());
    assert!(!opts.export.obsidian_wiki_links);
    assert!(!opts.export.obsidian_relative_assets);
    assert!(!opts.export.quick_save);
}

fn assert_defaults_elastic(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert!(!opts.elastic.enabled);
    assert!(opts.elastic.cpu_cores.is_none());
    assert!(opts.elastic.ram_budget_bytes.is_none());
    assert!(opts.elastic.db_path.is_none());
}

fn assert_defaults_pipeline(opts: &webfang_core::application::crawl_options::CrawlOptions) {
    assert!(!opts.pipeline_enabled);
    assert_eq!(
        opts.pipeline_output_format,
        webfang_core::domain::config::PipelineOutputFormat::Jsonl
    );
    assert!(!opts.crawl.autoscale_enabled);
    // CLI default_value = "hash" (via #[arg(default_value)])
    assert_eq!(opts.asset_naming, "hash");
}

/// Tests `CrawlOptions::default()` directly — hermetic, no env reads, no CLI parsing.
#[test]
fn test_args_to_crawl_options_defaults() {
    clean_env();
    let opts = webfang_core::application::crawl_options::CrawlOptions::default();

    assert_defaults_top_level(&opts);
    assert_defaults_crawl(&opts);
    assert_defaults_network(&opts);
    assert_defaults_export(&opts);
    assert_defaults_elastic(&opts);
    assert_defaults_pipeline(&opts);
}

#[test]
fn test_obsidian_tags_none_maps_to_empty_vec() {
    clean_env();
    let args = Args {
        obsidian: ObsidianArgs {
            obsidian_tags: None,
            ..args_with_all_fields_set().obsidian
        },
        ..args_with_all_fields_set()
    };
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);
    assert!(opts.export.obsidian_tags.is_empty());
}

#[test]
fn test_url_none_falls_back_to_example_com() {
    clean_env();
    let args = Args {
        crawler: CrawlerArgs {
            url: None,
            ..args_with_all_fields_set().crawler
        },
        ..args_with_all_fields_set()
    };
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);
    assert_eq!(opts.url.as_str(), "https://example.com/");
}

// ========================================================================
// Property-based tests with proptest
// ========================================================================

proptest! {
    #[cfg_attr(miri, ignore)] // proptest too slow under Miri interpreter (~2-11min per test)
    #[test]
    fn prop_bool_fields_roundtrip(
        wiki_links in proptest::bool::ANY,
        relative_assets in proptest::bool::ANY,
        quick_save in proptest::bool::ANY,
        rich_metadata in proptest::bool::ANY,
        single_page in proptest::bool::ANY,
        resume in proptest::bool::ANY,
        download_images in proptest::bool::ANY,
        download_documents in proptest::bool::ANY,
        quiet in proptest::bool::ANY,
        dry_run in proptest::bool::ANY,
        use_sitemap in proptest::bool::ANY,
        elastic in proptest::bool::ANY,
        clean_ai in proptest::bool::ANY,
        pipeline in proptest::bool::ANY,
        autoscale in proptest::bool::ANY,
    ) {
        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms: 0,
                max_pages: 1,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap,
                sitemap_url: None,
                single_page,
                resume,
                state_dir: None,
                download_images,
                download_documents,
                clean_ai,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet,
                dry_run,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                elastic,
                pipeline,
                ..Default::default()
            },
            obsidian: ObsidianArgs {
                obsidian_wiki_links: wiki_links,
                obsidian_tags: None,
                obsidian_relative_assets: relative_assets,
                vault: None,
                quick_save,
                obsidian_rich_metadata: rich_metadata,
            },
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        // Every bool field must roundtrip
        prop_assert_eq!(opts.export.obsidian_wiki_links, wiki_links);
        prop_assert_eq!(opts.export.obsidian_relative_assets, relative_assets);
        prop_assert_eq!(opts.export.quick_save, quick_save);
        prop_assert_eq!(opts.export.obsidian_rich_metadata, rich_metadata);
        prop_assert_eq!(opts.crawl.single_page, single_page);
        prop_assert_eq!(opts.crawl.resume, resume);
        prop_assert_eq!(opts.network.download_images, download_images);
        prop_assert_eq!(opts.network.download_documents, download_documents);
        prop_assert_eq!(opts.crawl.interactive, false); // no CLI flag sets it (#880)
        prop_assert_eq!(opts.quiet, quiet);
        prop_assert_eq!(opts.export.quiet, quiet);
        prop_assert_eq!(opts.export.dry_run, dry_run);
        prop_assert_eq!(opts.crawl.use_sitemap, use_sitemap);
        prop_assert_eq!(opts.elastic.enabled, elastic);
        prop_assert_eq!(opts.pipeline_enabled, pipeline);
        prop_assert_eq!(opts.crawl.autoscale_enabled, autoscale);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_numeric_fields_roundtrip(
        verbose in 0u8..4,
        max_depth in 0u8..20,
        delay_ms in 0u64..60_000,
        max_pages in 1usize..10_000,
        timeout_secs in 1u64..300,
        max_retries in 0u32..20,
        backoff_base_ms in 0u64..10_000,
        backoff_max_ms in 1u64..60_000,
        max_file_size in 1u64..1_000_000_000,
        download_timeout in 1u64..300,
        sitemap_depth in 0u8..10,
    ) {
        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms,
                max_pages,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: None,
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose,
                quiet: false,
                dry_run: false,
                max_depth,
                timeout_secs,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries,
                backoff_base_ms,
                backoff_max_ms,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size,
                download_timeout,
                sitemap_depth,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                elastic: false,
                ..Default::default()
            },
            obsidian: ObsidianArgs::default(),
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(opts.verbosity, verbose);
        prop_assert_eq!(opts.crawl.max_depth, max_depth);
        prop_assert_eq!(opts.network.delay_ms, delay_ms);
        prop_assert_eq!(opts.crawl.max_pages, max_pages);
        prop_assert_eq!(opts.network.timeout_secs, timeout_secs);
        prop_assert_eq!(opts.network.max_retries, max_retries);
        prop_assert_eq!(opts.network.backoff_base_ms, backoff_base_ms);
        prop_assert_eq!(opts.network.backoff_max_ms, backoff_max_ms);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_string_fields_roundtrip(
        selector in "[a-z]{1,20}",
        accept_language in "[a-z-]{1,30}",
        user_agent in proptest::option::of("[A-Za-z0-9/ .]{1,40}"),
        sitemap_url in proptest::option::of("https://[a-z]{1,10}\\.com/sitemap\\.xml".prop_map(|s| s.to_string())),
    ) {
        // Filter invalid URLs
        if let Some(ref u) = sitemap_url {
            if url::Url::parse(u).is_err() {
                return Ok(());
            }
        }

        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector,
                delay_ms: 0,
                max_pages: 1,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap: sitemap_url.is_some(),
                sitemap_url,
                single_page: false,
                resume: false,
                state_dir: None,
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet: false,
                dry_run: false,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language,
                user_agent,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                elastic: false,
                ..Default::default()
            },
            obsidian: ObsidianArgs::default(),
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let expected_selector = args.crawler.selector.clone();
        let expected_accept_language = args.crawler.accept_language.clone();
        let expected_user_agent = args.crawler.user_agent.clone();
        let expected_sitemap_url = args.crawler.sitemap_url.clone();

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(opts.crawl.selector, expected_selector);
        prop_assert_eq!(opts.network.accept_language, expected_accept_language);
        prop_assert_eq!(opts.network.user_agent, expected_user_agent);
        prop_assert_eq!(opts.crawl.sitemap_url, expected_sitemap_url);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_path_fields_roundtrip(
        output in "[a-z0-9/._-]{1,30}",
        vault in proptest::option::of("[a-z0-9/._-]{1,30}"),
        state_dir in proptest::option::of("[a-z0-9/._-]{1,30}"),
        db_path in proptest::option::of("[a-z0-9/._-]{1,30}"),
    ) {
        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms: 0,
                max_pages: 1,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: state_dir.as_deref().map(std::path::PathBuf::from),
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet: false,
                dry_run: false,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from(&output),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                db_path: db_path.as_deref().map(std::path::PathBuf::from),
                elastic: false,
                ..Default::default()
            },
            obsidian: ObsidianArgs {
                vault: vault.as_deref().map(std::path::PathBuf::from),
                ..Default::default()
            },
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(opts.export.output_dir, std::path::PathBuf::from(&output));
        prop_assert_eq!(opts.export.obsidian_vault, vault.map(std::path::PathBuf::from));
        prop_assert_eq!(opts.crawl.state_dir, state_dir.map(std::path::PathBuf::from));
        prop_assert_eq!(opts.elastic.db_path, db_path.map(std::path::PathBuf::from));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_concurrency_roundtrip(
        value in proptest::option::of(1usize..17),
    ) {
        let concurrency = match value {
            Some(v) => webfang_core::ConcurrencyConfig::new(v),
            None => webfang_core::ConcurrencyConfig::default(),
        };

        let expected_auto = concurrency.is_auto();
        let expected_value = concurrency.get();

        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms: 0,
                max_pages: 1,
                concurrency,
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: None,
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet: false,
                dry_run: false,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                elastic: false,
                ..Default::default()
            },
            obsidian: ObsidianArgs::default(),
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(
            opts.network.concurrency.is_auto(),
            expected_auto
        );
        prop_assert_eq!(
            opts.network.concurrency.get(),
            expected_value
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_obsidian_tags_roundtrip(
        tags in proptest::collection::vec("[a-z]{1,10}", 0..10),
    ) {
        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms: 0,
                max_pages: 1,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: None,
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet: false,
                dry_run: false,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                elastic: false,
                ..Default::default()
            },
            obsidian: ObsidianArgs {
                obsidian_tags: Some(tags.clone()),
                ..Default::default()
            },
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);
        prop_assert_eq!(opts.export.obsidian_tags, tags);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn prop_elastic_overrides_roundtrip(
        cpu_cores in proptest::option::of(1usize..32),
        ram_gb in proptest::option::of(1u64..128),
    ) {
        let ram_budget = ram_gb.map(|g| g * 1024 * 1024 * 1024);

        let args = Args {
            subcommand: None,
            positional_url: None,
            crawler: CrawlerArgs {
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                delay_ms: 0,
                max_pages: 1,
                concurrency: webfang_core::ConcurrencyConfig::default(),
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: None,
                download_images: false,
                download_documents: false,
                clean_ai: false,
                adaptive_selectors: false,
                extraction_fingerprint: false,
                verbose: 0,
                quiet: false,
                dry_run: false,
                max_depth: 0,
                timeout_secs: 1,
                include_patterns: vec![],
                exclude_patterns: vec![],
                max_retries: 0,
                backoff_base_ms: 0,
                backoff_max_ms: 0,
                accept_language: "en".into(),
                user_agent: None,
                max_file_size: 0,
                download_timeout: 0,
                sitemap_depth: 0,
                checkpoint_interval: 0,
                no_checkpoint: false,
                ignore_robots: false,
                ignore_waf: false,
                no_session_health: false,
                autoscale: false,
                h2_profile: "Chrome145".into(),
                js_strategy: webfang_core::domain::JsStrategy::Static,
                obscura_binary: "obscura".into(),
                asset_naming: "hash".into(),
                download_concurrency: Some(3),
                rate_limit_burst: None,
                download_assets: false,
                trace_file: None,
                dom_preprune: true,
                headers: vec![],
                cookies: vec![],
            },
            export: ExportArgs {
                output: std::path::PathBuf::from("out"),
                format: webfang_core::OutputFormat::Markdown,
                export_format: webfang_core::ExportFormat::Jsonl,
                cpu_cores,
                ram_budget,
                db_path: None,
                elastic: true,
                ..Default::default()
            },
            obsidian: ObsidianArgs::default(),
            ai: AiArgs::default(),
            tui: TuiArgs::default(),
        };

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(opts.elastic.enabled, true);
        prop_assert_eq!(opts.elastic.cpu_cores, cpu_cores);
        prop_assert_eq!(
            opts.elastic.ram_budget_bytes,
            ram_gb.map(|g| g * 1024 * 1024 * 1024)
        );
    }
}
