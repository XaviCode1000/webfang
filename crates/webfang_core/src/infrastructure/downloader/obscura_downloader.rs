//! Obscura subprocess downloader.
//!
//! Wraps the `obscura` CLI tool via `std::process::Command`, executed inside
//! `tokio::task::spawn_blocking` to avoid blocking the async runtime.
//! Returns **HTML** output (`fetch --dump html`, #793) so Readability,
//! CSS-selector extraction, and WAF inspection receive the format they expect;
//! the returned [`FetchedPage`] carries a `content-type: text/html` header
//! marker so downstream sniffing stays honest.
//!
//! The binary is configurable through `ObscuraDownloader::new`
//! (#787): a value carrying a path separator is invoked exactly as given
//! (absolute or relative path); a bare name is resolved from `PATH` by the
//! OS. The default is the bare name `obscura`. Hybrid preflight enforces a
//! minimum binary version of 0.2.0 (REQ-OBS-02, cli/preflight.rs).
//!
//! **Sessionless contract (REQ-OBS-03):** Layer 2 injects no cookies and keeps
//! no storage directory (`--storage-dir` is deliberately not wired — see
//! sdd/793-obscura-l2-contract/design.md §4). Pages behind a session are
//! expected to render empty here and escalate to Layer 3 (Chromiumoxide),
//! which injects [`super::cookie_bridge::CookieBridge`] cookies by domain.

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

/// Synthetic content-type for Layer 2 pages (#793): a subprocess downloader
/// has no real response headers, so we mark what obscura was asked to dump.
/// The lowercase key + `text/html` mime are the shape
/// `InspectionContext::from_lowercase_headers` and the WAF engine's
/// `is_html_content_type` consume.
const OBSCURA_CONTENT_TYPE: &str = "text/html; charset=utf-8";

fn obscura_html_headers() -> HashMap<String, String> {
    HashMap::from([("content-type".to_string(), OBSCURA_CONTENT_TYPE.to_string())])
}

/// Subprocess-based downloader that shells out to `obscura fetch --dump html`
/// (#793 — was markdown; the downstream pipeline is HTML-only).
///
/// No cookies, no connection pool — each invocation is independent and
/// sessionless (REQ-OBS-03).
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
                    .args(["fetch", "--dump", "html", &url_string])
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

                let html = String::from_utf8_lossy(&output.stdout).to_string();

                debug!(dump = "html", "Obscura returned {} bytes", html.len());

                Ok(FetchedPage {
                    url: url.clone(),
                    html,
                    status: 200,
                    headers: obscura_html_headers(),
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

    // ---- #793 — fake obscura executable --------------------------------

    const FAKE_HTML: &str = "<!DOCTYPE html><html><body><article>\
         <h1>Obscura Article</h1>\
         <p>Rendered body text long enough for Readability to keep the article.</p>\
         </article></body></html>";

    /// Write an executable fake `obscura` that records its argv to
    /// `<dir>/args.txt` per `fetch` invocation and prints fixed HTML on
    /// stdout (#793). Deterministic: no network, no real obscura.
    #[cfg(unix)]
    fn write_fake_obscura(dir: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin_path = dir.join("obscura");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"fetch\" ]; then\n  \
             echo \"$@\" >> \"{dir}/args.txt\"\n  \
             printf '%s' '{body}'\nelse\n  echo \"obscura 0.2.0\"\nfi\n",
            dir = dir.display(),
            body = body.replace('\'', "'\\''"),
        );
        std::fs::write(&bin_path, script).expect("write fake obscura");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x fake obscura");
        bin_path
    }

    /// #793 (REQ-OBS-01): Layer 2 must ask obscura for `--dump html` —
    /// never markdown — and the HTML printed on stdout lands verbatim in
    /// `FetchedPage.html`.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_dump_html_args_and_page() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML);

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com/page".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

        assert!(
            page.html.contains("<h1>Obscura Article</h1>"),
            "FetchedPage.html must carry the HTML obscura printed on stdout, got: {}",
            page.html
        );
        assert_eq!(page.status, 200);

        let args = std::fs::read_to_string(tmp.path().join("args.txt"))
            .expect("fake obscura must have recorded its argv");
        assert!(
            args.contains("--dump html"),
            "Layer 2 must request `fetch --dump html`, got argv: {args}"
        );
        assert!(
            !args.contains("markdown"),
            "Layer 2 must never request markdown, got argv: {args}"
        );
        assert!(
            args.contains("https://example.com/page"),
            "the fetched URL must reach obscura, got argv: {args}"
        );
    }

    /// #793 (REQ-OBS-01 S1.3 / REQ-OBS-03): the returned page is self-
    /// describing — a `text/html` content-type marker in the otherwise-empty
    /// header map, and no session cookies.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_page_headers_mark_html_and_stateless() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML);

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

        assert_eq!(
            page.headers.get("content-type").map(String::as_str),
            Some("text/html; charset=utf-8"),
            "Layer 2 must mark its output as text/html for downstream sniffing"
        );
        assert!(
            page.cookies.is_empty(),
            "Layer 2 is sessionless — it never returns cookies"
        );
    }

    /// #793 (REQ-OBS-01 S1.2): the HTML returned by Layer 2 must survive the
    /// real downstream extraction path — CSS selector matching AND
    /// Readability — the exact consumers that broke on markdown output.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_html_survives_extraction_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_fake_obscura(tmp.path(), FAKE_HTML);

        let dl = ObscuraDownloader::new(15, &bin_path);
        let url: Url = "https://example.com".parse().expect("valid test URL");
        let page = dl.fetch(&url).await.expect("fake obscura fetch");

        match crate::application::extraction::extract_with_selector(&page.html, "article", None) {
            crate::domain::ExtractResult::Matched(html) => {
                assert!(
                    html.contains("Obscura Article"),
                    "selector extraction must find the article in the dumped HTML"
                );
            },
            other => panic!("`article` must match in the dumped HTML, got: {other:?}"),
        }

        let article = crate::infrastructure::scraper::readability::parse(
            &page.html,
            Some("https://example.com"),
        )
        .expect("Readability must parse the dumped HTML");
        assert!(
            article.content.contains("Obscura Article"),
            "Readability content must retain the article body"
        );
        assert!(
            article.text_content.contains("Rendered body text"),
            "Readability text extraction must retain visible text"
        );
    }
}
