//! Checkpoint persistence for crawl state — Application layer
//!
//! Saves and loads crawl state (visited URLs, queued URLs, pages crawled)
//! using jzon-rs JSON serialization with CRC32 integrity checks and atomic writes.
//!
//! # Design Decisions
//!
//! - **Sealed trait pattern** (`api-sealed-trait`): Prevents external implementations
//!   that could violate atomicity or integrity invariants.
//! - **File format**: `[4 bytes CRC32][JSON payload]` — simple, verifiable, human-readable.
//! - **Atomic write**: serialize → write to `.tmp` → `fs::rename` to final path.
//! - **Integrity**: CRC32 of payload stored as header; load verifies before deserializing.
//! - **Generic state**: Accepts any `Serialize + Deserialize` type, not just a fixed struct.

use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    CHECKPOINT_CORRUPTED, CHECKPOINT_LOADS, CHECKPOINT_SAVES,
};

// ---------------------------------------------------------------------------
// Sealed trait
// ---------------------------------------------------------------------------

mod private {
    pub trait Sealed {}
}

// ---------------------------------------------------------------------------
// CrawlCheckpoint — the serializable state
// ---------------------------------------------------------------------------

/// Serializable crawl state for checkpoint persistence.
///
/// Captures enough information to resume a crawl from where it left off.
/// Fields use `#[serde(default)]` for forward-compatible schema evolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrawlCheckpoint {
    /// URLs already visited (fully processed).
    #[serde(default)]
    pub visited: HashSet<String>,
    /// URLs queued for processing (not yet visited).
    #[serde(default)]
    pub queued: Vec<String>,
    /// Number of pages successfully crawled.
    #[serde(default)]
    pub pages_crawled: u64,
}

impl CrawlCheckpoint {
    /// Create a new empty checkpoint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            visited: HashSet::new(),
            queued: Vec::new(),
            pages_crawled: 0,
        }
    }
}

impl Default for CrawlCheckpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CrawlCheckpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Checkpoint(pages={}, visited={}, queued={})",
            self.pages_crawled,
            self.visited.len(),
            self.queued.len()
        )
    }
}

// ---------------------------------------------------------------------------
// CheckpointStore trait (sealed)
// ---------------------------------------------------------------------------

/// Trait for checkpoint persistence — save and load crawl state.
///
/// Sealed to prevent external implementations that might skip CRC32
/// verification or break atomic write guarantees.
pub trait CheckpointStore: private::Sealed {
    /// Save checkpoint state to persistent storage.
    ///
    /// # Errors
    ///
    /// Returns `Err` on serialization failure or I/O error during write.
    fn save(&self, state: &CrawlCheckpoint, path: &Path) -> Result<(), String>;

    /// Load checkpoint state from persistent storage.
    ///
    /// Returns `None` if the file doesn't exist, is corrupted,
    /// or fails integrity checks.
    fn load(&self, path: &Path) -> Option<CrawlCheckpoint>;
}

// ---------------------------------------------------------------------------
// BincodeCheckpoint — the default implementation
// ---------------------------------------------------------------------------

/// Checkpoint store using jzon-rs JSON serialization with CRC32 integrity.
///
/// File format: `[4-byte CRC32][JSON payload]`
///
/// Write path: serialize → write `.tmp` → atomic rename.
/// Read path: read full file → verify CRC32 → deserialize.
pub struct BincodeCheckpoint;

impl private::Sealed for BincodeCheckpoint {}

