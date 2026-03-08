//! Adapters — External integrations (feature-gated)
//!
//! This layer contains adapters for external concerns:
//! - Asset downloading (images, documents)
//! - URL extraction from HTML
//! - MIME type detection
//!
//! These are feature-gated to keep the core library lightweight.

pub mod detector;
pub mod downloader;
pub mod extractor;

pub use detector::{get_extension, AssetType};
