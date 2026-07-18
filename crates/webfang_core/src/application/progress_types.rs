//! Progress tracking types for async-reactive TUI.
//!
//! This module defines the core data structures used for tracking
//! scraping progress and handling events in the reactive TUI system.

use std::time::{Instant, SystemTime};
use thiserror::Error;

/// Represents the progress of a scraping operation.
///
/// Variants:
/// - Started: Scraping operation has begun for a URL
/// - StatusChanged: Progress status updated (carries URL and new ScrapeStatus)
/// - Completed: Operation completed successfully for a URL
/// - Failed: Operation terminated with a non-recoverable error for a URL
/// - Finished: Final state after completion or failure of all URLs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrapeProgress {
    /// Scraping started for a specific URL
    Started {
        /// URL being scraped
        url: String,
    },
    /// Status changed for a specific URL
    StatusChanged {
        /// URL being scraped
        url: String,
        /// New status
        status: ScrapeStatus,
    },
    /// Successfully completed scraping a URL
    Completed {
        /// URL that was scraped
        url: String,
        /// Character count of extracted content
        chars: usize,
    },
    /// Failed to scrape a URL
    Failed {
        /// URL that failed
        url: String,
        /// Error details
        error: ScrapeError,
    },
    /// All URLs processed (final event)
    Finished {
        /// Total URLs processed
        total: usize,
        /// Successful count
        successful: usize,
        /// Failed count
        failed: usize,
    },
}

/// Represents the current operational status during scraping.
///
/// Status values progress through a typical web scraping pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrapeStatus {
    /// URL queued, not started
    Pending,
    /// Currently fetching HTTP response
    Fetching,
    /// Processing with Readability algorithm
    Extracting,
    /// Downloading assets (if enabled)
    Downloading,
    /// Successfully scraped
    Completed,
    /// Error during scrape
    Failed,
}

impl ScrapeStatus {
    /// Get display icon for status
    pub fn icon(&self) -> &'static str {
        match self {
            ScrapeStatus::Pending => "⏳",
            ScrapeStatus::Fetching => "🌐",
            ScrapeStatus::Extracting => "📄",
            ScrapeStatus::Downloading => "📥",
            ScrapeStatus::Completed => "✅",
            ScrapeStatus::Failed => "❌",
        }
    }

    /// Get short label for status
    pub fn label(&self) -> &'static str {
        match self {
            ScrapeStatus::Pending => "Pending",
            ScrapeStatus::Fetching => "Fetching",
            ScrapeStatus::Extracting => "Extracting",
            ScrapeStatus::Downloading => "Downloading",
            ScrapeStatus::Completed => "Completed",
            ScrapeStatus::Failed => "Failed",
        }
    }
}

/// Errors that can occur during the scraping process.
///
/// These errors are categorized by the failure mode and may include
/// WAF/CAPTCHA blocks, network issues, or parsing failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScrapeError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("HTTP error {0}: {1}")]
    Http(u16, String),

    #[error("WAF/CAPTCHA detected: {0}")]
    WafBlocked(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Other error: {0}")]
    Other(String),
}

impl ScrapeError {
    /// Get error type for classification
    pub fn error_type(&self) -> ErrorType {
        match self {
            ScrapeError::Network(_) => ErrorType::Network,
            ScrapeError::Http(status, _) => ErrorType::Http(*status),
            ScrapeError::WafBlocked(provider) => ErrorType::WafBlocked(provider.clone()),
            ScrapeError::Parse(_) => ErrorType::Parse("Parse error".to_string()),
            ScrapeError::Timeout(_) => ErrorType::Timeout,
            ScrapeError::Connection(_) => ErrorType::Connection,
            ScrapeError::Other(_) => ErrorType::Other,
        }
    }

    /// Get user-friendly message
    pub fn message(&self) -> String {
        match self {
            ScrapeError::Network(s) => format!("Network error: {s}"),
            ScrapeError::Http(status, msg) => format!("HTTP {status}: {msg}"),
            ScrapeError::WafBlocked(provider) => format!("WAF blocked ({provider})"),
            ScrapeError::Parse(s) => format!("Parse error: {s}"),
            ScrapeError::Timeout(s) => format!("Timeout: {s}"),
            ScrapeError::Connection(s) => format!("Connection error: {s}"),
            ScrapeError::Other(s) => s.clone(),
        }
    }
}

/// Application events for the reactive TUI event loop.
///
/// Events drive state updates in the reactive UI system.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    UserInput(String),        // User typed input (e.g., keyboard command)
    Progress(ScrapeProgress), // Scraping progress update
    Tick,                     // Timer tick (periodic refresh)
    Quit,                     // Request to exit application
    None,                     // No event (used for polling)
}

