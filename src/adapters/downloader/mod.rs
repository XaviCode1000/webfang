//! Asset downloading adapter
//!
//! Re-exports the infrastructure asset download functionality.
//! This module exists for architectural consistency.

#[cfg(any(feature = "images", feature = "documents"))]
pub use crate::infrastructure::scraper::asset_download::download_all;
