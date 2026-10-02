//! Shared `#[cfg(test)]` support for the per-handler unit-test modules.
//!
//! Every handler's inline `mod tests` needs the same trio: a `Container` over
//! a temp output dir, an `McpHandler` around a default `McpState`, and the
//! text of the first content block of a `CallToolResult`. They live here so
//! each handler does not carry its own copy (duplication ratchet, issue #516;
//! consolidated while landing the #1613 reason-code contract).

use rmcp::model::CallToolResult;
use tempfile::TempDir;
use webfang_core::di::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;

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
