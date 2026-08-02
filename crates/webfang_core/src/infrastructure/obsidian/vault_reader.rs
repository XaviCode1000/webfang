//! Obsidian vault note reader.
//!
//! Reads all Markdown notes from a detected vault directory:
//! - Filters by `.md` extension (ignores PDFs, images, attachments)
//! - Skips hidden directories (`.obsidian/`, `.trash/`, `.git/`, etc.)
//! - Computes SHA-256 content hash for staleness detection
//! - Extracts `mtime` as Unix epoch seconds
//! - Follows symlinks (Obsidian uses them for templates)
//! - Skips non-UTF-8 files with a tracing warning
//!
//! # Integration
//!
//! [`read_vault_notes`] receives the vault path from [`super::vault_detector`]
//! or from the MCP `vault_path` parameter. It does NOT duplicate detection
//! logic — the caller is responsible for providing a valid vault root.
//!
//! The output [`VaultNote`] fields align with
//! [`crate::domain::IndexedNoteMeta`] so the application layer
//! ([`crate::application::vault_search`]) can compare hashes directly
//! for staleness detection.

use std::path::Path;

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::ScraperError;

/// A Markdown note read from an Obsidian vault.
///
/// Fields mirror [`crate::domain::IndexedNoteMeta`] semantics so the
/// application layer can compare `content_hash` for staleness without
/// conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct VaultNote {
    /// Path relative to the vault root (e.g. `notes/rust.md`).
    pub path: String,
    /// Full UTF-8 content of the note.
    pub content: String,
    /// Last modification time (Unix epoch seconds).
    pub mtime_secs: i64,
    /// SHA-256 content hash (hex, lowercase).
    pub content_hash: String,
}

/// Read all Markdown notes from an Obsidian vault.
///
/// Walks the vault directory recursively, collecting every `.md` file
/// while skipping hidden directories (names starting with `.`).
/// Symlinks are followed (Obsidian uses them for template folders).
///
/// # Arguments
/// - `vault_path` — Root directory of the vault (should contain `.obsidian/`)
///
/// # Returns
/// A vector of [`VaultNote`] for every `.md` file found.
/// An empty vault returns `Ok(vec![])`.
///
/// # Errors
/// Returns [`ScraperError::Io`] if the vault directory cannot be read
/// or filesystem metadata is unavailable.
pub fn read_vault_notes(vault_path: &Path) -> Result<Vec<VaultNote>, ScraperError> {
    let mut notes = Vec::new();

    let walker = WalkDir::new(vault_path)
        .follow_links(true)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            // Always include the vault root (depth 0 is excluded by min_depth,
            // but filter_entry still sees it for pruning decisions).
            if entry.depth() == 0 {
                return true;
            }
            // Prune hidden directories entirely (`.obsidian/`, `.trash/`, `.git/`, …)
            if entry.file_type().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    return !name.starts_with('.');
                }
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // Symlink loops or permission errors on individual entries
                // should not abort the entire vault read.
                tracing::warn!("Skipping unreadable vault entry: {e}");
                continue;
            },
        };

        // Only process regular files with `.md` extension.
        if !entry.file_type().is_file() {
            continue;
        }
        let is_markdown = entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_markdown {
            continue;
        }

        let path = entry.path();

        // Read raw bytes, then validate UTF-8.
        let bytes = std::fs::read(path)?;
        let content = match String::from_utf8(bytes) {
            Ok(c) => c,
            Err(_) => {
                tracing::warn!("Skipping non-UTF-8 note: {}", path.display());
                continue;
            },
        };

        // mtime via filesystem metadata → Unix epoch seconds.
        let metadata = std::fs::metadata(path)?;
        let mtime_secs = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);

        let content_hash = sha256_hex(content.as_bytes());

        // Store path relative to the vault root for portability.
        let relative = path
            .strip_prefix(vault_path)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();

        notes.push(VaultNote {
            path: relative,
            content,
            mtime_secs,
            content_hash,
        });
    }

    tracing::debug!(
        vault = %vault_path.display(),
        notes = notes.len(),
        "Vault read complete"
    );

    Ok(notes)
}

