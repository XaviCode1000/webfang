//! Configuration types for the scraper.
//!
//! Contains [`ScraperConfig`] for scraper behavior and [`ConcurrencyConfig`]
//! for intelligent concurrency auto-detection.

// ============================================================================
// Output Format
// ============================================================================

use crate::adapters::downloader::AssetNamingStrategy;
use clap::ValueEnum;
use wreq_util::Profile;

/// Output format for scraped content.
///
/// # Examples
///
/// ```
/// use rust_scraper::OutputFormat;
///
/// let format = OutputFormat::Markdown;
/// assert_eq!(format, OutputFormat::Markdown);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
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
// Scraper Config
// ============================================================================

/// Scraper configuration for download and output behavior.
///
/// # Examples
///
/// ```
/// use rust_scraper::ScraperConfig;
///
/// // Default configuration
/// let config = ScraperConfig::default();
///
/// // Custom configuration with builder pattern
/// let config = ScraperConfig::default()
///     .with_images()
///     .with_documents()
///     .with_output_dir("./output".into())
///     .with_scraper_concurrency(5);
///
/// assert!(config.download_images);
/// assert!(config.download_documents);
/// assert_eq!(config.scraper_concurrency, 5);
/// ```
///
/// # Concurrency Recommendations
///
/// | Storage | Concurrency | Reason |
/// |---------|-------------|--------|
/// | HDD | 3 (default) | Avoids disk thrashing on mechanical drives |
/// | SSD | 5-8 | Faster random I/O |
/// | NVMe | 10+ | Very high IOPS |
#[derive(Debug, Clone)]
pub struct ScraperConfig {
    /// Enable image downloading (PNG, JPG, GIF, WEBP, SVG, BMP)
    pub download_images: bool,
    /// Enable document downloading (PDF, DOCX, XLSX, PPTX, etc.)
    pub download_documents: bool,
    /// Output directory for downloaded assets
    pub output_dir: std::path::PathBuf,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: Option<u64>,
    /// Timeout for individual asset downloads in seconds
    pub download_timeout_secs: u64,
    /// Maximum concurrent scrapers (default: 3 for HDD-aware on 4C CPU)
    pub scraper_concurrency: usize,
    /// Maximum concurrent asset downloads per page (default: 3)
    ///
    /// Separate from `scraper_concurrency` because asset downloads have a
    /// different I/O profile (bandwidth + disk writes vs. network + parsing).
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
}

impl Default for ScraperConfig {
    fn default() -> Self {
        Self {
            download_images: false,
            download_documents: false,
            output_dir: std::path::PathBuf::from("output"),
            max_file_size: Some(50 * 1024 * 1024), // 50MB default
            download_timeout_secs: 30,
            scraper_concurrency: 3, // HDD-aware: nproc - 1 for 4C CPU
            download_concurrency: 3, // Asset downloads: bandwidth + disk I/O
            max_pages: None,
            selector: "body".to_owned(),
            asset_h2_profile: Profile::Chrome145,
            asset_include_patterns: Vec::new(),
            asset_exclude_patterns: Vec::new(),
            asset_naming: AssetNamingStrategy::Hash,
        }
    }
}

