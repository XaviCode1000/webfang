//! Structured crawl options — single source of truth for scraper configuration.
//!
//! Replaces direct `&Args` access throughout the codebase. Built via
//! `From<Args>` conversion in the CLI layer, then passed by value or
//! reference to services that need configuration.

use std::path::PathBuf;

use url::Url;

use crate::domain::config::{ConcurrencyConfig, ExportFormat, OutputFormat, PipelineOutputFormat};
use crate::domain::JsStrategy;
use crate::infrastructure::autotuning::ElasticOverrides;

// ============================================================================
// Top-level options
// ============================================================================

/// AI semantic-cleaning configuration, grouped from the four AI CLI flags.
///
/// Source is CLI ONLY this phase (`preflight.rs`/TUI untouched). `model` is
/// plumbed through even though `ExportConfig` does not yet consume it.
#[derive(Debug, Clone, PartialEq)]
pub struct AiConfig {
    /// Relevance threshold for AI semantic filtering (0.0-1.0).
    pub threshold: f32,
    /// Maximum tokens per chunk for AI processing.
    pub max_tokens: usize,
    /// Run AI model in offline mode.
    pub offline: bool,
    /// AI model identifier (empty = default).
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            threshold: 0.3,
            max_tokens: 32768,
            offline: false,
            model: String::new(),
        }
    }
}

/// Complete crawl configuration extracted from CLI arguments.
///
/// This is the single source of truth for all scraper settings. Services
/// receive `&CrawlOptions` instead of `&Args`, making configuration
/// explicit and testable.
#[derive(Debug, Clone)]
pub struct CrawlOptions {
    /// Target URL to scrape.
    pub url: Url,
    /// Verbosity level (-v, -vv, -vvv).
    pub verbosity: u8,
    /// Quiet mode — suppress info/debug output.
    pub quiet: bool,
    /// Enable AI-powered semantic cleaning (semantic_cleaner / ONNX, requires `ai` feature).
    pub ai: bool,
    /// Crawl scope and discovery settings.
    pub crawl: CrawlLimits,
    /// HTTP and network settings.
    pub network: NetworkOptions,
    /// Output and export settings.
    pub export: ExportOptions,
    /// Elastic ingestion tuning.
    pub elastic: IngestionTuning,
    /// Enable item pipeline processing (validate → clean → output).
    pub pipeline_enabled: bool,
    /// Pipeline output format (jsonl, none).
    pub pipeline_output_format: PipelineOutputFormat,
    /// Batch processing settings.
    pub batch: BatchOptions,
    /// Asset naming strategy: "hash", "slug", or "content-disposition".
    pub asset_naming: String,
    /// Maximum concurrent asset downloads per page.
    pub download_concurrency: usize,
    /// AI semantic-cleaning settings (from CLI AI flags).
    pub ai_config: AiConfig,
}

/// Batch processing settings.
#[derive(Debug, Clone)]
pub struct BatchOptions {
    /// Enable batch mode — read URLs from stdin or file.
    pub enabled: bool,
    /// Path to a file containing URLs (one per line). None = read from stdin.
    pub batch_file: Option<PathBuf>,
    /// Maximum concurrent URLs in batch mode.
    pub concurrency: usize,
}

// ============================================================================
// Sub-structs
// ============================================================================

