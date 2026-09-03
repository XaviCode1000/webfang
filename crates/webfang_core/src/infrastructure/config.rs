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

// NOTE (issue #1099): `AutotuningConfig::{resolve, from_elastic}` now live in
// infrastructure::autotuning next to `ElasticConfig::resolve` (moved verbatim).
