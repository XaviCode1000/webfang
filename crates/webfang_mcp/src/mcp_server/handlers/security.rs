//! Security & Diagnostics tools — 4 tools for WAF detection and metrics
//!
//! Tools: detect_waf, verify_waf_integrity, list_waf_providers,
//! get_scrape_metrics

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;
use webfang_core::infrastructure::http::waf_engine::{InspectionContext, WafInspector, WafVerdict};

#[tool_router(router = tool_router_security, vis = "pub")]
impl McpHandler {
    /// Detect WAF/CAPTCHA challenge in HTML body
    #[tool(
        description = "Scan HTML body for WAF/CAPTCHA signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai, etc.). Runs in degraded mode (no HTTP context): reports only unambiguous challenge markers — vendor fingerprints need status via verify_waf_integrity. Returns provider name if detected."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn detect_waf(
        &self,
        Parameters(params): Parameters<DetectWafParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, security);

        match detect_waf_provider(&params.html) {
            Some(provider) => Ok(CallToolResult::success(vec![Content::text(format!(
                "WAF detected: {provider}"
            ))])),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no WAF detected",
            )])),
        }
    }

    /// Multi-layer WAF inspection (headers + body + entropy analysis)
    #[tool(
        description = "Multi-layer WAF inspection: checks control headers, body signatures via Aho-Corasick, and entropy analysis for silent challenges. Optionally pass status and content_type for context-aware detection (fingerprint evidence then blocks only on correlated WAF statuses 403/429/503/520-529); without them, runs degraded mode where only unambiguous challenge markers block and fingerprint/control-header evidence never blocks on mere presence. The status/content_type params are additive (tool signature backward compatible); control-header verdict semantics intentionally changed per issue #346 — mere-presence blocking was the bug."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn verify_waf_integrity(
        &self,
        Parameters(params): Parameters<VerifyWafIntegrityParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, security);

        let html = params.html.as_deref().unwrap_or("");
        let mut header_map = wreq::header::HeaderMap::new();
        if let Some(ref hdrs) = params.headers {
            for (key, value) in hdrs {
                if let (Ok(name), Ok(val)) = (
                    wreq::header::HeaderName::from_bytes(key.as_bytes()),
                    wreq::header::HeaderValue::from_str(value),
                ) {
                    header_map.insert(name, val);
                }
            }
        }
        // Additive optional context (REQ-WAF-09): the status/content_type params
        // are backward compatible (tool signature unchanged). The verdict is NOT
        // unchanged, though — without them this is degraded mode, where control
        // header (Fingerprint) evidence never blocks on mere presence. That is the
        // intentional #346 / REQ-WAF-05 fix (mere-presence blocking was the bug),
        // so degraded verdicts deliberately differ from the pre-#346 verify_integrity.
        let verdict = verify_waf_verdict(html, params.status, params.content_type, header_map);
        if verdict.is_blocked {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "WAF blocked: {}",
                verdict.evidence_chain()
            ))]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(
                "WAF integrity check passed",
            )]))
        }
    }

    /// List all supported WAF providers
    #[tool(
        description = "List all WAF/CAPTCHA providers that can be detected by the WAF inspector."
    )]
    #[instrument(skip(self))]
    async fn list_waf_providers(&self) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, security);

        let providers =
            webfang_core::infrastructure::http::waf_engine::WafInspector::supported_providers();
        Ok(CallToolResult::success(vec![Content::text(
            providers.join(", "),
        )]))
    }

    /// Get scrape metrics (request timing, status codes, pages scraped)
    #[tool(
        description = "Get scraping metrics including request timing, status code distribution, and pages scraped per domain."
    )]
    #[instrument(skip(self))]
    async fn get_scrape_metrics(&self) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, security);

        let metrics = serde_json::json!({
            "message": "Metrics collection requires active scraping session",
            "status": "available"
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&metrics).unwrap(),
        )]))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_security()
}

/// Inspect a body in degraded mode for the MCP `detect_waf` tool (REQ-WAF-09).
///
/// Returns the first evidence's provider when the degraded verdict blocks.
/// Without HTTP context only [`WafTier::Challenge`](WafInspector) markers block;
/// fingerprint evidence is collected but never blocks here — this is the
/// false-positive fix (#346).
fn detect_waf_provider(html: &str) -> Option<&'static str> {
    let verdict = WafInspector::inspect(html, &InspectionContext::default());
    if verdict.is_blocked {
        verdict.evidences.first().map(|e| e.provider)
    } else {
        None
    }
}