impl ScraperConfig {
    /// Create a new config with default values.
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::ScraperConfig;
    ///
    /// let config = ScraperConfig::new();
    /// assert!(!config.download_images);
    /// ```
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
    pub fn with_output_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.output_dir = dir;
        self
    }

    /// Set scraper concurrency limit.
    ///
    /// # Recommendations
    ///
    /// - **HDD**: 3 (default) — avoids disk thrashing
    /// - **SSD**: 5-8 — faster random I/O
    /// - **NVMe**: 10+ — very high IOPS
    #[must_use]
    pub fn with_scraper_concurrency(mut self, concurrency: usize) -> Self {
        self.scraper_concurrency = concurrency;
        self
    }

    /// Set download concurrency limit (assets per page).
    #[must_use]
    pub fn with_download_concurrency(mut self, concurrency: usize) -> Self {
        self.download_concurrency = concurrency;
        self
    }

    /// Check if any download is enabled.
    pub fn has_downloads(&self) -> bool {
        self.download_images || self.download_documents
    }

    /// Build a `DownloadConfig` from this scraper configuration.
    ///
    /// This is the single source of truth for mapping ScraperConfig → DownloadConfig,
    /// eliminating duplication between the orchestrator and fallback paths.
    pub fn to_download_config(&self) -> crate::adapters::downloader::DownloadConfig {
        crate::adapters::downloader::DownloadConfig {
            output_dir: self.output_dir.clone(),
            timeout_secs: self.download_timeout_secs,
            max_file_size: self.max_file_size.unwrap_or(50 * 1024 * 1024),
            concurrency_limit: self.download_concurrency,
            include_patterns: self.asset_include_patterns.clone(),
            exclude_patterns: self.asset_exclude_patterns.clone(),
            h2_profile: self.asset_h2_profile,
            asset_naming: self.asset_naming,
            ..Default::default()
        }
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
}

// ============================================================================
// Concurrency Config
// ============================================================================

/// Concurrency configuration with smart auto-detection.
///
/// Provides intelligent defaults based on hardware capabilities:
/// - **Auto-detection**: Uses `std::thread::available_parallelism()` to detect CPU cores
/// - **HDD-aware**: Limits concurrency on systems with limited I/O
/// - **Safe bounds**: Clamps values between 1 and 16
///
/// # Examples
///
/// ```
/// use rust_scraper::ConcurrencyConfig;
///
/// // Auto-detect (default)
/// let config = ConcurrencyConfig::default();
///
/// // Explicit value
/// let config = ConcurrencyConfig::new(5);
///
/// // Get the resolved value
/// let concurrency = config.resolve();
/// println!("Using {} concurrent workers", concurrency);
/// ```
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
            value: Some(value.clamp(1, 16)),
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

        let cores = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(2);

        let optimal = match cores {
            1 | 2 => 1,
            3 | 4 => 3,
            5..=7 => 5,
            _ => (cores - 1).min(8),
        };

        optimal.clamp(1, 16)
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

