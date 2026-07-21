//! URL Utility tools — 6 tools for URL manipulation
//!
//! Tools: validate_url, extract_domain, normalize_url,
//! match_url_pattern, is_internal_link, url_to_file_path

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;

#[tool_router(router = tool_router_url_utils, vis = "pub")]
impl McpHandler {
    /// Validate and parse a URL (RFC 3986 compliant)
    #[tool(
        description = "Validate and parse a URL. Returns parsed components (scheme, host, port, path, query) or error details."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn validate_url(
        &self,
        Parameters(params): Parameters<ValidateUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match url::Url::parse(&params.url) {
            Ok(u) => {
                let info = serde_json::json!({
                    "valid": true,
                    "scheme": u.scheme(),
                    "host": u.host_str().unwrap_or(""),
                    "port": u.port(),
                    "path": u.path(),
                    "query": u.query().unwrap_or(""),
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
            Err(e) => {
                let info = serde_json::json!({"valid": false, "error": e.to_string()});
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
        }
    }

    /// Extract domain/host from a URL
    #[tool(
        description = "Extract the domain (host) from a URL. E.g., 'https://www.example.com/path' → 'www.example.com'."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn extract_domain(
        &self,
        Parameters(params): Parameters<ExtractDomainParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match url::Url::parse(&params.url) {
            Ok(u) => {
                let domain = u.host_str().unwrap_or("");
                Ok(CallToolResult::success(vec![Content::text(domain)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Normalize a URL (remove fragments, preserve trailing slashes, remove default ports)
    #[tool(
        description = "Normalize a URL by removing fragments, preserving trailing slashes, and removing default ports."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn normalize_url(
        &self,
        Parameters(params): Parameters<NormalizeUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        use url_normalize::{normalize_url as normalize, Options, RemoveQueryParameters};

        if !params.url.contains("://") {
            return Ok(CallToolResult::error(vec![Content::text(
                "Invalid URL: no scheme found".to_string(),
            )]));
        }

        let opts = Options {
            strip_hash: true,
            remove_trailing_slash: false,
            remove_query_parameters: RemoveQueryParameters::All,
            sort_query_parameters: true,
            strip_www: false,
            force_https: false,
            ..Options::default()
        };

        match normalize(&params.url, &opts) {
            Ok(normalized) => Ok(CallToolResult::success(vec![Content::text(normalized)])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Match a URL against a glob pattern
    #[tool(
        description = "Check if a URL matches a glob-style pattern. Supports path patterns (start with '/') and host patterns."
    )]
    #[instrument(skip(self), fields(url = %params.url, pattern = %params.pattern))]
    async fn match_url_pattern(
        &self,
        Parameters(params): Parameters<MatchUrlPatternParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let matches = webfang_core::domain::matches_pattern(&params.url, &params.pattern);
        Ok(CallToolResult::success(vec![Content::text(
            matches.to_string(),
        )]))
    }

    /// Check if a URL is internal to a seed domain
    #[tool(
        description = "Check if a URL belongs to the same domain (or subdomain) as the seed domain."
    )]
    #[instrument(skip(self), fields(url = %params.url, seed_domain = %params.seed_domain))]
    async fn is_internal_link(
        &self,
        Parameters(params): Parameters<IsInternalLinkParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let is_internal = match (
            url::Url::parse(&params.url),
            url::Url::parse(&params.seed_domain),
        ) {
            (Ok(u), Ok(s)) => {
                let url_host = u.host_str().unwrap_or("");
                let seed_host = s.host_str().unwrap_or("");
                url_host == seed_host || url_host.ends_with(&format!(".{seed_host}"))
            },
            _ => false,
        };
        Ok(CallToolResult::success(vec![Content::text(
            is_internal.to_string(),
        )]))
    }

    /// Convert a URL to a domain-based file path
    #[tool(
        description = "Convert a URL to a domain-based file path. E.g., 'https://example.com/docs/page' → 'example.com/docs/page.md'."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn url_to_file_path(
        &self,
        Parameters(params): Parameters<ValidateUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .url_utils
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match webfang_core::adapters::url_path::OutputPath::from_url(&params.url) {
            Ok(output_path) => {
                let info = serde_json::json!({
                    "full_path": output_path.to_full_path(),
                    "relative_path": output_path.to_folder_path(),
                    "domain": output_path.domain().to_string(),
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info).unwrap(),
                )]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_url_utils()
}
