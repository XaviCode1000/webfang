//! Obscura subprocess downloader.
//!
//! Wraps the `obscura` CLI tool via `std::process::Command`, executed inside
//! `tokio::task::spawn_blocking` to avoid blocking the async runtime.
//! Returns raw markdown output for downstream processing.

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::time::timeout;
use tracing::{debug, instrument, Instrument};
use url::Url;

use super::{DownloadError, Downloader, FetchedPage};

/// Memory budget for one Obscura subprocess (~30 MB).
const OBSCURA_MEMORY_COST: usize = 30_000_000;

/// Default timeout per page fetch.
const DEFAULT_OBSCURA_TIMEOUT: Duration = Duration::from_secs(15);

/// Subprocess-based downloader that shells out to `obscura fetch --dump markdown`.
///
/// No cookies, no connection pool — each invocation is independent.
pub struct ObscuraDownloader {
    timeout: Duration,
}

impl ObscuraDownloader {
    pub(crate) fn new(timeout_secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs),
        }
    }
}

impl Default for ObscuraDownloader {
    fn default() -> Self {
        Self::new(DEFAULT_OBSCURA_TIMEOUT.as_secs())
    }
}

impl ObscuraDownloader {
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!("Obscura fetch: {}", url);

        let url_string = url.to_string();

        // Defense in depth: a typed Url always carries a scheme (e.g. https://) so it can
        // never be mistaken for an obscura flag; reject defensively anyway.
        if url_string.starts_with('-') {
            return Err(DownloadError::Internal(
                "URL inválida: no puede comenzar con '-'".to_string(),
            ));
        }

        let result = timeout(
            self.timeout,
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

                Ok(FetchedPage {
                    url: url.clone(),
                    html: markdown,
                    status: 200,
                    headers: HashMap::new(),
                    cookies: vec![],
                })
            },
            Ok(Ok(Err(e))) => Err(DownloadError::Internal(format!(
                "obscura process failed to start: {e}"
            ))),
            Ok(Err(_)) => Err(DownloadError::Timeout(self.timeout.as_secs())),
            Err(_) => Err(DownloadError::Timeout(self.timeout.as_secs())),
        }
    }
}

impl Downloader for ObscuraDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(self.fetch_inner(url))
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
        let dl = ObscuraDownloader::new(15);
        assert!(!dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 30_000_000);
    }

    #[test]
    fn test_obscura_default() {
        let dl = ObscuraDownloader::default();
        assert_eq!(dl.memory_cost(), OBSCURA_MEMORY_COST);
    }
}
