//! Crawl progress metrics (issue #356, Fase 4).
//!
//! Pure computation helper for periodic progress logging during a crawl.
//! Kept free of I/O and tracing so the metrics are unit-testable in isolation.

use std::time::Duration;

/// Computes crawl progress metrics for periodic logging.
#[derive(Debug, Clone, Copy)]
pub struct CrawlProgress {
    /// Pages crawled so far.
    pub pages_crawled: u64,
    /// Target page limit (`max_pages`).
    pub max_pages: usize,
    /// Time elapsed since the crawl started.
    pub elapsed: Duration,
}

impl CrawlProgress {
    /// Create a progress snapshot.
    pub fn new(pages_crawled: u64, max_pages: usize, elapsed: Duration) -> Self {
        Self {
            pages_crawled,
            max_pages,
            elapsed,
        }
    }

    /// Completion percentage (0.0–100.0) relative to `max_pages`.
    ///
    /// Returns 0.0 when `max_pages` is 0 (no target).
    pub fn progress_pct(&self) -> f64 {
        if self.max_pages == 0 {
            return 0.0;
        }
        (self.pages_crawled as f64 / self.max_pages as f64) * 100.0
    }

    /// Average pages per second over the elapsed time.
    ///
    /// Returns 0.0 when no time has elapsed.
    pub fn pages_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.pages_crawled as f64 / secs
    }

    /// Estimated seconds remaining to reach `max_pages` at the current rate.
    ///
    /// Returns 0 when the rate is zero or there is no target.
    pub fn eta_secs(&self) -> u64 {
        let rate = self.pages_per_sec();
        if rate <= 0.0 || self.max_pages == 0 {
            return 0;
        }
        let remaining = (self.max_pages as f64 - self.pages_crawled as f64).max(0.0);
        (remaining / rate) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_pct() {
        let p = CrawlProgress::new(50, 200, Duration::from_secs(10));
        assert!((p.progress_pct() - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_pages_per_sec() {
        let p = CrawlProgress::new(100, 200, Duration::from_secs(10));
        assert!((p.pages_per_sec() - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_eta_secs() {
        // 10 pages/s, 100 pages remaining → 10s ETA
        let p = CrawlProgress::new(100, 200, Duration::from_secs(10));
        assert_eq!(p.eta_secs(), 10);
    }

    #[test]
    fn test_eta_never_negative_past_target() {
        // Already past max_pages → ETA clamps to 0
        let p = CrawlProgress::new(250, 200, Duration::from_secs(10));
        assert_eq!(p.eta_secs(), 0);
    }

    #[test]
    fn test_zero_guards() {
        let p = CrawlProgress::new(0, 0, Duration::from_secs(0));
        assert_eq!(p.progress_pct(), 0.0);
        assert_eq!(p.pages_per_sec(), 0.0);
        assert_eq!(p.eta_secs(), 0);
    }
}
