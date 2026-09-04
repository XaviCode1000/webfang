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
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::exporter::StateStorePort;
use crate::domain::ExportState;
use crate::error::ScraperError;
use dirs::cache_dir;
use fs2::FileExt;
use tracing::{debug, info};

/// RAII wrapper around a state-file lock. While alive it holds the lock;
/// on drop it releases the lock **and deletes the lock file** so no `.lock`
/// orphan is left behind (#761 — same pattern as `jsonl_exporter::FileLock`,
/// #582).
///
/// `#[must_use]` warns if a caller acquires the lock but lets it drop
/// immediately (a likely bug — the lock would be released before any I/O).
#[must_use]
struct StateLock {
    handle: fs::File,
    lock_path: PathBuf,
}

impl StateLock {
    /// Acquire a lock at `<path>.json.lock`. `exclusive` selects write mode
    /// (exclusive) vs read mode (shared).
    fn acquire(path: &Path, exclusive: bool) -> crate::error::Result<Self> {
        let lock_path = path.with_extension("json.lock");
        // M1 FIX: Write PID metadata to lock file for debugging
        let mut lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)
            .map_err(ScraperError::Io)?;
        let op = if exclusive {
            "exclusive_write"
        } else {
            "shared_read"
        };
        let _ = writeln!(lock_file, "pid={} op={op}", std::process::id());
        let locked = if exclusive {
            lock_file.lock_exclusive()
        } else {
            FileExt::lock_shared(&lock_file)
        };
        locked.map_err(|e| {
            ScraperError::Io(std::io::Error::other(format!(
                "failed to acquire state lock: {e}"
            )))
        })?;
        Ok(Self {
            handle: lock_file,
            lock_path,
        })
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        // Release the OS-level lock, then delete the lock file. Both
        // best-effort: a failure here must not mask the real result (#761).
        // Fully qualified syntax: avoids unstable_name_collisions with future
        // std::fs::File::unlock (rust-lang/rust#48919).
        let _ = FileExt::unlock(&self.handle);
        let _ = fs::remove_file(&self.lock_path);
    }
}

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

        // Acquire shared lock to prevent reading during concurrent write.
        // The guard releases the lock AND removes the lock file on drop (#761).
        let _lock = StateLock::acquire(&path, false)?;

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

    /// Save export state to disk
    ///
    /// # Arguments
    ///
    /// * `state` - ExportState to save
    ///
    /// # Returns
    ///
    /// * `Ok(())` - State saved successfully
    /// * `Err(ScraperError)` - If directory creation or writing fails
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::infrastructure::export::StateStore;
    /// use webfang_core::domain::ExportState;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let store = StateStore::new("example.com");
    /// let mut state = ExportState::new("example.com")?;
    /// state.mark_processed("https://example.com/page1");
    /// store.save(&state)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn save(&self, state: &ExportState) -> crate::error::Result<()> {
        let path = self.get_state_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(ScraperError::Io)?; // IO error when creating directories
        }

        // Acquire exclusive file lock to prevent concurrent writes.
        // The guard releases the lock AND removes the lock file on drop (#761).
        let _lock = StateLock::acquire(&path, true)?;

        // Serialize to JSON
        let json = serde_json::to_string_pretty(state).map_err(ScraperError::Serialization)?; // Serialization error

        // Write to file atomically
        // Following **mem-with-capacity**: Pre-allocate file
        let mut temp_path = path.clone();
        temp_path.set_extension("tmp");

        let mut file = fs::File::create(&temp_path).map_err(ScraperError::Io)?; // IO error when creating file

        file.write_all(json.as_bytes()).map_err(ScraperError::Io)?; // IO error when writing to file

        // Atomic rename
        fs::rename(&temp_path, &path).map_err(ScraperError::Io)?; // IO error when moving file

        debug!(
            "Saved state for domain {}: {} URLs processed",
            self.domain,
            state.processed_urls.len()
        );

        Ok(())
    }

    const CURRENT_VERSION: u32 = 1;

    /// Load existing state or create a new one if it doesn't exist    ///
    /// Version-aware: if the persisted file has a different `version` than
    /// `CURRENT_VERSION`, it is discarded, an `info!` is emitted, and a fresh
    /// `ExportState::new(domain)` (version `CURRENT_VERSION`) is returned.
    /// `NotFound` also yields a fresh state. Corrupted JSON (Serialization)
    /// is propagated so `filter_processed_urls` can degrade to re-scrape.
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
            Ok(state) if state.version != Self::CURRENT_VERSION => {
                info!(
                    version = state.version,
                    expected = Self::CURRENT_VERSION,
                    domain = %self.domain,
                    "discarding stale StateStore version, returning fresh state"
                );
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

/// Domain seam implementation (#1097): delegates to the inherent methods.
/// No signature changes to the concrete; `CURRENT_VERSION` stays private.
impl StateStorePort for StateStore {
    fn get_state_path(&self) -> PathBuf {
        StateStore::get_state_path(self)
    }

    fn load(&self) -> crate::error::Result<ExportState> {
        StateStore::load(self)
    }

    fn save(&self, state: &ExportState) -> crate::error::Result<()> {
        StateStore::save(self, state)
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
    fn test_save_and_load_state() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");

        // Create a store with custom cache dir
        let mut store = StateStore::new("test.com");
        store.cache_dir = cache_dir.clone();

        // Create and save state
        let mut state = ExportState::new("test.com").expect("valid domain");
        state.mark_processed("https://test.com/page1");
        state.mark_processed("https://test.com/page2");

        let save_result = store.save(&state);
        assert!(save_result.is_ok());

        // Load state
        let loaded_state = store.load();
        assert!(loaded_state.is_ok());
        let loaded_state = loaded_state.unwrap();

        assert_eq!(loaded_state.domain(), "test.com");
        assert_eq!(loaded_state.processed_urls.len(), 2);
        assert!(loaded_state.is_processed("https://test.com/page1"));
        assert!(loaded_state.is_processed("https://test.com/page2"));
    }

    /// #761: after save and load complete, no `.json.lock` orphan may
    /// remain on disk — the RAII guard removes it on drop.
    #[test]
    fn test_lockfile_removed_after_save_and_load() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");

        let mut store = StateStore::new("lockfile.test");
        store.cache_dir = cache_dir;

        let mut state = ExportState::new("lockfile.test").expect("valid domain");
        state.mark_processed("https://lockfile.test/page1");
        store.save(&state).expect("save must succeed");

        let lock_path = store.get_state_path().with_extension("json.lock");
        assert!(
            !lock_path.exists(),
            "lockfile must be removed after save, found: {}",
            lock_path.display()
        );

        let _ = store.load().expect("load must succeed");
        assert!(
            !lock_path.exists(),
            "lockfile must be removed after load, found: {}",
            lock_path.display()
        );
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

    #[test]
    fn test_atomic_save() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("webfang/state");

        let mut store = StateStore::new("atomic.com");
        store.cache_dir = cache_dir.clone();

        let state = ExportState::new("atomic.com").expect("valid domain");

        // Save should succeed
        let result = store.save(&state);
        assert!(result.is_ok());

        // Verify final file exists
        let final_path = store.get_state_path();
        assert!(final_path.exists());

        // Verify no temp file remains
        let mut temp_path = final_path.clone();
        temp_path.set_extension("tmp");
        assert!(!temp_path.exists());
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
            state.version, 1,
            "stale v0 must be discarded and replaced with fresh v1"
        );
        assert_eq!(state.domain(), "stale-zero.com");
        assert!(
            state.processed_urls.is_empty(),
            "stale processed_urls must be discarded"
        );
        assert_eq!(state.total_exported(), 0);
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
        assert_eq!(state.version, 1);
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
            state.version, 0,
            "load() must return raw version without discarding"
        );
        assert_eq!(state.processed_urls.len(), 1);
    }
}
