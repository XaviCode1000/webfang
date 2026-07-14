//! Crawler domain entities — re-export facade
//!
//! This module re-exports from the new cohesive sub-modules for backward compatibility.
//! New code should import from the specific sub-modules directly:
//!
//! - `crate::domain::site::CrawlerConfig`
//! - `crate::domain::crawl_job::DiscoveredUrl`
//! - `crate::domain::result::CrawlResult`
//! - `crate::domain::error::CrawlError`
//! - `crate::domain::pattern_matching::matches_pattern`

// Re-export for backward compatibility
pub use crate::domain::crawl_job::{ContentType, DiscoveredUrl};
pub use crate::domain::error::CrawlError;
pub use crate::domain::pattern_matching::matches_pattern;
pub use crate::domain::result::CrawlResult;
pub use crate::domain::site::{CrawlerConfig, CrawlerConfigBuilder};
