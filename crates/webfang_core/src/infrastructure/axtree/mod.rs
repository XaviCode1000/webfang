//! Accessibility-tree (AXTree) snapshot engine over raw CDP (#788).
//!
//! chromiumoxide 0.7.0 ships the `AxNode` type but no typed
//! `Accessibility.getFullAXTree` command, so this module defines a local
//! `GetFullAXTree` command and runs it over the existing chromiumoxide
//! `Page` connection, reusing the launch → navigate → close lifecycle of
//! `ChromiumoxideDownloader::fetch`.
//!
//! The pure compact serializer lives in `compact` (chromium-gated because it
//! borrows the crate `AxNode` type); the snapshot/format types here are
//! feature-agnostic so the non-chromium stub keeps an identical signature.

#[cfg(feature = "chromium")]
mod compact;

use url::Url;

use super::downloader::DownloadError;

/// Snapshot serialization formats (spec R3).
///
/// Only [`Compact`](Self::Compact) is implemented in this slice; `playwright-mcp`
/// returns an honest unsupported error (ai.rs precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum SnapshotFormat {
    /// Interactive-only `@eN`-referenced node list with a `token_estimate`.
    #[default]
    Compact,
    /// Playwright MCP AXSnapshot format — deferred to a follow-up change.
    PlaywrightMcp,
}

/// A single compact node: `@eN` ref, accessible name, and role.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CompactNode {
    /// Snapshot-scoped reference (`@e1`, `@e2`, …) — valid ONLY within the
    /// snapshot that created it (RDD causal invariant).
    #[serde(rename = "ref")]
    pub r#ref: String,
    /// Accessible name.
    pub name: String,
    /// Accessible role.
    pub role: String,
}

/// Compact accessibility snapshot — interactive nodes plus a token estimate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CompactSnapshot {
    /// Emitted nodes, each with a snapshot-scoped `@eN` ref.
    pub nodes: Vec<CompactNode>,
    /// `Σ(2 + name_chars/4 + role_chars/4)` over the emitted nodes.
    pub token_estimate: usize,
}

/// Reject unsupported snapshot formats before any browser work (spec R3).
///
/// # Errors
/// Returns `DownloadError::Internal` for formats outside the shipped slice.
pub(crate) fn require_supported_format(format: SnapshotFormat) -> Result<(), DownloadError> {
    match format {
        SnapshotFormat::Compact => Ok(()),
        SnapshotFormat::PlaywrightMcp => Err(DownloadError::Internal(
            "playwright-mcp format is not implemented (use compact)".to_string(),
        )),
    }
}

/// Raw CDP `Accessibility.getFullAXTree` command (chromiumoxide 0.7.0 has no
/// typed variant — see `chromiumoxide_cdp/src/cdp.rs`).
#[cfg(feature = "chromium")]
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct GetFullAXTree {
    /// When true, also fetch descendants' relatives for each node.
    #[serde(rename = "fetchRelatives")]
    #[serde(skip_serializing_if = "Option::is_none")]
    fetch_relatives: Option<bool>,
}

#[cfg(feature = "chromium")]
impl GetFullAXTree {
    /// CDP method identifier.
    pub(crate) const IDENTIFIER: &'static str = "Accessibility.getFullAXTree";
}

#[cfg(feature = "chromium")]
impl chromiumoxide::types::Method for GetFullAXTree {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        Self::IDENTIFIER.into()
    }
}

/// Response envelope for [`GetFullAXTree`] — reuses the crate `AxNode` type
/// (no new member types, spec R1 edge scenario).
#[cfg(feature = "chromium")]
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub(crate) struct GetFullAXTreeResponse {
    /// The full accessibility tree of the current page.
    nodes: Vec<chromiumoxide::cdp::browser_protocol::accessibility::AxNode>,
}

#[cfg(feature = "chromium")]
impl chromiumoxide::types::Command for GetFullAXTree {
    type Response = GetFullAXTreeResponse;
}

