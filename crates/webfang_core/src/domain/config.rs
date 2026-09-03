//! Shared configuration types for the domain layer.
//!
//! These types are shared across CLI, application, and infrastructure layers.
//! The domain layer owns these types; other layers import from here.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use super::budget::clamp::{clamp_budget, MAX_CONCURRENCY_CEILING};
use wreq_util::Profile;

// Re-export ExportFormat from entities (it's defined there with serde derives)
pub use super::entities::ExportFormat;

// Re-export HttpClientConfig — owned by the domain layer (see `http_config`).
pub use crate::domain::http_config::HttpClientConfig;

/// Pipeline output format — determines how pipeline items are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum PipelineOutputFormat {
    /// Write items as JSON Lines to a file (default).
    #[default]
    Jsonl,
    /// No pipeline output — items are processed but not written.
    None,
}

/// Output format for individual scraped content files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum OutputFormat {
    /// Markdown format with YAML frontmatter (recommended for RAG)
    #[default]
    Markdown,
    /// Structured JSON with metadata
    Json,
    /// Plain text without formatting
    Text,
}

// ============================================================================
// Asset naming strategy — moved from adapters::downloader (domain owns VO)
// ============================================================================

/// Strategy for generating downloaded asset filenames.
///
/// Domain owns this VO so `ScraperConfig` (domain) does not depend on
/// `adapters::downloader` (outward). Adapters re-exports this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetNamingStrategy {
    /// SHA-256 hash of content (first 12 hex chars). Dedup-friendly.
    #[default]
    Hash,
    /// Last path segment of the URL (e.g. `rust-book.pdf`).
    Slug,
    /// `filename=` from `Content-Disposition` header, falls back to `Hash`.
    ContentDisposition,
}

// ============================================================================
// ScraperConfig — moved from infrastructure::config (VO, domain owns)
// ============================================================================

/// Scraper configuration for download and output behavior. Domain-owned VO.
///
/// Mirrors `infrastructure::config::ScraperConfig` but lives in `domain::config`
/// so `application::*` can depend on it without outward `application→infrastructure`
/// violations (ADR-0010). Infrastructure keeps a `pub use` shim.
#[derive(Debug, Clone)]
pub struct ScraperConfig {
    /// Enable image downloading (PNG, JPG, GIF, WEBP, SVG, BMP)
    pub download_images: bool,
    /// Enable document downloading (PDF, DOCX, XLSX, PPTX, etc.)
    pub download_documents: bool,
    /// Output directory for downloaded assets
    pub output_dir: PathBuf,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: Option<u64>,
    /// Timeout for individual asset downloads in seconds
    pub download_timeout_secs: u64,
    /// Maximum concurrent scrapers (default: 3 for HDD-aware on 4C CPU)
    pub scraper_concurrency: usize,
    /// Maximum concurrent asset downloads per page (default: 3)
    pub download_concurrency: usize,
    /// Maximum pages to scrape (None = unlimited)
    pub max_pages: Option<usize>,
    /// CSS selector for content extraction (default: "body")
    pub selector: String,
    /// H2/TLS profile for asset downloads
    pub asset_h2_profile: Profile,
    /// URL glob patterns to include for asset downloads (empty = allow all)
    pub asset_include_patterns: Vec<String>,
    /// URL glob patterns to exclude for asset downloads (always applied)
    pub asset_exclude_patterns: Vec<String>,
    /// Strategy for naming downloaded asset files
    pub asset_naming: AssetNamingStrategy,
    /// Enable adaptive CSS selector repair (2-tier cascade)
    pub adaptive_selectors: bool,
    /// Bypass WAF/CAPTCHA detection entirely (REQ-WAF-07).
    pub ignore_waf: bool,
    /// Enable DOM pre-pruning before Readability (removes invisible/empty wrappers).
    pub dom_preprune: bool,
}

impl Default for ScraperConfig {
    fn default() -> Self {
        Self {
            download_images: false,
            download_documents: false,
            output_dir: PathBuf::from("output"),
            max_file_size: Some(50 * 1024 * 1024),
            download_timeout_secs: 30,
            scraper_concurrency: 3,
            download_concurrency: 3,
            max_pages: None,
            selector: "body".to_owned(),
            asset_h2_profile: Profile::Chrome145,
            asset_include_patterns: Vec::new(),
            asset_exclude_patterns: Vec::new(),
            asset_naming: AssetNamingStrategy::Hash,
            adaptive_selectors: false,
            ignore_waf: false,
            dom_preprune: true,
        }
    }
}

impl ScraperConfig {
    /// Create a new config with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable image downloading.
    #[must_use]
    pub fn with_images(mut self) -> Self {
        self.download_images = true;
        self
    }

