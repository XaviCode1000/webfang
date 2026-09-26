//! State Store for RAG Export Pipeline
//!
//! Manages persistence of export state to support resume functionality.
//! Tracks processed URLs to avoid duplicate exports.
//!
//! # Design Decisions
//!
//! - **proj-mod-by-feature**: Organized by feature (export/state_store)
//! - **err-thiserror-lib**: Uses project's error system
//! - **mem-with-capacity**: Pre-allocates when size is known
//! - **own-borrow-over-clone**: Accepts references where possible

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::entities::StateVersion;
use crate::domain::exporter::StateStorePort;
use crate::domain::ExportState;
use crate::error::ScraperError;
use dirs::cache_dir;
use tracing::{debug, info, warn};

/// StateStore manages persistence of export state for a specific domain
///
/// Following **proj-mod-by-feature**: Export state management is a feature
/// Following **own-borrow-over-clone**: Accepts &str for domain
#[derive(Debug)]
pub struct StateStore {
    /// Domain this state store belongs to (e.g., "example.com")
    domain: String,
    /// Base cache directory path
    cache_dir: PathBuf,
}

impl StateStore {
    /// Create a new StateStore for a specific domain
    ///
    /// # Arguments
    ///
    /// * `domain` - Domain name for this state store
    ///
    /// # Returns
    ///
    /// A new StateStore instance
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// ```
    #[must_use]
    pub fn new(domain: &str) -> Self {
        // Get cache directory using dirs crate
        // Following **mem-with-capacity**: Pre-allocate path buffer
        let mut cache_dir = cache_dir().unwrap_or_else(|| PathBuf::from(".cache"));
        cache_dir.push("webfang");
        cache_dir.push("state");

        Self {
            domain: domain.to_string(),
            cache_dir,
        }
    }

    /// Set custom cache directory
    ///
    /// # Arguments
    ///
    /// * `cache_dir` - Custom cache directory path
    pub fn set_cache_dir(&mut self, cache_dir: PathBuf) {
        self.cache_dir = cache_dir;
    }

    /// Get the full path to the state file.
    ///
    /// The domain is confined to a single safe component at join time
    /// (#1125), so a hostile domain can never escape `cache_dir`.
    ///
    /// # Returns
    ///
    /// PathBuf containing the full path to the state JSON file
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// let path = store.get_state_path();
    /// ```
    #[must_use]
    pub fn get_state_path(&self) -> PathBuf {
        let mut path = self.cache_dir.clone();
        path.push(format!(
            "{}.json",
            confine_filename_component(&self.domain, "unknown")
        ));
        path
    }

    /// Load existing export state from disk
    ///
    /// # Returns
    ///
    /// * `Ok(ExportState)` - Loaded state
    /// * `Err(ScraperError)` - If file doesn't exist or parsing fails
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// match store.load() {
    ///     Ok(state) => println!("Loaded {} processed URLs", state.processed_urls.len()),
    ///     Err(e) => println!("No existing state: {}", e),
    /// }
    /// ```
    pub fn load(&self) -> crate::error::Result<ExportState> {
        let path = self.get_state_path();

        // Check if file exists to provide more informative error messages
        if !path.exists() {
            debug!("State file does not exist: {}", path.display());
            // Create an IO error with NotFound kind to make load_or_default work correctly
            let err = std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("State file not found: {}", path.display()),
            );
            return Err(ScraperError::Io(err));
        }

        // No lock on the read. The previous shared `StateLock` used the same
        // unlink-on-drop pattern removed from `record_store::StoreLock` in #1230:
        // `flock(2)` guards an inode, so deleting the sentinel makes the lock
        // unenforceable anyway. An unlocked read is still correct because every
        // writer replaces the file with `rename(2)` — a reader sees either the
        // old or the new bytes, never a torn file.

        // Read and parse JSON file
        let content = fs::read_to_string(&path).map_err(ScraperError::Io)?; // IO error when reading file

        let state: ExportState =
            serde_json::from_str(&content).map_err(ScraperError::Serialization)?; // Serialization error when parsing JSON

        debug!(
            "Loaded state for domain {}: {} URLs processed",
            self.domain,
            state.processed_urls.len()
        );

