//! Shared macros for MCP handlers

/// Acquire a per-category semaphore permit with error mapping.
///
/// Usage: `let _permit = acquire_semaphore!(self, category_name);`
/// Where category_name is one of: ai, scraping, export, obsidian, content, url_utils, security, assets
#[macro_export]
macro_rules! acquire_semaphore {
    ($self:expr, $category:ident) => {
        $self
            .state
            .semaphores
            .$category
            .acquire()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("semaphore error: {e}"), None))?
    };
}
