//! Semantic Cleaner trait definition — Domain layer
//!
//! Defines the contract for AI-powered semantic cleaning operations.
//! Following Clean Architecture: this trait is pure domain logic with no
//! infrastructure dependencies.
//!
//! # Design Decisions
//!
//! - **Sealed trait pattern** (`api-sealed-trait`): Prevents external implementations
//!   that could violate invariants. Only infrastructure/ai module can implement.
//! - **Async interface** (boxed futures): Async methods are manually desugared to
//!   `Pin<Box<dyn Future<Output = T> + Send>>` to preserve dyn-compatibility
//!   (`Box<dyn SemanticCleaner>`) without depending on the `async-trait` macro.
//! - **Explicit error types**: Uses `SemanticError` for type-safe error handling
//!   (`err-thiserror-lib`, `err-context-chain`)
//! - **Borrowed input**: Accepts `&str` not `&String` (`own-borrow-over-clone`)
//! - **Owned output**: Returns `Vec<DocumentChunk>` for ownership transfer

use std::future::Future;
use std::pin::Pin;

use crate::domain::DocumentChunk;
use crate::error::SemanticError;

/// Sealed trait to prevent external implementations
///
/// Following `api-sealed-trait` rust-skill: This module is sealed so that
/// only crates within webfang can implement `SemanticCleaner`.
/// This prevents users from creating invalid implementations that could
/// violate memory safety or caching invariants.
pub mod private {
    /// Marker trait for the sealed pattern — only implementable within this crate.
    pub trait Sealed {}
}

/// Semantic Cleaner — AI-powered content cleaning interface
///
/// This trait defines the contract for cleaning HTML content using AI models.
/// Implementations embed chunks through an
/// [`EmbeddingPort`](crate::domain::embedding_port::EmbeddingPort) to:
/// - Split content into semantic chunks
/// - Validate chunk sizes (character budget)
/// - Prepare content for embedding generation
///
/// # Examples
///
/// ```no_run
/// # use webfang_core::domain::semantic_cleaner::SemanticCleaner;
/// # async fn example(cleaner: &dyn SemanticCleaner) -> Result<(), Box<dyn std::error::Error>> {
/// let html = "<html><body><p>Hello World</p></body></html>";
/// let chunks = cleaner.clean("https://example.com", html).await?;
/// println!("Generated {} chunks", chunks.len());
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`SemanticError`] in these cases:
/// - [`SemanticError::ModelLoad`]: Failed to load ONNX model from cache
/// - [`SemanticError::Tokenize`]: Tokenization failed (invalid input)
/// - [`SemanticError::Inference`]: ONNX inference failed
/// - [`SemanticError::ChunkTooLarge`]: Content exceeds the configured character budget
/// - [`SemanticError::Download`]: Model download failed (if auto-download enabled)
///
/// # Implementation Notes
///
/// - Implementations MUST be thread-safe (`Send + Sync`)
/// - Implementations MUST cache models to avoid reloading
/// - Implementations MUST use memory-mapped files for large models (`mem-zero-copy`)
pub trait SemanticCleaner: private::Sealed + Send + Sync {
    /// Clean HTML content and split into semantic chunks
    ///
    /// This is the main entry point for semantic cleaning. It:
    /// 1. Strips HTML tags and extracts text
    /// 2. Splits text into semantic chunks (paragraphs, sections)
    /// 3. Validates chunk sizes against the configured character budget
    /// 4. Scores relevance against the centroid of all chunk embeddings
    /// 5. Returns chunks ready for embedding generation
    ///
    /// # Arguments
    ///
    /// * `url` - Source URL of the HTML content (used for logging and diagnostics)
    /// * `html` - Raw HTML content to clean (borrowed, `&str` not `&String`)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<DocumentChunk>)` - Successfully cleaned chunks
    /// * `Err(SemanticError)` - Cleaning failed
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::ChunkTooLarge`] if any chunk exceeds the
    /// configured character budget (98 304 characters by default).
    ///
    /// Returns [`SemanticError::Tokenize`] if the input contains invalid UTF-8
    /// or special characters that break tokenization.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use webfang_core::domain::semantic_cleaner::SemanticCleaner;
    /// # async fn example(cleaner: &dyn SemanticCleaner) -> Result<(), Box<dyn std::error::Error>> {
    /// let html = "<article><h1>Title</h1><p>Content here...</p></article>";
    /// let chunks = cleaner.clean("https://example.com/page", html).await?;
    ///
    /// for chunk in &chunks {
    ///     println!("Chunk {}: {} chars", chunk.id, chunk.content.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Performance
    ///
    /// - **First call**: May trigger model download (~90MB) + load (~100-500ms)
    /// - **Subsequent calls**: Cache hit, ~10-50ms per page
    /// - **Memory**: Uses memory-mapped files, ~90MB virtual memory (not RSS)
    ///
    /// # Note
    ///
    /// Desugared to a boxed future (`Pin<Box<dyn Future + Send>>`) instead of
    /// `async fn` so the trait stays dyn-compatible (`Box<dyn SemanticCleaner>`).
    /// The single lifetime `'a` ties `&self`, `url`, and `html` to the returned
    /// future, matching the `async-trait` macro's default desugaring.
    fn clean<'a>(
        &'a self,
        url: &'a str,
        html: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>>;

    /// Get the chunk-size guard: the maximum number of CHARACTERS accepted
    /// per chunk.
    ///
    /// This is useful for validating content before processing. The unit is
    /// characters, not tokens (ADR-0004): the implementation embeds through an
    /// [`EmbeddingPort`](crate::domain::embedding_port::EmbeddingPort) and does
    /// not tokenize, so a token count would be a number it cannot compute — and
    /// the budget has to mean the same thing for a local model and a remote
    /// embedding endpoint.
    ///
    /// # Returns
    ///
    /// Maximum number of characters accepted per chunk. This is a
    /// chunk-rejection guard, NOT a context-window or generation limit: the
    /// effective ceiling is the embedding backend's own limit, which a remote
    /// endpoint reports as HTTP 400/413.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use webfang_core::domain::semantic_cleaner::SemanticCleaner;
    /// fn example(cleaner: &dyn SemanticCleaner) {
    ///     let max_chars = cleaner.max_chars();
    ///     println!("Chunks up to {} characters are accepted", max_chars);
    /// }
    /// ```
    fn max_chars(&self) -> usize;

    /// Check if the cleaner can embed
    ///
    /// # Returns
    ///
    /// * `true` - The cleaner is ready to embed
    /// * `false` - The cleaner needs its embedding backend to be rebuilt
    ///
    /// Implementations that build their embedding port eagerly (the shipped
    /// one does) answer `true` unconditionally: a cleaner exists only if its
    /// port was built successfully.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use webfang_core::domain::semantic_cleaner::SemanticCleaner;
    /// fn example(cleaner: &dyn SemanticCleaner) {
    ///     if cleaner.is_ready() {
    ///         println!("Ready to embed");
    ///     } else {
    ///         println!("Embedding backend needs rebuilding");
    ///     }
    /// }
    /// ```
    fn is_ready(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test that the trait is sealed (this won't compile if someone tries to implement it externally)
    // This is a compile-time check, not a runtime test
    #[test]
    fn test_trait_is_sealed() {
        // If this compiles, the seal is working
        // External crates cannot implement SemanticCleaner
        fn _assert_sealed<T: SemanticCleaner>(_cleaner: T) {}

        // This test just needs to compile - no runtime assertions needed
    }
}
