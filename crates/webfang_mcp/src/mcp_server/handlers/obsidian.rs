//! Obsidian Integration tools — 3 tools for vault operations
//!
//! Tools: detect_obsidian_vault, build_obsidian_uri, open_in_obsidian

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;

#[tool_router(router = tool_router_obsidian, vis = "pub")]
impl McpHandler {
    /// Detect Obsidian vault path using multi-priority detection
    #[tool(
        description = "Detect Obsidian vault path using multi-priority detection: CLI flag → env var → config file → registry → auto-scan."
    )]
    #[instrument(skip(self), fields(vault_path = ?params.vault_path))]
    async fn detect_obsidian_vault(
        &self,
        Parameters(params): Parameters<DetectVaultParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let cli_path = params
            .vault_path
            .as_ref()
            .map(|p| std::path::Path::new(p.as_str()));
        match webfang_core::infrastructure::obsidian::vault_detector::detect_vault(
            cli_path, None, None,
        ) {
            Some(path) => Ok(CallToolResult::success(vec![Content::text(
                path.display().to_string(),
            )])),
            None => Ok(CallToolResult::success(vec![Content::text(
                "no vault detected",
            )])),
        }
    }

    /// Build obsidian:// URI protocol link
    #[tool(
        description = "Build an obsidian:// URI protocol link to open a specific note in the Obsidian app."
    )]
    #[instrument(skip(self), fields(vault_name = %params.vault_name, file_path = %params.file_path))]
    async fn build_obsidian_uri(
        &self,
        Parameters(params): Parameters<BuildObsidianUriParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let uri = webfang_core::infrastructure::obsidian::uri::build_obsidian_uri(
            &params.vault_name,
            &params.file_path,
        );
        Ok(CallToolResult::success(vec![Content::text(uri)]))
    }

    /// Open a note in Obsidian app via URI protocol
    #[tool(
        description = "Open a note in the Obsidian app using the obsidian:// URI protocol. Launches the Obsidian application."
    )]
    #[instrument(skip(self), fields(vault_name = %params.vault_name, file_path = %params.file_path))]
    async fn open_in_obsidian(
        &self,
        Parameters(params): Parameters<BuildObsidianUriParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let uri = webfang_core::infrastructure::obsidian::uri::build_obsidian_uri(
            &params.vault_name,
            &params.file_path,
        );
        match tokio::process::Command::new("open").arg(&uri).spawn() {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Opened in Obsidian: {uri}"
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "failed to open Obsidian: {e}"
            ))])),
        }
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_obsidian()
}
