//! Obscura subprocess downloader.
//!
//! Wraps the `obscura` CLI tool via `std::process::Command`, executed inside
//! `tokio::task::spawn_blocking` to avoid blocking the async runtime.
//! Returns raw markdown output for downstream processing.
//!
//! The binary is configurable through [`ObscuraDownloader::new`]
//! (#787): a value carrying a path separator is invoked exactly as given
//! (absolute or relative path); a bare name is resolved from `PATH` by the
//! OS. The default is the bare name `obscura`.

use std::collections::HashMap;
use std::path::PathBuf;
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

/// Default binary name, resolved from `PATH` at spawn time.
pub const DEFAULT_OBSCURA_BINARY: &str = "obscura";

/// Subprocess-based downloader that shells out to `obscura fetch --dump markdown`.
///
/// No cookies, no connection pool — each invocation is independent.
pub struct ObscuraDownloader {
    timeout: Duration,
    binary: PathBuf,
}

impl ObscuraDownloader {
    /// Build a downloader running `binary` for every fetch (#787).
    ///
    /// `binary` is either the name of an executable resolved from `PATH`
    /// (e.g. `"obscura"`) or an explicit path invoked as-is. An empty value
    /// falls back to [`DEFAULT_OBSCURA_BINARY`] so `Command::new("")` never
    /// reaches the OS.
    pub(crate) fn new(timeout_secs: u64, binary: impl Into<PathBuf>) -> Self {
        let binary = binary.into();
        let binary = if binary.as_os_str().is_empty() {
            PathBuf::from(DEFAULT_OBSCURA_BINARY)
        } else {
            binary
        };
        Self {
            timeout: Duration::from_secs(timeout_secs),
            binary,
        }
    }

    /// The configured obscura binary (path or bare name) — test accessor.
    #[cfg(test)]
    pub(crate) fn binary(&self) -> &std::path::Path {
        &self.binary
    }
}

impl Default for ObscuraDownloader {
    fn default() -> Self {
        Self::new(DEFAULT_OBSCURA_TIMEOUT.as_secs(), DEFAULT_OBSCURA_BINARY)
    }
}

impl ObscuraDownloader {
    #[instrument(skip(self), fields(url = %url, binary = %self.binary.display()))]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!(
            binary = %self.binary.display(),
            "Obscura fetch: {url}"
        );

        let url_string = url.to_string();
        let binary = self.binary.clone();

        // Defense in depth: a typed Url always carries a scheme (e.g. https://) so it can
        // never be mistaken for an obscura flag; reject defensively anyway.
        // LCOV_EXCL_START defensive: defense-in-depth — a typed Url always carries a scheme, never a leading '-'
        if url_string.starts_with('-') {
            return Err(DownloadError::Internal(
                "URL inválida: no puede comenzar con '-'".to_string(),
            ));
        }
        // LCOV_EXCL_STOP

        let result = timeout(
            self.timeout,
            tokio::task::spawn_blocking(move || {
                Command::new(&binary)
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
        let dl = ObscuraDownloader::new(15, "obscura");
        assert!(!dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 30_000_000);
    }

    #[test]
    fn test_obscura_default() {
        let dl = ObscuraDownloader::default();
        assert_eq!(dl.memory_cost(), OBSCURA_MEMORY_COST);
        assert_eq!(dl.binary(), PathBuf::from(DEFAULT_OBSCURA_BINARY).as_path());
    }

    /// #787: the configured binary is stored and used at spawn time — a
    /// custom path is not silently replaced by the bare `PATH` name.
    #[test]
    fn test_obscura_stores_configured_binary() {
        let dl = ObscuraDownloader::new(15, "/opt/tools/obscura");
        assert_eq!(dl.binary(), PathBuf::from("/opt/tools/obscura").as_path());
    }

    /// #787: an empty binary value falls back to the default bare name so
    /// `Command::new("")` never reaches the OS.
    #[test]
    fn test_obscura_empty_binary_falls_back_to_default() {
        let dl = ObscuraDownloader::new(15, "");
        assert_eq!(dl.binary(), PathBuf::from(DEFAULT_OBSCURA_BINARY).as_path());
    }

    /// #787: a configured missing binary surfaces as a spawn failure, never
    /// as a fallback to whatever `obscura` happens to be on PATH.
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
    #[tokio::test]
    async fn test_obscura_missing_binary_fails_fetch() {
        let dl = ObscuraDownloader::new(5, "/definitely/not/here/obscura");
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let err = dl
            .fetch(&url)
            .await
            .expect_err("a missing obscura binary must fail the fetch");
        match err {
            DownloadError::Internal(msg) => {
                assert!(
                    msg.contains("failed to start"),
                    "error must name the spawn failure, got: {msg}"
                );
            },
            other => panic!("expected spawn Internal error, got: {other:?}"),
        }
    }

    /// #787: a bare-name binary that is not on PATH also fails at spawn —
    /// no binary resolution happens beyond what the OS provides.
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
    #[tokio::test]
    async fn test_obscura_bare_name_missing_from_path_fails_fetch() {
        let dl = ObscuraDownloader::new(5, "definitely-not-a-binary-7f3k");
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let result = dl
            .fetch(&url)
            .await
            .expect_err("a binary missing from PATH must fail the fetch");
        assert!(
            matches!(result, DownloadError::Internal(_)),
            "expected a spawn failure, got: {result:?}"
        );
    }
}
