//! JSON-based checkpoint store for persisting crawl state.
//!
//! Serializes visited URLs and queue state to disk for crash recovery
//! and resume support. Uses jzon-rs for SIMD-accelerated JSON serialization.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::domain::CrawlError;

/// Serializable crawl checkpoint saved to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BincodeCheckpoint {
    /// URLs already visited.
    pub visited: Vec<String>,
    /// URLs remaining in the queue.
    pub queued: Vec<String>,
    /// Total pages crawled so far.
    pub pages_crawled: u64,
    /// Checkpoint version for forward compatibility.
    pub version: u32,
}

impl Default for BincodeCheckpoint {
    fn default() -> Self {
        Self {
            visited: Vec::new(),
            queued: Vec::new(),
            pages_crawled: 0,
            version: 1,
        }
    }
}

impl BincodeCheckpoint {
    /// Load a checkpoint from disk, or return default if file doesn't exist.
    pub fn load(path: &Path) -> Result<Self, CrawlError> {
        if !path.exists() {
            debug!("No checkpoint file at {}, starting fresh", path.display());
            return Ok(Self::default());
        }

        let data = std::fs::read(path)
            .map_err(|e| CrawlError::Checkpoint(format!("failed to read checkpoint: {e}")))?;

        let checkpoint: Self = jzon_serde::from_slice(&data).map_err(|e| {
            CrawlError::Checkpoint(format!("failed to deserialize checkpoint: {e}"))
        })?;

        info!(
            "Loaded checkpoint: {} visited, {} queued, {} pages",
            checkpoint.visited.len(),
            checkpoint.queued.len(),
            checkpoint.pages_crawled
        );

        Ok(checkpoint)
    }

    /// Save checkpoint to disk atomically (write to temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), CrawlError> {
        let data = jzon_serde::to_string(self)
            .map_err(|e| CrawlError::Checkpoint(format!("failed to serialize checkpoint: {e}")))?
            .into_bytes();

        let tmp_path = path.with_extension("tmp");

        std::fs::write(&tmp_path, &data)
            .map_err(|e| CrawlError::Checkpoint(format!("failed to write checkpoint: {e}")))?;

        std::fs::rename(&tmp_path, path)
            .map_err(|e| CrawlError::Checkpoint(format!("failed to rename checkpoint: {e}")))?;

        debug!(
            "Saved checkpoint: {} visited, {} queued, {} pages",
            self.visited.len(),
            self.queued.len(),
            self.pages_crawled
        );

        Ok(())
    }

    /// Build a checkpoint from current crawl state.
    pub fn from_state(visited: &HashSet<String>, queued: &[String], pages_crawled: u64) -> Self {
        Self {
            visited: visited.iter().cloned().collect(),
            queued: queued.to_vec(),
            pages_crawled,
            version: 1,
        }
    }
}

/// Path helper for checkpoint files.
#[derive(Debug, Clone)]
pub struct CheckpointPath {
    base_dir: PathBuf,
}

impl CheckpointPath {
    /// Create a new CheckpointPath for the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Get the checkpoint file path.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        self.base_dir.join("crawl_checkpoint.json")
    }

    /// Ensure the base directory exists.
    pub fn ensure_dir(&self) -> Result<(), CrawlError> {
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| CrawlError::Checkpoint(format!("failed to create checkpoint dir: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_checkpoint_is_empty() {
        let cp = BincodeCheckpoint::default();
        assert!(cp.visited.is_empty());
        assert!(cp.queued.is_empty());
        assert_eq!(cp.pages_crawled, 0);
        assert_eq!(cp.version, 1);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let mut visited = HashSet::new();
        visited.insert("https://a.com".to_string());
        visited.insert("https://b.com".to_string());

        let cp = BincodeCheckpoint::from_state(&visited, &["https://c.com".to_string()], 42);
        cp.save(&path).unwrap();

        let loaded = BincodeCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.visited.len(), 2);
        assert_eq!(loaded.queued.len(), 1);
        assert_eq!(loaded.pages_crawled, 42);
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        let cp = BincodeCheckpoint::load(Path::new("/nonexistent/path/checkpoint.bin")).unwrap();
        assert_eq!(cp.pages_crawled, 0);
    }

    #[test]
    fn test_checkpoint_path_helper() {
        let tmp = TempDir::new().unwrap();
        let cp = CheckpointPath::new(tmp.path());
        cp.ensure_dir().unwrap();
        assert!(cp
            .file()
            .to_string_lossy()
            .contains("crawl_checkpoint.json"));
    }
}
