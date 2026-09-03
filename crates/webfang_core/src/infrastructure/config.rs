//! Configuration types for the scraper — shim re-exporting domain VOs.
//!
//! `ScraperConfig` family now lives in `crate::domain::config` (ADR-0010).
//! This module keeps a `pub use` shim so `crate::infrastructure::config::ScraperConfig`
//! and `webfang_core::ScraperConfig` remain valid.
//!
//! Infrastructure-specific extensions now live next to their targets:
//! - `ScraperConfig::to_download_config` → adapters::downloader
//! - `AutotuningConfig::{resolve, from_elastic}` → infrastructure::autotuning
//! (retired by issue #1099; this shim will be deleted).

// Domain-owned VOs — canonical definitions
pub use crate::domain::config::{
    AssetNamingStrategy, AutotuningConfig, ElasticOverrides, ScraperConfig,
};
// ConcurrencyConfig already re-exported in domain, keep shim
pub use crate::domain::config::ConcurrencyConfig;
// OutputFormat shim (domain owns it; keep infra path working)
pub use crate::domain::config::OutputFormat;

// NOTE (issue #1099): `ScraperConfig::to_download_config` now lives in
// adapters::downloader next to `DownloadConfig` (the single mapping source).

// ============================================================================
// Extension: AutotuningConfig resolve helpers (infra-owned logic)
// ============================================================================

impl AutotuningConfig {
    /// Resolve the autotuning snapshot.
    ///
    /// Priority: `cpu_override`/`ram_override` > `WEBFANG_*` env > auto-detected.
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

    /// Build a snapshot from a resolved `ElasticConfig`.
    #[must_use]
    pub fn from_elastic(elastic: &crate::infrastructure::autotuning::ElasticConfig) -> Self {
        Self {
            cpu_cores: elastic.cpu_cores,
            ram_budget_bytes: elastic.ram_budget_bytes,
        }
    }
}

// ============================================================================
// Tests — ScraperConfig + AutotuningConfig via shim
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
        assert!(!config.ignore_waf);
    }

    #[test]
    fn test_scraper_config_with_ignore_waf() {
        let config = ScraperConfig::default().with_ignore_waf(true);
        assert!(config.ignore_waf);
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

    #[test]
    fn test_autotuning_config_resolve_with_overrides() {
        let cfg = AutotuningConfig::resolve(Some(4), Some(8 * 1024 * 1024 * 1024));
        assert_eq!(cfg.cpu_cores, 4);
        assert_eq!(cfg.ram_budget_bytes, 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_autotuning_config_resolve_without_overrides_is_sane() {
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