/// Run the `verify_waf_integrity` inspection (REQ-WAF-09).
///
/// `status` and `content_type` are additive optional context, so the API shape
/// (tool signature) is backward compatible. The verdict semantics are NOT
/// unchanged, though: when both are absent the inspection runs degraded mode,
/// where only Challenge-tier (T1) markers block and control-header / fingerprint
/// evidence never blocks on mere presence. That is the intentional fix for
/// issue #346 / REQ-WAF-05 — mere-presence blocking of control headers was the
/// bug — so degraded-mode verdicts deliberately differ from the pre-#346
/// `verify_integrity`.
fn verify_waf_verdict(
    html: &str,
    status: Option<u16>,
    content_type: Option<String>,
    headers: wreq::header::HeaderMap,
) -> WafVerdict {
    let ctx = InspectionContext {
        status,
        content_type,
        headers,
        ignore_waf: false,
    };
    WafInspector::inspect(html, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use webfang_core::infrastructure::http::waf_engine::WafTier;

    // ========================================================================
    // TASK-12 — detect_waf degraded mode (REQ-WAF-09)
    // ========================================================================

    #[test]
    fn detect_waf_provider_challenge_t1_is_detected() {
        // Degraded mode: a Challenge-tier (T1) marker blocks even without context.
        let html = r#"<div id="cf-turnstile" data-sitekey="abc"></div>"#;
        assert_eq!(detect_waf_provider(html), Some("Cloudflare Turnstile"));
    }

    #[test]
    fn detect_waf_provider_fingerprint_t2_never_blocks_degraded() {
        // Degraded mode: a bare vendor name (T2) is evidence only and never
        // blocks without a correlated WAF status — the false-positive fix.
        let html = r#"<html><body>powered by cloudflare</body></html>"#;
        assert_eq!(detect_waf_provider(html), None);
    }

    #[test]
    fn detect_waf_provider_clean_body_is_none() {
        assert_eq!(
            detect_waf_provider("<html><body>normal</body></html>"),
            None
        );
    }

    // ========================================================================
    // TASK-12 — verify_waf_integrity additive context (REQ-WAF-09)
    // ========================================================================

    #[test]
    fn verify_waf_degraded_mode_does_not_block_on_control_headers_without_status() {
        // REL-01: the name states exactly what is pinned. No status / content-type
        // → degraded mode, where a T2 control header alone never blocks on mere
        // presence. This is the intentional #346 / REQ-WAF-05 verdict change (the
        // pre-#346 verify_integrity DID block on mere presence — that was the bug);
        // only the additive optional params are backward compatible, not verdicts.
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert("x-datadome-response", "1".parse().unwrap());
        let verdict = verify_waf_verdict("<html>clean</html>", None, None, headers);
        assert!(
            !verdict.is_blocked,
            "T2 header alone must not block (degraded)"
        );
        assert!(!verdict.evidences.is_empty(), "evidence is still collected");
    }

    #[test]
    fn verify_waf_verdict_without_context_blocks_t1() {
        // Degraded parity: a T1 challenge still blocks with no context.
        let verdict = verify_waf_verdict("Just a moment...", None, None, Default::default());
        assert!(verdict.is_blocked);
    }

    #[test]
    fn verify_waf_verdict_with_waf_status_blocks_t2() {
        // New capability: supplying a correlated WAF status (403) makes a
        // bare vendor name (T2) block.
        let verdict = verify_waf_verdict(
            "<html>blocked by akamai</html>",
            Some(403),
            Some("text/html".to_string()),
            Default::default(),
        );
        assert!(verdict.is_blocked, "T2 + 403 must block");
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.tier == WafTier::Fingerprint));
    }

    #[test]
    fn verify_waf_verdict_with_ok_status_passes_t2() {
        // New capability: the same T2 body at status 200 passes.
        let verdict = verify_waf_verdict(
            "<html>blocked by akamai</html>",
            Some(200),
            Some("text/html".to_string()),
            Default::default(),
        );
        assert!(!verdict.is_blocked, "T2 + 200 must pass");
    }
}
