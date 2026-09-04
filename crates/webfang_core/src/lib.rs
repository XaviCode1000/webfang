#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![cfg_attr(not(test), deny(clippy::disallowed_types))]
//! WebFang Core — Core scraping library
//!
//! Contains domain, application, and infrastructure layers for web scraping.
//! AI, MCP, and TUI adapters live in separate crates.
//!
//! # Architecture
//!
//! ```text
//! Domain (entities, errors)
//!     ↓
//! Application (services, use cases)
//!     ↓
//! Infrastructure (HTTP, parsers, converters)
//!     ↓
//! Adapters (detectors, downloaders)
//! ```
//!
//! **Dependency Rule:** Dependencies point inward. Domain never imports frameworks.

// ============================================================================
// Lints
// ============================================================================
#![deny(clippy::correctness)]
#![warn(clippy::suspicious)]
#![warn(clippy::style)]
#![warn(clippy::complexity)]
#![warn(clippy::perf)]
#![deny(missing_docs)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![allow(clippy::module_name_repetitions)]

// ============================================================================
// Modules
// ============================================================================

pub mod config;
pub mod di;
pub mod error;

pub mod domain;

pub mod adapters;
pub mod application;
pub mod cli;

pub mod extractor;
pub mod infrastructure;

/// Shared test doubles for unit tests (compiled only under `cfg(test)`).
#[cfg(test)]
pub(crate) mod test_fixtures;

// ============================================================================
// Re-exports
// ============================================================================

// Domain layer
pub use domain::{
    ContentType, CrawlError, CrawlErrorCategory, CrawlResult, CrawlerConfig, CrawlerConfigBuilder,
    DiscoveredUrl, DownloadedAsset, ScrapedContent, SessionId, ValidUrl,
};

// Application layer
pub use application::{
    batch::{BatchJob, BatchProcessor, BatchProgress, BatchResult},
    crawl_options::CrawlOptions,
    crawl_site, crawl_site_with_options, crawl_with_sitemap, create_http_client,
    create_http_client_with_config, detect_spa_content, discover_urls_single_fetch,
    extract_content, extract_domain,
    http_client::{HttpClient, HttpClientConfig, HttpError},
    is_allowed, is_excluded, is_internal_link, matches_pattern, scrape_multiple_with_limit,
    scrape_single_url, scrape_with_config, scrape_with_readability, EngineOptions,
    SpaDetectionResult,
};

// Adaptive selector types (feature-gated)
#[cfg(feature = "adaptive-selectors")]
pub use application::adaptive_engine::{AdaptiveRepairOutcome, AdaptiveSelectorEngine};

// Infrastructure layer
pub use infrastructure::{
    converter, crawler,
    export::{jsonl_exporter, state_store, vector_exporter},
    http,
    network::session_pool::{DomainSessionPool, SessionManager, SessionPoolConfig},
    output::file_saver,
    scraper::readability,
};

// Checkpoint types (application layer — consolidated from infrastructure)
pub use application::crawler::checkpoint::{
    BannedDomain, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
};

// Adapters
pub use adapters::url_path::{Domain, OutputPath, UrlPath};
pub use infrastructure::user_agent::{get_random_user_agent_from_pool, UserAgentCache};

// Export factory
pub use application::export_factory::{create_exporter, domain_from_url, process_results};

// CLI
pub use cli::{
    config::{init_logging_dual, is_no_color, should_emit_emoji, ConfigDefaults},
    error::{CliError, CliExit},
    summary::ScrapeSummary,
    Args, Commands, Shell,
};

// Observability - includes LogGuard for RAII logging
pub use infrastructure::observability::LogGuard;

// Config types (domain-owned; see ADR-0010 + ADR-0012 sub-slice 1)
pub use domain::config::{
    AssetNamingStrategy, AutotuningConfig, ConcurrencyConfig, ElasticOverrides, ExportFormat,
    OutputFormat, PipelineOutputFormat, ScraperConfig,
};

// Error and result types
pub use error::{Result, ScraperError};

// File saver
pub use infrastructure::output::file_saver::{save_results, ObsidianOptions};

// URL validation
pub use domain::url_validation::validate_and_parse_url;

// ============================================================================
// Build metadata
// ============================================================================

#[doc(hidden)]
pub(crate) mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// Return the extended version string including git commit and build date.
pub fn version_string() -> String {
    let commit = built_info::GIT_COMMIT_HASH_SHORT.unwrap_or("unknown");
    let build = built_info::BUILT_TIME_UTC;
    format!(
        "webfang {} (commit: {}, build: {})",
        env!("CARGO_PKG_VERSION"),
        commit,
        build
    )
}
