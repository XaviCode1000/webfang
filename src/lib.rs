//! Rust Scraper — Production-ready web scraper with Clean Architecture
//!
//! **Rust Scraper** is a high-performance, async web scraper designed for
//! building RAG (Retrieval-Augmented Generation) datasets.
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
//! Adapters (TUI, CLI, detectors)
//! ```
//!
//! **Dependency Rule:** Dependencies point inward. Domain never imports frameworks.
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::{scrape_with_readability, ScraperConfig};
//!
//! # #[tokio::main]
//! # async fn main() -> anyhow::Result<()> {
//! let url = url::Url::parse("https://example.com")?;
//! let config = ScraperConfig::default();
//! let results = scrape_with_readability(&url, &config).await?;
//! # Ok(())
//! # }
//! ```

// ============================================================================
// Lints
// ============================================================================
#![deny(clippy::correctness)]
#![warn(clippy::suspicious)]
#![warn(clippy::style)]
#![warn(clippy::complexity)]
#![warn(clippy::perf)]
#![allow(missing_docs)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![allow(clippy::module_name_repetitions)]

// ============================================================================
// Modules
// ============================================================================

pub mod config;
pub mod di;
pub mod error;

pub mod domain;
#[cfg(feature = "ai")]
pub use domain::semantic_cleaner::SemanticCleaner;
#[cfg(feature = "ai")]
pub use domain::DocumentChunk;

pub mod adapters;
pub mod application;
pub mod cli;

pub mod extractor;
pub mod infrastructure;

// ============================================================================
// Re-exports
// ============================================================================

// Domain layer
pub use domain::{
    ContentType, CrawlError, CrawlResult, CrawlerConfig, CrawlerConfigBuilder, DiscoveredUrl,
    DownloadedAsset, ExportFormat, JsRenderError, JsRenderer, ScrapedContent, ValidUrl,
};
#[cfg(feature = "ai")]
pub use error::SemanticError;

// Application layer
pub use application::{
    crawl_site, crawl_with_sitemap, create_http_client, detect_spa_content, discover_urls_for_tui,
    extract_domain,
    http_client::{HttpClient, HttpClientConfig, HttpError},
    is_allowed, is_excluded, is_internal_link, matches_pattern, scrape_multiple_with_limit,
    scrape_single_url_for_tui, scrape_urls_for_tui, scrape_with_config, scrape_with_readability,
    SpaDetectionResult,
};

// Infrastructure layer
pub use infrastructure::{
    converter, crawler,
    export::{jsonl_exporter, state_store, vector_exporter},
    http,
    output::file_saver,
    scraper::readability,
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

// Config types
pub use infrastructure::config::{
    AutotuningConfig, ConcurrencyConfig, OutputFormat, ScraperConfig,
};

// Error and result types
pub use clap::{Parser, ValueEnum};
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
        "rust_scraper {} (commit: {}, build: {})",
        env!("CARGO_PKG_VERSION"),
        commit,
        build
    )
}
