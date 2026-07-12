//! Domain error types
//!
//! Error types for the domain layer, organized by concern.

mod crawl_error;
mod domain_error;

pub use crawl_error::CrawlError;
pub use domain_error::DomainError;
