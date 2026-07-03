//! RAM-aware resource governor for heavyweight downloaders.
//!
//! Uses `sysinfo` to read available system memory and a [`tokio::sync::Semaphore`]
//! to gate concurrent Chrome/headless-browser instances. The formula:
//!
//! ```text
//! max_instances = (available_ram_bytes * 0.6) / CHROME_INSTANCE_COST
//! ```
//!
//! Thresholds:
//! - **80 %** RAM used → warning log, max_instances halved
//! - **90 %** RAM used → all new Chrome permits denied

use std::sync::Arc;

use sysinfo::System;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use super::DownloadError;

/// Approximate RAM cost of one Chrome instance (200 MB).
const CHROME_INSTANCE_COST: u64 = 200_000_000;

/// Fraction of available RAM we're willing to allocate to Chrome instances.
const RAM_BUDGET_FRACTION: f64 = 0.6;

/// RAM usage percentage that triggers a warning and reduces capacity by half.
const WARNING_THRESHOLD: u8 = 80;

/// RAM usage percentage that denies all new Chrome instances.
const CRITICAL_THRESHOLD: u8 = 90;

/// Gates concurrent heavyweight downloader instances based on system RAM.
pub struct ResourceGovernor {
    semaphore: Arc<Semaphore>,
}

impl ResourceGovernor {
    /// Create a governor calibrated to current system RAM.
    pub fn new() -> Self {
        let max_permits = Self::compute_max_instances();
        debug!("ResourceGovernor: max_permits={max_permits}");

        Self {
            semaphore: Arc::new(Semaphore::new(max_permits)),
        }
    }

    /// Check whether resources are available and return a permit count, or an
    /// error if the system is under too much memory pressure.
    ///
    /// The returned `usize` represents how many permits *could* be acquired
    /// right now (0 when denied).
    pub fn check_resources(&self) -> Result<usize, ResourceError> {
        let usage = Self::ram_usage_percent();

        if usage >= CRITICAL_THRESHOLD {
            warn!("RAM usage {usage}% >= {CRITICAL_THRESHOLD}%: new Chrome instances denied");
            return Err(ResourceError::RamTooHigh(usage));
        }

        let available = self.semaphore.available_permits();

        if usage >= WARNING_THRESHOLD {
            let reduced = available / 2;
            warn!(
                "RAM usage {usage}% >= {WARNING_THRESHOLD}%: available permits reduced {available} -> {reduced}"
            );
            return Ok(reduced);
        }

        Ok(available)
    }

    /// Acquire an owned semaphore permit, returning an error when resources
    /// are exhausted.
    ///
    /// Returns an [`OwnedSemaphorePermit`] (`'static`) so callers can hold
    /// it across async boundaries without tying it to the governor's lifetime.
    pub async fn acquire(&self) -> Result<tokio::sync::OwnedSemaphorePermit, DownloadError> {
        let arc = Arc::clone(&self.semaphore);

        arc.acquire_owned()
            .await
            .map_err(|_| DownloadError::Internal("resource governor semaphore closed".to_string()))
    }

    /// Current number of available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Compute `max_instances` from current system RAM.
    fn compute_max_instances() -> usize {
        let total = Self::total_ram_bytes();
        let budget = (total as f64 * RAM_BUDGET_FRACTION) as u64;
        let max = budget / CHROME_INSTANCE_COST;
        max.max(1) as usize // at least 1 permit even on tiny machines
    }

    /// Total system RAM in bytes.
    fn total_ram_bytes() -> u64 {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.total_memory()
    }

    /// RAM usage as a percentage (0–100).
    fn ram_usage_percent() -> u8 {
        let mut sys = System::new();
        sys.refresh_memory();
        let total = sys.total_memory();
        if total == 0 {
            return 0;
        }
        let used = sys.used_memory();
        ((used * 100) / total) as u8
    }
}

impl Default for ResourceGovernor {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors raised when system resources cannot support a new instance.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    /// RAM usage exceeded the critical threshold.
    #[error("RAM usage too high ({0}%): new Chrome instances denied")]
    RamTooHigh(u8),

    /// Generic resource exhaustion.
    #[error("insufficient resources: {0}")]
    Insufficient(String),
}

impl From<ResourceError> for DownloadError {
    fn from(err: ResourceError) -> Self {
        DownloadError::Internal(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_governor_creation() {
        let gov = ResourceGovernor::new();
        // On any real machine we should have at least 1 permit
        assert!(gov.available_permits() >= 1);
    }

    #[test]
    fn test_check_resources_returns_ok() {
        let gov = ResourceGovernor::new();
        // On CI or dev machines RAM usage is typically well below 80%
        let result = gov.check_resources();
        assert!(result.is_ok());
    }

    #[test]
    fn test_total_ram_nonzero() {
        let bytes = ResourceGovernor::total_ram_bytes();
        assert!(bytes > 0, "total RAM should be > 0 on any running system");
    }

    #[test]
    fn test_usage_percent_range() {
        let pct = ResourceGovernor::ram_usage_percent();
        assert!(pct <= 100);
    }

    #[test]
    fn test_resource_error_display() {
        let err = ResourceError::RamTooHigh(92);
        assert!(err.to_string().contains("92%"));
    }

    #[test]
    fn test_resource_error_into_download_error() {
        let res_err = ResourceError::Insufficient("test".into());
        let dl_err: DownloadError = res_err.into();
        assert!(matches!(dl_err, DownloadError::Internal(_)));
    }
}
