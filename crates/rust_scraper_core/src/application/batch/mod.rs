//! Batch processing module — Planner-Worker pattern for parallel crawl execution
//!
//! This module provides batch processing capabilities for crawling multiple URLs
//! concurrently with progress tracking and graceful shutdown.
//!
//! # Architecture
//!
//! ```text
//! BatchJob (input)
//!     ↓
//! BatchProcessor (worker pool with semaphore)
//!     ↓
//! BatchResult (output)
//! ```
//!
//! The [`BatchProcessor`] wraps the existing `crawl_site()` function to process
//! multiple URLs concurrently while respecting concurrency limits via
//! [`tokio::sync::Semaphore`].

pub mod manager;
pub mod processor;

pub use manager::{BatchManager, BatchManagerSummary};
pub use processor::{BatchProcessor, BatchResult};

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Status of a batch job
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchJobStatus {
    /// Job is waiting to be processed
    Pending,
    /// Job is currently being processed
    Running,
    /// Job completed successfully
    Completed,
    /// Job failed with an error message
    Failed(String),
}

impl fmt::Display for BatchJobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Running => write!(f, "Running"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed(msg) => write!(f, "Failed: {msg}"),
        }
    }
}

/// Thread-safe progress tracking for batch operations
///
/// Uses atomic counters for lock-free updates from concurrent tasks.
#[derive(Debug, Clone)]
pub struct BatchProgress {
    inner: Arc<BatchProgressInner>,
}

#[derive(Debug)]
struct BatchProgressInner {
    total: AtomicUsize,
    completed: AtomicUsize,
    failed: AtomicUsize,
    in_progress: AtomicUsize,
}

impl BatchProgress {
    /// Create a new progress tracker with the given total
    pub fn new(total: usize) -> Self {
        Self {
            inner: Arc::new(BatchProgressInner {
                total: AtomicUsize::new(total),
                completed: AtomicUsize::new(0),
                failed: AtomicUsize::new(0),
                in_progress: AtomicUsize::new(0),
            }),
        }
    }

    /// Mark one task as started
    pub fn start_one(&self) {
        self.inner.in_progress.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark one task as completed
    pub fn complete_one(&self) {
        self.inner.in_progress.fetch_sub(1, Ordering::Relaxed);
        self.inner.completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark one task as failed
    pub fn fail_one(&self) {
        self.inner.in_progress.fetch_sub(1, Ordering::Relaxed);
        self.inner.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the total number of tasks
    #[must_use]
    pub fn total(&self) -> usize {
        self.inner.total.load(Ordering::Relaxed)
    }

    /// Get the number of completed tasks
    #[must_use]
    pub fn completed(&self) -> usize {
        self.inner.completed.load(Ordering::Relaxed)
    }

    /// Get the number of failed tasks
    #[must_use]
    pub fn failed(&self) -> usize {
        self.inner.failed.load(Ordering::Relaxed)
    }

    /// Get the number of tasks currently in progress
    #[must_use]
    pub fn in_progress(&self) -> usize {
        self.inner.in_progress.load(Ordering::Relaxed)
    }

    /// Check if all tasks are complete
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.completed() + self.failed() >= self.total()
    }

    /// Get completion percentage (0.0 - 100.0)
    #[must_use]
    pub fn percent(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 100.0;
        }
        let done = self.completed() + self.failed();
        (done as f64 / total as f64) * 100.0
    }
}

impl Default for BatchProgress {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A batch job containing URLs to crawl
#[derive(Debug, Clone)]
pub struct BatchJob {
    /// Unique identifier for this batch job
    pub id: String,
    /// URLs to crawl
    pub urls: Vec<String>,
    /// Shared configuration for all URLs in the batch
    pub config: crate::domain::CrawlerConfig,
    /// Current status of the job
    pub status: BatchJobStatus,
    /// Thread-safe progress tracking
    pub progress: BatchProgress,
}

impl BatchJob {
    /// Create a new batch job
    pub fn new(id: String, urls: Vec<String>, config: crate::domain::CrawlerConfig) -> Self {
        let total = urls.len();
        Self {
            id,
            urls,
            config,
            status: BatchJobStatus::Pending,
            progress: BatchProgress::new(total),
        }
    }

    /// Mark the job as running
    pub fn start(&mut self) {
        self.status = BatchJobStatus::Running;
    }

    /// Mark the job as completed
    pub fn complete(&mut self) {
        self.status = BatchJobStatus::Completed;
    }

    /// Mark the job as failed
    pub fn fail(&mut self, reason: String) {
        self.status = BatchJobStatus::Failed(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_batch_job_creation() {
        let config = crate::domain::CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let job = BatchJob::new(
            "test-1".to_string(),
            vec![
                "https://example.com".to_string(),
                "https://example.com/about".to_string(),
            ],
            config,
        );

        assert_eq!(job.id, "test-1");
        assert_eq!(job.urls.len(), 2);
        assert_eq!(job.status, BatchJobStatus::Pending);
        assert_eq!(job.progress.total(), 2);
    }

    #[test]
    fn test_batch_job_status_transitions() {
        let config = crate::domain::CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new("test-1".to_string(), vec![], config);

        assert_eq!(job.status, BatchJobStatus::Pending);

        job.start();
        assert_eq!(job.status, BatchJobStatus::Running);

        job.complete();
        assert_eq!(job.status, BatchJobStatus::Completed);
    }

    #[test]
    fn test_batch_job_failure() {
        let config = crate::domain::CrawlerConfig::new(Url::parse("https://example.com").unwrap());
        let mut job = BatchJob::new("test-1".to_string(), vec![], config);

        job.fail("network error".to_string());
        assert_eq!(
            job.status,
            BatchJobStatus::Failed("network error".to_string())
        );
    }

    #[test]
    fn test_batch_progress_tracking() {
        let progress = BatchProgress::new(3);

        assert_eq!(progress.total(), 3);
        assert_eq!(progress.completed(), 0);
        assert_eq!(progress.failed(), 0);
        assert_eq!(progress.in_progress(), 0);
        assert!(!progress.is_complete());

        progress.start_one();
        assert_eq!(progress.in_progress(), 1);

        progress.complete_one();
        assert_eq!(progress.completed(), 1);
        assert_eq!(progress.in_progress(), 0);

        progress.start_one();
        progress.fail_one();
        assert_eq!(progress.failed(), 1);
        assert_eq!(progress.in_progress(), 0);

        progress.start_one();
        progress.complete_one();
        assert_eq!(progress.completed(), 2);
        assert!(progress.is_complete());
        assert!((progress.percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_batch_progress_percent() {
        let progress = BatchProgress::new(4);

        progress.complete_one();
        assert!((progress.percent() - 25.0).abs() < f64::EPSILON);

        progress.complete_one();
        assert!((progress.percent() - 50.0).abs() < f64::EPSILON);

        progress.fail_one();
        assert!((progress.percent() - 75.0).abs() < f64::EPSILON);

        progress.complete_one();
        assert!((progress.percent() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_batch_progress_empty() {
        let progress = BatchProgress::new(0);
        assert!(progress.is_complete());
        assert!((progress.percent() - 100.0).abs() < f64::EPSILON);
    }
}
