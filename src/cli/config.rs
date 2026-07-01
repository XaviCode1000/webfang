//! CLI Configuration Module
//!
//! T-010, T-011, T-012: Configuration defaults loading, NO_COLOR support.

use std::path::Path;

/// Default configuration values that can be overridden by a TOML file.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ConfigDefaults {
    /// Default output format (markdown, text, json)
    pub format: Option<String>,
    /// Default export format (jsonl, vector, auto)
    pub export_format: Option<String>,
    /// Default concurrency level (number or "auto")
    pub concurrency: Option<String>,
    /// Default CSS selector
    pub selector: Option<String>,
    /// Default maximum pages to scrape
    pub max_pages: Option<usize>,
    /// Default delay between requests (ms)
    pub delay_ms: Option<u64>,
    /// Default log level
    pub log_level: Option<String>,
    /// Whether to use sitemap by default
    pub use_sitemap: Option<bool>,
    /// Default Obsidian wiki-links setting
    pub obsidian_wiki_links: Option<bool>,
    /// Default Obsidian tags (comma-separated string)
    pub obsidian_tags: Option<String>,
    /// Default Obsidian relative assets setting
    pub obsidian_relative_assets: Option<bool>,
    /// Default Obsidian vault path
    pub vault_path: Option<String>,
}

impl ConfigDefaults {
    /// Load configuration from a TOML file, falling back to defaults.
    ///
    /// Returns defaults if the file doesn't exist or can't be parsed.
    pub fn load(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!(
                "Warning: Failed to parse config {}: {}, using defaults",
                path.display(),
                e
            );
            Self::default()
        })
    }
}

/// Check if NO_COLOR env var is set (any non-empty value).
pub fn is_no_color() -> bool {
    std::env::var("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Whether emoji should be emitted in output.
pub fn should_emit_emoji() -> bool {
    !is_no_color()
}

/// Initialize logging with configurable level, routing ALL output to stderr.
#[cfg(not(feature = "otel"))]
pub fn init_logging(level: &str) {
    init_logging_dual(level, false, is_no_color());
}

/// Initialize logging with configurable level (otel-enabled variant).
#[cfg(feature = "otel")]
pub fn init_logging(level: &str) {
    init_logging_dual(level, false, is_no_color(), None);
}

/// Dual-mode logging: forces stderr, supports quiet mode and NO_COLOR.
///
/// # Arguments
///
/// * `level` - Log level: "error", "warn", "info", "debug", "trace"
/// * `quiet` - If true, only warn+level output is shown
/// * `no_color` - If true, ANSI colors are disabled
#[cfg(not(feature = "otel"))]
pub fn init_logging_dual(level: &str, quiet: bool, no_color: bool) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = if quiet {
        EnvFilter::new("rust_scraper=warn,tokio=warn,reqwest=warn")
    } else {
        EnvFilter::new(format!("rust_scraper={level},tokio=warn,reqwest=warn"))
    };

    let fmt_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(!no_color)
        .with_target(true)
        .pretty();

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(filter)
        .init();
}

/// Dual-mode logging with optional OTel layer.
#[cfg(feature = "otel")]
pub fn init_logging_dual(
    level: &str,
    quiet: bool,
    no_color: bool,
    otel_layer: Option<tracing_opentelemetry::OpenTelemetryLayer<tracing_subscriber::Registry, opentelemetry_sdk::trace::Tracer>>,
) {
    use tracing_subscriber::{fmt, prelude::*};

    let filter = if quiet {
        tracing_subscriber::EnvFilter::new("rust_scraper=warn,tokio=warn,reqwest=warn")
    } else {
        tracing_subscriber::EnvFilter::new(format!("rust_scraper={level},tokio=warn,reqwest=warn"))
    };

    let fmt_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(!no_color)
        .with_target(true)
        .pretty();

    // OTel layer must be added directly on Registry, before EnvFilter
    tracing_subscriber::registry()
        .with(otel_layer)
        .with(filter)
        .with(fmt_layer)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_defaults_when_no_file() {
        let config = ConfigDefaults::load(Path::new("/nonexistent/path/config.toml"));
        assert!(config.format.is_none());
        assert!(config.concurrency.is_none());
        assert!(config.log_level.is_none());
    }

    #[test]
    fn test_load_from_valid_toml() {
        let tmp = std::env::temp_dir().join("rust_scraper_test_config.toml");
        let content = r#"
format = "json"
concurrency = "auto"
log_level = "debug"
max_pages = 20
"#;
        std::fs::write(&tmp, content).unwrap();
        let config = ConfigDefaults::load(&tmp);
        assert_eq!(config.format, Some("json".to_string()));
        assert_eq!(config.concurrency, Some("auto".to_string()));
        assert_eq!(config.log_level, Some("debug".to_string()));
        assert_eq!(config.max_pages, Some(20));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_is_no_color_default() {
        // Default should be false (no env var set in test)
        let val = is_no_color();
        assert!(!val);
    }

    #[test]
    fn test_should_emit_emoji_default() {
        assert!(should_emit_emoji());
    }

    #[cfg(feature = "otel")]
    mod otel_layer {
        use super::*;

        #[test]
        fn test_init_logging_dual_accepts_none_layer() {
            init_logging_dual("info", false, false, None);
        }

        #[test]
        fn test_init_logging_dual_accepts_some_layer() {
            let config =
                crate::infrastructure::observability::otel::OtelConfig::from_env();
            let (_guard, layer) =
                crate::infrastructure::observability::otel::init_otel_tracing(config)
                    .unwrap();
            init_logging_dual("info", false, false, Some(layer));
        }
    }
}
