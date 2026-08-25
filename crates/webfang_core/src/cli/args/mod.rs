/// AI-powered semantic cleaning arguments.
pub mod ai;
/// Crawler and discovery configuration arguments.
pub mod crawler;
/// Export format and output configuration arguments.
pub mod export;
/// Obsidian vault integration arguments.
pub mod obsidian;
/// Terminal UI configuration arguments.
pub mod tui;

pub use ai::AiArgs;
pub use crawler::CrawlerArgs;
pub use export::ExportArgs;
pub use obsidian::ObsidianArgs;
pub use tui::TuiArgs;

/// Test-only helpers shared by the per-group arg modules.
#[cfg(test)]
pub(crate) mod test_support {
    /// Runs `f` with every ambient environment variable that could leak into
    /// clap's `env` fallbacks (`WEBFANG_*`, `AI_MODEL_ID`) temporarily
    /// removed, restoring them afterwards.
    ///
    /// Hermeticity: several args declare `env = "WEBFANG_..."` fallbacks, so a
    /// bare parse absorbs ambient variables and default assertions fail in
    /// environments that export them (the scheduled bug-discovery job poisons
    /// exactly these names — issue #926). Tests must observe the declared
    /// defaults, not the ambient shell.
    pub(crate) fn with_clap_env_cleared<T>(f: impl FnOnce() -> T) -> T {
        let saved: Vec<(String, String)> = std::env::vars()
            .filter(|(name, _)| name.starts_with("WEBFANG_") || name == "AI_MODEL_ID")
            .collect();
        for (name, _) in &saved {
            std::env::remove_var(name);
        }
        let result = f();
        for (name, value) in saved {
            std::env::set_var(&name, value);
        }
        result
    }
}

use clap::Parser;

/// CLI Arguments for the webfang binary.
///
/// Parsed using `clap` with derive macros.
///
/// # Examples
///
/// ```no_run
/// use webfang_core::Args;
/// use clap::Parser;
///
/// let args = Args::parse_from([
///     "webfang",
///     "--url", "https://example.com",
///     "--output", "./output",
///     "--export-format", "jsonl",
///     "--resume",
/// ]);
///
/// assert_eq!(args.crawler.url.as_deref(), Some("https://example.com"));
/// ```
#[derive(Parser, Debug, Default)]
#[command(name = "webfang", version)]
#[command(
    about = "High-performance web scraper with WAF evasion and AI-powered content cleaning",
    after_help = "EXIT CODES:\n  0    Success\n  2    No URLs discovered\n  3    All scrapers failed\n  64   Bad CLI arguments (usage error)\n  69   WAF block or network error\n  74   I/O error\n  76   Protocol error\n  78   Configuration error\n\nEXAMPLES:\n  webfang https://example.com\n  webfang -u https://example.com\n  webfang -u https://example.com --ai\n  webfang -u https://example.com -f jsonl\n  webfang -u https://example.com -v\n  webfang -u https://example.com -vv  # DEBUG\n  webfang --url-list urls.txt --resume"
)]
#[command(subcommand_negates_reqs = true)]
pub struct Args {
    /// Subcommands
    #[command(subcommand)]
    pub subcommand: Option<Commands>,

    /// URL to scrape (positional shorthand — equivalent to --url)
    #[arg(value_name = "URL", conflicts_with = "url")]
    pub positional_url: Option<String>,

    /// Crawler and discovery configuration.
    #[command(flatten)]
    pub crawler: CrawlerArgs,

    /// Export format and output configuration.
    #[command(flatten)]
    pub export: ExportArgs,

    /// Obsidian vault integration settings.
    #[command(flatten)]
    pub obsidian: ObsidianArgs,

    /// AI-powered semantic cleaning settings.
    #[command(flatten)]
    pub ai: AiArgs,

    /// Terminal UI configuration.
    #[command(flatten)]
    pub tui: TuiArgs,
}

/// Subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    /// Generate shell completion scripts
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