    /// Enable document downloading.
    #[must_use]
    pub fn with_documents(mut self) -> Self {
        self.download_documents = true;
        self
    }

    /// Set custom output directory.
    #[must_use]
    pub fn with_output_dir(mut self, dir: PathBuf) -> Self {
        self.output_dir = dir;
        self
    }

    /// Set scraper concurrency limit.
    #[must_use]
    pub fn with_scraper_concurrency(mut self, concurrency: usize) -> Self {
        self.scraper_concurrency = concurrency;
        self
    }

    /// Set download concurrency limit (assets per page).
    #[must_use]
    pub fn with_download_concurrency(mut self, concurrency: usize) -> Self {
        self.download_concurrency = concurrency.clamp(1, usize::MAX);
        self
    }

    /// Set maximum file size for asset downloads in bytes.
    #[must_use]
    pub fn with_max_file_size(mut self, max_file_size: Option<u64>) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    /// Set timeout for individual asset downloads in seconds.
    #[must_use]
    pub fn with_download_timeout(mut self, timeout_secs: u64) -> Self {
        self.download_timeout_secs = timeout_secs;
        self
    }

    /// Set the WAF/CAPTCHA detection bypass flag (REQ-WAF-07).
    #[must_use]
    pub fn with_ignore_waf(mut self, ignore_waf: bool) -> Self {
        self.ignore_waf = ignore_waf;
        self
    }

    /// Check if any download is enabled.
    pub fn has_downloads(&self) -> bool {
        self.download_images || self.download_documents
    }

    /// Set maximum page limit.
    #[must_use]
    pub fn with_max_pages(mut self, pages: usize) -> Self {
        self.max_pages = Some(pages);
        self
    }

    /// Set CSS selector for content extraction.
    #[must_use]
    pub fn with_selector(mut self, selector: String) -> Self {
        self.selector = selector;
        self
    }

    /// Set H2/TLS profile for asset downloads.
    #[must_use]
    pub fn with_asset_h2_profile(mut self, v: Profile) -> Self {
        self.asset_h2_profile = v;
        self
    }

    /// Set URL glob patterns to include for asset downloads.
    #[must_use]
    pub fn with_asset_include_patterns(mut self, v: Vec<String>) -> Self {
        self.asset_include_patterns = v;
        self
    }

    /// Set URL glob patterns to exclude for asset downloads.
    #[must_use]
    pub fn with_asset_exclude_patterns(mut self, v: Vec<String>) -> Self {
        self.asset_exclude_patterns = v;
        self
    }

    /// Set strategy for naming downloaded asset files.
    #[must_use]
    pub fn with_asset_naming(mut self, v: AssetNamingStrategy) -> Self {
        self.asset_naming = v;
        self
    }

    /// Enable/disable DOM pre-pruning before Readability.
    #[must_use]
    pub fn with_dom_preprune(mut self, enabled: bool) -> Self {
        self.dom_preprune = enabled;
        self
    }
}

// ============================================================================
// Elastic / Autotuning VOs — domain owns the DTOs, infra owns resolve logic
// ============================================================================

/// Overrides supplied by CLI flags — domain-owned DTO so `application::crawl_options`
/// can depend on it without `application→infrastructure` (ADR-0010).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElasticOverrides {
    /// `--cpu-cores` override.
    pub cpu_cores: Option<usize>,
    /// `--ram-budget` override (bytes).
    pub ram_budget_bytes: Option<u64>,
    /// `--max-resource-mb` override (bytes).
    pub max_resource_bytes: Option<u64>,
    /// `--db-path` override.
    pub db_path: Option<PathBuf>,
}

/// Hardware-autotuning snapshot — domain-owned VO for `ElasticIngestion` wiring.
///
/// Pure DTO; `resolve`/`from_elastic` live in `infrastructure::config` as an
/// `impl AutotuningConfig` for the domain type so `domain` stays free of
/// `infrastructure::autotuning` imports (inward-only).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutotuningConfig {
    /// Detected/overridden CPU core count.
    pub cpu_cores: usize,
    /// Detected/overridden RAM budget in bytes.
    pub ram_budget_bytes: u64,
}

/// Concurrency configuration with smart auto-detection.
///
/// Provides intelligent defaults based on hardware capabilities:
/// - **Auto-detection**: derives CPU cores from the process-wide hardware
///   seam ([`crate::domain::budget::detector::system_parallelism`], cgroup
///   limits included — never raw `available_parallelism`) so "auto" means
///   the same thing everywhere (#897 item 3)
/// - **HDD-aware**: Limits concurrency on systems with limited I/O
/// - **Safe bounds**: Clamps values between 1 and 16
#[derive(Debug, Clone)]
pub struct ConcurrencyConfig {
    /// Explicit concurrency value (None = auto-detect)
    value: Option<usize>,
    /// Whether to use auto-detection
    auto_detect: bool,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            value: None,
            auto_detect: true,
        }
    }
}

