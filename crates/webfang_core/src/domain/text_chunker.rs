//! Text chunker port — domain trait for content segmentation.
//!
//! Defines the contract for splitting text into semantic chunks.
//! The concrete Markdown implementation lives in `webfang_ai`
//! ([`MarkdownChunker`]); the HTML implementation uses [`HtmlChunker`].
//!
//! This trait enables the application layer to orchestrate chunking
//! without depending on `webfang_ai` directly (dependency inversion).

use crate::error::SemanticError;

/// Domain trait for text segmentation into semantic chunks.
///
/// Implementations split raw text into chunks suitable for embedding.
/// The trait is dyn-compatible (no generics, no `async`) so it can be
/// stored as `Arc<dyn TextChunker>` in the Container.
pub trait TextChunker: Send + Sync {
    /// Split text into semantic chunks.
    ///
    /// Returns the text content of each chunk. Metadata (headings,
    /// source path) is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Tokenize`] if the text is empty or
    /// cannot be processed.
    fn chunk_text(&self, text: &str) -> Result<Vec<String>, SemanticError>;
}