/// Shell type for completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    /// Bash shell completions.
    Bash,
    /// Elvish shell completions.
    Elvish,
    /// Fish shell completions.
    Fish,
    /// PowerShell completions.
    PowerShell,
    /// Zsh shell completions.
    Zsh,
}

impl From<Shell> for clap_complete::Shell {
    fn from(s: Shell) -> Self {
        match s {
            Shell::Bash => clap_complete::Shell::Bash,
            Shell::Elvish => clap_complete::Shell::Elvish,
            Shell::Fish => clap_complete::Shell::Fish,
            Shell::PowerShell => clap_complete::Shell::PowerShell,
            Shell::Zsh => clap_complete::Shell::Zsh,
        }
    }
}

impl Args {
    /// Build [`ElasticOverrides`] (PR5) from the elastic-ingestion CLI flags.
    ///
    /// `--ram-budget` is already parsed to bytes by the clap value parser, which
    /// accepts suffixed values (`8GB`, `2048MB`, plain bytes) and rejects zero
    /// or malformed input at the boundary (#653). The result feeds
    /// [`ElasticConfig::resolve`] → Rayon pool size, byte-weighted semaphore,
    /// and SQLite path.
    ///
    /// [`ElasticConfig::resolve`]: crate::infrastructure::autotuning::ElasticConfig::resolve
    /// [`ElasticOverrides`]: crate::infrastructure::autotuning::ElasticOverrides
    #[must_use]
    pub fn elastic_overrides(&self) -> crate::infrastructure::autotuning::ElasticOverrides {
        use crate::infrastructure::autotuning::ElasticOverrides;
        ElasticOverrides {
            cpu_cores: self.export.cpu_cores,
            ram_budget_bytes: self.export.ram_budget,
            max_resource_bytes: None,
            db_path: self.export.db_path.clone(),
        }
    }
}

// ============================================================================
// From<Args> for CrawlOptions
// ============================================================================

