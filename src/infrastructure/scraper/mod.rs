//! Scraping implementations
//!
//! Contains the actual scraping logic:
//! - Readability algorithm wrapper
//! - Fallback text extraction
//! - Asset downloading (feature-gated)

#[cfg(any(feature = "images", feature = "documents"))]
pub mod asset_download;
pub mod fallback;
pub mod readability;
