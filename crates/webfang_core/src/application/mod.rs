//! Application layer — Use cases and orchestration
//!
//! This layer contains the business logic that orchestrates the domain objects
//! using infrastructure services. It depends on both domain and infrastructure.

pub mod asset_download;
pub mod batch;
pub mod container;
pub mod crawl_options;
pub mod crawl_result_repository;
pub mod crawler;
pub mod crawler_service;
pub mod deduplicator;
pub mod diagnostic;
pub mod elastic_ingestion;
pub mod error_mapping;
pub mod export_factory;
pub mod export_utils;
pub mod extraction;
pub mod http_client;
pub mod pipeline;
pub mod progress_observer;
pub mod rate_limiter;
pub mod scraper_service;
pub mod spa_detection;
/// Resolve extracted page titles to guaranteed non-empty strings.
pub mod title_resolver;
pub mod url_filter;
pub mod vault_search;

#[cfg(feature = "adaptive-selectors")]
pub mod adaptive_engine;

pub use batch::{
    BatchJob, BatchManager, BatchManagerSummary, BatchProcessor, BatchProgress, BatchResult,
};
pub use crawler::bounded_sink::{BoundedFileSink, BoundedSinkError, CapturedPageReader};
pub use crawler::collector::{ResultsAdapter, ResultsCollector};
pub use crawler::content_sink::{CapturedPage, CrawlContentSink, InMemoryContentSink};
pub use crawler::engine::EngineOptions;
pub use crawler::{
    crawl_site, crawl_site_capturing, crawl_site_with_options, crawl_with_sitemap,
    discover_urls_for_tui, extract_content, scrape_single_url_for_tui,
};
pub use deduplicator::UrlDeduplicator;
pub use http_client::create_http_client;
pub use http_client::create_http_client_with_config;
pub use http_client::HttpClientPort;
pub use rate_limiter::{RateLimiterConfig, SharedRateLimiter};
pub use scraper_service::{
    detect_spa_content, scrape_multiple_with_limit, scrape_with_config, scrape_with_readability,
    SpaDetectionResult,
};
pub use title_resolver::resolve_title;
pub use url_filter::{extract_domain, is_allowed, is_excluded, is_internal_link, matches_pattern};

#[deprecated(
    since = "0.5.0",
    note = "Tipos de progreso migrados a webfang_core::domain::entities::progress. Este shim será removido."
)]
/// Re-export de tipos de progreso desde el dominio para retrocompatibilidad.
///
/// La ubicación canónica es ahora `webfang_core::domain::entities::progress`.
pub mod progress_types {
    pub use crate::domain::entities::progress::*;
}