impl std::fmt::Display for ConcurrencyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_auto() {
            write!(f, "auto")
        } else if let Some(value) = self.value {
            write!(f, "{value}")
        } else {
            write!(f, "auto")
        }
    }
}

impl ConcurrencyConfig {
    /// Create a new config with explicit value.
    ///
    /// # Arguments
    ///
    /// * `value` - Explicit concurrency value (will be clamped 1-16)
    #[must_use]
    pub fn new(value: usize) -> Self {
        Self {
            value: Some(clamp_budget(value, NonZeroUsize::MIN, MAX_CONCURRENCY_CEILING).get()),
            auto_detect: false,
        }
    }

    /// Create auto-detecting config (default).
    #[must_use]
    pub fn auto() -> Self {
        Self::default()
    }

    /// Resolve the actual concurrency value.
    ///
    /// Uses auto-detection based on CPU cores:
    /// - 1-2 cores: 1 (avoid overwhelming system)
    /// - 4 cores: 3 (HDD-aware default)
    /// - 8+ cores: min(cores - 1, 8)
    pub fn resolve(&self) -> usize {
        if let Some(value) = self.value {
            return value;
        }

        // #897 item 3: derive from the canonical hardware seam instead of
        // calling `available_parallelism` directly, so "auto" resolves
        // identically in every subsystem (cgroup limits included).
        let cores = crate::domain::budget::detector::system_parallelism().get();

        let optimal = match cores {
            1 | 2 => 1,
            3 | 4 => 3,
            5..=7 => 5,
            _ => (cores - 1).min(8),
        };

        clamp_budget(optimal, NonZeroUsize::MIN, MAX_CONCURRENCY_CEILING).get()
    }

    /// Check if this config uses auto-detection.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.auto_detect && self.value.is_none()
    }

    /// Get the raw value if explicitly set.
    #[must_use]
    pub fn get(&self) -> Option<usize> {
        self.value
    }
}

impl From<&str> for ConcurrencyConfig {
    fn from(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        if s == "auto" || s.is_empty() {
            Self::default()
        } else {
            s.parse().map(ConcurrencyConfig::new).unwrap_or_else(|_| {
                tracing::warn!("Invalid concurrency '{s}', using auto-detect");
                Self::default()
            })
        }
    }
}

impl std::str::FromStr for ConcurrencyConfig {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let s = s.trim().to_lowercase();
        if s == "auto" || s.is_empty() {
            Ok(Self::default())
        } else {
            s.parse::<usize>().map(ConcurrencyConfig::new)
        }
    }
}

impl clap::builder::ValueParserFactory for ConcurrencyConfig {
    type Parser = ConcurrencyValueParser;

    fn value_parser() -> Self::Parser {
        ConcurrencyValueParser
    }
}

/// Custom value parser for clap concurrency arguments.
#[derive(Debug, Clone)]
pub struct ConcurrencyValueParser;

impl clap::builder::TypedValueParser for ConcurrencyValueParser {
    type Value = ConcurrencyConfig;

