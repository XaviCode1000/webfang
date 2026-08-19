//! Scraping implementations
//!
//! Contains the actual scraping logic:
//! - Readability algorithm wrapper
//! - Fallback text extraction
//! - DOM pre-pruning

pub mod author_extractor;
pub mod dom_inspector;
pub mod dom_pruner;
pub mod fallback;
pub mod readability;
