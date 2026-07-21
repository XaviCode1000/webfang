//! Shared configuration types for the domain layer.
//!
//! These types are shared across CLI, application, and infrastructure layers.
//! The domain layer owns these types; other layers import from here.

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

/// Concurrency configuration with smart auto-detection.
///
/// Provides intelligent defaults based on hardware capabilities:
/// - **Auto-detection**: Uses `std::thread::available_parallelism()` to detect CPU cores
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
        self.auto_detect
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
}