/// Crawl scope: depth, patterns, sitemap, and behavioral flags.
#[derive(Debug, Clone)]
pub struct CrawlLimits {
    /// CSS selector for content extraction.
    pub selector: String,
    /// Maximum depth to crawl (0 = only seed URL).
    pub max_depth: u8,
    /// Maximum pages to scrape.
    pub max_pages: usize,
    /// Scrape only the seed URL without discovery or crawling.
    pub single_page: bool,
    /// URL patterns to include (glob-style).
    pub include_patterns: Vec<String>,
    /// URL patterns to exclude (glob-style).
    pub exclude_patterns: Vec<String>,
    /// Interactive mode with TUI URL selector.
    pub interactive: bool,
    /// Resume mode — skip URLs already processed.
    pub resume: bool,
    /// Custom state directory for resume mode.
    pub state_dir: Option<PathBuf>,
    /// Use sitemap for URL discovery.
    pub use_sitemap: bool,
    /// Explicit sitemap URL.
    pub sitemap_url: Option<String>,
    /// Pages between automatic checkpoint saves (0 = disabled).
    pub checkpoint_interval: u64,
    /// Disable checkpoint persistence entirely.
    pub no_checkpoint: bool,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
    /// Disable session pool health checks.
    pub no_session_health: bool,
    /// Enable autoscaled concurrency based on system RAM.
    pub autoscale_enabled: bool,
}

/// HTTP client and network behavior settings.
#[derive(Debug, Clone)]
pub struct NetworkOptions {
    /// Custom user-agent string (None = random from pool).
    pub user_agent: Option<String>,
    /// Accept-Language header value.
    pub accept_language: String,
    /// Concurrency configuration.
    pub concurrency: ConcurrencyConfig,
    /// Delay between requests in milliseconds.
    pub delay_ms: u64,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Base delay for exponential backoff (ms).
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (ms).
    pub backoff_max_ms: u64,
    /// Download images from the page.
    pub download_images: bool,
    /// Download documents from the page.
    pub download_documents: bool,
    /// Force JavaScript rendering for SPA sites.
    pub force_js_render: bool,
    /// TLS/HTTP2 profile name (e.g. Chrome145).
    pub h2_profile: String,
    /// JavaScript rendering strategy (static, hybrid, full).
    pub js_strategy: JsStrategy,
    /// Path to the obscura binary (default: "obscura").
    pub obscura_binary: String,
}

/// Output format, export format, and Obsidian integration settings.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Output format for individual files (markdown, text, json).
    pub output_format: OutputFormat,
    /// Export format for RAG pipeline (jsonl, vector, auto).
    pub export_format: ExportFormat,
    /// Output directory for scraped content.
    pub output_dir: PathBuf,
    /// Dry-run mode — discover URLs and print without scraping.
    pub dry_run: bool,
    /// Quiet mode — suppress info/debug output.
    pub quiet: bool,
    /// Path to Obsidian vault (auto-detects if not provided).
    pub obsidian_vault: Option<PathBuf>,
    /// Add rich metadata to frontmatter.
    pub obsidian_rich_metadata: bool,
    /// Tags to include in YAML frontmatter.
    pub obsidian_tags: Vec<String>,
    /// Convert same-domain links to Obsidian [[wiki-link]] syntax.
    pub obsidian_wiki_links: bool,
    /// Rewrite downloaded asset paths as relative to the .md file.
    pub obsidian_relative_assets: bool,
    /// Quick-save mode: save directly to vault _inbox folder.
    pub quick_save: bool,
}

/// Elastic ingestion pipeline tuning (hardware autotuning overrides).
#[derive(Debug, Clone, Default)]
pub struct IngestionTuning {
    /// Enable elastic ingestion pipeline.
    pub enabled: bool,
    /// CPU core override for the Rayon pool (None = auto-detect).
    pub cpu_cores: Option<usize>,
    /// RAM budget override in bytes (None = auto-detect).
    pub ram_budget_bytes: Option<u64>,
    /// SQLite database path override.
    pub db_path: Option<PathBuf>,
    /// Per-resource byte ceiling override.
    pub max_resource_bytes: Option<u64>,
    /// Write extracted vectors to a JSONL file (`<path>`) or stdout (`-`) for
    /// RAG pipelines. Backed by the dependency-free `StreamRepository` (no
    /// SQLite needed). Available in every build, including the core binary.
    pub output_vectors: Option<String>,
}

// ============================================================================
// Default implementations
// ============================================================================

