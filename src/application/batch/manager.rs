//! Batch manager — queue-based orchestrator for multiple batch jobs
//!
//! The [`BatchManager`] holds a queue of [`BatchJob`]s and processes them
//! through the existing [`BatchProcessor`]. It adds:
//!
//! - Job queue management (submit, status查询)
//! - URL ingestion from stdin or files
//! - Batch result aggregation
//!
//! # Architecture
//!
//! ```text
//! CLI (--batch / --batch-file)
//!     ↓
//! BatchManager::from_urls()
//!     ↓
//! BatchJob (created per input set)
//!     ↓
//! BatchProcessor::process_batch()
//!     ↓
//! BatchResult
//! ```

use std::path::Path;

use tracing::info;

use super::processor::{BatchError, BatchProcessor, BatchResult};
use super::{BatchJob, BatchJobStatus};
use crate::domain::CrawlerConfig;

/// Queue-based manager for batch crawl jobs
///
/// Holds pending jobs and processes them through [`BatchProcessor`].
/// Use [`BatchManager::from_urls`] to create from a list of URLs,
/// or [`BatchManager::from_file`] / [`BatchManager::from_stdin`] for I/O sources.
pub struct BatchManager {
    processor: BatchProcessor,
    jobs: Vec<BatchJob>,
}

impl BatchManager {
    /// Create a new batch manager with the given concurrency limit
    #[must_use]
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            processor: BatchProcessor::new(max_concurrent),
            jobs: Vec::new(),
        }
    }

    /// Create a batch manager from a list of URLs using a shared config
    ///
    /// Each URL becomes a single-item batch job. All jobs share the same
    /// [`CrawlerConfig`] (seed URL is overridden per job).
    pub fn from_urls(urls: Vec<String>, config: CrawlerConfig, max_concurrent: usize) -> Self {
        let mut manager = Self::new(max_concurrent);
        for (i, url) in urls.iter().enumerate() {
            let job = BatchJob::new(format!("job-{i}"), vec![url.clone()], config.clone());
            manager.jobs.push(job);
        }
        manager
    }

    /// Create a batch manager from a single batch job
    pub fn with_job(mut self, job: BatchJob) -> Self {
        self.jobs.push(job);
        self
    }

    /// Read URLs from a file (one URL per line, blank lines and `#` comments ignored)
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be read.
    pub fn from_file(
        path: &Path,
        config: CrawlerConfig,
        max_concurrent: usize,
    ) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        let urls = Self::parse_url_lines(&content);
        Ok(Self::from_urls(urls, config, max_concurrent))
    }

    /// Read URLs from stdin (one URL per line)
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if stdin cannot be read.
    pub fn from_stdin(
        config: CrawlerConfig,
        max_concurrent: usize,
    ) -> Result<Self, std::io::Error> {
        let mut content = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut content)?;
        let urls = Self::parse_url_lines(&content);
        Ok(Self::from_urls(urls, config, max_concurrent))
    }

    /// Parse URL lines from text, skipping blanks and comments
    fn parse_url_lines(content: &str) -> Vec<String> {
        content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(String::from)
            .collect()
    }

    /// Process all queued jobs sequentially
    ///
    /// Each job is processed via [`BatchProcessor::process_batch`].
    /// Returns a vector of [`BatchResult`] for each job.
    pub async fn process_all(&self) -> Vec<Result<BatchResult, BatchError>> {
        let mut results = Vec::with_capacity(self.jobs.len());
        for job in &self.jobs {
            info!("Processing batch job: {}", job.id);
            results.push(self.processor.process_batch(job.clone()).await);
        }
        results
    }

    /// Process all queued jobs and return an aggregated summary
    pub async fn process_all_summary(&self) -> BatchManagerSummary {
        let results = self.process_all().await;
        let mut summary = BatchManagerSummary::default();

        for result in &results {
            match result {
                Ok(r) => {
                    summary.total_urls += r.total;
                    summary.succeeded += r.succeeded;
                    summary.failed += r.failed;
                    summary.errors.extend(r.errors.clone());
                },
                Err(e) => {
                    summary.failed += 1;
                    summary
                        .errors
                        .push(("batch-error".to_string(), format!("{e}")));
                },
            }
        }

        summary
    }

    /// Get the number of queued jobs
    #[must_use]
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Get all job statuses
    #[must_use]
    pub fn statuses(&self) -> Vec<(&str, &BatchJobStatus)> {
        self.jobs
            .iter()
            .map(|j| (j.id.as_str(), &j.status))
            .collect()
    }
}

/// Aggregated summary across all batch jobs in a manager
#[derive(Debug, Default, Clone)]
pub struct BatchManagerSummary {
    pub total_urls: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub errors: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn test_config() -> CrawlerConfig {
        CrawlerConfig::new(Url::parse("https://example.com").unwrap())
    }

    #[test]
    fn test_manager_from_urls() {
        let urls = vec![
            "https://example.com".to_string(),
            "https://example.com/about".to_string(),
        ];
        let manager = BatchManager::from_urls(urls, test_config(), 3);
        assert_eq!(manager.job_count(), 2);
    }

    #[test]
    fn test_manager_parse_url_lines() {
        let content = r#"
# comment line
https://example.com

https://example.com/about
  # indented comment
https://example.com/blog
"#;
        let urls = BatchManager::parse_url_lines(content);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com");
        assert_eq!(urls[1], "https://example.com/about");
        assert_eq!(urls[2], "https://example.com/blog");
    }

    #[test]
    fn test_manager_parse_empty() {
        let urls = BatchManager::parse_url_lines("");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_manager_parse_only_comments() {
        let urls = BatchManager::parse_url_lines("# just a comment\n# another");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_manager_from_file() {
        let dir = std::env::temp_dir().join("batch_manager_test");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("urls.txt");
        std::fs::write(&file_path, "https://a.com\nhttps://b.com\n").unwrap();

        let manager = BatchManager::from_file(&file_path, test_config(), 2).unwrap();
        assert_eq!(manager.job_count(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manager_statuses() {
        let urls = vec!["https://a.com".to_string()];
        let manager = BatchManager::from_urls(urls, test_config(), 1);
        let statuses = manager.statuses();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].0, "job-0");
        assert_eq!(*statuses[0].1, BatchJobStatus::Pending);
    }

    #[tokio::test]
    async fn test_process_all_empty() {
        let manager = BatchManager::new(3);
        let results = manager.process_all().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_process_all_summary_empty() {
        let manager = BatchManager::new(3);
        let summary = manager.process_all_summary().await;
        assert_eq!(summary.total_urls, 0);
        assert_eq!(summary.succeeded, 0);
        assert_eq!(summary.failed, 0);
    }

    #[test]
    fn test_manager_with_job() {
        let config = test_config();
        let job = BatchJob::new(
            "custom-job".to_string(),
            vec!["https://example.com".to_string()],
            config,
        );
        let manager = BatchManager::new(2).with_job(job);
        assert_eq!(manager.job_count(), 1);
        assert_eq!(manager.statuses()[0].0, "custom-job");
    }
}