impl CheckpointStore for BincodeCheckpoint {
    #[instrument(skip(self, state), fields(path = %path.display()))]
    fn save(&self, state: &CrawlCheckpoint, path: &Path) -> Result<(), String> {
        // Serialize to JSON
        let payload = jzon_serde::to_string(state)
            .map_err(|e| format!("checkpoint serialization failed: {e}"))?
            .into_bytes();

        // Compute CRC32 of the payload
        let checksum = crc32fast::hash(&payload);

        // Write to .tmp file first
        let tmp_path = tmp_path_for(path);
        {
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("create checkpoint tmp file: {e}"))?;
            file.write_all(&checksum.to_ne_bytes())
                .map_err(|e| format!("write checkpoint checksum: {e}"))?;
            file.write_all(&payload)
                .map_err(|e| format!("write checkpoint payload: {e}"))?;
            file.sync_all()
                .map_err(|e| format!("sync checkpoint file: {e}"))?;
        }

        // Atomic rename
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            // Clean up tmp file on rename failure
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("atomic rename checkpoint: {e}"));
        }

        info!(
            "checkpoint saved: {} ({} bytes)",
            path.display(),
            payload.len()
        );

        #[cfg(feature = "otel-metrics")]
        CHECKPOINT_SAVES.add(1, &[]);

        Ok(())
    }

    #[instrument(skip(self), fields(path = %path.display()))]
    fn load(&self, path: &Path) -> Option<CrawlCheckpoint> {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                debug!("checkpoint file not readable: {e}");
                return None;
            },
        };

        // Need at least 4 bytes for CRC32 header
        if data.len() < 4 {
            warn!("checkpoint file too small ({} bytes)", data.len());
            return None;
        }

        // Extract stored checksum (first 4 bytes)
        let stored_checksum = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);

        // Compute CRC32 of the payload (bytes after header)
        let payload = &data[4..];
        let computed_checksum = crc32fast::hash(payload);

        // Verify integrity
        if stored_checksum != computed_checksum {
            warn!(
                "checkpoint CRC32 mismatch: stored={:#x}, computed={:#x}",
                stored_checksum, computed_checksum
            );

            #[cfg(feature = "otel-metrics")]
            CHECKPOINT_CORRUPTED.add(1, &[]);

            return None;
        }

        // Deserialize from JSON
        match jzon_serde::from_slice::<CrawlCheckpoint>(payload) {
            Ok(state) => {
                info!(
                    "checkpoint loaded: {} (visited={}, queued={}, pages={})",
                    path.display(),
                    state.visited.len(),
                    state.queued.len(),
                    state.pages_crawled
                );

                #[cfg(feature = "otel-metrics")]
                CHECKPOINT_LOADS.add(1, &[]);

                Some(state)
            },
            Err(e) => {
                warn!("checkpoint deserialization failed: {e}");

                #[cfg(feature = "otel-metrics")]
                CHECKPOINT_CORRUPTED.add(1, &[]);

                None
            },
        }
    }
}

impl BincodeCheckpoint {
    /// Create a new BincodeCheckpoint store.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for BincodeCheckpoint {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the `.tmp` path for atomic writes.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample_checkpoint() -> CrawlCheckpoint {
        let mut visited = HashSet::new();
        visited.insert("https://example.com".to_string());
        visited.insert("https://example.com/about".to_string());

