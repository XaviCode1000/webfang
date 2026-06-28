//! CLI Arguments for the rust_scraper binary.
//!
//! Parsed using `clap` with derive macros.

use crate::{ConcurrencyConfig, ExportFormat, OutputFormat};
use clap::Parser;

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
#[derive(Parser, Debug)]
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

    /// Interactive mode with TUI URL selector
    #[arg(long, env = "RUST_SCRAPER_INTERACTIVE")]
    #[clap(next_help_heading = "Behavior")]
    pub interactive: bool,

    /// Open configuration TUI to set all scraper options interactively
    #[arg(long, env = "RUST_SCRAPER_CONFIG_TUI")]
    #[clap(next_help_heading = "Behavior")]
    pub config_tui: bool,

    /// Use AI-powered semantic cleaning for better RAG output
    #[cfg(feature = "ai")]
    #[arg(long, default_value = "false", env = "RUST_SCRAPER_CLEAN_AI")]
    #[clap(next_help_heading = "Behavior")]
    pub clean_ai: bool,

    /// Feature flag placeholder when AI is not enabled
    #[cfg(not(feature = "ai"))]
    #[arg(
        long,
        default_value = "false",
        hide = true,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::autotuning::ElasticOverrides;
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
}
