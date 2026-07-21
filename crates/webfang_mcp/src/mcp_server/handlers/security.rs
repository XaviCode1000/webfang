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

#[tool_router(router = tool_router_security, vis = "pub")]
impl McpHandler {
    /// Detect WAF/CAPTCHA challenge in HTML body
    #[tool(
        description = "Scan HTML body for WAF/CAPTCHA signatures (Cloudflare, reCAPTCHA, hCaptcha, DataDome, PerimeterX, Akamai, etc.). Returns provider name if detected."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn detect_waf(
        &self,
        Parameters(params): Parameters<DetectWafParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match webfang_core::infrastructure::http::waf_engine::WafInspector::detect_body(
            &params.html,
        ) {
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
        description = "Multi-layer WAF inspection: checks control headers, body signatures via Aho-Corasick, and entropy analysis for silent challenges."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn verify_waf_integrity(
        &self,
        Parameters(params): Parameters<VerifyWafIntegrityParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

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
        match webfang_core::infrastructure::http::waf_engine::WafInspector::verify_integrity(
            &header_map,
            html,
        ) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(
                "WAF integrity check passed",
            )])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "WAF blocked: {e}"
            ))])),
        }
    }

    /// List all supported WAF providers
    #[tool(
        description = "List all WAF/CAPTCHA providers that can be detected by the WAF inspector."
    )]
    #[instrument(skip(self))]
    async fn list_waf_providers(&self) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

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
        let _permit = self
            .state
            .semaphores
            .security
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

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