impl From<Args> for crate::application::crawl_options::CrawlOptions {
    /// Convert CLI arguments into structured [`crate::application::crawl_options::CrawlOptions`].
    ///
    /// This is an owned, lossless conversion — every field in `Args` maps
    /// to exactly one field in `CrawlOptions`. The `url` field is parsed
    /// from `Option<String>` into `Url` (panics if invalid; CLI validation
    /// guarantees validity before this point). An explicit
    /// `--rate-limit-burst 0` likewise panics with the Spanish boundary
    /// error (#897 item 2): the preflight pipeline rejects it first, so
    /// reaching this conversion with 0 means validation was bypassed.
    #[allow(clippy::too_many_lines)]
    fn from(args: Args) -> Self {
        // Capture BEFORE the move into NetworkOptions: explicit operator
        // concurrency feeds the budget model as a crawl-tier override.
        let explicit_crawl = args.crawler.concurrency.get();
        let explicit_batch = args.export.batch_concurrency;
        let explicit_download = args.crawler.download_concurrency;
        use crate::application::crawl_options::{
            CrawlLimits, ExportOptions, IngestionTuning, NetworkOptions,
        };

        let url = url_from_args(&args);

        let overrides = args.elastic_overrides();
        let ai_config = build_ai_config(&args);

        Self {
            url,
            verbosity: args.crawler.verbose,
            quiet: args.crawler.quiet,
            ai: args.crawler.clean_ai,
            adaptive_selectors: args.crawler.adaptive_selectors,
            extraction_fingerprint: args.crawler.extraction_fingerprint,
            crawl: CrawlLimits {
                selector: args.crawler.selector,
                max_depth: args.crawler.max_depth,
                max_pages: args.crawler.max_pages,
                single_page: args.crawler.single_page,
                include_patterns: args.crawler.include_patterns,
                exclude_patterns: args.crawler.exclude_patterns,
                // No CLI flag sets this anymore (#880 removed --interactive);
                // internal knob, defaults to false.
                interactive: false,
                resume: args.crawler.resume,
                state_dir: args.crawler.state_dir,
                use_sitemap: args.crawler.use_sitemap,
                sitemap_url: args.crawler.sitemap_url,
                checkpoint_interval: args.crawler.checkpoint_interval,
                no_checkpoint: args.crawler.no_checkpoint,
                ignore_robots: args.crawler.ignore_robots,
                ignore_waf: args.crawler.ignore_waf,
                no_session_health: args.crawler.no_session_health,
                autoscale_enabled: args.crawler.autoscale,
                dom_preprune: args.crawler.dom_preprune,
            },
            network: NetworkOptions {
                user_agent: args.crawler.user_agent,
                accept_language: args.crawler.accept_language,
                concurrency: args.crawler.concurrency,
                delay_ms: args.crawler.delay_ms,
                timeout_secs: args.crawler.timeout_secs,
                max_retries: args.crawler.max_retries,
                backoff_base_ms: args.crawler.backoff_base_ms,
                backoff_max_ms: args.crawler.backoff_max_ms,
                download_images: args.crawler.download_images || args.crawler.download_assets,
                download_documents: args.crawler.download_documents || args.crawler.download_assets,
                max_file_size: Some(args.crawler.max_file_size),
                download_timeout_secs: args.crawler.download_timeout,
                h2_profile: args.crawler.h2_profile,
                js_strategy: args.crawler.js_strategy,
                obscura_binary: args.crawler.obscura_binary,
                custom_headers: args
                    .crawler
                    .headers
                    .iter()
                    .filter_map(|h| {
                        h.split_once(':')
                            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                    })
                    .collect(),
                initial_cookies: args
                    .crawler
                    .cookies
                    .iter()
                    .filter_map(|c| {
                        c.split_once('=')
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                    })
                    .collect(),
            },
            export: ExportOptions {
                output_format: args.export.format,
                export_format: args.export.export_format,
                output_dir: args.export.output,
                dry_run: args.crawler.dry_run,
                quiet: args.crawler.quiet,
                // #762: capture --vault EXPLICITNESS at parse time (config
                // merge + autodetection can fill `obsidian_vault` later).
                vault_is_explicit: args.obsidian.vault.is_some(),
                obsidian_vault: args.obsidian.vault,
                obsidian_rich_metadata: args.obsidian.obsidian_rich_metadata,
                obsidian_tags: args.obsidian.obsidian_tags.unwrap_or_default(),
                obsidian_wiki_links: args.obsidian.obsidian_wiki_links,
                obsidian_relative_assets: args.obsidian.obsidian_relative_assets,
                quick_save: args.obsidian.quick_save,
            },
            elastic: IngestionTuning {
                enabled: args.export.elastic,
                cpu_cores: overrides.cpu_cores,
                ram_budget_bytes: overrides.ram_budget_bytes,
                db_path: overrides.db_path,
                max_resource_bytes: overrides.max_resource_bytes,
                output_vectors: args.export.output_vectors.clone(),
            },
            pipeline_enabled: args.export.pipeline,
            pipeline_output_format: args.export.pipeline_output,
            batch: crate::application::crawl_options::BatchOptions {
                enabled: args.export.batch || args.export.batch_file.is_some(),
                batch_file: args.export.batch_file,
                concurrency: args.export.batch_concurrency,
            },
            asset_naming: args.crawler.asset_naming,
            download_concurrency: args.crawler.download_concurrency,
            ai_config,
            budget_overrides: crate::domain::budget::BudgetOverrides {
                // #897 item 2 ("Zero Silent Loss"): an explicit `0` is
                // rejected by `parse_rate_limit_burst`, and that rejection
                // must mirror the preflight pipeline's hard error — never
                // a silent degrade to the derived default. The legacy
                // `From<Args>` path cannot return `Result`, so — like the
                // `url` field above — a rejected value panics; the binary
                // validates via `normalize()` before this conversion.
                rate_burst: args.crawler.rate_limit_burst.as_deref().and_then(|raw| {
                    let parsed = crate::cli::args::crawler::parse_rate_limit_burst(raw)
                        .unwrap_or_else(|err| panic!("{err}"));
                    parsed.map(|v| {
                        crate::domain::budget::BurstPermits::new(v).unwrap_or_else(|_| {
                            panic!("--rate-limit-burst debe ser >= 1 (recibido {v})")
                        })
                    })
                }),
                // Explicit `--concurrency` (when not "auto") feeds the
                // model as a crawl override — same explicit-wins rule as
                // the preflight pipeline path.
                crawl: explicit_crawl
                    .and_then(|v| crate::domain::budget::tiers::CrawlConcurrency::new(v).ok()),
                batch: explicit_batch
                    .and_then(|v| crate::domain::budget::tiers::BatchConcurrency::new(v).ok()),
                asset: explicit_download
                    .and_then(|v| crate::domain::budget::tiers::DownloadConcurrency::new(v).ok()),
            },
        }
    }
}

