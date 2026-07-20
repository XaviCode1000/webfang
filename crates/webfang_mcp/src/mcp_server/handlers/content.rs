//! Content Processing tools — 7 tools for HTML/Markdown transformation
//!
//! Tools: extract_links, clean_html, convert_html_to_markdown,
//! highlight_code_blocks, convert_wiki_links, generate_frontmatter,
//! generate_rich_metadata

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;

#[tool_router(router = tool_router_content, vis = "pub")]
impl McpHandler {
    /// Remove boilerplate from HTML (scripts, nav, sidebar, footer, SVG)
    #[tool(
        description = "Remove boilerplate from HTML including scripts, styles, navigation, sidebar, footer, and SVG elements. Returns cleaned HTML."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn clean_html(
        &self,
        Parameters(params): Parameters<CleanHtmlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let cleaned =
            webfang_core::infrastructure::converter::html_cleaner::clean_html(&params.html);
        Ok(CallToolResult::success(vec![Content::text(cleaned)]))
    }

    /// Convert HTML to Markdown
    #[tool(
        description = "Convert HTML to Markdown, preserving headings, code blocks, lists, and formatting."
    )]
    #[instrument(skip(self), fields(html_len = params.html.len()))]
    async fn convert_html_to_markdown(
        &self,
        Parameters(params): Parameters<HtmlToMarkdownParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let md = webfang_core::infrastructure::converter::html_to_markdown::convert_to_markdown(
            &params.html,
        );
        Ok(CallToolResult::success(vec![Content::text(md)]))
    }

    /// Extract all href links from HTML
    #[tool(
        description = "Extract all href links from HTML content. Returns list of raw href values."
    )]
    #[instrument(skip(self), fields(base_url = %params.base_url))]
    async fn extract_links(
        &self,
        Parameters(params): Parameters<ExtractLinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        match webfang_core::infrastructure::crawler::extract_links(&params.html, &params.base_url) {
            Ok(links) => {
                let content = serde_json::to_string_pretty(&links)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }

    /// Add syntax highlighting to fenced code blocks
    #[tool(
        description = "Add syntax highlighting to fenced code blocks in Markdown using syntect. Returns Markdown with highlighted code."
    )]
    #[instrument(skip(self), fields(markdown_len = params.markdown.len()))]
    async fn highlight_code_blocks(
        &self,
        Parameters(params): Parameters<HighlightCodeParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let highlighted =
            webfang_core::infrastructure::converter::syntax_highlight::highlight_code_blocks(
                &params.markdown,
            );
        Ok(CallToolResult::success(vec![Content::text(highlighted)]))
    }

    /// Convert HTTP links to Obsidian [[wiki-link]] syntax
    #[tool(
        description = "Convert same-domain HTTP links to Obsidian [[wiki-link]] syntax for internal note linking."
    )]
    #[instrument(skip(self), fields(base_domain = %params.base_domain))]
    async fn convert_wiki_links(
        &self,
        Parameters(params): Parameters<ConvertWikiLinksParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let wikilinks = webfang_core::infrastructure::converter::wikilinks::convert_wiki_links(
            &params.markdown,
            &params.base_domain,
        );
        Ok(CallToolResult::success(vec![Content::text(wikilinks)]))
    }

    /// Generate YAML frontmatter for a scraped document
    #[tool(
        description = "Generate YAML frontmatter with title, URL, date, author, excerpt, and optional rich metadata."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn generate_frontmatter(
        &self,
        Parameters(params): Parameters<GenerateFrontmatterParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let title = params.title.as_deref().unwrap_or("Untitled");
        let url = params.url.as_deref().unwrap_or("");
        let fm = webfang_core::infrastructure::output::frontmatter::generate_with_metadata(
            title,
            url,
            None,
            None,
            None,
            &[],
            None,
        );
        Ok(CallToolResult::success(vec![Content::text(fm)]))
    }

    /// Generate rich metadata from scraped content (word count, reading time, language, content type)
    #[tool(
        description = "Generate rich metadata from scraped content including word count, reading time (200 WPM), language detection, and content type classification."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn generate_rich_metadata(
        &self,
        Parameters(params): Parameters<GenerateRichMetadataParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .content
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let content = params.content.as_deref().unwrap_or("");

        let word_count =
            webfang_core::infrastructure::obsidian::metadata::compute_word_count(content);
        let reading_time =
            webfang_core::infrastructure::obsidian::metadata::compute_reading_time(word_count);

        let meta = serde_json::json!({
            "word_count": word_count,
            "reading_time_minutes": reading_time,
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&meta).unwrap(),
        )]))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_content()
}
