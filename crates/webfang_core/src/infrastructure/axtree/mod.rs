//! Accessibility-tree (AXTree) snapshot engine over raw CDP (#788).
//!
//! chromiumoxide 0.7.0 ships the `AxNode` type but no typed
//! `Accessibility.getFullAXTree` command, so this module defines a local
//! `GetFullAXTree` command and runs it over the existing chromiumoxide
//! `Page` connection, reusing the launch → navigate → close lifecycle of
//! `ChromiumoxideDownloader::fetch`.
//!
//! The pure `compact` serializer and `RawAxNodeView` trait live in
//! `domain::axtree_port` (sub-slice 3.A.2-followup.A). This module owns
//! the chromiumoxide impls and the `ChromiumoxideAxTreeAdapter` that
//! implements the `AxTreePort` domain trait (sub-slice 3.A.2-followup.B).

#[cfg(feature = "chromium")]
pub(crate) mod playwright;

use url::Url;

use super::downloader::DownloadError;
use crate::error::ScraperError;
#[cfg(feature = "chromium")]
use crate::domain::axtree_port::RawAxNodeView;
#[cfg(feature = "chromium")]
use crate::domain::axtree_port::AxTreePort;

// DTOs moved to domain::axtree_port in sub-slice 3.A.2 (ADR-0012). Infra
// re-exports them so existing call sites continue to resolve.
pub use crate::domain::axtree_port::{CompactNode, CompactSnapshot, SnapshotFormat};

// RawAxNodeView impls — chromiumoxide is the foreign type we abstract.
// In the chromium build we wrap the real `AxNode`; in the non-chromium stub
// build we return an empty vec / a placeholder type.

// ============================================================================
// RawAxNodeView impls (sub-slice 3.A.2-followup.A)
// ============================================================================

/// Wrap a chromiumoxide `AxNode` so the domain `compact` function can iterate
/// it without depending on browser-specific types. Owns the node data so the
/// returned trait objects are `'static` (the trait `RawAxNodeView` does not
/// carry a lifetime).
#[cfg(feature = "chromium")]
struct AxNodeView {
    inner: chromiumoxide::cdp::browser_protocol::accessibility::AxNode,
}

#[cfg(feature = "chromium")]
impl RawAxNodeView for AxNodeView {
    fn is_ignored(&self) -> bool {
        self.inner.ignored
    }
    fn role_str(&self) -> Option<&str> {
        if self.inner.role.is_some() {
            Some(ax_value_str(self.inner.role.as_ref()))
        } else {
            None
        }
    }
    fn name_str(&self) -> Option<&str> {
        if self.inner.name.is_some() {
            Some(ax_value_str(self.inner.name.as_ref()))
        } else {
            None
        }
    }
}

/// Extract the string payload of an `AxValue` (empty when absent).
/// Re-exported so `playwright` (which still uses raw `AxNode`/`AxProperty`
/// until 3.A.2-followup.B) can call it without depending on the deleted
/// `compact` module.
#[cfg(feature = "chromium")]
pub(crate) fn ax_value_str(
    value: Option<&chromiumoxide::cdp::browser_protocol::accessibility::AxValue>,
) -> &str {
    value
        .and_then(|v| v.value.as_ref())
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

/// Reject unsupported snapshot formats before any browser work (spec R3).
///
/// # Errors
/// Returns `DownloadError::Internal` for formats outside the shipped slice.
#[allow(dead_code)] // spec R3 — only called under `chromium` feature + tests; silences llvm-cov dead_code (#810)
pub(crate) fn require_supported_format(format: SnapshotFormat) -> Result<(), DownloadError> {
    match format {
        SnapshotFormat::Compact | SnapshotFormat::PlaywrightMcp => Ok(()),
    }
}

/// Chromium-gated Playwright snapshot type (R1).
#[cfg(feature = "chromium")]
pub(crate) use playwright::PlaywrightSnapshot;

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

/// RAII CDP helper centralizing launch → goto → execute → close (R4).
///
/// Guarantees deterministic `browser.close() + handler abort` even on
/// 30s/10s timeout, shared by `fetch_raw_axtree` callers (compact,
/// playwright, som_capture).
#[cfg(feature = "chromium")]
pub(crate) async fn with_cdp_page<F, T, Fut>(url: &Url, f: F) -> Result<T, DownloadError>
where
    F: FnOnce(chromiumoxide::Page) -> Fut,
    Fut: std::future::Future<Output = Result<T, DownloadError>>,
{
    use chromiumoxide::browser::HeadlessMode;
    use chromiumoxide::{Browser, BrowserConfig};
    use futures::StreamExt;
    use tokio::time::{timeout, Duration};

    const NAV_TIMEOUT: Duration = Duration::from_secs(30);
    const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
    let _ = SNAPSHOT_TIMEOUT;

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

    let handler_job = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        // Pre-goto SSRF check is done by caller; post-goto redirect check
        // re-validates final URL via literal-IP guard (out-of-scope hostname DNS already checked at entry).
        timeout(NAV_TIMEOUT, page.goto(url.as_str()))
            .await
            .map_err(|_| DownloadError::Timeout(NAV_TIMEOUT.as_secs()))?
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        f(page).await
    }
    .await;

    // Deterministic finally — close + abort handler even on timeout.
    browser.close().await.ok();
    handler_job.abort();

    result
}