/// Custom value parser for clap (accepts "auto" or number).
impl From<&str> for ConcurrencyConfig {
    fn from(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        if s == "auto" || s.is_empty() {
            Self::default()
        } else {
            s.parse().map(ConcurrencyConfig::new).unwrap_or_else(|_| {
                eprintln!("Warning: Invalid concurrency '{s}', using auto-detect");
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

// ============================================================================
// Elastic Ingestion Autotuning Config (Issue #51)
// ============================================================================

/// Hardware-autotuning configuration snapshot for the elastic ingestion
/// pipeline (Issue #51).
///
/// Serializable so it can be written to / read from a config file. Holds the
/// two core auto-detected sizing values; the fuller
/// [`crate::infrastructure::autotuning::ElasticConfig`] adds DB/pool parameters.
///
/// Resolution priority (frozen design decision #12): explicit override >
/// `RUST_SCRAPER_*` env var > auto-detected default.
///
/// # Examples
///
/// ```
/// use rust_scraper::AutotuningConfig;
///
/// // Explicit overrides win.
/// let cfg = AutotuningConfig::resolve(Some(4), Some(8 * 1024 * 1024 * 1024));
/// assert_eq!(cfg.cpu_cores, 4);
/// assert_eq!(cfg.ram_budget_bytes, 8 * 1024 * 1024 * 1024);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutotuningConfig {
    /// Detected/overridden CPU core count.
    pub cpu_cores: usize,
    /// Detected/overridden RAM budget in bytes.
    pub ram_budget_bytes: u64,
}

impl AutotuningConfig {
    /// Resolve the autotuning snapshot.
    ///
    /// Priority: `cpu_override`/`ram_override` > `RUST_SCRAPER_CPU_CORES` /
    /// `RUST_SCRAPER_RAM_BUDGET` env > auto-detected defaults.
    #[must_use]
    pub fn resolve(cpu_override: Option<usize>, ram_override: Option<u64>) -> Self {
        use crate::infrastructure::autotuning;
        Self {
            cpu_cores: autotuning::resolve_cpu_cores(cpu_override, autotuning::env_cpu_cores()),
            ram_budget_bytes: autotuning::resolve_ram_budget(
                ram_override,
                autotuning::env_ram_budget(),
            ),
        }
    }

    /// Build a snapshot from a resolved [`ElasticConfig`].
    #[must_use]
    pub fn from_elastic(elastic: &crate::infrastructure::autotuning::ElasticConfig) -> Self {
        Self {
            cpu_cores: elastic.cpu_cores,
            ram_budget_bytes: elastic.ram_budget_bytes,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scraper_config_default() {
        let config = ScraperConfig::default();
        assert!(!config.download_images);
        assert!(!config.download_documents);
        assert!(!config.has_downloads());
        assert_eq!(config.scraper_concurrency, 3);
    }

    #[test]
    fn test_scraper_config_with_images() {
        let config = ScraperConfig::default().with_images();
        assert!(config.download_images);
        assert!(config.has_downloads());
    }

    #[test]
    fn test_scraper_config_with_documents() {
        let config = ScraperConfig::default().with_documents();
        assert!(config.download_documents);
        assert!(config.has_downloads());
    }

    #[test]
    fn test_scraper_config_with_concurrency() {
        let config = ScraperConfig::default().with_scraper_concurrency(5);
        assert_eq!(config.scraper_concurrency, 5);
    }

    #[test]
    fn test_concurrency_config_new() {
        let config = ConcurrencyConfig::new(5);
        assert_eq!(config.resolve(), 5);
    }

    #[test]
    fn test_concurrency_config_auto() {
        let config = ConcurrencyConfig::auto();
        let value = config.resolve();
        assert!((1..=16).contains(&value));
    }

    #[test]
    fn test_concurrency_config_clamp() {
        let config = ConcurrencyConfig::new(100);
        assert_eq!(config.resolve(), 16);
    }

    #[test]
    fn test_concurrency_config_display() {
        let auto = ConcurrencyConfig::auto();
        assert_eq!(format!("{}", auto), "auto");

        let explicit = ConcurrencyConfig::new(5);
        assert_eq!(format!("{}", explicit), "5");
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

    #[test]
    fn test_autotuning_config_resolve_with_overrides() {
        let cfg = AutotuningConfig::resolve(Some(4), Some(8 * 1024 * 1024 * 1024));
        assert_eq!(cfg.cpu_cores, 4);
        assert_eq!(cfg.ram_budget_bytes, 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_autotuning_config_resolve_without_overrides_is_sane() {
        // No overrides → falls through env (likely unset) → auto-detected defaults.
        let cfg = AutotuningConfig::resolve(None, None);
        assert!(cfg.cpu_cores > 0, "cpu_cores must be positive");
        assert!(
            cfg.ram_budget_bytes > 0,
            "ram_budget_bytes must be positive"
        );
    }

    #[test]
    fn test_autotuning_config_serializes_roundtrip() {
        let cfg = AutotuningConfig {
            cpu_cores: 8,
            ram_budget_bytes: 16 * 1024 * 1024 * 1024,
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(json.contains("\"cpu_cores\":8"), "json: {json}");
        assert!(json.contains("\"ram_budget_bytes\":"));
        let back: AutotuningConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, cfg);
    }

    #[test]
    fn test_autotuning_config_from_elastic() {
        let elastic = crate::infrastructure::autotuning::ElasticConfig {
            cpu_cores: 6,
            ram_budget_bytes: 12 * 1024 * 1024 * 1024,
            max_resource_bytes: 25 * 1024 * 1024,
            db_pool_size: 6,
            db_path: std::path::PathBuf::from("/tmp/elastic.db"),
        };
        let snap = AutotuningConfig::from_elastic(&elastic);
        assert_eq!(snap.cpu_cores, 6);
        assert_eq!(snap.ram_budget_bytes, 12 * 1024 * 1024 * 1024);
    }
}
