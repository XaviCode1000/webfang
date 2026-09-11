use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::downloader_port::DownloadError;

/// RAII helper that creates a unique user-data-dir for each Chromium launch.
///
/// The directory is created under the system temporary directory with a name
/// composed of the process ID, current time in nanoseconds, and an incrementing
/// sequence number to avoid collisions. On drop, the directory is removed
/// (best-effort).
///
/// Creation failures are converted to `DownloadError::Internal`.
pub(crate) struct ChromeProfileDir {
    path: PathBuf,
}

impl ChromeProfileDir {
    /// Create a new unique user-data-dir.
    ///
    /// Returns `Err(DownloadError::Internal)` if the directory cannot be
    /// created.
    pub(crate) fn new() -> Result<Self, DownloadError> {
        // Static counter to avoid collisions within the same nanosecond.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

        let mut dir = env::temp_dir();
        dir.push(format!(
            "webfang-chrome-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            seq
        ));

        fs::create_dir_all(&dir).map_err(|e| {
            DownloadError::Internal(format!("failed to create Chrome user data dir: {e}"))
        })?;

        Ok(Self { path: dir })
    }

    /// Get the path to the user data directory.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ChromeProfileDir {
    fn drop(&mut self) {
        // Best-effort cleanup; ignore errors.
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_chrome_profile_dir_creates_unique_dirs() {
        let dirs: Vec<PathBuf> = (0..10)
            .map(|_| ChromeProfileDir::new().unwrap().path().to_path_buf())
            .collect();

        // All directories should be unique.
        let mut sorted = dirs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), dirs.len(), "duplicate directories generated");

        // All directories should exist.
        for dir in &dirs {
            assert!(dir.exists(), "directory does not exist: {dir:?}");
        }
    }

    #[test]
    fn test_chrome_profile_dir_cleans_up_on_drop() {
        let dir = {
            let tmp = ChromeProfileDir::new().unwrap();
            let path = tmp.path().to_path_buf();
            assert!(path.exists(), "directory should exist before drop");
            path
        };
        // After dropping, the directory should be removed.
        assert!(!dir.exists(), "directory should be removed after drop");
    }

    #[test]
    fn test_chrome_profile_dir_is_unique_across_threads() {
        let mut handles = Vec::new();
        let mut dirs = Vec::new();

        for _ in 0..10 {
            let handle = thread::spawn(|| ChromeProfileDir::new().unwrap().path().to_path_buf());
            handles.push(handle);
        }

        for handle in handles {
            dirs.push(handle.join().unwrap());
        }

        // All directories should be unique.
        let mut sorted = dirs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            dirs.len(),
            "duplicate directories generated across threads"
        );
    }

    #[test]
    fn test_chrome_profile_dir_not_default() {
        let profile = ChromeProfileDir::new().unwrap();
        let dir = profile.path();
        let default_dir = env::temp_dir().join("chromiumoxide-runner");
        assert_ne!(dir, default_dir, "profile dir should not be the default");
    }
}
