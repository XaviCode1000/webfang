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
        let _permit = acquire_semaphore!(self, url_utils);

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
                    serde_json::to_string_pretty(&info)
                        .expect("serializing JSON to a string cannot fail"),
                )]))
            },
            Err(e) => {
                let info = serde_json::json!({"valid": false, "error": e.to_string()});
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info)
                        .expect("serializing JSON to a string cannot fail"),
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
        let _permit = acquire_semaphore!(self, url_utils);

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
        let _permit = acquire_semaphore!(self, url_utils);

        // Delegate to core normalize_url with strip_www=false (MCP preserves www prefix)
        let normalized = webfang_core::infrastructure::crawler::normalize_url(&params.url, false);

        // Core returns as-is for non-URLs; MCP tool reports error for invalid input
        if !params.url.contains("://") {
            return Ok(CallToolResult::error(vec![Content::text(
                "Invalid URL: no scheme found".to_string(),
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(normalized)]))
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
        let _permit = acquire_semaphore!(self, url_utils);

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
        let _permit = acquire_semaphore!(self, url_utils);

        let seed_host = normalize_seed_host(&params.seed_domain);
        let is_internal = url::Url::parse(&params.url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .map(|url_host| url_host == seed_host || url_host.ends_with(&format!(".{seed_host}")))
            .unwrap_or(false);
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
        let _permit = acquire_semaphore!(self, url_utils);

        match webfang_core::adapters::url_path::OutputPath::from_url(&params.url) {
            Ok(output_path) => {
                let info = serde_json::json!({
                    "full_path": output_path.to_full_path(),
                    "relative_path": output_path.to_folder_path(),
                    "domain": output_path.domain().to_string(),
                });
                Ok(CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&info)
                        .expect("serializing JSON to a string cannot fail"),
                )]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }
}

/// Normalize a seed domain input to a bare host string.
///
/// Accepts both bare domains (`example.com`) and full URLs (`https://example.com/path`).
/// For URLs, extracts the host component. For bare domains, strips any path suffix.
fn normalize_seed_host(seed: &str) -> String {
    if let Ok(u) = url::Url::parse(seed) {
        if let Some(host) = u.host_str() {
            return host.to_string();
        }
    }
    // Bare domain — strip any accidental path component
    seed.split('/').next().unwrap_or(seed).to_string()
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_url_utils()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_seed_host_bare_domain() {
        assert_eq!(normalize_seed_host("example.com"), "example.com");
    }

    #[test]
    fn normalize_seed_host_with_scheme() {
        assert_eq!(normalize_seed_host("https://example.com"), "example.com");
    }

    #[test]
    fn normalize_seed_host_with_scheme_and_path() {
        assert_eq!(
            normalize_seed_host("https://example.com/path"),
            "example.com"
        );
    }

    #[test]
    fn normalize_seed_host_bare_with_path() {
        assert_eq!(normalize_seed_host("example.com/path"), "example.com");
    }

    #[test]
    fn normalize_seed_host_subdomain() {
        assert_eq!(normalize_seed_host("blog.example.com"), "blog.example.com");
    }

    #[test]
    fn normalize_seed_host_with_scheme_subdomain() {
        assert_eq!(
            normalize_seed_host("https://blog.example.com"),
            "blog.example.com"
        );
    }

    #[test]
    fn normalize_seed_host_empty() {
        assert_eq!(normalize_seed_host(""), "");
    }

    // --- Integration-style tests for the classification logic ---

    fn classify(url: &str, seed_domain: &str) -> bool {
        let seed_host = normalize_seed_host(seed_domain);
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .map(|url_host| url_host == seed_host || url_host.ends_with(&format!(".{seed_host}")))
            .unwrap_or(false)
    }

    #[test]
    fn internal_link_bare_domain() {
        assert!(classify("https://example.com/a", "example.com"));
    }

    #[test]
    fn internal_link_subdomain_bare() {
        assert!(classify("https://blog.example.com/x", "example.com"));
    }

    #[test]
    fn internal_link_with_scheme() {
        assert!(classify("https://example.com/a", "https://example.com"));
    }

    #[test]
    fn external_link() {
        assert!(!classify("https://otro.com/a", "example.com"));
    }

    #[test]
    fn internal_link_seed_with_path() {
        assert!(classify("https://example.com/a", "example.com/path"));
    }

    #[test]
    fn invalid_url_returns_false() {
        assert!(!classify("not-a-url", "example.com"));
    }

    #[test]
    fn port_in_url_does_not_break() {
        assert!(classify("https://example.com:8080/a", "example.com"));
    }
}
