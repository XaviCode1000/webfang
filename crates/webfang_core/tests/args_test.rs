use clap::Parser;
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use webfang_core::cli::args::{AiArgs, Args, CrawlerArgs, ExportArgs, LlmArgs, ObsidianArgs};
use webfang_core::domain::ValidUrl;
use webfang_core::infrastructure::autotuning::ElasticOverrides;

/// Remove poisoned env vars once before any arg-parsing test runs.
static ENV_GUARD: std::sync::Once = std::sync::Once::new();
fn clean_env() {
    ENV_GUARD.call_once(|| {
        // Snapshot the poisoned keys under ENV_LOCK so the iteration cannot
        // race a concurrent mutation, then drop the lock before mutating:
        // `env_remove` acquires ENV_LOCK itself (permanent, process-wide
        // removal — no restore-on-drop; #1126).
        let poisoned: Vec<String> = {
            let _lock = webfang_test_utils::env_lock();
            std::env::vars()
                .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
                .map(|(k, _)| k)
                .collect()
        };
        for key in poisoned {
            webfang_test_utils::env_remove(&key);
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
            url: Some(ValidUrl::parse("https://example.com/test").unwrap()),
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
            retention_days: 30,
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
        llm: LlmArgs::default(),
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
    assert_eq!(opts.export.retention_days, 30);
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
    // With no AI flag, ai_config must be Default (0.3/32768/false/"");
    // with flags it must reflect the flag values (tested separately in
    // test_ai_config_parity_* below).
}

// ========================================================================
// AiConfig parity tests (Scenario 2.3.S1, 2.3.S3)
// ========================================================================

/// EVERY AI flag reaches `AiConfig` in EVERY cargo configuration.
///
/// ADR-0004 paso 6 removed the `ai` Cargo feature and the whole local-ONNX
/// stack with it, so there is no configuration left where the semantic
/// cleaner is unreachable: the cleaner runs over whatever `EmbeddingPort` the
/// run built, remote included. The `AiConfig` projection is therefore ONE
/// reader, not a `cfg`-gated pair, and this test pins the projection in the
/// only configuration that exists.
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
        "--max-chars",
        "1024",
        "--offline",
    ])
    .expect("flags must parse");

    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    assert_eq!(
        opts.ai_config,
        AiConfig {
            threshold: 0.5,
            max_chars: 1024,
            offline: true,
        }
    );
}

/// The default `AiConfig` — the values the runtime actually runs with.
///
/// This replaces the pair the `ai` feature used to split the world into
/// (`test_ai_config_parity_no_flags` for an `ai` build, and
/// `test_ai_config_defaults_without_ai_feature` for a non-`ai` one): the two
/// asserted the SAME numbers, because the `AiConfig` projection stopped being
/// `cfg`-dependent. One test, one set of numbers.
#[test]
fn test_ai_config_parity_no_flags() {
    clean_env();
    use webfang_core::application::crawl_options::AiConfig;

    let _guard = webfang_test_utils::EnvGuard::clean(&[
        "WEBFANG_THRESHOLD",
        "WEBFANG_MAX_CHARS",
        "WEBFANG_OFFLINE",
    ]);

    let args = Args::try_parse_from(["webfang"]).expect("minimal parse must succeed");
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    // Default values must reproduce the prior hardcoded behavior (Scenario 2.3.S3)
    assert_eq!(
        opts.ai_config,
        AiConfig {
            threshold: 0.3,
            max_chars: 98_304,
            offline: false,
        }
    );
    assert_eq!(
        opts.ai_config,
        AiConfig::default(),
        "the spec defaults and `AiConfig::default()` must agree — a divergence \
         would mean the runtime runs with values the spec never advertised"
    );
}

/// `--max-chars` renders in EVERY cargo configuration (ADR-0004), so its value
/// is carried in every configuration: a flag that parses and is then dropped
/// would be exactly the silent loss the retirement of the token-budget shim
/// was meant to avoid.
#[test]
fn max_chars_reaches_ai_config() {
    clean_env();
    let _guard = webfang_test_utils::EnvGuard::clean(&["WEBFANG_MAX_CHARS"]);

    let args = Args::try_parse_from(["webfang", "--max-chars", "2048"])
        .expect("the ungated flag must parse");
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    assert_eq!(
        opts.ai_config.max_chars, 2048,
        "--max-chars must reach AiConfig.max_chars"
    );
}