/// Type of error for classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorType {
    /// Network-level error
    Network,
    /// HTTP error with status code
    Http(u16),
    /// WAF/CAPTCHA blocked
    WafBlocked(String),
    /// Parse error
    Parse(String),
    /// Request timeout
    Timeout,
    /// Connection error
    Connection,
    /// Other/unknown
    Other,
}

/// Represents a single error occurrence with metadata.
///
/// Errors are timestamped and may carry contextual information
/// to help diagnose issues during the scraping process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEntry {
    /// When the error occurred
    pub timestamp: SystemTime,
    /// URL that caused the error
    pub url: String,
    /// Classification of error
    pub error_type: ErrorType,
    /// Human-readable message
    pub message: String,
}

/// State for a single URL in the batch
#[derive(Debug, Clone)]
pub struct UrlState {
    /// The URL
    pub url: String,
    /// Current scraping status
    pub status: ScrapeStatus,
    /// Character count (if completed)
    pub chars: Option<usize>,
    /// Error (if failed)
    pub error: Option<ScrapeError>,
}

/// Aggregated progress state for the entire batch
#[derive(Debug, Clone)]
pub struct ProgressState {
    /// All URLs being scraped
    pub urls: Vec<UrlState>,
    /// Total count
    pub total: usize,
    /// Completed count
    pub completed: usize,
    /// Failed count
    pub failed: usize,
    /// Errors encountered (detailed log)
    pub errors: Vec<ErrorEntry>,
    /// Start time for ETA calculation
    pub start_time: Option<Instant>,
    /// Estimated time remaining in seconds
    pub eta_seconds: Option<u64>,
}

impl ProgressState {
    /// Creates a new progress state for the given URLs.
    pub fn new(urls: Vec<String>) -> Self {
        let total = urls.len();
        let url_states = urls
            .into_iter()
            .map(|url| UrlState {
                url,
                status: ScrapeStatus::Pending,
                chars: None,
                error: None,
            })
            .collect();
        Self {
            urls: url_states,
            total,
            completed: 0,
            failed: 0,
            errors: Vec::new(),
            start_time: None,
            eta_seconds: None,
        }
    }

    /// Initialize start time if not set.
    fn ensure_start_time(&mut self) {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
    }

    /// Update progress state from a ScrapeProgress event.
    pub fn update(&mut self, progress: ScrapeProgress) {
        match progress {
            ScrapeProgress::Started { url } => {
                self.ensure_start_time();
                if let Some(state) = self.urls.iter_mut().find(|s| s.url == url) {
                    state.status = ScrapeStatus::Fetching;
                }
            },
            ScrapeProgress::StatusChanged { url, status } => {
                self.ensure_start_time();
                if let Some(state) = self.urls.iter_mut().find(|s| s.url == url) {
                    state.status = status;
                }
            },
            ScrapeProgress::Completed { url, chars } => {
                self.completed += 1;
                if let Some(state) = self.urls.iter_mut().find(|s| s.url == url) {
                    state.status = ScrapeStatus::Completed;
                    state.chars = Some(chars);
                }
                self.update_eta();
            },
            ScrapeProgress::Failed { url, error } => {
                self.failed += 1;
                if let Some(state) = self.urls.iter_mut().find(|s| s.url == url) {
                    state.status = ScrapeStatus::Failed;
                    state.error = Some(error.clone());
                }
                // Record error in log
                self.errors.push(ErrorEntry {
                    timestamp: SystemTime::now(),
                    url: url.clone(),
                    error_type: error.error_type(),
                    message: error.message(),
                });
                self.update_eta();
            },
            ScrapeProgress::Finished {
                total: _,
                successful: _,
                failed: _,
            } => {
                // Final event — counts already tracked via Completed/Failed updates
            },
        }
    }

    /// Calculate progress percentage
    pub fn percentage(&self) -> f64 {
        let processed = self.completed + self.failed;
        if self.total == 0 {
            0.0
        } else {
            (processed as f64 / self.total as f64) * 100.0
        }
    }

    /// Update ETA based on current progress
    pub fn update_eta(&mut self) {
        let Some(start) = self.start_time else {
            return;
        };
        let elapsed = start.elapsed().as_secs_f64();
        let processed = self.completed + self.failed;
        if processed > 0 && self.total > processed {
            let per_url = elapsed / processed as f64;
            let remaining = (self.total - processed) as f64;
            self.eta_seconds = Some((per_url * remaining) as u64);
        } else {
            self.eta_seconds = Some(0);
        }
    }

