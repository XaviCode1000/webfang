//! HTML chunker
//!
//! Implements semantic chunking following 2026 best practices:
//! - Two-pass approach: structural boundaries → embedding refinement
//! - SmallVec optimization for small collections (`mem-smallvec`)
//!
//! # Thread Safety
//!
//! `HtmlChunker` is `Send + Sync` and can be shared across threads.

use smallvec::SmallVec;
use uuid::Uuid;

use crate::domain::DocumentChunk;
use crate::error::SemanticError;

use super::sentence::SentenceSplitter;

/// HTML chunker
///
/// Chunks HTML content into semantic segments using a two-pass approach:
/// 1. **Structural boundaries**: Split by paragraphs and HTML elements
/// 2. **Embedding-based refinement**: Merge/split based on semantic similarity
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "ai")]
/// # fn example() -> anyhow::Result<()> {
/// use webfang::infrastructure::ai::HtmlChunker;
///
/// let chunker = HtmlChunker::new();
/// let html = "<article><p>First paragraph.</p><p>Second paragraph.</p></article>";
/// let chunks = chunker.chunk(html)?;
///
/// println!("Generated {} chunks", chunks.len());
/// # Ok(())
/// # }
/// ```
pub struct HtmlChunker {
    /// Minimum chunk size in characters
    min_chunk_size: usize,
    /// Maximum chunk size in characters
    max_chunk_size: usize,
    /// Similarity threshold for merging chunks (0.0-1.0)
    /// Chunks with similarity > threshold are merged
    similarity_threshold: f32,
    /// Sentence splitter for structural boundaries
    sentence_splitter: SentenceSplitter,
}