/// The retired shims are not flags any more (issue #2119), so clap answers
/// with its standard "unexpected argument" error. The env doors are NOT
/// rejected — an env var is not a flag — they are ignored, and that is
/// asserted hermetically here: a stale `WEBFANG_MAX_TOKENS` must leave the
/// budget alone.
///
/// The env side is what a stale systemd unit actually carries, so it is the
/// half worth pinning in-process; the flag side is pinned behaviorally in
/// `tests/max_chars_bound_test.rs`, which spawns the binary.
#[test]
fn the_retired_shims_are_unknown_arguments_and_their_env_vars_are_ignored() {
    clean_env();

    for flag in ["--max-tokens", "--ai-model"] {
        let err = Args::try_parse_from(["webfang", flag, "1"])
            .expect_err("a retired shim must not parse as a flag");
        assert!(
            err.to_string().contains("unexpected argument"),
            "`{flag}` must be rejected by clap as an unknown argument; got: {err}"
        );
    }

    let _guard = webfang_test_utils::EnvGuard::with(&[
        ("WEBFANG_MAX_TOKENS", "4096"),
        ("WEBFANG_AI_MODEL_ID", "granite-311m"),
        ("AI_MODEL_ID", "granite-311m"),
        ("WEBFANG_AI_ENGINE", "single"),
    ]);

    let args = Args::try_parse_from(["webfang"]).expect("minimal parse must succeed");
    let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

    assert_eq!(
        opts.ai_config.max_chars, 98_304,
        "a stale WEBFANG_MAX_TOKENS must NOT be converted into a budget"
    );
    assert_eq!(
        opts.ai_config.threshold, 0.3,
        "a stale AI model env var must not change any AI setting"
    );
}

// ========================================================================
// #827 — a poisoned AI model env var must not break unrelated CLI paths
// (slice 5b / #980). The env names are IGNORED since the shims were retired
// (issue #2119), but a poisoned value must still not break arg parsing.
// ========================================================================

/// A poisoned `WEBFANG_AI_MODEL_ID` must not make every invocation fail.
///
/// The variable is not read by anything any more: the flag it used to feed is
/// gone, so clap never sees it and the value is simply inert. The contract
/// that survives from #827 / #980 is the hermeticity one: a poisoned
/// environment must not take the whole CLI down.
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

/// #1239: the seed URL is hardened at the argv boundary. A malformed URL
/// is a clap usage error (exit 64), never a panic — before this change
/// `url_from_args` `.expect()`ed and aborted the process (exit 134).
#[test]
fn malformed_seed_url_is_a_usage_error_not_a_panic() {
    clean_env();
    let result = Args::try_parse_from(["webfang", "--url", "ht!tp://x"]);

    let err = result.expect_err("malformed URL must be rejected at the argv boundary");
    assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    assert!(
        err.to_string().contains("URL inválida"),
        "the Spanish boundary error must surface in the usage message: {err}"
    );
}

/// #1239: a credentialed seed URL parses successfully, but the credential
/// strip applies AT THE BOUNDARY — the stored value never observes the
/// credentials (the behavioral trace invariant lives in
/// `export_test::trace_file_never_leaks_url_credentials`).
#[test]
fn credentialed_seed_url_is_stripped_at_the_boundary() {
    clean_env();
    let args = Args::try_parse_from(["webfang", "--url", "https://user:secretpass@example.com/x"])
        .expect("credentialed http(s) URL is scheme-valid and must parse");

    let url = args.crawler.url.expect("--url was provided");
    assert_eq!(
        url.as_str(),
        "https://example.com/x",
        "credentials must be stripped before anything downstream runs"
    );
    assert!(
        !url.as_str().contains("secretpass"),
        "the password must not survive in any form"
    );
}

/// #1239: the scheme allow-list (#675-2) applies at the argv boundary.
#[test]
fn non_http_seed_url_is_rejected_at_the_boundary() {
    clean_env();
    let result = Args::try_parse_from(["webfang", "--url", "ftp://example.com/file"]);
    assert!(
        result.is_err(),
        "ftp:// seed must be rejected by the scheme allow-list"
    );
}

/// The legacy bare `AI_MODEL_ID` must not break arg parsing either (#827,
/// #980 slice 5b). Nothing reads it any more, so the value is inert — but an
/// inherited environment must never take the CLI down.
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

/// The retired model flag is not a flag any more (issue #2119): clap answers
/// it with its standard "unexpected argument" error, and the raw-string
/// policy that used to carry its value is gone with it.
#[test]
fn test_ai_model_flag_is_not_accepted_anymore() {
    clean_env();

    let err = Args::try_parse_from(["webfang", "--ai-model", "bogus"])
        .expect_err("the retired --ai-model must not parse");
    assert!(
        err.to_string().contains("unexpected argument"),
        "the retired flag must be answered by clap as an unknown argument; got: {err}"
    );
}

// ========================================================================
// #791 — --dom-preprune flag tests
// ========================================================================