/// SHA-256 hex digest of the bytes (lowercase, dependency-free hex encoding).
///
/// Same pattern as `elastic_ingestion::sha256_hex` — kept module-local
/// to avoid cross-layer coupling (infra → application).
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    let mut out = String::with_capacity(hash.len() * 2);
    for b in hash {
        use std::fmt::Write;
        // write! into a String is infallible.
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a synthetic vault with `.obsidian/` marker.
    fn create_vault(tmp: &Path) {
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
    }

    #[test]
    fn reads_all_md_notes() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::create_dir_all(tmp.path().join("notes")).unwrap();
        fs::write(
            tmp.path().join("notes").join("rust.md"),
            "# Rust\nGreat language",
        )
        .unwrap();
        fs::write(tmp.path().join("notes").join("go.md"), "# Go\nAlso great").unwrap();
        fs::write(tmp.path().join("readme.md"), "# Readme").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 3);

        let paths: Vec<&str> = notes.iter().map(|n| n.path.as_str()).collect();
        assert!(paths.contains(&"notes/rust.md"));
        assert!(paths.contains(&"notes/go.md"));
        assert!(paths.contains(&"readme.md"));
    }

    #[test]
    fn ignores_hidden_directories() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::create_dir_all(tmp.path().join(".trash")).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(
            tmp.path().join(".obsidian").join("config.md"),
            "should be ignored",
        )
        .unwrap();
        fs::write(
            tmp.path().join(".trash").join("deleted.md"),
            "should be ignored",
        )
        .unwrap();
        fs::write(tmp.path().join(".git").join("hook.md"), "should be ignored").unwrap();
        fs::write(tmp.path().join("visible.md"), "# Visible").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].path, "visible.md");
    }

    #[test]
    fn ignores_non_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::write(tmp.path().join("note.md"), "# Note").unwrap();
        fs::write(tmp.path().join("image.png"), b"\x89PNG").unwrap();
        fs::write(tmp.path().join("doc.pdf"), b"%PDF").unwrap();
        fs::create_dir_all(tmp.path().join("attachments")).unwrap();
        fs::write(tmp.path().join("attachments").join("file.txt"), "text").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].path, "note.md");
    }

    #[test]
    fn empty_vault_returns_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert!(notes.is_empty());
    }

    #[test]
    fn content_hash_is_correct_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        // Known SHA-256 vector: SHA-256("hello") =
        // 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        fs::write(tmp.path().join("hello.md"), "hello").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].content_hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn content_hash_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        fs::write(tmp.path().join("empty.md"), "").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(
            notes[0].content_hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn mtime_is_positive_epoch_seconds() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::write(tmp.path().join("note.md"), "# Note").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        // Any file created "now" should have mtime > 2020-01-01 epoch.
        assert!(notes[0].mtime_secs > 1_577_836_800);
    }

    #[test]
    fn skips_non_utf8_files() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::write(tmp.path().join("valid.md"), "# Valid").unwrap();
        // Invalid UTF-8 sequence
        fs::write(tmp.path().join("binary.md"), &[0xFF, 0xFE, 0x00, 0x01]).unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].path, "valid.md");
    }

    #[test]
    fn follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());

        // Create a real directory outside the vault with a note
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("linked.md"), "# Linked note").unwrap();

        // Symlink it into the vault
        #[cfg(unix)]
        std::os::unix::fs::symlink(external.path(), tmp.path().join("templates")).unwrap();

        #[cfg(unix)]
        {
            let notes = read_vault_notes(tmp.path()).unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].path, "templates/linked.md");
            assert_eq!(notes[0].content, "# Linked note");
        }
    }

    #[test]
    fn nested_directories() {
        let tmp = tempfile::tempdir().unwrap();
        create_vault(tmp.path());
        fs::create_dir_all(tmp.path().join("a").join("b").join("c")).unwrap();
        fs::write(
            tmp.path().join("a").join("b").join("c").join("deep.md"),
            "deep",
        )
        .unwrap();
        fs::write(tmp.path().join("top.md"), "top").unwrap();

        let notes = read_vault_notes(tmp.path()).unwrap();
        assert_eq!(notes.len(), 2);

        let paths: Vec<&str> = notes.iter().map(|n| n.path.as_str()).collect();
        assert!(paths.contains(&"a/b/c/deep.md"));
        assert!(paths.contains(&"top.md"));
    }

    #[test]
    fn vault_note_debug_and_clone() {
        let note = VaultNote {
            path: "test.md".to_owned(),
            content: "# Test".to_owned(),
            mtime_secs: 1_700_000_000,
            content_hash: "abc123".to_owned(),
        };
        let cloned = note.clone();
        assert_eq!(note, cloned);
        let debug = format!("{note:?}");
        assert!(debug.contains("test.md"));
    }
}