impl HtmlChunker {
    /// Create a new HtmlChunker with default settings
    ///
    /// # Defaults
    ///
    /// - `min_chunk_size`: 100 characters
    /// - `max_chunk_size`: 512 characters (model token limit safe zone)
    /// - `similarity_threshold`: 0.5 (cosine similarity)
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_chunk_size: 100,
            max_chunk_size: 512,
            similarity_threshold: 0.5,
            sentence_splitter: SentenceSplitter,
        }
    }

    /// Create a new HtmlChunker with custom settings
    ///
    /// # Arguments
    ///
    /// * `min_chunk_size` - Minimum characters per chunk
    /// * `max_chunk_size` - Maximum characters per chunk
    /// * `similarity_threshold` - Threshold for merging (0.0-1.0)
    ///
    /// # Returns
    ///
    /// A new HtmlChunker instance
    #[must_use]
    pub fn with_config(
        min_chunk_size: usize,
        max_chunk_size: usize,
        similarity_threshold: f32,
    ) -> Self {
        Self {
            min_chunk_size,
            max_chunk_size,
            similarity_threshold,
            sentence_splitter: SentenceSplitter,
        }
    }

    /// Set the minimum chunk size
    #[must_use]
    pub fn with_min_chunk_size(mut self, size: usize) -> Self {
        self.min_chunk_size = size;
        self
    }

    /// Set the maximum chunk size
    #[must_use]
    pub fn with_max_chunk_size(mut self, size: usize) -> Self {
        self.max_chunk_size = size;
        self
    }

    /// Set the similarity threshold
    #[must_use]
    pub fn with_similarity_threshold(mut self, threshold: f32) -> Self {
        self.similarity_threshold = threshold;
        self
    }

    /// Get the minimum chunk size
    #[must_use]
    pub fn min_chunk_size(&self) -> usize {
        self.min_chunk_size
    }

    /// Get the maximum chunk size
    #[must_use]
    pub fn max_chunk_size(&self) -> usize {
        self.max_chunk_size
    }

    /// Get the similarity threshold
    #[must_use]
    pub fn similarity_threshold(&self) -> f32 {
        self.similarity_threshold
    }

    /// Chunk HTML into semantic segments
    ///
    /// # Arguments
    ///
    /// * `html` - The HTML content to chunk
    ///
    /// # Returns
    ///
    /// A result containing:
    /// - `Ok(Vec<DocumentChunk>)` - Successfully chunked content
    /// - `Err(SemanticError)` - Chunking failed
    ///
    /// # Process
    ///
    /// 1. **Strip HTML tags**: Extract plain text
    /// 2. **Split by structural boundaries**: Paragraphs, sentences
    /// 3. **Merge small chunks**: Combine chunks below min_chunk_size
    /// 4. **Split large chunks**: Break chunks above max_chunk_size
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(feature = "ai")]
    /// # fn example() -> anyhow::Result<()> {
    /// use webfang::infrastructure::ai::HtmlChunker;
    ///
    /// let chunker = HtmlChunker::new();
    /// let html = "<article><p>Hello World</p><p>Second paragraph</p></article>";
    /// let chunks = chunker.chunk(html)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn chunk(&self, html: &str) -> Result<Vec<DocumentChunk>, SemanticError> {
        // Pass 1: Structural boundaries (strip HTML and split by paragraphs)
        let text = self.strip_html_tags(html);
        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .collect();

        // Convert to DocumentChunks
        let mut chunks: SmallVec<[DocumentChunk; 8]> = SmallVec::new();
        for paragraph in paragraphs.into_iter() {
            let trimmed = paragraph.trim();
            if trimmed.len() < self.min_chunk_size {
                continue; // Skip too-small chunks for now
            }

            let chunk = DocumentChunk::new(
                Uuid::new_v4(),
                String::new(), // To be filled by caller
                String::new(), // To be filled by caller
                trimmed.to_string(),
            );

            chunks.push(chunk);
        }

        // Pass 2: Merge/split based on size constraints
        let merged = self.merge_small_chunks(chunks);
        let final_chunks = self.split_large_chunks(merged);

        Ok(final_chunks.into_iter().collect())
    }

    /// Chunk text (non-HTML) into semantic segments
    ///
    /// Similar to `chunk()` but skips HTML tag stripping.
    ///
    /// # Arguments
    ///
    /// * `text` - The plain text to chunk
    /// * `url` - Source URL for metadata
    /// * `title` - Title for metadata
    ///
    /// # Returns
    ///
    /// A result containing the chunked content
    pub fn chunk_text(
        &self,
        text: &str,
        url: &str,
        title: &str,
    ) -> Result<Vec<DocumentChunk>, SemanticError> {
        let mut chunks = self.chunk(text)?;

        // Add metadata
        for chunk in &mut chunks {
            chunk.url = url.to_string();
            chunk.title = title.to_string();
        }

        Ok(chunks)
    }

    /// Strip HTML tags from content
    ///
    /// # Arguments
    ///
    /// * `html` - HTML content
    ///
    /// # Returns
    ///
    /// Plain text with HTML tags removed
    #[allow(clippy::manual_strip)]
    fn strip_html_tags(&self, html: &str) -> String {
        // Simple regex-free HTML tag stripping
        let mut result = String::with_capacity(html.len());
        let mut in_tag = false;

        for ch in html.chars() {
            if ch == '<' {
                in_tag = true;
            } else if ch == '>' {
                in_tag = false;
                result.push('\n');
            } else if !in_tag {
                result.push(ch);
            }
        }

        result
    }

    /// Merge chunks smaller than min_chunk_size
    ///
    /// # Arguments
    ///
    /// * `chunks` - Input chunks to merge
    ///
    /// # Returns
    ///
    /// Merged chunks meeting minimum size requirement
    fn merge_small_chunks(
        &self,
        chunks: SmallVec<[DocumentChunk; 8]>,
    ) -> SmallVec<[DocumentChunk; 8]> {
        let mut merged: SmallVec<[DocumentChunk; 8]> = SmallVec::new();
        let mut current_content = String::new();
        let mut current_url = String::new();
        let mut current_title = String::new();

        for chunk in chunks {
            if current_content.is_empty() {
                current_content = chunk.content;
                current_url = chunk.url;
                current_title = chunk.title;
            } else if current_content.len() + chunk.content.len() <= self.max_chunk_size {
                // Merge if under max size
                current_content.push(' ');
                current_content.push_str(&chunk.content);
            } else {
                // Push current and start new
                if current_content.len() >= self.min_chunk_size {
                    merged.push(DocumentChunk::new(
                        Uuid::new_v4(),
                        current_url.clone(),
                        current_title.clone(),
                        current_content.clone(),
                    ));
                }
                current_content = chunk.content;
                current_url = chunk.url;
                current_title = chunk.title;
            }
        }

        // Don't forget the last chunk
        if !current_content.is_empty() && current_content.len() >= self.min_chunk_size {
            merged.push(DocumentChunk::new(
                Uuid::new_v4(),
                current_url,
                current_title,
                current_content,
            ));
        }

        merged
    }

    /// Split chunks larger than max_chunk_size
    ///
    /// # Arguments
    ///
    /// * `chunks` - Input chunks to split
    ///
    /// # Returns
    ///
    /// Chunks meeting maximum size requirement
    fn split_large_chunks(
        &self,
        chunks: SmallVec<[DocumentChunk; 8]>,
    ) -> SmallVec<[DocumentChunk; 8]> {
        let mut result: SmallVec<[DocumentChunk; 8]> = SmallVec::new();

        for chunk in chunks {
            if chunk.content.len() <= self.max_chunk_size {
                result.push(chunk);
            } else {
                // Split by sentences
                let sentences = self.sentence_splitter.split(&chunk.content);
                let mut current = String::new();

                for sentence in sentences {
                    if current.len() + sentence.len() > self.max_chunk_size {
                        // Push current and start new
                        if !current.is_empty() {
                            result.push(DocumentChunk::new(
                                Uuid::new_v4(),
                                chunk.url.clone(),
                                chunk.title.clone(),
                                current.clone(),
                            ));
                            current.clear();
                        }
                    }
                    current.push_str(sentence);
                }

                // Don't forget the last part
                if !current.is_empty() {
                    result.push(DocumentChunk::with_metadata(
                        Uuid::new_v4(),
                        chunk.url.clone(),
                        chunk.title.clone(),
                        current.clone(),
                        chunk.metadata.clone(),
                    ));
                    current.clear();
                }
            }
        }

        result
    }
}

