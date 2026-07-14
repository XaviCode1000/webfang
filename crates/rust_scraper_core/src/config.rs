//! Centralized configuration with validation
//!
//! Provides a single entry point for all application configuration,
//! with validation and feature gating.

use crate::domain::config::HttpClientConfig;
use crate::domain::site::CrawlerConfig;
use crate::infrastructure::config::ScraperConfig;
use crate::infrastructure::output::file_saver::ObsidianOptions;

/// Central application configuration
///
/// Combines all configuration types into a single validated structure.
/// Following Clean Architecture: configuration is infrastructure concern.
#[derive(Debug, Clone)]
pub struct Config {
    /// Scraping configuration
    pub scraper: ScraperConfig,
    /// Crawling configuration
    pub crawler: CrawlerConfig,
    /// HTTP client configuration
    pub http: HttpClientConfig,
    /// Obsidian integration options
    pub obsidian: ObsidianOptions,
    /// AI feature settings
    pub ai: AiConfig,
}

/// AI-specific configuration
#[derive(Debug, Clone)]
pub struct AiConfig {
    /// Relevance threshold for semantic filtering
    pub threshold: f32,
    /// Maximum tokens per chunk
    pub max_tokens: usize,
    /// Offline mode
    pub offline: bool,
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self {
            scraper: ScraperConfig::default(),
            crawler: CrawlerConfig::builder("https://example.com".parse().unwrap()).build(),
            http: HttpClientConfig::default(),
            obsidian: ObsidianOptions::default(),
            ai: AiConfig {
                threshold: 0.3,
                max_tokens: 512,
                offline: false,
            },
        }
    }

    /// Validate the configuration
    ///
    /// Returns an error if any configuration is invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate scraper config
        if self.scraper.scraper_concurrency == 0 {
            return Err(ConfigError::InvalidConcurrency);
        }

        // Validate crawler config
        if self.crawler.max_pages == 0 {
            return Err(ConfigError::InvalidMaxPages);
        }

        // Validate HTTP config
        if self.http.max_retries > 10 {
            return Err(ConfigError::InvalidRetries);
        }

        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration validation errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("scraper concurrency must be > 0")]
    InvalidConcurrency,
    #[error("max pages must be > 0")]
    InvalidMaxPages,
    #[error("max retries must be <= 10")]
    InvalidRetries,
}
