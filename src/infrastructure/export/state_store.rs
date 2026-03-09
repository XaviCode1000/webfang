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
use std::path::PathBuf;

use crate::domain::ExportState;
use crate::error::ScraperError;
use dirs::cache_dir;
use tracing::{debug, info};

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
    /// use rust_scraper::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// ```
    #[must_use]
    pub fn new(domain: &str) -> Self {
        // Get cache directory using dirs crate
        // Following **mem-with-capacity**: Pre-allocate path buffer
        let mut cache_dir = cache_dir().unwrap_or_else(|| PathBuf::from(".cache"));
        cache_dir.push("rust-scraper");
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

    /// Get the full path to the state file
    ///
    /// # Returns
    ///
    /// PathBuf containing the full path to the state JSON file
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// let path = store.get_state_path();
    /// ```
    #[must_use]
    pub fn get_state_path(&self) -> PathBuf {
        let mut path = self.cache_dir.clone();
        path.push(format!("{}.json", self.domain));
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
    /// use rust_scraper::infrastructure::export::StateStore;
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

        // Read and parse JSON file
        let content = fs::read_to_string(&path).map_err(|e| ScraperError::Io(e))?; // IO error when reading file

        let state: ExportState =
            serde_json::from_str(&content).map_err(|e| ScraperError::Serialization(e))?; // Serialization error when parsing JSON

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
    /// use rust_scraper::infrastructure::export::StateStore;
    /// use rust_scraper::domain::ExportState;
    ///
    /// let store = StateStore::new("example.com");
    /// let mut state = ExportState::new("example.com");
    /// state.mark_processed("https://example.com/page1");
    /// store.save(&state)?;
    /// ```
    pub fn save(&self, state: &ExportState) -> crate::error::Result<()> {
        let path = self.get_state_path();

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ScraperError::Io(e))?; // IO error when creating directories
        }

        // Serialize to JSON
        let json =
            serde_json::to_string_pretty(state).map_err(|e| ScraperError::Serialization(e))?; // Serialization error

        // Write to file atomically
        // Following **mem-with-capacity**: Pre-allocate file
        let mut temp_path = path.clone();
        temp_path.set_extension("tmp");

        let mut file = fs::File::create(&temp_path).map_err(|e| ScraperError::Io(e))?; // IO error when creating file

        file.write_all(json.as_bytes())
            .map_err(|e| ScraperError::Io(e))?; // IO error when writing to file

        // Atomic rename
        fs::rename(&temp_path, &path).map_err(|e| ScraperError::Io(e))?; // IO error when moving file

        debug!(
            "Saved state for domain {}: {} URLs processed",
            self.domain,
            state.processed_urls.len()
        );

        Ok(())
    }

    /// Mark a URL as processed in the state
    ///
    /// # Arguments
    ///
    /// * `state` - Mutable reference to ExportState
    /// * `url` - URL to mark as processed
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::infrastructure::export::StateStore;
    /// use rust_scraper::domain::ExportState;
    ///
    /// let store = StateStore::new("example.com");
    /// let mut state = ExportState::new("example.com");
    /// store.mark_processed(&mut state, "https://example.com/page1");
    /// ```
    pub fn mark_processed(&self, state: &mut ExportState, url: &str) {
        state.mark_processed(url);
        debug!("Marked URL as processed: {}", url);
    }

    /// Check if a URL has been processed
    ///
    /// # Arguments
    ///
    /// * `state` - Reference to ExportState
    /// * `url` - URL to check
    ///
    /// # Returns
    ///
    /// `true` if URL was processed, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::infrastructure::export::StateStore;
    /// use rust_scraper::domain::ExportState;
    ///
    /// let store = StateStore::new("example.com");
    /// let mut state = ExportState::new("example.com");
    /// store.mark_processed(&mut state, "https://example.com/page1");
    /// assert!(store.is_processed(&state, "https://example.com/page1"));
    /// ```
    #[must_use]
    pub fn is_processed(&self, state: &ExportState, url: &str) -> bool {
        let processed = state.is_processed(url);
        debug!("URL {} processed: {}", url, processed);
        processed
    }

    /// Load existing state or create a new one if it doesn't exist
    ///
    /// # Returns
    ///
    /// * `Ok(ExportState)` - Loaded or newly created state
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::infrastructure::export::StateStore;
    ///
    /// let store = StateStore::new("example.com");
    /// let state = store.load_or_default().unwrap();
    /// ```
    pub fn load_or_default(&self) -> crate::error::Result<ExportState> {
        match self.load() {
            Ok(state) => {
                info!("Loaded existing state for domain: {}", self.domain);
                Ok(state)
            }
            Err(ScraperError::Io(io_err)) => {
                // If it's an IO error, check if it's a "file not found" error
                // For "file not found", return a new state; otherwise propagate the error
                if io_err.kind() == std::io::ErrorKind::NotFound {
                    info!("Creating new state for domain: {}", self.domain);
                    Ok(ExportState::new(&self.domain))
                } else {
                    // Propagate other IO errors (permissions, disk full, etc.)
                    Err(ScraperError::Io(io_err))
                }
            }
            Err(e) => {
                // If it's another kind of error (like serialization), return it
                Err(e)
            }
        }
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
        assert!(path_str.contains("rust-scraper/state/test.domain.json"));
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
        cache_dir.push("rust-scraper/state");

        // Create a store with custom cache dir
        let mut store = StateStore::new("test.com");
        store.cache_dir = cache_dir.clone();

        // Create and save state
        let mut state = ExportState::new("test.com");
        state.mark_processed("https://test.com/page1");
        state.mark_processed("https://test.com/page2");

        let save_result = store.save(&state);
        assert!(save_result.is_ok());

        // Load state
        let loaded_state = store.load();
        assert!(loaded_state.is_ok());
        let loaded_state = loaded_state.unwrap();

        assert_eq!(loaded_state.domain, "test.com");
        assert_eq!(loaded_state.processed_urls.len(), 2);
        assert!(loaded_state.is_processed("https://test.com/page1"));
        assert!(loaded_state.is_processed("https://test.com/page2"));
    }

    #[test]
    fn test_mark_processed() {
        let store = StateStore::new("test.com");
        let mut state = ExportState::new("test.com");

        assert!(!store.is_processed(&state, "https://test.com/page1"));

        store.mark_processed(&mut state, "https://test.com/page1");
        assert!(store.is_processed(&state, "https://test.com/page1"));

        // Test duplicate marking doesn't duplicate
        store.mark_processed(&mut state, "https://test.com/page1");
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_load_or_default_existing() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("rust-scraper/state");
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
        assert_eq!(state.domain, "existing.com");
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_load_or_default_new() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().to_path_buf();

        let mut store = StateStore::new("new.com");
        store.cache_dir = cache_dir;

        let state = store.load_or_default().unwrap();
        assert_eq!(state.domain, "new.com");
        assert_eq!(state.processed_urls.len(), 0);
    }

    #[test]
    fn test_atomic_save() {
        let dir = tempdir().unwrap();
        let mut cache_dir = dir.path().to_path_buf();
        cache_dir.push("rust-scraper/state");

        let mut store = StateStore::new("atomic.com");
        store.cache_dir = cache_dir.clone();

        let state = ExportState::new("atomic.com");

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
}
