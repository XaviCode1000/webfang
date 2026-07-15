//! Obsidian integration module
//!
//! This module provides Obsidian-specific functionality:
//! - Vault auto-detection
//! - Rich metadata generation
//! - Obsidian URI protocol support

pub mod metadata;
pub mod uri;
pub mod vault_detector;

pub use metadata::{
    compute_reading_time, compute_word_count, detect_content_type, detect_language,
    ObsidianRichMetadata,
};
pub use uri::{build_obsidian_uri, extract_vault_name, open_in_obsidian, open_note};
pub use vault_detector::detect_vault;