        Ok(state)
    }

    /// Load existing state or create a new one if it doesn't exist    ///
    /// Version-aware: if the persisted file has a different `version` than
    /// [`StateVersion::CURRENT`], it is discarded, a `warn!` is emitted (visible
    /// at the default level, #1587), the pre-migration file is preserved as a
    /// `.bak` sibling, and a fresh `ExportState::new(domain)` (version
    /// `CURRENT`) is returned.
    /// `NotFound` also yields a fresh state. Corrupted JSON (Serialization)
    /// is propagated so `filter_processed_urls` can degrade to re-scrape.
    /// Unknown (future) versions never reach this comparison: they are rejected
    /// at the `ExportState` serde boundary (#1162) and propagate as
    /// `Serialization` errors through the same degrade-to-rescrape path.
    ///
    /// # Returns
    ///
    /// * `Ok(ExportState)` - Loaded or newly created state
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// let state = store.load_or_default().unwrap();
    /// ```
    pub fn load_or_default(&self) -> crate::error::Result<ExportState> {
        match self.load() {
            Ok(state) if state.version != StateVersion::CURRENT => {
                let path = self.get_state_path();
                warn!(
                    version = state.version.get(),
                    expected = StateVersion::CURRENT.get(),
                    domain = %self.domain,
                    path = %path.display(),
                    "discarding stale StateStore version, returning fresh state; pre-migration file preserved as .bak"
                );
                preserve_pre_migration_backup(&path);
                ExportState::new(&self.domain)
            },
            Ok(state) => {
                info!("Loaded existing state for domain: {}", self.domain);
                Ok(state)
            },
            Err(ScraperError::Io(io_err)) => {
                // If it's an IO error, check if it's a "file not found" error
                // For "file not found", return a new state; otherwise propagate the error
                if io_err.kind() == std::io::ErrorKind::NotFound {
                    info!("Creating new state for domain: {}", self.domain);
                    ExportState::new(&self.domain)
                } else {
                    // Propagate other IO errors (permissions, disk full, etc.)
                    Err(ScraperError::Io(io_err))
                }
            },
            Err(e) => {
                // If it's another kind of error (like serialization), return it
                Err(e)
            },
        }
    }
}

/// Preserve the pre-migration state file as a `.bak` sibling (#1587).
///
/// The stale-version discard returns a fresh state while the old file is
/// still on disk, so the next save would silently overwrite work the new
/// schema refused to read. Copying first keeps the original bytes available
/// for inspection. Best-effort: a backup failure is logged, never fatal —
/// losing the backup must not fail a run that already decided to start fresh.
/// An existing backup is kept as-is so the FIRST (pre-migration) bytes win
/// over later fresh-version writes.
fn preserve_pre_migration_backup(path: &Path) {
    let backup = path.with_extension("json.bak");
    if backup.exists() {
        return;
    }
    if let Err(e) = fs::copy(path, &backup) {
        warn!(
            path = %path.display(),
            backup = %backup.display(),
            error = %e,
            "pre-migration state backup failed; continuing with fresh state"
        );
    }
}

/// Domain seam implementation (#1097): delegates to the inherent methods.
/// No signature changes to the concrete; the version vocabulary
/// ([`StateVersion::CURRENT`]) is owned by the domain.
impl StateStorePort for StateStore {
    fn get_state_path(&self) -> PathBuf {
        StateStore::get_state_path(self)
    }

    fn load(&self) -> crate::error::Result<ExportState> {
        StateStore::load(self)
    }

    fn load_or_default(&self) -> crate::error::Result<ExportState> {
        StateStore::load_or_default(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_state_store_creation() {
        let store = StateStore::new("example.com");
        assert_eq!(store.domain, "example.com");
        assert!(store.get_state_path().ends_with("example.com.json"));
    }

    #[test]
    fn test_state_path_generation() {
        let store = StateStore::new("test.domain");
        let path = store.get_state_path();

        // Verify path structure
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("webfang/state/test.domain.json"));
    }

    #[test]
    fn hostile_domain_is_confined_inside_cache_dir() {
        // Issue #1125: a hostile domain must collapse to a single safe
        // component; the state file's parent is always `cache_dir`.
        for hostile in ["../escape", "..\\escape", "..", "sub/escape"] {
            let mut store = StateStore::new(hostile);
            store.set_cache_dir(PathBuf::from("/tmp/cache"));
            let path = store.get_state_path();
            assert_eq!(
                path.parent(),
                Some(PathBuf::from("/tmp/cache").as_path()),
                "domain {hostile:?} escaped: {}",
                path.display()
            );
            assert!(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.contains('/') && !n.contains('\\') && n.ends_with(".json")),
                "domain {hostile:?} produced unsafe file name: {}",
                path.display()
            );
        }
    }