impl Default for CrawlLimits {
    fn default() -> Self {
        Self {
            selector: "body".to_owned(),
            max_depth: 2,
            max_pages: 10,
            single_page: false,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            interactive: false,
            resume: false,
            state_dir: None,
            use_sitemap: false,
            sitemap_url: None,
            checkpoint_interval: 100,
            no_checkpoint: false,
            ignore_robots: false,
            no_session_health: false,
            autoscale_enabled: false,
        }
    }
}

impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            user_agent: None,
            accept_language: "en-US,en;q=0.9".to_owned(),
            concurrency: ConcurrencyConfig::default(),
            delay_ms: 1000,
            timeout_secs: 30,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            download_images: false,
            download_documents: false,
            force_js_render: false,
            h2_profile: "Chrome145".to_owned(),
            js_strategy: JsStrategy::default(),
            obscura_binary: "obscura".to_owned(),
        }
    }
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            output_format: OutputFormat::Markdown,
            export_format: ExportFormat::Jsonl,
            output_dir: PathBuf::from("output"),
            dry_run: false,
            quiet: false,
            obsidian_vault: None,
            obsidian_rich_metadata: false,
            obsidian_tags: Vec::new(),
            obsidian_wiki_links: false,
            obsidian_relative_assets: false,
            quick_save: false,
        }
    }
}

impl Default for CrawlOptions {
    fn default() -> Self {
        // Use a safe default URL for the default impl.
        // In practice, CrawlOptions is always built from Args where url is validated.
        let url = Url::parse("https://example.com").expect("hardcoded default URL must parse");
        Self {
            url,
            verbosity: 0,
            quiet: false,
            ai: false,
            crawl: CrawlLimits::default(),
            network: NetworkOptions::default(),
            export: ExportOptions::default(),
            elastic: IngestionTuning::default(),
            pipeline_enabled: false,
            pipeline_output_format: PipelineOutputFormat::default(),
            batch: BatchOptions::default(),
            asset_naming: "hash".to_string(),
            download_concurrency: 3,
            ai_config: AiConfig::default(),
        }
    }
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_file: None,
            concurrency: 5,
        }
    }
}

// ============================================================================
// From<ElasticOverrides> for IngestionTuning
// ============================================================================