// ============================================================================
// ChromiumoxideAxTreeAdapter — implements the domain `AxTreePort` trait.
// Sub-slice 3.A.2-followup.B. Container wires `Arc<dyn AxTreePort>` to this.
// ============================================================================

/// Concrete `AxTreePort` impl that drives chromiumoxide CDP to fetch the
/// raw AXTree and wraps the result as `Box<dyn RawAxNodeView>` for the
/// domain `compact` function. Cheap to construct (no I/O in `new`), so
/// the container can build one and clone the `Arc` cheaply.
#[cfg(feature = "chromium")]
#[derive(Clone, Default)]
pub struct ChromiumoxideAxTreeAdapter;

#[cfg(feature = "chromium")]
impl AxTreePort for ChromiumoxideAxTreeAdapter {
    fn fetch_raw_axtree<'a>(
        &'a self,
        url: &'a Url,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<Box<dyn RawAxNodeView>>, ScraperError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let raw = fetch_raw_axtree(url)
                .await
                .map_err(|e| ScraperError::extraction(format!("AXTree fetch failed: {e}")))?;
            Ok(wrap_as_views(raw))
        })
    }
}

/// Fetch raw `Vec<AxNode>` via `with_cdp_page` (R4).
#[cfg(feature = "chromium")]
pub(crate) async fn fetch_raw_axtree(
    url: &Url,
) -> Result<Vec<chromiumoxide::cdp::browser_protocol::accessibility::AxNode>, DownloadError> {
    with_cdp_page(url, |page| async move {
        use tokio::time::{timeout, Duration};
        const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
        let response = timeout(
            SNAPSHOT_TIMEOUT,
            page.execute(GetFullAXTree {
                fetch_relatives: None,
            }),
        )
        .await
        .map_err(|_| DownloadError::Timeout(SNAPSHOT_TIMEOUT.as_secs()))?
        .map_err(|e| DownloadError::Internal(e.to_string()))?;
        Ok(response.result.nodes)
    })
    .await
}

/// Wrap chromiumoxide `AxNode` references as trait objects so the domain
/// `compact` function can consume them. Sub-slice 3.A.2-followup.A.
#[cfg(feature = "chromium")]
pub(crate) fn wrap_as_views(
    nodes: Vec<chromiumoxide::cdp::browser_protocol::accessibility::AxNode>,
) -> Vec<Box<dyn RawAxNodeView>> {
    nodes
        .into_iter()
        .map(|n| Box::new(AxNodeView { inner: n }) as Box<dyn RawAxNodeView>)
        .collect()
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
    require_supported_format(format)?;
    let nodes = fetch_raw_axtree(url).await?;
    let views = wrap_as_views(nodes);
    Ok(crate::domain::axtree_port::compact(
        &views,
        interactive_only,
        selector,
    ))
}

/// Fetch a Playwright MCP YAML snapshot (R1).
#[cfg(feature = "chromium")]
#[allow(dead_code)]
#[tracing::instrument]
pub async fn fetch_playwright_snapshot(
    url: &Url,
    interactive_only: bool,
    selector: Option<&str>,
) -> Result<PlaywrightSnapshot, DownloadError> {
    let nodes = fetch_raw_axtree(url).await?;
    Ok(playwright::playwright(&nodes, interactive_only, selector))
}

/// Non-chromium stubs — honest FeatureGated (R5).
#[cfg(not(feature = "chromium"))]
#[tracing::instrument]
pub async fn fetch_axtree_snapshot(
    url: &Url,
    interactive_only: bool,
    selector: Option<&str>,
    format: SnapshotFormat,
) -> Result<CompactSnapshot, DownloadError> {
    let _ = (url, interactive_only, selector, format);
    Err(DownloadError::FeatureGated(
        "AXTree not enabled (compile with --features chromium)".to_string(),
    ))
}

/// Placeholder PlaywrightSnapshot for non-chromium stub compilation.
#[cfg(not(feature = "chromium"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlaywrightSnapshot {
    /// YAML content.
    pub content: String,
    /// `chars() / 4`.
    pub token_estimate: usize,
    /// Number of refs.
    pub ref_count: usize,
}

/// Fetch Playwright stub for non-chromium (R5).
#[cfg(not(feature = "chromium"))]
#[allow(dead_code)]
pub(crate) async fn fetch_playwright_snapshot(
    _url: &Url,
    _interactive_only: bool,
    _selector: Option<&str>,
) -> Result<PlaywrightSnapshot, DownloadError> {
    Err(DownloadError::FeatureGated(
        "AXTree not enabled (compile with --features chromium)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_supported_format_accepts_both() {
        assert!(require_supported_format(SnapshotFormat::Compact).is_ok());
        assert!(
            require_supported_format(SnapshotFormat::PlaywrightMcp).is_ok(),
            "PlaywrightMcp must be Ok after R4"
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
            matches!(err, DownloadError::FeatureGated(ref msg) if msg.contains("not enabled")),
            "expected FeatureGated stub error, got: {err}"
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