/// Fetch a compact accessibility snapshot for `url` via a fresh headless
/// Chromium session (CDP `Accessibility.getFullAXTree`).
///
/// Mirrors the launch → navigate → execute → close lifecycle of
/// `ChromiumoxideDownloader::fetch`; sessions are not reused between calls.
///
/// # Errors
/// Returns [`DownloadError`] on launch/navigation/CDP failure or timeout.
#[cfg(feature = "chromium")]
#[tracing::instrument]
pub async fn fetch_axtree_snapshot(
    url: &Url,
    interactive_only: bool,
    selector: Option<&str>,
    format: SnapshotFormat,
) -> Result<CompactSnapshot, DownloadError> {
    use chromiumoxide::browser::HeadlessMode;
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt;
    use tokio::time::{timeout, Duration};

    const NAV_TIMEOUT: Duration = Duration::from_secs(30);
    const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

    require_supported_format(format)?;

    if !url.scheme().starts_with("http") {
        return Err(DownloadError::InvalidUrl(format!(
            "unsupported scheme: {}",
            url.scheme()
        )));
    }

    let config = BrowserConfig::builder()
        .headless_mode(HeadlessMode::True)
        .no_sandbox()
        .build()
        // LCOV_EXCL_LINE defensive: browser-config-build — static builder flags cannot fail at runtime
        .map_err(DownloadError::Internal)?;

    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| DownloadError::Internal(format!("Chrome launch failed: {e}")))?;

    // Process CDP messages in an isolated task to prevent hangs.
    let handler_job = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|e| DownloadError::Internal(e.to_string()))?;

    timeout(NAV_TIMEOUT, page.goto(url.as_str()))
        .await
        .map_err(|_| DownloadError::Timeout(NAV_TIMEOUT.as_secs()))?
        .map_err(|e| DownloadError::Internal(e.to_string()))?;

    let response = timeout(
        SNAPSHOT_TIMEOUT,
        page.execute(GetFullAXTree {
            fetch_relatives: None,
        }),
    )
    .await
    .map_err(|_| DownloadError::Timeout(SNAPSHOT_TIMEOUT.as_secs()))?
    .map_err(|e| DownloadError::Internal(e.to_string()))?;

    // Deterministic shutdown — prevents zombie processes.
    browser.close().await.ok();
    handler_job.await.ok();

    Ok(compact::compact(
        &response.result.nodes,
        interactive_only,
        selector,
    ))
}

/// Non-chromium stub — identical signature, honest "not enabled" error
/// (mirrors `chromiumoxide_downloader.rs`).
#[cfg(not(feature = "chromium"))]
#[tracing::instrument]
pub async fn fetch_axtree_snapshot(
    url: &Url,
    interactive_only: bool,
    selector: Option<&str>,
    format: SnapshotFormat,
) -> Result<CompactSnapshot, DownloadError> {
    let _ = (url, interactive_only, selector, format);
    Err(DownloadError::Internal(
        "AXTree not enabled (compile with --features chromium)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_supported_format_rejects_playwright_mcp() {
        assert!(require_supported_format(SnapshotFormat::Compact).is_ok());
        let err = require_supported_format(SnapshotFormat::PlaywrightMcp).unwrap_err();
        assert!(
            matches!(err, DownloadError::Internal(ref msg) if msg.contains("playwright-mcp")),
            "expected unsupported-format error, got: {err}"
        );
    }

    #[cfg(not(feature = "chromium"))]
    #[tokio::test]
    async fn fetch_axtree_snapshot_stub_returns_not_enabled() {
        let url: Url = "https://example.com".parse().unwrap();
        let err = fetch_axtree_snapshot(&url, true, None, SnapshotFormat::Compact)
            .await
            .unwrap_err();
        assert!(
            matches!(err, DownloadError::Internal(ref msg) if msg.contains("not enabled")),
            "expected stub error, got: {err}"
        );
    }

    #[cfg(feature = "chromium")]
    #[test]
    fn get_full_axtree_serializes_with_skip_when_none() {
        let cmd = GetFullAXTree {
            fetch_relatives: None,
        };
        assert_eq!(
            serde_json::to_value(&cmd).unwrap(),
            serde_json::json!({}),
            "fetch_relatives must be skipped when None"
        );
        let cmd = GetFullAXTree {
            fetch_relatives: Some(true),
        };
        assert_eq!(
            serde_json::to_value(&cmd).unwrap(),
            serde_json::json!({ "fetchRelatives": true })
        );
    }

    #[cfg(feature = "chromium")]
    #[test]
    fn get_full_axtree_identifier_is_accessibility_method() {
        use chromiumoxide::types::Method as _;
        let cmd = GetFullAXTree {
            fetch_relatives: None,
        };
        assert_eq!(cmd.identifier(), "Accessibility.getFullAXTree");
    }
}