    /// Get current URL being processed (first non-completed/non-pending)
    pub fn current_url(&self) -> Option<&str> {
        self.urls
            .iter()
            .find(|s| {
                matches!(
                    s.status,
                    ScrapeStatus::Fetching | ScrapeStatus::Extracting | ScrapeStatus::Downloading
                )
            })
            .map(|s| s.url.as_str())
    }

    /// Check if operation is complete
    pub fn is_complete(&self) -> bool {
        self.completed + self.failed >= self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    fn sample_urls() -> Vec<String> {
        vec![
            "https://example.com/1".to_string(),
            "https://example.com/2".to_string(),
            "https://example.com/3".to_string(),
        ]
    }

    #[test]
    fn test_progress_state_new() {
        let urls = sample_urls();
        let state = ProgressState::new(urls.clone());

        assert_eq!(state.total, 3);
        assert_eq!(state.completed, 0);
        assert_eq!(state.failed, 0);
        assert_eq!(state.errors.len(), 0);
        assert_eq!(state.urls.len(), 3);
        assert!(state.start_time.is_none());
        assert!(state.eta_seconds.is_none());

        // Check all URLs start as Pending and match input URLs
        for (i, url_state) in state.urls.iter().enumerate() {
            assert_eq!(url_state.status, ScrapeStatus::Pending);
            assert_eq!(url_state.url, urls[i]);
        }
    }

    #[test]
    fn test_progress_state_update_started() {
        let mut state = ProgressState::new(sample_urls());
        let url = state.urls[0].url.clone();

        state.update(ScrapeProgress::Started { url: url.clone() });

        assert!(state.start_time.is_some());
        assert_eq!(state.urls[0].status, ScrapeStatus::Fetching);
    }

    #[test]
    fn test_progress_state_update_completed() {
        let mut state = ProgressState::new(sample_urls());
        let url = state.urls[0].url.clone();

        state.update(ScrapeProgress::Started { url: url.clone() });
        state.update(ScrapeProgress::Completed { url, chars: 1234 });

        assert_eq!(state.completed, 1);
        assert_eq!(state.urls[0].status, ScrapeStatus::Completed);
        assert_eq!(state.urls[0].chars, Some(1234));
        assert!(state.eta_seconds.is_some());
    }

    #[test]
    fn test_progress_state_update_failed() {
        let mut state = ProgressState::new(sample_urls());
        let url = state.urls[0].url.clone();
        let error = ScrapeError::Network("connection refused".to_string());

        state.update(ScrapeProgress::Started { url: url.clone() });
        state.update(ScrapeProgress::Failed {
            url,
            error: error.clone(),
        });

        assert_eq!(state.failed, 1);
        assert_eq!(state.urls[0].status, ScrapeStatus::Failed);
        assert_eq!(
            state.urls[0].error.as_ref().unwrap().to_string(),
            error.to_string()
        );
        assert_eq!(state.errors.len(), 1);
        assert_eq!(state.errors[0].url, "https://example.com/1");
    }

    #[test]
    fn test_progress_state_percentage() {
        let mut state = ProgressState::new(sample_urls());
        assert_eq!(state.percentage(), 0.0);

        // Complete 1 out of 3
        let url1 = state.urls[0].url.clone();
        state.update(ScrapeProgress::Started { url: url1.clone() });
        state.update(ScrapeProgress::Completed {
            url: url1,
            chars: 100,
        });
        assert!((state.percentage() - 33.33).abs() < 0.1);

        // Fail 1 more (2/3 processed)
        let url2 = state.urls[1].url.clone();
        state.update(ScrapeProgress::Started { url: url2.clone() });
        state.update(ScrapeProgress::Failed {
            url: url2,
            error: ScrapeError::Http(404, "Not Found".to_string()),
        });
        assert!((state.percentage() - 66.66).abs() < 0.1);
    }

    #[test]
    fn test_progress_state_current_url() {
        let mut state = ProgressState::new(sample_urls());

        // No active URL
        assert!(state.current_url().is_none());

        // Start first URL
        let url1 = state.urls[0].url.clone();
        state.update(ScrapeProgress::Started { url: url1.clone() });
        assert_eq!(state.current_url(), Some(url1.as_str()));

        // Complete first, start second
        state.update(ScrapeProgress::Completed {
            url: url1,
            chars: 100,
        });
        let url2 = state.urls[1].url.clone();
        state.update(ScrapeProgress::Started { url: url2.clone() });
        assert_eq!(state.current_url(), Some(url2.as_str()));
    }

    #[test]
    fn test_progress_state_is_complete() {
        let mut state = ProgressState::new(sample_urls());
        assert!(!state.is_complete());

        // Complete all
        for i in 0..3 {
            let url = state.urls[i].url.clone();
            state.update(ScrapeProgress::Started { url: url.clone() });
            state.update(ScrapeProgress::Completed { url, chars: 100 });
        }
        assert!(state.is_complete());
    }

    #[test]
    fn test_scrape_status_icon() {
        assert_eq!(ScrapeStatus::Pending.icon(), "⏳");
        assert_eq!(ScrapeStatus::Fetching.icon(), "🌐");
        assert_eq!(ScrapeStatus::Extracting.icon(), "📄");
        assert_eq!(ScrapeStatus::Downloading.icon(), "📥");
        assert_eq!(ScrapeStatus::Completed.icon(), "✅");
        assert_eq!(ScrapeStatus::Failed.icon(), "❌");
    }

    #[test]
    fn test_scrape_status_label() {
        assert_eq!(ScrapeStatus::Pending.label(), "Pending");
        assert_eq!(ScrapeStatus::Fetching.label(), "Fetching");
        assert_eq!(ScrapeStatus::Extracting.label(), "Extracting");
        assert_eq!(ScrapeStatus::Downloading.label(), "Downloading");
        assert_eq!(ScrapeStatus::Completed.label(), "Completed");
        assert_eq!(ScrapeStatus::Failed.label(), "Failed");
    }

    #[test]
    fn test_scrape_error_methods() {
        let net = ScrapeError::Network("timeout".to_string());
        assert_eq!(net.error_type(), ErrorType::Network);
        assert!(net.message().contains("Network error"));

        let waf = ScrapeError::WafBlocked("Cloudflare".to_string());
        assert!(matches!(waf.error_type(), ErrorType::WafBlocked(p) if p == "Cloudflare"));
        assert!(waf.message().contains("WAF blocked"));
    }

    #[test]
    fn test_scrape_progress_variants() {
        let url = "https://example.com".to_string();
        let started = ScrapeProgress::Started { url: url.clone() };
        let status = ScrapeProgress::StatusChanged {
            url: url.clone(),
            status: ScrapeStatus::Fetching,
        };
        let completed = ScrapeProgress::Completed {
            url: url.clone(),
            chars: 100,
        };
        let failed = ScrapeProgress::Failed {
            url: url.clone(),
            error: ScrapeError::Network("err".into()),
        };
        let finished = ScrapeProgress::Finished {
            total: 5,
            successful: 3,
            failed: 2,
        };

        assert!(matches!(started, ScrapeProgress::Started { .. }));
        assert!(matches!(status, ScrapeProgress::StatusChanged { .. }));
        assert!(matches!(completed, ScrapeProgress::Completed { .. }));
        assert!(matches!(failed, ScrapeProgress::Failed { .. }));
        assert!(matches!(finished, ScrapeProgress::Finished { .. }));
    }

    #[test]
    fn test_url_state_transitions() {
        let url_state = UrlState {
            url: "https://example.com".to_string(),
            status: ScrapeStatus::Pending,
            chars: None,
            error: None,
        };
        assert_eq!(url_state.status, ScrapeStatus::Pending);
    }

    #[test]
    fn test_error_entry_creation() {
        let entry = ErrorEntry {
            timestamp: SystemTime::UNIX_EPOCH,
            url: "https://example.com/404".to_string(),
            error_type: ErrorType::Http(404),
            message: "Not found".to_string(),
        };

        assert_eq!(entry.url, "https://example.com/404");
        assert!(matches!(entry.error_type, ErrorType::Http(404)));
        assert_eq!(entry.message, "Not found");
    }

    #[test]
    fn test_eta_calculation() {
        let mut state = ProgressState::new(sample_urls());
        let url1 = state.urls[0].url.clone();
        let url2 = state.urls[1].url.clone();
        let url3 = state.urls[2].url.clone();

        // Start first
        state.update(ScrapeProgress::Started { url: url1.clone() });
        sleep(Duration::from_millis(100));
        // Complete first
        state.update(ScrapeProgress::Completed {
            url: url1.clone(),
            chars: 100,
        });

        // ETA should be set
        assert!(state.eta_seconds.is_some());

        // Start second, complete it
        state.update(ScrapeProgress::Started { url: url2.clone() });
        state.update(ScrapeProgress::Completed {
            url: url2.clone(),
            chars: 100,
        });

        // Start third, fail it
        state.update(ScrapeProgress::Started { url: url3.clone() });
        let err = ScrapeError::Timeout("timed out".to_string());
        state.update(ScrapeProgress::Failed {
            url: url3,
            error: err,
        });

        assert_eq!(state.completed, 2);
        assert_eq!(state.failed, 1);
        assert!(state.is_complete());
    }
}
