//! CLI Arguments for the rust_scraper binary.
//!
//! Parsed using `clap` with derive macros.

use crate::domain::config::{ConcurrencyConfig, ExportFormat, OutputFormat, PipelineOutputFormat};
use crate::domain::JsStrategy;
use clap::Parser;

/// Validate `--download-concurrency`: must be >= 1. A value of 0 would make
/// `buffer_unordered(0)` hang forever (deadlock, D1). Rejecting here satisfies
/// the "Zero Silent Loss" philosophy with a clear CLI error instead of a hang.
fn parse_download_concurrency(s: &str) -> Result<usize, String> {
    let v: usize = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido para --download-concurrency"))?;
    if v == 0 {
        return Err(
            "--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)"
                .to_string(),
        );
    }
    Ok(v)
}

/// CLI Arguments for the rust_scraper binary.
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::Args;
/// use clap::Parser;
///
/// let args = Args::parse_from([
///     "rust_scraper",
///     "--url", "https://example.com",
///     "--output", "./output",
///     "--export-format", "jsonl",
///     "--resume",
/// ]);
///
/// assert_eq!(args.url, "https://example.com");
/// ```
#[derive(Parser, Debug, Default)]
#[command(name = "rust_scraper", version)]
#[command(about = "Production-ready web scraper with Clean Architecture", long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Args {
    /// Subcommands
    #[command(subcommand)]
    pub subcommand: Option<Commands>,

    // ========== Target ==========
    /// URL to scrape (required unless using a subcommand)
    #[arg(short, long, env = "RUST_SCRAPER_URL")]
    #[clap(next_help_heading = "Target")]
    pub url: Option<String>,

    /// CSS selector for content extraction
    #[arg(short, long, default_value = "body", env = "RUST_SCRAPER_SELECTOR")]
    #[clap(next_help_heading = "Target")]
    pub selector: String,

    // ========== Output ==========
    /// Output directory for scraped content
    #[arg(short, long, default_value = "output", env = "RUST_SCRAPER_OUTPUT")]
    #[clap(next_help_heading = "Output")]
    pub output: std::path::PathBuf,

    /// Output format for individual files (markdown, text, json)
    #[arg(
        short = 'f',
        long,
        default_value = "markdown",
        value_enum,
        env = "RUST_SCRAPER_FORMAT"
    )]
    #[clap(next_help_heading = "Output")]
    pub format: OutputFormat,

    /// Export format for RAG pipeline (jsonl, vector, auto)
    #[arg(
        long,
        default_value = "jsonl",
        value_enum,
        env = "RUST_SCRAPER_EXPORT_FORMAT"
    )]
    #[clap(next_help_heading = "Output")]
    pub export_format: ExportFormat,

    // ========== Obsidian Output ==========
    /// Convert same-domain links to Obsidian [[wiki-link]] syntax
    #[arg(
        long,
        default_value = "false",
        env = "RUST_SCRAPER_OBSIDIAN_WIKI_LINKS"
    )]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_wiki_links: bool,

    /// Tags to include in YAML frontmatter (comma-separated)
    #[arg(long, env = "RUST_SCRAPER_OBSIDIAN_TAGS", value_delimiter = ',')]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_tags: Option<Vec<String>>,

    /// Rewrite downloaded asset paths as relative to the .md file
    #[arg(
        long,
        default_value = "false",
        env = "RUST_SCRAPER_OBSIDIAN_RELATIVE_ASSETS"
    )]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_relative_assets: bool,

    /// Path to Obsidian vault (auto-detects if not provided)
    #[arg(long, env = "RUST_SCRAPER_OBSIDIAN_VAULT")]
    #[clap(next_help_heading = "Obsidian")]
    pub vault: Option<std::path::PathBuf>,

    /// Quick-save mode: save directly to vault _inbox folder
    #[arg(
        long,
        default_value = "false",
        env = "RUST_SCRAPER_OBSIDIAN_QUICK_SAVE"
    )]
    #[clap(next_help_heading = "Obsidian")]
    pub quick_save: bool,

    /// Add rich metadata to frontmatter
    #[arg(
        long,
        default_value = "false",
        env = "RUST_SCRAPER_OBSIDIAN_RICH_METADATA"
    )]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_rich_metadata: bool,

    // ========== Discovery ==========
    /// Delay between requests in milliseconds
    #[arg(long, default_value = "1000", env = "RUST_SCRAPER_DELAY_MS")]
    #[clap(next_help_heading = "Discovery")]
    pub delay_ms: u64,

    /// Maximum pages to scrape
    #[arg(long, default_value = "10", env = "RUST_SCRAPER_MAX_PAGES")]
    #[clap(next_help_heading = "Discovery")]
    pub max_pages: usize,

    /// Concurrency level (auto or number)
    #[arg(long, default_value_t = ConcurrencyConfig::default(), env = "RUST_SCRAPER_CONCURRENCY")]
    #[clap(next_help_heading = "Discovery")]
    pub concurrency: ConcurrencyConfig,

    /// Use sitemap for URL discovery
    #[arg(long, env = "RUST_SCRAPER_USE_SITEMAP")]
    #[clap(next_help_heading = "Discovery")]
    pub use_sitemap: bool,

    /// Explicit sitemap URL
    #[arg(long, requires = "use_sitemap", env = "RUST_SCRAPER_SITEMAP_URL")]
    #[clap(next_help_heading = "Discovery")]
    pub sitemap_url: Option<String>,

    // ========== Behavior ==========
    /// Scrape only the seed URL without discovery or crawling
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_SINGLE_PAGE")]
    #[clap(next_help_heading = "Behavior")]
    pub single_page: bool,

    /// Resume mode - skip URLs already processed
    #[arg(long, env = "RUST_SCRAPER_RESUME")]
    #[clap(next_help_heading = "Behavior")]
    pub resume: bool,

    /// Custom state directory for resume mode
    #[arg(long, env = "RUST_SCRAPER_STATE_DIR")]
    #[clap(next_help_heading = "Behavior")]
    pub state_dir: Option<std::path::PathBuf>,

    /// Download images from the page
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_DOWNLOAD_IMAGES")]
    #[clap(next_help_heading = "Behavior")]
    pub download_images: bool,

    /// Download documents from the page
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_DOWNLOAD_DOCUMENTS")]
    #[clap(next_help_heading = "Behavior")]
    pub download_documents: bool,

    /// Download all assets (images + documents) from the page
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_DOWNLOAD_ASSETS")]
    #[clap(next_help_heading = "Behavior")]
    pub download_assets: bool,

    /// Unified TUI mode: config form (collapsible sections) → URL selector → scraping
    #[arg(long, env = "RUST_SCRAPER_TUI")]
    #[clap(next_help_heading = "Behavior")]
    pub tui: bool,

    /// [DEPRECATED] Use --tui instead. Interactive mode with TUI URL selector
    #[arg(long, env = "RUST_SCRAPER_INTERACTIVE", hide = true)]
    #[clap(next_help_heading = "Behavior")]
    pub interactive: bool,

    /// [DEPRECATED] Use --tui instead. Open configuration TUI
    #[arg(long, env = "RUST_SCRAPER_CONFIG_TUI", hide = true)]
    #[clap(next_help_heading = "Behavior")]
    pub config_tui: bool,

    /// Use AI-powered semantic cleaning for better RAG output
    #[cfg(feature = "ai")]
    #[arg(
        long,
        default_value = "false",
        visible_alias = "ai",
        env = "RUST_SCRAPER_CLEAN_AI"
    )]
    #[clap(next_help_heading = "Behavior")]
    pub clean_ai: bool,

    /// Feature flag placeholder when AI is not enabled
    #[cfg(not(feature = "ai"))]
    #[arg(
        long,
        default_value = "false",
        hide = true,
        visible_alias = "ai",
        env = "RUST_SCRAPER_CLEAN_AI"
    )]
    pub clean_ai: bool,

    /// Force JavaScript rendering for SPA sites (not yet implemented)
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_FORCE_JS_RENDER")]
    #[clap(next_help_heading = "Behavior")]
    pub force_js_render: bool,

    // ========== Display ==========
    /// Verbosity level (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, env = "RUST_SCRAPER_VERBOSE")]
    #[clap(next_help_heading = "Display")]
    pub verbose: u8,

    /// Quiet mode — suppress info/debug output
    #[arg(short = 'q', long, default_value = "false", env = "RUST_SCRAPER_QUIET")]
    #[clap(next_help_heading = "Display")]
    pub quiet: bool,

    /// Dry-run mode — discover URLs and print without scraping
    #[arg(
        short = 'n',
        long,
        default_value = "false",
        env = "RUST_SCRAPER_DRY_RUN"
    )]
    #[clap(next_help_heading = "Display")]
    pub dry_run: bool,

    /// Path to write OTel spans as JSONL for offline debugging
    #[arg(long, env = "RUST_SCRAPER_TRACE_FILE")]
    #[clap(next_help_heading = "Display")]
    pub trace_file: Option<std::path::PathBuf>,

    // ========== Crawler Settings ==========
    /// Maximum depth to crawl (0 = only seed URL)
    #[arg(long, default_value = "2", env = "RUST_SCRAPER_MAX_DEPTH")]
    #[clap(next_help_heading = "Crawler Settings")]
    pub max_depth: u8,

    /// Request timeout in seconds
    #[arg(long, default_value = "30", env = "RUST_SCRAPER_TIMEOUT_SECS")]
    #[clap(next_help_heading = "Crawler Settings")]
    pub timeout_secs: u64,

    /// URL patterns to include (glob-style)
    #[arg(
        long = "include-pattern",
        env = "RUST_SCRAPER_INCLUDE",
        value_delimiter = ','
    )]
    #[clap(next_help_heading = "Crawler Settings")]
    pub include_patterns: Vec<String>,

    /// URL patterns to exclude (glob-style)
    #[arg(
        long = "exclude-pattern",
        env = "RUST_SCRAPER_EXCLUDE",
        value_delimiter = ','
    )]
    #[clap(next_help_heading = "Crawler Settings")]
    pub exclude_patterns: Vec<String>,

    /// Estrategia de nombre de archivo para assets descargados: hash (default), slug, content-disposition
    #[arg(long, default_value = "hash", value_parser = ["hash", "slug", "content-disposition"])]
    pub asset_naming: String,

    /// Maximum concurrent asset downloads per page (default: 3)
    #[arg(
        long,
        default_value = "3",
        env = "RUST_SCRAPER_DOWNLOAD_CONCURRENCY",
        value_parser = parse_download_concurrency,
        help = "Máximo de descargas de assets concurrentes por página (mínimo 1)"
    )]
    pub download_concurrency: usize,

    // ========== HTTP Client Settings ==========
    /// Maximum number of retry attempts
    #[arg(long, default_value = "3", env = "RUST_SCRAPER_MAX_RETRIES")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub max_retries: u32,

    /// Base delay for exponential backoff (ms)
    #[arg(long, default_value = "1000", env = "RUST_SCRAPER_BACKOFF_BASE_MS")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub backoff_base_ms: u64,

    /// Maximum delay for exponential backoff (ms)
    #[arg(long, default_value = "10000", env = "RUST_SCRAPER_BACKOFF_MAX_MS")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub backoff_max_ms: u64,

    /// Accept-Language header value
    #[arg(
        long,
        default_value = "en-US,en;q=0.9",
        env = "RUST_SCRAPER_ACCEPT_LANGUAGE"
    )]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub accept_language: String,

    /// Custom User-Agent header value (overrides Chrome 145 default)
    #[arg(long, env = "RUST_SCRAPER_USER_AGENT")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub user_agent: Option<String>,

    // ========== Download Settings ==========
    /// Maximum file size to download in bytes (default: 50MB)
    #[arg(long, default_value = "52428800", env = "RUST_SCRAPER_MAX_FILE_SIZE")]
    #[clap(next_help_heading = "Download Settings")]
    pub max_file_size: u64,

    /// Timeout for individual asset downloads in seconds
    #[arg(long, default_value = "30", env = "RUST_SCRAPER_DOWNLOAD_TIMEOUT")]
    #[clap(next_help_heading = "Download Settings")]
    pub download_timeout: u64,

    // ========== AI Settings (feature-gated) ==========
    /// Relevance threshold for AI semantic filtering (0.0-1.0)
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "0.3", env = "RUST_SCRAPER_THRESHOLD")]
    #[clap(next_help_heading = "AI Settings")]
    pub threshold: f32,

    /// Maximum tokens per chunk for AI processing
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "512", env = "RUST_SCRAPER_MAX_TOKENS")]
    #[clap(next_help_heading = "AI Settings")]
    pub max_tokens: usize,

    /// Run AI model in offline mode
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_OFFLINE", action = clap::ArgAction::SetTrue)]
    #[clap(next_help_heading = "AI Settings")]
    pub offline: bool,

    // ========== Sitemap Settings ==========
    /// Maximum recursion depth for sitemap indexes
    #[arg(long, default_value = "3", env = "RUST_SCRAPER_SITEMAP_DEPTH")]
    #[clap(next_help_heading = "Sitemap Settings")]
    pub sitemap_depth: u8,

    // ========== Elastic Ingestion (Issue #51, PR5) ==========
    /// CPU core override for the elastic ingestion Rayon pool (else auto-detect)
    #[arg(long, env = "RUST_SCRAPER_CPU_CORES")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub cpu_cores: Option<usize>,

    /// RAM budget override for the byte-weighted semaphore (`8GB`, `2048MB`, or bytes)
    #[arg(long, env = "RUST_SCRAPER_RAM_BUDGET")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub ram_budget: Option<String>,

    /// SQLite database path override for persisted resources/chunks
    #[arg(long, env = "RUST_SCRAPER_DB_PATH")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub db_path: Option<std::path::PathBuf>,

    /// Enable elastic ingestion pipeline (streaming, SQLite dedup, Rayon CPU bridge)
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_ELASTIC")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub elastic: bool,

    /// Write extracted vectors to a JSONL file for RAG pipelines. Use `-` for
    /// stdout. No SQLite dependency — available in every build (core binary too).
    #[arg(long, env = "RUST_SCRAPER_OUTPUT_VECTORS")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub output_vectors: Option<String>,

    // ========== Competitive Features Phase 1 ==========
    /// Pages between automatic checkpoint saves (0 = disabled)
    /// NOTE: Checkpoint is for programmatic use (Engine API) only.
    /// CLI --resume uses StateStore instead of checkpoints.
    #[arg(long, default_value = "100", env = "RUST_SCRAPER_CHECKPOINT_INTERVAL")]
    #[clap(next_help_heading = "Competitive Features")]
    pub checkpoint_interval: u64,

    /// Disable checkpoint persistence entirely
    /// NOTE: Checkpoint is for programmatic use (Engine API) only.
    /// CLI --resume uses StateStore instead of checkpoints.
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_NO_CHECKPOINT")]
    #[clap(next_help_heading = "Competitive Features")]
    pub no_checkpoint: bool,

    /// Skip robots.txt enforcement
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_IGNORE_ROBOTS")]
    #[clap(next_help_heading = "Competitive Features")]
    pub ignore_robots: bool,

    /// Enable autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_AUTOSCALE")]
    #[clap(next_help_heading = "Competitive Features")]
    pub autoscale: bool,

    /// Disable session pool health checks
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_NO_SESSION_HEALTH")]
    #[clap(next_help_heading = "Competitive Features")]
    pub no_session_health: bool,

    /// TLS/HTTP2 profile name (default: Chrome145)
    #[arg(long, default_value = "Chrome145", env = "RUST_SCRAPER_H2_PROFILE")]
    #[clap(next_help_heading = "Competitive Features")]
    pub h2_profile: String,

    /// JavaScript rendering strategy: static (wreq only), hybrid (3-layer), full (Chromiumoxide only)
    #[arg(
        long,
        default_value = "static",
        value_enum,
        env = "RUST_SCRAPER_JS_STRATEGY"
    )]
    #[clap(next_help_heading = "JS Rendering")]
    pub js_strategy: JsStrategy,

    /// Path to the obscura binary (default: "obscura")
    #[arg(long, default_value = "obscura", env = "RUST_SCRAPER_OBSCURA_BINARY")]
    #[clap(next_help_heading = "JS Rendering")]
    pub obscura_binary: String,

    // ========== Batch Processing ==========
    /// Enable batch mode — read URLs from stdin (one per line)
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_BATCH")]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch: bool,

    /// Path to a file containing URLs to crawl (one per line)
    #[arg(long, env = "RUST_SCRAPER_BATCH_FILE")]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch_file: Option<std::path::PathBuf>,

    /// Maximum concurrent URLs in batch mode
    #[arg(long, default_value = "5", env = "RUST_SCRAPER_BATCH_CONCURRENCY")]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch_concurrency: usize,

    // ========== Item Pipeline ==========
    /// Enable item pipeline processing (validate → clean → output)
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_PIPELINE")]
    #[clap(next_help_heading = "Item Pipeline")]
    pub pipeline: bool,

    /// Pipeline output format: jsonl (default), none
    #[arg(
        long,
        default_value = "jsonl",
        value_enum,
        env = "RUST_SCRAPER_PIPELINE_OUTPUT"
    )]
    #[clap(next_help_heading = "Item Pipeline")]
    pub pipeline_output: PipelineOutputFormat,
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
    Bash,
    Elvish,
    Fish,
    PowerShell,
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
    /// `--ram-budget` is parsed via [`parse_ram_bytes`] so it accepts suffixed
    /// values (`8GB`, `2048MB`, plain bytes). The result feeds
    /// [`ElasticConfig::resolve`] → Rayon pool size, byte-weighted semaphore,
    /// and SQLite path.
    ///
    /// [`ElasticConfig::resolve`]: crate::infrastructure::autotuning::ElasticConfig::resolve
    /// [`parse_ram_bytes`]: crate::infrastructure::autotuning::parse_ram_bytes
    /// [`ElasticOverrides`]: crate::infrastructure::autotuning::ElasticOverrides
    #[must_use]
    pub fn elastic_overrides(&self) -> crate::infrastructure::autotuning::ElasticOverrides {
        use crate::infrastructure::autotuning::{parse_ram_bytes, ElasticOverrides};
        ElasticOverrides {
            cpu_cores: self.cpu_cores,
            ram_budget_bytes: self.ram_budget.as_deref().and_then(parse_ram_bytes),
            max_resource_bytes: None,
            db_path: self.db_path.clone(),
        }
    }
}

