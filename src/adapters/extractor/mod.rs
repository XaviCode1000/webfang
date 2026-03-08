//! URL extraction adapter
//!
//! Extracts image and document URLs from HTML content.

pub use crate::adapters::detector::get_extension;

// Re-export extractor module (it's in the root, not in adapters)
pub use crate::extractor::{extract_all_assets, extract_documents, extract_images, AssetUrl};
