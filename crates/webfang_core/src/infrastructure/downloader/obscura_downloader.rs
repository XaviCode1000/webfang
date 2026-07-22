//! Obscura subprocess downloader.
//!
//! Wraps the `obscura` CLI tool via `std::process::Command`, executed inside
//! `tokio::task::spawn_blocking` to avoid blocking the async runtime.
//! Returns raw markdown output for downstream processing.

use std::process::Command;
use std::time::Duration;

use tokio::time::timeout;
use tracing::{debug, instrument, Instrument};
use url::Url;

use super::{DownloadError, Downloader, FetchedPage};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    DOWNLOAD_OBSCURA_LATENCY, DOWNLOAD_OBSCURA_TIMEOUT,
};
#[cfg(feature = "otel-metrics")]
use std::time::Instant;

/// Memory budget for one Obscura subprocess (~30 MB).
const OBSCURA_MEMORY_COST: usize = 30_000_000;

/// Hard timeout per page fetch.
const OBSCURA_TIMEOUT: Duration = Duration::from_secs(15);

/// Subprocess-based downloader that shells out to `obscura fetch --dump markdown`.
///
/// No cookies, no connection pool — each invocation is independent.
pub struct ObscuraDownloader;

impl ObscuraDownloader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ObscuraDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for ObscuraDownloader {
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!("Obscura fetch: {}", url);

        #[cfg(feature = "otel-metrics")]
        let start = Instant::now();

        let url_string = url.to_string();

        let result = timeout(
            OBSCURA_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                Command::new("obscura")
                    .args(["fetch", "--dump", "markdown", &url_string])
                    .output()
            })
            .in_current_span(),
        )
        .await;

        match result {
            Ok(Ok(Ok(output))) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(DownloadError::Internal(format!(
                        "obscura exited with {}: {stderr}",
                        output.status
                    )));
                }

                let markdown = String::from_utf8_lossy(&output.stdout).to_string();

                debug!("Obscura returned {} bytes", markdown.len());

                #[cfg(feature = "otel-metrics")]
                DOWNLOAD_OBSCURA_LATENCY.record(start.elapsed().as_secs_f64(), &[]);

                Ok(FetchedPage {
                    url: url.clone(),
                    html: markdown,
                    status: 200,
                    cookies: vec![],
                })
            },
            Ok(Ok(Err(e))) => {
                #[cfg(feature = "otel-metrics")]
                DOWNLOAD_OBSCURA_LATENCY.record(start.elapsed().as_secs_f64(), &[]);
                Err(DownloadError::Internal(format!(
                    "obscura process failed to start: {e}"
                )))
            },
            Ok(Err(_)) => {
                #[cfg(feature = "otel-metrics")]
                {
                    DOWNLOAD_OBSCURA_LATENCY.record(start.elapsed().as_secs_f64(), &[]);
                    DOWNLOAD_OBSCURA_TIMEOUT.add(1, &[]);
                }
                Err(DownloadError::Timeout(OBSCURA_TIMEOUT.as_secs()))
            },
            Err(_) => {
                #[cfg(feature = "otel-metrics")]
                {
                    DOWNLOAD_OBSCURA_LATENCY.record(start.elapsed().as_secs_f64(), &[]);
                    DOWNLOAD_OBSCURA_TIMEOUT.add(1, &[]);
                }
                Err(DownloadError::Timeout(OBSCURA_TIMEOUT.as_secs()))
            },
        }
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        OBSCURA_MEMORY_COST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_obscura_downloader_basics() {
        let dl = ObscuraDownloader::new();
        assert!(!dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 30_000_000);
    }

    #[test]
    fn test_obscura_default() {
        let dl = ObscuraDownloader;
        assert_eq!(dl.memory_cost(), OBSCURA_MEMORY_COST);
    }
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    #[test]
    fn test_obscura_instruments_init() {
        let _ = &*super::DOWNLOAD_OBSCURA_LATENCY;
        let _ = &*super::DOWNLOAD_OBSCURA_TIMEOUT;
    }
}