impl From<ElasticOverrides> for IngestionTuning {
    fn from(overrides: ElasticOverrides) -> Self {
        Self {
            enabled: true,
            cpu_cores: overrides.cpu_cores,
            ram_budget_bytes: overrides.ram_budget_bytes,
            db_path: overrides.db_path,
            max_resource_bytes: overrides.max_resource_bytes,
            output_vectors: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_crawl_limits() {
        let limits = CrawlLimits::default();
        assert_eq!(limits.selector, "body");
        assert_eq!(limits.max_depth, 2);
        assert_eq!(limits.max_pages, 10);
        assert!(!limits.single_page);
        assert!(limits.include_patterns.is_empty());
        assert!(limits.exclude_patterns.is_empty());
        assert!(!limits.interactive);
        assert!(!limits.resume);
        assert!(limits.state_dir.is_none());
        assert!(!limits.use_sitemap);
        assert!(limits.sitemap_url.is_none());
        assert_eq!(limits.checkpoint_interval, 100);
        assert!(!limits.no_checkpoint);
        assert!(!limits.ignore_robots);
        assert!(!limits.no_session_health);
    }

    #[test]
    fn test_default_network_options() {
        let net = NetworkOptions::default();
        assert!(net.user_agent.is_none());
        assert_eq!(net.accept_language, "en-US,en;q=0.9");
        assert!(net.concurrency.is_auto());
        assert_eq!(net.delay_ms, 1000);
        assert_eq!(net.timeout_secs, 30);
        assert_eq!(net.max_retries, 3);
        assert_eq!(net.backoff_base_ms, 1000);
        assert_eq!(net.backoff_max_ms, 10000);
        assert!(!net.download_images);
        assert!(!net.download_documents);
        assert!(!net.force_js_render);
        assert_eq!(net.h2_profile, "Chrome145");
        assert_eq!(net.js_strategy, JsStrategy::Static);
        assert_eq!(net.obscura_binary, "obscura");
    }

    #[test]
    fn test_default_export_options() {
        let export = ExportOptions::default();
        assert_eq!(export.output_format, OutputFormat::Markdown);
        assert_eq!(export.export_format, ExportFormat::Jsonl);
        assert_eq!(export.output_dir, PathBuf::from("output"));
        assert!(!export.dry_run);
        assert!(!export.quiet);
        assert!(export.obsidian_vault.is_none());
        assert!(!export.obsidian_rich_metadata);
        assert!(export.obsidian_tags.is_empty());
        assert!(!export.obsidian_wiki_links);
        assert!(!export.obsidian_relative_assets);
        assert!(!export.quick_save);
    }

    #[test]
    fn test_default_ingestion_tuning() {
        let tuning = IngestionTuning::default();
        assert!(!tuning.enabled);
        assert!(tuning.cpu_cores.is_none());
        assert!(tuning.ram_budget_bytes.is_none());
        assert!(tuning.db_path.is_none());
        assert!(tuning.max_resource_bytes.is_none());
    }

    #[test]
    fn test_default_crawl_options() {
        let opts = CrawlOptions::default();
        assert_eq!(opts.url.as_str(), "https://example.com/");
        assert_eq!(opts.verbosity, 0);
        assert!(!opts.quiet);
        assert_eq!(opts.asset_naming, "hash");
    }

    #[test]
    fn test_from_elastic_overrides() {
        let overrides = ElasticOverrides {
            cpu_cores: Some(4),
            ram_budget_bytes: Some(8 * 1024 * 1024 * 1024),
            max_resource_bytes: Some(25 * 1024 * 1024),
            db_path: Some(PathBuf::from("/tmp/test.db")),
        };
        let tuning = IngestionTuning::from(overrides);
        assert!(tuning.enabled);
        assert_eq!(tuning.cpu_cores, Some(4));
        assert_eq!(tuning.ram_budget_bytes, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(tuning.max_resource_bytes, Some(25 * 1024 * 1024));
        assert_eq!(tuning.db_path, Some(PathBuf::from("/tmp/test.db")));
    }

    #[test]
    fn test_from_elastic_overrides_empty() {
        let overrides = ElasticOverrides::default();
        let tuning = IngestionTuning::from(overrides);
        assert!(tuning.enabled);
        assert!(tuning.cpu_cores.is_none());
        assert!(tuning.ram_budget_bytes.is_none());
        assert!(tuning.max_resource_bytes.is_none());
        assert!(tuning.db_path.is_none());
    }

    #[test]
    fn test_ai_config_defaults() {
        let config = AiConfig::default();
        assert_eq!(config.threshold, 0.3);
        assert_eq!(config.max_tokens, 32768);
        assert!(!config.offline);
        assert_eq!(config.model, "");
    }

    #[test]
    fn test_ai_config_custom_values() {
        let config = AiConfig {
            threshold: 0.7,
            max_tokens: 2048,
            offline: true,
            model: "granite-311m".to_string(),
        };
        assert_eq!(config.threshold, 0.7);
        assert_eq!(config.max_tokens, 2048);
        assert!(config.offline);
        assert_eq!(config.model, "granite-311m");
    }

    #[test]
    fn test_crawl_options_has_ai_config_field() {
        let opts = CrawlOptions::default();
        let ai = opts.ai_config;
        assert_eq!(ai.threshold, 0.3);
        assert_eq!(ai.max_tokens, 32768);
        assert!(!ai.offline);
        assert_eq!(ai.model, "");
    }

    #[test]
    fn test_ai_config_is_clone() {
        let config = AiConfig {
            threshold: 0.5,
            max_tokens: 1024,
            offline: true,
            model: "test".to_string(),
        };
        let cloned = config.clone();
        assert_eq!(config, cloned);
    }
}
