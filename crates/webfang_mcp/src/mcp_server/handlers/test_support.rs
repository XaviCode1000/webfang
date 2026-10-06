//! Shared `#[cfg(test)]` support for the `mcp_server` unit-test modules.
//!
//! Every handler's inline `mod tests` needs the same trio: a `Container` over
//! a temp output dir, an `McpHandler` around a default `McpState`, and the
//! text of the first content block of a `CallToolResult`. They live here so
//! each handler does not carry its own copy (duplication ratchet, issue #516;
//! consolidated while landing the #1613 reason-code contract).
//!
//! The tracing-capture helpers ([`SharedBufWriter`],
//! [`ensure_global_subscriber`]) are shared by the whole `mcp_server` test
//! surface, not only by the handler ones: `mcp_server/auth.rs` captures a
//! `tracing::warn!` too (#1778). `ensure_global_subscriber` exists as ONE copy
//! for the same reason as the trio — it is the #417/#664/#1638 pattern, and
//! the duplication ratchet (`scripts/check_duplication.sh`, jscpd
//! `--min-tokens 50`, actively descending under #1757) counts each additional
//! verbatim copy as a regression.

//! The robots-harness fixture
//! ([`test_handler_with_robots_and_no_ssrf`]) extends the same idea to the
//! #749 enforcement tests: the both-hatch [`EnvGuard`](webfang_test_utils::EnvGuard),
//! a handler with a real robots fetcher, and the wiremock `/robots.txt`
//! route, so each robots test does not carry its own copy (issue #1885).

use rmcp::model::CallToolResult;
use tempfile::TempDir;
use webfang_core::di::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;
use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;
use webfang_test_utils::EnvGuard;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::McpHandler;
use crate::mcp_server::state::McpState;

/// Build the canonical test [`Container`] over a temp output dir.
///
/// Handlers that need a customized `McpState` (hermetic Obsidian detection,
/// export roots, robots fetcher, ...) call this and then apply their builders,
/// instead of re-declaring the config construction.
pub(crate) async fn container(tmp: &TempDir) -> Container {
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://example.com").expect("valid url"));
    let scraper_config = ScraperConfig {
        output_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    Container::new(crawler_config, scraper_config)
        .await
        .expect("create container")
}

/// A handler over the default state built on [`container`], plus the temp dir
/// (which the caller must keep alive: it backs the configured output dir).
pub(crate) async fn test_handler() -> (McpHandler, TempDir) {
    let tmp = TempDir::new().expect("create temp dir");
    let state = McpState::new(container(&tmp).await);
    (McpHandler::new(state), tmp)
}

/// Shared async fixture for the #749 robots-enforcement tests: both SSRF
/// hatches off, a handler with a real robots fetcher, and a `MockServer`
/// serving the canonical `/robots.txt` (`Disallow: /private`).
///
/// This is the setup trio every robots test repeats — the both-hatch guard,
/// the robots-fetcher handler, and the wiremock robots route (ai, scraping,
/// and export handler tests, issue #1885). Construction never touches the
/// network, so the fixture stays offline-friendly like the handlers it
/// builds on. Tests that need more routes (e.g. a robots-allowed page)
/// mount them on the returned server themselves.
///
/// The guard uses [`EnvGuard::wiremock_robots`], the canonical constructor
/// for the robots chain (#1329): both hatches set to the exact `"1"` the
/// guards demand — the same envs and the same values as the ad-hoc form it
/// replaces — and both restored on drop.
pub(crate) async fn test_handler_with_robots_and_no_ssrf(
) -> (McpHandler, TempDir, EnvGuard, MockServer) {
    let guard = EnvGuard::wiremock_robots();
    let tmp = TempDir::new().expect("create temp dir");
    let state = McpState::new(container(&tmp).await).with_robots_fetcher(std::sync::Arc::new(
        RobotsFetcher::with_default_profile(5).expect("fetcher construction is offline"),
    ));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private\n"),
        )
        .mount(&server)
        .await;
    (McpHandler::new(state), tmp, guard, server)
}

/// The text of the first content block of a tool result, or an empty string
/// when the result carries no content array (callers assert on the text).
pub(crate) fn result_text(result: &CallToolResult) -> String {
    serde_json::to_value(result)
        .ok()
        .and_then(|v| v.get("content").and_then(|c| c.as_array()).cloned())
        .and_then(|arr| arr.first().cloned())
        .and_then(|first| {
            first
                .get("text")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Shared capture sink behind a test tracing subscriber, so a span can be
/// READ rather than reviewed (#1615 DF-L2). Was a per-file copy in the
/// `content` and `security` handler tests.
#[derive(Clone)]
pub(crate) struct SharedBufWriter(pub(crate) std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedBufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture buffer lock is never poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Installs a process-global sink subscriber so no callsite in this binary can
/// be poisoned with `Interest::never()` (issues #417, #664, #1638, #1778).
///
/// `tracing` caches per-callsite `Interest` process-wide through a one-time
/// compare-exchange. A sibling test that reaches a callsite with no subscriber
/// — e.g. an ordinary `status_of(..).await` assertion on the same rejection
/// path — makes `Dispatch::none()` register `Interest::never()`, permanently
/// disabling that callsite for every other thread. Under libtest (one process,
/// many threads) a capture test then observes an empty buffer and passes
/// vacuously; under nextest every test is its own process, which is why the
/// flake only ever surfaces in the libtest-based `Coverage` lane.
///
/// Setting a *global* default rebuilds the cached interest of every
/// already-registered callsite, so calling this once at the top of a capture
/// test is enough to undo a poison that already happened. Output goes to
/// `io::sink`: this subscriber exists only to make the dispatch non-none,
/// never to be read — the per-test capture is scoped separately.
pub(crate) fn ensure_global_subscriber() {
    static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();
    GLOBAL_SUBSCRIBER_INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish(),
        );
    });
}
