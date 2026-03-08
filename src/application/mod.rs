//! Application layer — Use cases and orchestration
//!
//! This layer contains the business logic that orchestrates the domain objects
//! using infrastructure services. It depends on both domain and infrastructure.

pub mod http_client;
pub mod scraper_service;

pub use http_client::create_http_client;
pub use scraper_service::{
    scrape_multiple_with_limit, scrape_with_config, scrape_with_readability,
};