    fn parse_ref(
        &self,
        _cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let value = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;

        let value = value.trim().to_lowercase();
        if value.is_empty() || value == "auto" {
            return Ok(ConcurrencyConfig::default());
        }

        value
            .parse::<usize>()
            .map(ConcurrencyConfig::new)
            .map_err(|_| {
                clap::Error::raw(
                    clap::error::ErrorKind::InvalidValue,
                    format!(
                        "'{value}' is not a valid concurrency value (expected number or 'auto')"
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_output_format_default() {
        let format = PipelineOutputFormat::default();
        assert_eq!(format, PipelineOutputFormat::Jsonl);
    }

    #[test]
    fn test_pipeline_output_format_variants() {
        let jsonl = PipelineOutputFormat::Jsonl;
        let none = PipelineOutputFormat::None;
        assert_ne!(jsonl, none);
    }

    #[test]
    fn test_output_format_default() {
        let format = OutputFormat::default();
        assert_eq!(format, OutputFormat::Markdown);
    }

    #[test]
    fn test_output_format_variants() {
        let md = OutputFormat::Markdown;
        let json = OutputFormat::Json;
        let text = OutputFormat::Text;
        assert_ne!(md, json);
        assert_ne!(md, text);
        assert_ne!(json, text);
    }

    #[test]
    fn test_export_format_default() {
        let format = ExportFormat::default();
        assert_eq!(format, ExportFormat::Jsonl);
    }

    #[test]
    fn test_export_format_variants() {
        let jsonl = ExportFormat::Jsonl;
        let vector = ExportFormat::Vector;
        let auto = ExportFormat::Auto;
        assert_ne!(jsonl, vector);
        assert_ne!(jsonl, auto);
        assert_ne!(vector, auto);
    }

    #[test]
    fn test_concurrency_config_default_is_auto() {
        let config = ConcurrencyConfig::default();
        assert!(config.is_auto());
    }

    #[test]
    fn test_concurrency_config_new_explicit() {
        let config = ConcurrencyConfig::new(5);
        assert!(!config.is_auto());
        assert_eq!(config.resolve(), 5);
    }

    #[test]
    fn test_concurrency_config_clamps() {
        let config = ConcurrencyConfig::new(100);
        assert_eq!(config.resolve(), 16);

        let config = ConcurrencyConfig::new(0);
        assert_eq!(config.resolve(), 1);
    }

    #[test]
    fn test_concurrency_config_display() {
        let auto = ConcurrencyConfig::auto();
        assert_eq!(format!("{auto}"), "auto");

        let explicit = ConcurrencyConfig::new(5);
        assert_eq!(format!("{explicit}"), "5");
    }

    #[test]
    fn test_concurrency_config_from_str() {
        let config = ConcurrencyConfig::from("5");
        assert_eq!(config.resolve(), 5);

        let config = ConcurrencyConfig::from("auto");
        assert!(config.is_auto());

        let config = ConcurrencyConfig::from("");
        assert!(config.is_auto());
    }

    #[test]
    fn test_concurrency_config_from_str_invalid() {
        let config = ConcurrencyConfig::from("not-a-number");
        assert!(config.is_auto());
    }

    /// Characterization sweep: every explicit value must produce exactly
    /// what the canonical budget clamp produces (single-source invariant).
    #[test]
    fn explicit_values_match_canonical_clamp_sweep() {
        use std::num::NonZeroUsize;

        let one = NonZeroUsize::new(1).expect("1 is non-zero");
        for value in [0, 1, 2, 3, 15, 16, 17, 100, usize::MAX] {
            let expected = crate::domain::budget::clamp::clamp_budget(
                value,
                one,
                crate::domain::budget::clamp::MAX_CONCURRENCY_CEILING,
            );
            assert_eq!(
                ConcurrencyConfig::new(value).resolve(),
                expected.get(),
                "explicit {value} diverges from canonical clamp"
            );
        }
    }

    /// Characterization of the auto-detection table: reference impl of
    /// TODAY'S `resolve()` math (1-2→1, 3-4→3, 5-7→5, else min(cores−1, 8)).
    /// Guards the "auto" path while the clamp sites delegate.
    #[test]
    fn auto_path_matches_legacy_table() {
        // #897 item 3: derive from the canonical hardware seam instead of
        // calling `available_parallelism` directly, so "auto" resolves
        // identically in every subsystem (cgroup limits included).
        let cores = crate::domain::budget::detector::system_parallelism().get();
        let expected = match cores {
            1 | 2 => 1,
            3 | 4 => 3,
            5..=7 => 5,
            _ => (cores - 1).min(8),
        }
        .clamp(1, 16);
        assert_eq!(ConcurrencyConfig::default().resolve(), expected);
    }

    // === TDD RED for 990 task 1.2: ScraperConfig must live in domain::config ===
    #[test]
    fn scraper_config_lives_in_domain() {
        // RED: this test references ScraperConfig that does NOT yet exist in
        // domain::config — compile should fail until GREEN moves it.
        let cfg = crate::domain::config::ScraperConfig::default();
        assert!(!cfg.download_images);
        assert_eq!(cfg.scraper_concurrency, 3);
    }

    #[test]
    fn scraper_config_builder_in_domain() {
        let cfg = crate::domain::config::ScraperConfig::default()
            .with_images()
            .with_scraper_concurrency(5);
        assert!(cfg.download_images);
        assert_eq!(cfg.scraper_concurrency, 5);
    }

    // TDD RED for 990 task 2.1: downloader_port must exist in domain
    #[test]
    fn downloader_port_trait_exists() {
        let err = crate::domain::downloader_port::DownloadError::Timeout(5);
        assert!(err.to_string().contains("timed out"));
    }

    // TDD RED for 990 task 2.2: crawler_port SitemapConfig must exist
    #[test]
    fn crawler_port_sitemap_exists() {
        let cfg = crate::domain::crawler_port::SitemapConfig::default();
        assert_eq!(cfg.max_depth, 3);
        assert_eq!(cfg.concurrency, 5);
    }
}
