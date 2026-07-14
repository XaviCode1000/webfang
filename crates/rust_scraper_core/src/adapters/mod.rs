//! Adapters — External integrations
//!
//! This layer contains adapters for external concerns:
//! - Asset downloading (images, documents)
//! - URL extraction from HTML
//! - MIME type detection
//!
//! TUI adapter lives in the separate `rust_scraper_tui` crate.

pub mod detector;
pub mod downloader;
pub mod extractor;
pub mod url_path;

pub use detector::{get_extension, AssetType};