    #[test]
    fn test_load_nonexistent_state() {
        let store = StateStore::new("nonexistent");
        let result = store.load();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_load_or_default_existing() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        fs::create_dir_all(&cache_dir).unwrap();

        // Create a test state file
        let state_path = cache_dir.join("existing.com.json");
        let mut file = File::create(&state_path).unwrap();
        writeln!(
            file,
            r#"{{
            "domain": "existing.com",
            "processed_urls": ["https://existing.com/page1"],
            "last_export": null,
            "total_exported": 1
        }}"#
        )
        .unwrap();

        let mut store = StateStore::new("existing.com");
        store.cache_dir = cache_dir;

        let state = store.load_or_default().unwrap();
        assert_eq!(state.domain(), "existing.com");
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_load_or_default_new() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().to_path_buf();

        let mut store = StateStore::new("new.com");
        store.cache_dir = cache_dir;

        let state = store.load_or_default().unwrap();
        assert_eq!(state.domain(), "new.com");
        assert_eq!(state.processed_urls.len(), 0);
    }

    // --- Sprint 0 Gate 0: version gate RED tests ---

    #[test]
    fn test_load_or_default_discards_stale_version_zero() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("stale-zero.com.json");
        let mut file = File::create(&state_path).unwrap();
        writeln!(
            file,
            r#"{{"domain":"stale-zero.com","processed_urls":["https://stale-zero.com/a"],"last_export":null,"total_exported":1,"version":0}}"#
        )
        .unwrap();
        let mut store = StateStore::new("stale-zero.com");
        store.cache_dir = cache_dir;
        let state = store.load_or_default().unwrap();
        assert_eq!(
            state.version,
            StateVersion::CURRENT,
            "stale v0 must be discarded and replaced with fresh v1"
        );
        assert_eq!(state.domain(), "stale-zero.com");
        assert!(
            state.processed_urls.is_empty(),
            "stale processed_urls must be discarded"
        );
        assert_eq!(state.total_exported(), 0);
    }

    /// #1587: discarding a stale version must preserve the pre-migration
    /// file as a `.bak` sibling so the next save cannot silently overwrite
    /// work the new schema refused to read.
    #[test]
    fn test_load_or_default_stale_version_preserves_bak() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("stale-bak.com.json");
        let original = r#"{"domain":"stale-bak.com","processed_urls":["https://stale-bak.com/a"],"last_export":null,"total_exported":1,"version":0}"#;
        std::fs::write(&state_path, original).unwrap();
        let mut store = StateStore::new("stale-bak.com");
        store.cache_dir = cache_dir;

        let state = store.load_or_default().unwrap();
        assert_eq!(state.version, StateVersion::CURRENT);
        assert!(state.processed_urls.is_empty());

        let backup = state_path.with_extension("json.bak");
        assert!(
            backup.exists(),
            "pre-migration file must be preserved as .bak"
        );
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            original,
            "backup must carry the exact pre-migration bytes"
        );
    }

    #[test]
    fn test_load_or_default_keeps_current_version_one() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("current-one.com.json");
        let mut file = File::create(&state_path).unwrap();
        writeln!(
            file,
            r#"{{"domain":"current-one.com","processed_urls":["https://current-one.com/a"],"last_export":null,"total_exported":1,"version":1}}"#
        )
        .unwrap();
        let mut store = StateStore::new("current-one.com");
        store.cache_dir = cache_dir;
        let state = store.load_or_default().unwrap();
        assert_eq!(state.version, StateVersion::CURRENT);
        assert_eq!(state.processed_urls.len(), 1);
        assert_eq!(state.processed_urls[0], "https://current-one.com/a");
    }

    #[test]
    fn test_load_or_default_corrupt_propagates_error() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("corrupt.com.json");
        std::fs::write(&state_path, "not json at all {{{").unwrap();
        let mut store = StateStore::new("corrupt.com");
        store.cache_dir = cache_dir;
        let err = store.load_or_default().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Serialization") || msg.contains("expected") || msg.contains("parse"),
            "corrupt JSON must propagate Serialization error, got: {msg}"
        );
    }

    /// #1162: a future version never reaches the stale-discard comparison —
    /// the `ExportState` boundary rejects it, and `load_or_default` propagates
    /// the error so the caller degrades to re-scrape (same path as corrupt).
    #[test]
    fn test_load_or_default_unknown_version_propagates_error() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("future.com.json");
        std::fs::write(
            &state_path,
            r#"{"domain":"future.com","processed_urls":[],"last_export":null,"total_exported":0,"version":999}"#,
        )
        .unwrap();
        let mut store = StateStore::new("future.com");
        store.cache_dir = cache_dir;
        let err = store.load_or_default().unwrap_err();
        assert!(
            err.to_string().contains("no soportada"),
            "future version must propagate the Spanish boundary error, got: {err}"
        );
    }

    #[test]
    fn test_load_does_not_discard_stale_version() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let state_path = cache_dir.join("load-raw.com.json");
        let mut file = File::create(&state_path).unwrap();
        writeln!(
            file,
            r#"{{"domain":"load-raw.com","processed_urls":["https://load-raw.com/a"],"last_export":null,"total_exported":1,"version":0}}"#
        )
        .unwrap();
        let mut store = StateStore::new("load-raw.com");
        store.cache_dir = cache_dir;
        let state = store.load().unwrap();
        assert_eq!(
            state.version,
            StateVersion::LEGACY,
            "load() must return raw version without discarding"
        );
        assert_eq!(state.processed_urls.len(), 1);
    }
}
