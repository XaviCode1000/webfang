//! Adapters — External integrations
//!
//! This layer contains adapters for external concerns:
//! - Asset downloading (images, documents)
//! - URL extraction from HTML
//! - MIME type detection

pub mod detector;
pub mod downloader;
pub mod extractor;
pub mod url_path;

pub use detector::{get_extension, AssetType};