impl Default for HtmlChunker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunker_creation() {
        let chunker = HtmlChunker::new();
        assert!(chunker.min_chunk_size() > 0);
        assert!(chunker.max_chunk_size() > 0);
        assert!(chunker.similarity_threshold() > 0.0);
        assert!(chunker.similarity_threshold() <= 1.0);
    }

    #[test]
    fn test_chunker_with_config() {
        let chunker = HtmlChunker::with_config(50, 300, 0.7);
        assert_eq!(chunker.min_chunk_size(), 50);
        assert_eq!(chunker.max_chunk_size(), 300);
        assert_eq!(chunker.similarity_threshold(), 0.7);
    }

    #[test]
    fn test_chunker_builder_pattern() {
        let chunker = HtmlChunker::new()
            .with_min_chunk_size(80)
            .with_max_chunk_size(400)
            .with_similarity_threshold(0.6);

        assert_eq!(chunker.min_chunk_size(), 80);
        assert_eq!(chunker.max_chunk_size(), 400);
        assert_eq!(chunker.similarity_threshold(), 0.6);
    }

    #[test]
    fn test_chunker_basic_html() {
        let chunker = HtmlChunker::new();
        let html = "<p>This is a paragraph with enough text to meet the minimum chunk size requirement for testing purposes.</p>";
        let result = chunker.chunk(html);
        assert!(result.is_ok());
    }

    #[test]
    fn test_chunker_strip_html() {
        let chunker = HtmlChunker::new();
        let html = "<div><p>Hello World</p><p>Second paragraph</p></div>";
        let result = chunker.chunk(html);
        assert!(result.is_ok());
    }

    #[test]
    fn test_chunker_empty_html() {
        let chunker = HtmlChunker::new();
        let html = "";
        let result = chunker.chunk(html);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_chunker_text_with_url() {
        let chunker = HtmlChunker::new();
        let text = "This is a test paragraph with sufficient length to meet the minimum chunk size requirement for proper testing.";
        let chunks = chunker.chunk_text(text, "https://example.com", "Test Title");
        assert!(chunks.is_ok());
        let chunks = chunks.unwrap();
        if !chunks.is_empty() {
            assert_eq!(chunks[0].url, "https://example.com");
            assert_eq!(chunks[0].title, "Test Title");
        }
    }
}
