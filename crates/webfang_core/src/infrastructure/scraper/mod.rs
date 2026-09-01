//! Scraping implementations
//!
//! Contains the actual scraping logic:
//! - Readability algorithm wrapper
//! - Fallback text extraction
//!
//! DOM pre-pruning and author extraction moved to `domain::scraper_port`
//! (ADR-0012 sub-slice 3.D).

pub mod dom_inspector;
pub mod fallback;
pub mod readability;
