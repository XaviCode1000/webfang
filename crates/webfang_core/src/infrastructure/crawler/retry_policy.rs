//! Retry Policy Module
//!
//! Implements exponential backoff retry logic for failed operations.
//! Handles timeouts and network errors with configurable retry limits.

use std::time::Duration;
use tracing::instrument;

/// Errors that can occur during retry operations
#[derive(Debug, thiserror::Error)]
pub enum RetryError {
    #[error("max retries exceeded: {0}")]
    MaxRetriesExceeded(String),
    #[error("operation timeout")]
    Timeout,
}

/// Result type for retry operations
pub type Result<T> = std::result::Result<T, RetryError>;

/// Handles retry logic with exponential backoff
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
    backoff_multiplier: f64,
}

impl RetryPolicy {
    /// Create new retry policy with default settings
    pub fn new() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 1000, // 1 second
            max_delay_ms: 30000, // 30 seconds
            backoff_multiplier: 2.0,
        }
    }

    /// Create retry policy with custom max attempts
    pub fn with_max_attempts(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            ..Self::new()
        }
    }

    /// Execute operation with retry logic
    #[instrument(
        name = "retry_execute",
        skip(self, operation),
        fields(
            max_attempts = self.max_attempts,
            base_delay_ms = self.base_delay_ms,
            max_delay_ms = self.max_delay_ms,
            attempt,
            error
        )
    )]
    pub async fn execute_with_retry<F, Fut, T, E>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, E>>,
        E: std::fmt::Display,
    {
        let mut attempt = 0;
        let mut delay = self.base_delay_ms;

        loop {
            attempt += 1;

            match operation().await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    if attempt >= self.max_attempts {
                        tracing::error!(
                            max_retries_exceeded = true,
                            total_attempts = attempt,
                            last_error = %error
                        );
                        return Err(RetryError::MaxRetriesExceeded(error.to_string()));
                    }

                    tracing::warn!(
                        attempt,
                        error = %error,
                        retry_delay_ms = delay,
                        "Operation failed, retrying"
                    );

                    tokio::time::sleep(Duration::from_millis(delay)).await;

                    // Calculate next delay with exponential backoff
                    delay = ((delay as f64) * self.backoff_multiplier) as u64;
                    delay = delay.min(self.max_delay_ms);
                },
            }
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_retry_policy_creation() {
        let policy = RetryPolicy::new();
        assert_eq!(policy.max_attempts, 3);
    }

    #[cfg_attr(miri, ignore)] // tokio time-driver does not advance under Miri (hangs on sleep)
    #[tokio::test]
    async fn test_execute_with_retry_success() {
        let policy = RetryPolicy::new();

        let attempts = AtomicUsize::new(0);
        let result = policy
            .execute_with_retry(|| {
                let attempts = &attempts;
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    if count < 2 {
                        Err("temporary failure".to_string())
                    } else {
                        Ok("success".to_string())
                    }
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[cfg_attr(miri, ignore)] // tokio time-driver does not advance under Miri (hangs on sleep)
    #[tokio::test]
    async fn test_execute_with_retry_failure() {
        let policy = RetryPolicy::with_max_attempts(2);

        let attempts = AtomicUsize::new(0);
        let result: Result<String> = policy
            .execute_with_retry(|| {
                let _attempts = &attempts;
                async move { Err::<String, _>("persistent failure".to_string()) }
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_with_retry_immediate_success() {
        let policy = RetryPolicy::new();

        let attempts = AtomicUsize::new(0);
        let result: Result<String> = policy
            .execute_with_retry(|| {
                let _attempts = &attempts;
                async move { Ok::<String, String>("immediate success".to_string()) }
            })
            .await;

        assert!(result.is_ok());
    }
}