#[test]
fn test_dom_preprune_defaults_to_true() {
    // The AI model env vars must be unset too: they are ignored since the
    // shims were retired (issue #2119), but CI bug-discovery poisons them and
    // the harness must stay hermetic regardless.
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
    // clean_env() unsets the AI model env vars (ignored since the shims were
    // retired, issue #2119) so the flag under test is the only variable in
    // play. EnvGuard::with then sets only that one. Two EnvGuards cannot be
    // nested (ENV_LOCK deadlock).
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

/// Baseline `CrawlerArgs` for the property tests below: fixed probe values
/// for every field the properties do not vary, so each test only names its
/// own axis and takes the rest via struct-update syntax (`..prop_crawler_args()`).
fn prop_crawler_args() -> CrawlerArgs {
    CrawlerArgs {
        url: Some(ValidUrl::parse("https://example.com/prop").unwrap()),
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
        post_load_wait: webfang_core::domain::PostLoadWait::Idle,
        asset_naming: "hash".into(),
        download_concurrency: Some(3),
        rate_limit_burst: None,
        download_assets: false,
        trace_file: None,
        dom_preprune: true,
        headers: vec![],
        cookies: vec![],
    }
}

/// Baseline `ExportArgs` for the property tests: the fixed probe values
/// every test shares, with the varied dimensions overridden per test via
/// struct-update syntax (`..prop_export_args()`).
fn prop_export_args() -> ExportArgs {
    ExportArgs {
        output: std::path::PathBuf::from("out"),
        format: webfang_core::OutputFormat::Markdown,
        export_format: webfang_core::ExportFormat::Jsonl,
        elastic: false,
        ..Default::default()
    }
}

/// Baseline `Args` for the property tests: the caller supplies the three
/// group structs under test, and the `subcommand` / `positional_url` /
/// `ai` / `llm` scaffolding is written once here.
fn prop_args(crawler: CrawlerArgs, export: ExportArgs, obsidian: ObsidianArgs) -> Args {
    Args {
        subcommand: None,
        positional_url: None,
        crawler,
        export,
        obsidian,
        ai: AiArgs::default(),
        llm: LlmArgs::default(),
    }
}

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
        let args = prop_args(
            CrawlerArgs {
                use_sitemap,
                single_page,
                resume,
                download_images,
                download_documents,
                clean_ai,
                quiet,
                dry_run,
                autoscale,
                ..prop_crawler_args()
            },
            ExportArgs {
                elastic,
                pipeline,
                ..prop_export_args()
            },
            ObsidianArgs {
                obsidian_wiki_links: wiki_links,
                obsidian_relative_assets: relative_assets,
                quick_save,
                obsidian_rich_metadata: rich_metadata,
                ..Default::default()
            },
        );

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
        let args = prop_args(
            CrawlerArgs {
                delay_ms,
                max_pages,
                verbose,
                max_depth,
                timeout_secs,
                max_retries,
                backoff_base_ms,
                backoff_max_ms,
                max_file_size,
                download_timeout,
                sitemap_depth,
                ..prop_crawler_args()
            },
            prop_export_args(),
            ObsidianArgs::default(),
        );

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

        let args = prop_args(
            CrawlerArgs {
                selector: selector.clone(),
                use_sitemap: sitemap_url.is_some(),
                sitemap_url: sitemap_url.clone(),
                accept_language: accept_language.clone(),
                user_agent: user_agent.clone(),
                ..prop_crawler_args()
            },
            prop_export_args(),
            ObsidianArgs::default(),
        );

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
        let args = prop_args(
            CrawlerArgs {
                state_dir: state_dir.as_deref().map(std::path::PathBuf::from),
                ..prop_crawler_args()
            },
            ExportArgs {
                output: std::path::PathBuf::from(&output),
                db_path: db_path.as_deref().map(std::path::PathBuf::from),
                ..prop_export_args()
            },
            ObsidianArgs {
                vault: vault.as_deref().map(std::path::PathBuf::from),
                ..Default::default()
            },
        );

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

        let args = prop_args(
            CrawlerArgs {
                concurrency,
                ..prop_crawler_args()
            },
            prop_export_args(),
            ObsidianArgs::default(),
        );

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
        let args = prop_args(
            prop_crawler_args(),
            prop_export_args(),
            ObsidianArgs {
                obsidian_tags: Some(tags.clone()),
                ..Default::default()
            },
        );

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

        let args = prop_args(
            prop_crawler_args(),
            ExportArgs {
                cpu_cores,
                ram_budget,
                elastic: true,
                ..prop_export_args()
            },
            ObsidianArgs::default(),
        );

        let opts = webfang_core::application::crawl_options::CrawlOptions::from(args);

        prop_assert_eq!(opts.elastic.enabled, true);
        prop_assert_eq!(opts.elastic.cpu_cores, cpu_cores);
        prop_assert_eq!(
            opts.elastic.ram_budget_bytes,
            ram_gb.map(|g| g * 1024 * 1024 * 1024)
        );
    }
}
