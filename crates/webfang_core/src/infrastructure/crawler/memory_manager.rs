//! Memory Manager Module
//!
//! Manages memory usage for large sitemap processing through page iterators
//! and disk swapping for extremely large datasets.

use crate::domain::UrlBatch;
use std::collections::VecDeque;
use url::Url;

/// Errors that can occur during memory management
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("memory limit exceeded: {0} MB")]
    MemoryLimitExceeded(usize),
    #[error("disk swap failed: {0}")]
    DiskSwapFailed(String),
}

/// Result type for memory operations
pub type Result<T> = std::result::Result<T, MemoryError>;

/// Manages memory usage for large URL collections
pub struct MemoryManager {
    memory_limit_mb: usize,
    enable_disk_swap: bool,
    temp_dir: Option<std::path::PathBuf>,
}

impl MemoryManager {
    /// Create new memory manager with default settings
    pub fn new() -> Self {
        Self {
            memory_limit_mb: 500,
            enable_disk_swap: false,
            temp_dir: None,
        }
    }

    /// Create with custom memory limit
    pub fn with_memory_limit(memory_limit_mb: usize) -> Self {
        Self {
            memory_limit_mb,
            ..Self::new()
        }
    }

    /// Enable disk swapping with temp directory
    pub fn with_disk_swap(temp_dir: std::path::PathBuf) -> Self {
        Self {
            enable_disk_swap: true,
            temp_dir: Some(temp_dir),
            ..Self::new()
        }
    }

    /// Create an iterator that yields URL batches for paginated processing
    ///
    /// This method implements the page iterator pattern, chunking large URL
    /// collections into manageable batches to avoid memory issues.
    pub fn create_page_iterator(
        &self,
        urls: Vec<Url>,
        batch_size: usize,
    ) -> impl Iterator<Item = Result<UrlBatch>> {
        let _total_urls = urls.len();
        let batch_size = if batch_size == 0 { 1 } else { batch_size };

        let mut queue: VecDeque<Url> = VecDeque::from(urls);
        let mut current_batch = 0u32;

        std::iter::from_fn(move || {
            if queue.is_empty() {
                return None;
            }

            let batch_urls: Vec<Url> = queue.drain(..batch_size.min(queue.len())).collect();

            let has_more = !queue.is_empty();
            let batch_id = current_batch;
            current_batch += 1;

            Some(Ok(UrlBatch {
                urls: batch_urls,
                batch_id,
                has_more,
            }))
        })
    }

    /// Handle disk swapping for extremely large URL collections
    ///
    /// When the number of URLs exceeds the memory limit, this method
    /// writes URLs to disk in chunks and provides a reference to the
    /// on-disk storage.
    pub fn handle_disk_swapping(&self, urls: &[Url]) -> Result<()> {
        if !self.enable_disk_swap {
            // Disk swapping disabled, check memory limit
            // Each URL takes roughly 2KB when stored in memory
            let estimated_bytes = urls.len() * 2000;
            let estimated_mb = estimated_bytes / (1024 * 1024);
            if estimated_mb >= self.memory_limit_mb {
                return Err(MemoryError::MemoryLimitExceeded(estimated_mb));
            }
            return Ok(());
        }

        // Disk swapping enabled
        let temp_dir = self
            .temp_dir
            .as_ref()
            .ok_or_else(|| MemoryError::DiskSwapFailed("temp directory not set".to_string()))?;

        // Write URLs to disk in chunks
        let chunk_size = 10_000;
        for (chunk_idx, chunk) in urls.chunks(chunk_size).enumerate() {
            let file_path = temp_dir.join(format!("urls_chunk_{chunk_idx}.txt"));
            let mut content = String::new();
            for url in chunk {
                content.push_str(url.as_str());
                content.push('\n');
            }

            std::fs::write(&file_path, content).map_err(|e| {
                MemoryError::DiskSwapFailed(format!("failed to write chunk {chunk_idx}: {e}"))
            })?;
        }

        Ok(())
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_memory_manager_creation() {
        let manager = MemoryManager::new();
        assert_eq!(manager.memory_limit_mb, 500);
        assert!(!manager.enable_disk_swap);
    }

    #[test]
    fn test_create_page_iterator() {
        let manager = MemoryManager::new();
        let urls = vec![
            Url::parse("https://example.com/1").unwrap(),
            Url::parse("https://example.com/2").unwrap(),
            Url::parse("https://example.com/3").unwrap(),
        ];

        let mut iterator = manager.create_page_iterator(urls, 2);
        let batch1 = iterator.next().unwrap().unwrap();
        assert_eq!(batch1.urls.len(), 2);
        assert_eq!(batch1.batch_id, 0);
        assert!(batch1.has_more);

        let batch2 = iterator.next().unwrap().unwrap();
        assert_eq!(batch2.urls.len(), 1);
        assert_eq!(batch2.batch_id, 1);
        assert!(!batch2.has_more);
    }

    #[test]
    fn test_handle_disk_swapping_without_swap() {
        let manager = MemoryManager::new();
        let urls = vec![
            Url::parse("https://example.com/1").unwrap(),
            Url::parse("https://example.com/2").unwrap(),
        ];

        // Should succeed without disk swap enabled
        let result = manager.handle_disk_swapping(&urls);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_disk_swapping_with_swap() {
        let temp_dir = TempDir::new().unwrap();
        let manager = MemoryManager::with_disk_swap(temp_dir.path().to_path_buf());

        let urls: Vec<Url> = (0..15_000)
            .map(|i| Url::parse(&format!("https://example.com/page{}", i)).unwrap())
            .collect();

        // Should write chunks to disk
        let result = manager.handle_disk_swapping(&urls);
        assert!(result.is_ok());

        // Verify chunks were written
        let chunks = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().to_string_lossy().contains("urls_chunk_"))
            .count();
        assert!(chunks > 0, "Expected at least one chunk file");
    }

    #[cfg_attr(miri, ignore)] // 525-URL loop hangs under Miri (100x slowdown makes test exceed timeout)
    #[test]
    fn test_handle_disk_swapping_memory_limit_exceeded() {
        let manager = MemoryManager::with_memory_limit(1); // 1MB limit
                                                           // 525 URLs * 2000 bytes = 1,050,000 bytes >= 1,048,576 bytes (1MB)
        let url_count_that_exceeds = 525;
        let urls: Vec<Url> = (0..url_count_that_exceeds)
            .map(|i| Url::parse(&format!("https://example.com/page{}", i)).unwrap())
            .collect();

        // Should fail due to memory limit when disk swap disabled
        let result = manager.handle_disk_swapping(&urls);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MemoryError::MemoryLimitExceeded(_)
        ));
    }
}