/// Parse the CLI URL into a [`url::Url`] for the `From<Args> -> CrawlOptions`
/// conversion.
///
/// `From::from` returns `Self`, so a parse failure cannot be propagated;
/// CLI validation guarantees the URL is valid before this point, so the
/// `expect` documents a true invariant.
#[allow(clippy::expect_used)]
fn url_from_args(args: &Args) -> url::Url {
    url::Url::parse(args.crawler.url.as_deref().unwrap_or("https://example.com"))
        .expect("URL must be valid — CLI validation ensures this")
}

#[cfg(feature = "ai")]
fn build_ai_config(args: &Args) -> crate::application::crawl_options::AiConfig {
    crate::application::crawl_options::AiConfig {
        threshold: args.ai.threshold,
        max_tokens: args.ai.max_tokens,
        offline: args.ai.offline,
        model: args.ai.ai_model.clone().unwrap_or_default(),
    }
}

#[cfg(not(feature = "ai"))]
fn build_ai_config(_args: &Args) -> crate::application::crawl_options::AiConfig {
    crate::application::crawl_options::AiConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Remove poisoned env vars once before any arg-parsing test runs.
    /// CI bug-discovery sets WEBFANG_*=POISON / AI_MODEL_ID=POISON which
    /// clap reads via `env = "..."` attributes, breaking hermeticity.
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

    // ========================================================================
    // TASK-13 — --ignore-waf flag + propagation (REQ-WAF-07)
    // ========================================================================

    #[test]
    fn ignore_waf_flag_defaults_to_false() {
        clean_env();
        let args =
            Args::try_parse_from(["webfang", "-u", "https://example.com"]).expect("valid args");
        assert!(!args.crawler.ignore_waf, "ignore_waf defaults to false");
    }

    #[test]
    fn ignore_waf_flag_parses() {
        clean_env();
        let args = Args::try_parse_from(["webfang", "-u", "https://example.com", "--ignore-waf"])
            .expect("valid args");
        assert!(args.crawler.ignore_waf, "--ignore-waf sets the flag");
    }

    #[test]
    fn ignore_waf_maps_into_crawl_options() {
        clean_env();
        let args = Args::try_parse_from(["webfang", "-u", "https://example.com", "--ignore-waf"])
            .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert!(
            opts.crawl.ignore_waf,
            "ignore_waf must propagate Args -> CrawlOptions"
        );
    }

    // ========================================================================
    // #897 item 2 — Zero Silent Loss: an explicit `--rate-limit-burst 0`
    // must NEVER silently degrade to the derived default in `From<Args>`;
    // it must be rejected as loudly as the preflight pipeline rejects it.
    // ========================================================================

    #[test]
    #[should_panic(expected = "--rate-limit-burst debe ser >= 1")]
    fn from_args_rate_limit_burst_zero_is_rejected_not_silently_defaulted() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--rate-limit-burst",
            "0",
        ])
        .expect("clap accepts the raw string; rejection happens at conversion");
        let _ = crate::application::crawl_options::CrawlOptions::from(args);
    }

    // ========================================================================
    // #344 — Positional URL argument
    // ========================================================================

    #[test]
    fn positional_url_parses() {
        clean_env();
        let args = Args::try_parse_from(["webfang", "https://example.com"]).expect("valid args");
        assert_eq!(
            args.positional_url.as_deref(),
            Some("https://example.com"),
            "positional URL captured"
        );
    }

    #[test]
    fn positional_url_with_flags() {
        clean_env();
        let args = Args::try_parse_from(["webfang", "https://example.com", "--max-pages", "5"])
            .expect("valid args");
        assert_eq!(args.positional_url.as_deref(), Some("https://example.com"));
        assert_eq!(args.crawler.max_pages, 5);
    }

    #[test]
    fn positional_url_conflicts_with_flag() {
        clean_env();
        let result =
            Args::try_parse_from(["webfang", "https://example.com", "-u", "https://other.com"]);
        assert!(result.is_err(), "positional + -u must conflict");
    }

    #[test]
    fn completions_still_work() {
        clean_env();
        let args = Args::try_parse_from(["webfang", "completions", "bash"]).expect("valid args");
        assert!(
            matches!(
                args.subcommand,
                Some(Commands::Completions { shell: Shell::Bash })
            ),
            "completions subcommand parses without URL"
        );
    }

    #[test]
    fn no_args_no_url_is_ok() {
        clean_env();
        let args = Args::try_parse_from(["webfang"]).expect("valid args");
        assert!(args.positional_url.is_none());
        assert!(args.crawler.url.is_none());
    }

    // ========================================================================
    // #640 — --batch-concurrency 0 must be rejected at parse time (exit 64)
    // ========================================================================

    #[test]
    fn batch_concurrency_zero_is_rejected() {
        clean_env();
        let result = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--batch-concurrency",
            "0",
        ]);
        assert!(
            result.is_err(),
            "--batch-concurrency 0 must fail clap validation, not panic at runtime"
        );
    }

    #[test]
    fn batch_concurrency_positive_is_accepted() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--batch-concurrency",
            "3",
        ])
        .expect("valid args");
        assert_eq!(args.export.batch_concurrency, Some(3));
    }

    #[test]
    fn batch_concurrency_omitted_is_none_auto() {
        clean_env();
        let args =
            Args::try_parse_from(["webfang", "-u", "https://example.com"]).expect("valid args");
        // Omitted flag = auto: the budget model derives the tier.
        assert_eq!(args.export.batch_concurrency, None);
    }

    // ========================================================================
    // #675 — --cookie and --header flags
    // ========================================================================

    #[test]
    fn header_flag_parses_single() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.headers,
            vec!["Authorization: Bearer TOKEN".to_string()]
        );
    }

    #[test]
    fn header_flag_parses_multiple() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
            "-H",
            "X-Custom: value",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.headers,
            vec![
                "Authorization: Bearer TOKEN".to_string(),
                "X-Custom: value".to_string()
            ]
        );
    }

    #[test]
    fn cookie_flag_parses_single() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
        ])
        .expect("valid args");
        assert_eq!(args.crawler.cookies, vec!["session=abc123".to_string()]);
    }

    #[test]
    fn cookie_flag_parses_multiple() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
            "--cookie",
            "csrf=xyz",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.cookies,
            vec!["session=abc123".to_string(), "csrf=xyz".to_string()]
        );
    }

    #[test]
    fn headers_propagate_to_crawl_options() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
        ])
        .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            opts.network.custom_headers,
            vec![("Authorization".to_string(), "Bearer TOKEN".to_string())]
        );
    }

    #[test]
    fn cookies_propagate_to_crawl_options() {
        clean_env();
        let args = Args::try_parse_from([
            "webfang",
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
        ])
        .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            opts.network.initial_cookies,
            vec![("session".to_string(), "abc123".to_string())]
        );
    }
}
