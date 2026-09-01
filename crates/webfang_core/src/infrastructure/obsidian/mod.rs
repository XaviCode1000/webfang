//! Obsidian integration module
//!
//! This module provides Obsidian-specific functionality:
//! - Vault auto-detection
//! - Vault note reading
//! - Rich metadata generation
//! - Obsidian URI protocol support

pub mod metadata;
pub mod uri;
pub mod vault_detector;
pub mod vault_reader;

pub use metadata::{
    compute_reading_time, compute_word_count, detect_content_type, detect_language,
    ObsidianRichMetadata,
};
pub use uri::{
    build_obsidian_uri, extract_vault_name, open_in_obsidian, open_note, DispatchStatus,
};
pub use vault_detector::{
    detect_vault, detect_vault_hermetic, detect_vault_with_root, is_valid_vault,
};
pub use vault_reader::{read_vault_notes, VaultFsReader, VaultNote};