// ============================================================================
// From<Args> for CrawlOptions
// ============================================================================

impl From<Args> for crate::application::crawl_options::CrawlOptions {
    /// Convert CLI arguments into structured [`CrawlOptions`].
    ///
    /// This is an owned, lossless conversion — every field in `Args` maps
    /// to exactly one field in `CrawlOptions`. The `url` field is parsed
    /// from `Option<String>` into `Url` (panics if invalid; CLI validation
    /// guarantees validity before this point).
    fn from(args: Args) -> Self {
        use crate::application::crawl_options::{
            CrawlLimits, ExportOptions, IngestionTuning, NetworkOptions,
        };

        let url = url::Url::parse(args.url.as_deref().unwrap_or("https://example.com"))
            .expect("URL must be valid — CLI validation ensures this");

        let overrides = args.elastic_overrides();

        Self {
            url,
            verbosity: args.verbose,
            quiet: args.quiet,
            ai: args.clean_ai,
            crawl: CrawlLimits {
                selector: args.selector,
                max_depth: args.max_depth,
                max_pages: args.max_pages,
                single_page: args.single_page,
                include_patterns: args.include_patterns,
                exclude_patterns: args.exclude_patterns,
                interactive: args.interactive,
                resume: args.resume,
                state_dir: args.state_dir,
                use_sitemap: args.use_sitemap,
                sitemap_url: args.sitemap_url,
                checkpoint_interval: args.checkpoint_interval,
                no_checkpoint: args.no_checkpoint,
                ignore_robots: args.ignore_robots,
                no_session_health: args.no_session_health,
                autoscale_enabled: args.autoscale,
            },
            network: NetworkOptions {
                user_agent: args.user_agent,
                accept_language: args.accept_language,
                concurrency: args.concurrency,
                delay_ms: args.delay_ms,
                timeout_secs: args.timeout_secs,
                max_retries: args.max_retries,
                backoff_base_ms: args.backoff_base_ms,
                backoff_max_ms: args.backoff_max_ms,
                download_images: args.download_images || args.download_assets,
                download_documents: args.download_documents || args.download_assets,
                force_js_render: args.force_js_render,
                h2_profile: args.h2_profile,
                js_strategy: args.js_strategy,
                obscura_binary: args.obscura_binary,
            },
            export: ExportOptions {
                output_format: args.format,
                export_format: args.export_format,
                output_dir: args.output,
                dry_run: args.dry_run,
                quiet: args.quiet,
                obsidian_vault: args.vault,
                obsidian_rich_metadata: args.obsidian_rich_metadata,
                obsidian_tags: args.obsidian_tags.unwrap_or_default(),
                obsidian_wiki_links: args.obsidian_wiki_links,
                obsidian_relative_assets: args.obsidian_relative_assets,
                quick_save: args.quick_save,
            },
            elastic: IngestionTuning {
                enabled: args.elastic,
                cpu_cores: overrides.cpu_cores,
                ram_budget_bytes: overrides.ram_budget_bytes,
                db_path: overrides.db_path,
                max_resource_bytes: overrides.max_resource_bytes,
                output_vectors: args.output_vectors.clone(),
            },
            pipeline_enabled: args.pipeline,
            pipeline_output_format: args.pipeline_output,
            batch: crate::application::crawl_options::BatchOptions {
                enabled: args.batch || args.batch_file.is_some(),
                batch_file: args.batch_file,
                concurrency: args.batch_concurrency,
            },
            asset_naming: args.asset_naming,
            download_concurrency: args.download_concurrency,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::autotuning::ElasticOverrides;
    use proptest::prelude::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_elastic_flags_parsed_from_cli() {
        let args = Args::try_parse_from([
            "rust_scraper",
            "--cpu-cores",
            "4",
            "--ram-budget",
            "8GB",
            "--db-path",
            "/tmp/elastic.db",
        ])
        .expect("flags must parse");
        assert_eq!(args.cpu_cores, Some(4));
        assert_eq!(args.ram_budget.as_deref(), Some("8GB"));
        assert_eq!(args.db_path.as_deref(), Some(Path::new("/tmp/elastic.db")));

        let overrides = args.elastic_overrides();
        assert_eq!(overrides.cpu_cores, Some(4));
        assert_eq!(overrides.ram_budget_bytes, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(overrides.db_path, Some(PathBuf::from("/tmp/elastic.db")));
    }

    #[test]
    fn test_elastic_flags_default_to_none() {
        let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse must succeed");
        assert_eq!(args.cpu_cores, None);
        assert_eq!(args.ram_budget, None);
        assert_eq!(args.db_path, None);
        // No overrides → equals the all-None default.
        assert_eq!(args.elastic_overrides(), ElasticOverrides::default());
    }

    #[test]
    fn test_ram_budget_accepts_plain_bytes_and_suffixes() {
        let args = Args::try_parse_from(["rust_scraper", "--ram-budget", "2048MB"])
            .expect("suffixed ram-budget must parse");
        assert_eq!(
            args.elastic_overrides().ram_budget_bytes,
            Some(2048 * 1024 * 1024)
        );
    }

    // ========================================================================
    // Args → CrawlOptions full parity test
    // ========================================================================

    /// Build a minimal `Args` with **every** field set to a non-default,
    /// identifiable value so we can assert 1:1 mapping into `CrawlOptions`.
    fn args_with_all_fields_set() -> Args {
        Args {
            subcommand: None,

            // Target
            url: Some("https://example.com/test".into()),
            selector: "article.main".into(),

            // Output
            output: std::path::PathBuf::from("/tmp/test-output"),
            format: crate::OutputFormat::Json,
            export_format: crate::ExportFormat::Vector,

            // Obsidian
            obsidian_wiki_links: true,
            obsidian_tags: Some(vec!["tag-a".into(), "tag-b".into()]),
            obsidian_relative_assets: true,
            vault: Some(std::path::PathBuf::from("/tmp/vault")),
            quick_save: true,
            obsidian_rich_metadata: true,

            // Discovery
            delay_ms: 500,
            max_pages: 25,
            concurrency: crate::ConcurrencyConfig::new(8),
            use_sitemap: true,
            sitemap_url: Some("https://example.com/sitemap.xml".into()),

            // Behavior
            single_page: true,
            resume: true,
            state_dir: Some(std::path::PathBuf::from("/tmp/state")),
            download_images: true,
            download_documents: true,
            interactive: true,
            config_tui: true,
            clean_ai: true,
            force_js_render: true,

            // Display
            verbose: 3,
            quiet: true,
            dry_run: true,

            // Crawler settings
            max_depth: 5,
            timeout_secs: 60,
            include_patterns: vec!["/blog/**".into(), "/docs/**".into()],
            exclude_patterns: vec!["/admin/**".into()],

            // HTTP client
            max_retries: 7,
            backoff_base_ms: 2000,
            backoff_max_ms: 30_000,
            accept_language: "es-ES,es;q=0.9".into(),
            user_agent: Some("TestAgent/1.0".into()),

            // Download settings
            max_file_size: 100_000_000,
            download_timeout: 120,

            // Sitemap
            sitemap_depth: 4,

            // Elastic ingestion
            cpu_cores: Some(6),
            ram_budget: Some("4GB".into()),
            db_path: Some(std::path::PathBuf::from("/tmp/test.db")),
            elastic: true,

            // Competitive Features Phase 1
            checkpoint_interval: 50,
            no_checkpoint: true,
            ignore_robots: true,
            no_session_health: true,
            autoscale: true,
            h2_profile: "Chrome131".into(),

            // JS Rendering
            js_strategy: crate::domain::JsStrategy::Hybrid,
            obscura_binary: "/usr/local/bin/obscura".into(),

            // Item Pipeline
            pipeline: true,
            pipeline_output: PipelineOutputFormat::None,

            // Batch Processing
            batch: true,
            batch_file: Some(std::path::PathBuf::from("/tmp/urls.txt")),
            batch_concurrency: 8,

            // Asset download
            asset_naming: "slug".into(),

            ..Default::default()
        }
    }

    #[test]
    fn test_args_to_crawl_options_full_parity() {
        let args = args_with_all_fields_set();
        let opts = crate::application::crawl_options::CrawlOptions::from(args);

        // ── Top-level ──────────────────────────────────────────────────────
        assert_eq!(opts.url.as_str(), "https://example.com/test");
        assert_eq!(opts.verbosity, 3);
        assert!(opts.quiet);

        // ── CrawlLimits ────────────────────────────────────────────────────
        assert_eq!(opts.crawl.selector, "article.main");
        assert_eq!(opts.crawl.max_depth, 5);
        assert_eq!(opts.crawl.max_pages, 25);
        assert!(opts.crawl.single_page);
        assert_eq!(
            opts.crawl.include_patterns,
            vec!["/blog/**".to_owned(), "/docs/**".to_owned()]
        );
        assert_eq!(opts.crawl.exclude_patterns, vec!["/admin/**".to_owned()]);
        assert!(opts.crawl.interactive);
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

        // ── NetworkOptions ─────────────────────────────────────────────────
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
        assert!(opts.network.force_js_render);
        assert_eq!(opts.network.h2_profile, "Chrome131");
        assert_eq!(opts.network.js_strategy, crate::domain::JsStrategy::Hybrid);
        assert_eq!(opts.network.obscura_binary, "/usr/local/bin/obscura");

        // ── ExportOptions ──────────────────────────────────────────────────
        assert_eq!(opts.export.output_format, crate::OutputFormat::Json);
        assert_eq!(opts.export.export_format, crate::ExportFormat::Vector);
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

        // ── IngestionTuning ────────────────────────────────────────────────
        assert!(opts.elastic.enabled);
        assert_eq!(opts.elastic.cpu_cores, Some(6));
        assert_eq!(opts.elastic.ram_budget_bytes, Some(4 * 1024 * 1024 * 1024));
        assert_eq!(
            opts.elastic.db_path,
            Some(std::path::PathBuf::from("/tmp/test.db"))
        );

        // ── Item Pipeline ─────────────────────────────────────────────────
        assert!(opts.pipeline_enabled);
        assert_eq!(
            opts.pipeline_output_format,
            crate::domain::config::PipelineOutputFormat::None
        );

        // ── Asset naming ─────────────────────────────────────────────────
        assert_eq!(opts.asset_naming, "slug");
    }

    #[test]
    fn test_args_to_crawl_options_defaults() {
        let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse must succeed");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);

        // url defaults to example.com when None
        assert_eq!(opts.url.as_str(), "https://example.com/");
        assert_eq!(opts.verbosity, 0);
        assert!(!opts.quiet);

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
        assert!(!opts.network.force_js_render);

        assert_eq!(opts.export.output_format, crate::OutputFormat::Markdown);
        assert_eq!(opts.export.export_format, crate::ExportFormat::Jsonl);
        assert_eq!(opts.export.output_dir, std::path::PathBuf::from("output"));
        assert!(!opts.export.dry_run);
        assert!(!opts.export.quiet);
        assert!(opts.export.obsidian_vault.is_none());
        assert!(!opts.export.obsidian_rich_metadata);
        assert!(opts.export.obsidian_tags.is_empty());
        assert!(!opts.export.obsidian_wiki_links);
        assert!(!opts.export.obsidian_relative_assets);
        assert!(!opts.export.quick_save);

        assert!(!opts.elastic.enabled);
        assert!(opts.elastic.cpu_cores.is_none());
        assert!(opts.elastic.ram_budget_bytes.is_none());
        assert!(opts.elastic.db_path.is_none());

        assert!(!opts.pipeline_enabled);
        assert_eq!(
            opts.pipeline_output_format,
            crate::domain::config::PipelineOutputFormat::Jsonl
        );
        assert!(!opts.crawl.autoscale_enabled);
        // CLI default_value = "hash" (via #[arg(default_value)])
        assert_eq!(opts.asset_naming, "hash");
    }

    #[test]
    fn test_obsidian_tags_none_maps_to_empty_vec() {
        let args = Args {
            obsidian_tags: None,
            ..args_with_all_fields_set()
        };
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert!(opts.export.obsidian_tags.is_empty());
    }

    #[test]
    fn test_url_none_falls_back_to_example_com() {
        let args = Args {
            url: None,
            ..args_with_all_fields_set()
        };
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
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
            interactive in proptest::bool::ANY,
            config_tui in proptest::bool::ANY,
            force_js_render in proptest::bool::ANY,
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
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: wiki_links,
                obsidian_tags: None,
                obsidian_relative_assets: relative_assets,
                vault: None,
                quick_save,
                obsidian_rich_metadata: rich_metadata,
                delay_ms: 0,
                max_pages: 1,
                concurrency: crate::ConcurrencyConfig::default(),
                use_sitemap,
                sitemap_url: None,
                single_page,
                resume,
                state_dir: None,
                download_images,
                download_documents,
                interactive,
                config_tui,
                clean_ai,
                force_js_render,
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
                cpu_cores: None,
                ram_budget: None,
                db_path: None,
                elastic,
                pipeline,
                autoscale,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

            // Every bool field must roundtrip
            prop_assert_eq!(opts.export.obsidian_wiki_links, wiki_links);
            prop_assert_eq!(opts.export.obsidian_relative_assets, relative_assets);
            prop_assert_eq!(opts.export.quick_save, quick_save);
            prop_assert_eq!(opts.export.obsidian_rich_metadata, rich_metadata);
            prop_assert_eq!(opts.crawl.single_page, single_page);
            prop_assert_eq!(opts.crawl.resume, resume);
            prop_assert_eq!(opts.network.download_images, download_images);
            prop_assert_eq!(opts.network.download_documents, download_documents);
            prop_assert_eq!(opts.crawl.interactive, interactive);
            prop_assert_eq!(opts.network.force_js_render, force_js_render);
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
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: None,
                obsidian_relative_assets: false,
                vault: None,
                quick_save: false,
                obsidian_rich_metadata: false,
                delay_ms,
                max_pages,
                concurrency: crate::ConcurrencyConfig::default(),
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
                cpu_cores: None,
                ram_budget: None,
                db_path: None,
                elastic: false,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

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
                url: Some("https://example.com/prop".into()),
                selector,
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: None,
                obsidian_relative_assets: false,
                vault: None,
                quick_save: false,
                obsidian_rich_metadata: false,
                delay_ms: 0,
                max_pages: 1,
                concurrency: crate::ConcurrencyConfig::default(),
                use_sitemap: sitemap_url.is_some(),
                sitemap_url,
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
                cpu_cores: None,
                ram_budget: None,
                db_path: None,
                elastic: false,
                ..Default::default()
            };

            let expected_selector = args.selector.clone();
            let expected_accept_language = args.accept_language.clone();
            let expected_user_agent = args.user_agent.clone();
            let expected_sitemap_url = args.sitemap_url.clone();

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

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
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from(&output),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: None,
                obsidian_relative_assets: false,
                vault: vault.as_deref().map(std::path::PathBuf::from),
                quick_save: false,
                obsidian_rich_metadata: false,
                delay_ms: 0,
                max_pages: 1,
                concurrency: crate::ConcurrencyConfig::default(),
                use_sitemap: false,
                sitemap_url: None,
                single_page: false,
                resume: false,
                state_dir: state_dir.as_deref().map(std::path::PathBuf::from),
                download_images: false,
                download_documents: false,
                interactive: false,
                config_tui: false,
                clean_ai: false,
                force_js_render: false,
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
                cpu_cores: None,
                ram_budget: None,
                db_path: db_path.as_deref().map(std::path::PathBuf::from),
                elastic: false,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

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
                Some(v) => crate::ConcurrencyConfig::new(v),
                None => crate::ConcurrencyConfig::default(),
            };

            let expected_auto = concurrency.is_auto();
            let expected_value = concurrency.get();

            let args = Args {
                subcommand: None,
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: None,
                obsidian_relative_assets: false,
                vault: None,
                quick_save: false,
                obsidian_rich_metadata: false,
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
                interactive: false,
                config_tui: false,
                clean_ai: false,
                force_js_render: false,
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
                cpu_cores: None,
                ram_budget: None,
                db_path: None,
                elastic: false,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

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
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: Some(tags.clone()),
                obsidian_relative_assets: false,
                vault: None,
                quick_save: false,
                obsidian_rich_metadata: false,
                delay_ms: 0,
                max_pages: 1,
                concurrency: crate::ConcurrencyConfig::default(),
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
                cpu_cores: None,
                ram_budget: None,
                db_path: None,
                elastic: false,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);
            prop_assert_eq!(opts.export.obsidian_tags, tags);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn prop_elastic_overrides_roundtrip(
            cpu_cores in proptest::option::of(1usize..32),
            ram_gb in proptest::option::of(1u64..128),
        ) {
            let ram_budget = ram_gb.map(|g| format!("{g}GB"));

            let args = Args {
                subcommand: None,
                url: Some("https://example.com/prop".into()),
                selector: "body".into(),
                output: std::path::PathBuf::from("out"),
                format: crate::OutputFormat::Markdown,
                export_format: crate::ExportFormat::Jsonl,
                obsidian_wiki_links: false,
                obsidian_tags: None,
                obsidian_relative_assets: false,
                vault: None,
                quick_save: false,
                obsidian_rich_metadata: false,
                delay_ms: 0,
                max_pages: 1,
                concurrency: crate::ConcurrencyConfig::default(),
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
                cpu_cores,
                ram_budget: ram_budget.clone(),
                db_path: None,
                elastic: true,
                ..Default::default()
            };

            let opts = crate::application::crawl_options::CrawlOptions::from(args);

            prop_assert_eq!(opts.elastic.enabled, true);
            prop_assert_eq!(opts.elastic.cpu_cores, cpu_cores);
            prop_assert_eq!(
                opts.elastic.ram_budget_bytes,
                ram_gb.map(|g| g * 1024 * 1024 * 1024)
            );
        }
    }
}