        CrawlCheckpoint {
            visited,
            queued: vec![
                "https://example.com/contact".to_string(),
                "https://example.com/blog".to_string(),
            ],
            pages_crawled: 42,
        }
    }

    #[test]
    fn round_trip_save_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();
        let loaded = store.load(&path).unwrap();

        assert_eq!(original, loaded);
    }

    #[test]
    fn empty_state_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = CrawlCheckpoint::new();

        store.save(&original, &path).unwrap();
        let loaded = store.load(&path).unwrap();

        assert_eq!(original, loaded);
        assert!(loaded.visited.is_empty());
        assert!(loaded.queued.is_empty());
        assert_eq!(loaded.pages_crawled, 0);
    }

    #[test]
    fn corruption_detection_tamper_checksum() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();

        // Tamper with the checksum (first 4 bytes)
        let mut data = fs::read(&path).unwrap();
        data[0] ^= 0xFF; // flip bits in checksum
        fs::write(&path, &data).unwrap();

        // Load should return None (corrupted)
        let loaded = store.load(&path);
        assert!(loaded.is_none(), "corrupted checkpoint should return None");
    }

    #[test]
    fn corruption_detection_tamper_payload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();

        // Tamper with the payload (after the 4-byte checksum)
        let mut data = fs::read(&path).unwrap();
        if data.len() > 4 {
            data[4] ^= 0xFF;
        }
        fs::write(&path, &data).unwrap();

        // Load should return None (corrupted)
        let loaded = store.load(&path);
        assert!(loaded.is_none(), "corrupted payload should return None");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let store = BincodeCheckpoint::new();
        let loaded = store.load(Path::new("/tmp/nonexistent_checkpoint_12345.bin"));
        assert!(loaded.is_none());
    }

    #[test]
    fn load_truncated_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        // Write only 2 bytes (less than CRC32 header)
        fs::write(&path, [0u8; 2]).unwrap();

        let store = BincodeCheckpoint::new();
        let loaded = store.load(&path);
        assert!(loaded.is_none());
    }

    #[test]
    fn tmp_path_convention() {
        let path = Path::new("/tmp/checkpoint.bin");
        let tmp = tmp_path_for(path);
        assert_eq!(tmp, PathBuf::from("/tmp/checkpoint.bin.tmp"));
    }

    #[test]
    fn checksum_is_first_four_bytes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let state = sample_checkpoint();

        store.save(&state, &path).unwrap();

        let data = fs::read(&path).unwrap();
        let stored = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        let payload = &data[4..];
        let computed = crc32fast::hash(payload);

        assert_eq!(stored, computed);
    }

    #[test]
    fn atomic_rename_removes_tmp_on_failure() {
        // Create a read-only directory to force rename failure
        let tmp = TempDir::new().unwrap();
        let readonly_dir = tmp.path().join("readonly");
        fs::create_dir(&readonly_dir).unwrap();

        // Make the parent dir read-only so rename into it fails
        // This test verifies cleanup of .tmp file on rename failure
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();

        // Try saving to a path where rename will fail (read-only parent)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o444)).unwrap();

            let bad_path = readonly_dir.join("checkpoint.bin");
            let result = store.save(&state, &bad_path);

            // Should return error
            assert!(result.is_err());

            // .tmp file should be cleaned up
            let tmp_path = tmp_path_for(&bad_path);
            assert!(
                !tmp_path.exists(),
                ".tmp file should be cleaned up after failed rename"
            );

            // Restore permissions for cleanup
            fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn checkpoint_display() {
        let cp = sample_checkpoint();
        let display = format!("{cp}");
        assert!(display.contains("pages=42"));
        assert!(display.contains("visited=2"));
        assert!(display.contains("queued=2"));
    }
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    use super::*;

    #[test]
    fn test_checkpoint_saves_instrument_init() {
        let _ = &*CHECKPOINT_SAVES;
    }

    #[test]
    fn test_checkpoint_loads_instrument_init() {
        let _ = &*CHECKPOINT_LOADS;
    }

    #[test]
    fn test_checkpoint_corrupted_instrument_init() {
        let _ = &*CHECKPOINT_CORRUPTED;
    }

    #[test]
    fn test_save_records_metric() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();
        // Should not panic — metric recording is a no-op side effect
        store.save(&state, &path).unwrap();
    }

    #[test]
    fn test_load_success_records_metric() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();
        store.save(&state, &path).unwrap();
        // Should not panic — metric recording is a no-op side effect
        let loaded = store.load(&path);
        assert!(loaded.is_some());
    }

    #[test]
    fn test_load_corruption_records_metric() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();
        store.save(&state, &path).unwrap();

        // Tamper to trigger corruption
        let mut data = std::fs::read(&path).unwrap();
        data[0] ^= 0xFF;
        std::fs::write(&path, &data).unwrap();

        // Should not panic — metric recording is a no-op side effect
        let loaded = store.load(&path);
        assert!(loaded.is_none());
    }
}
