//! Markdown chunker for Obsidian vault notes.
//!
//! Heading-aware chunking that respects Obsidian-specific syntax:
//! - YAML frontmatter is stripped (metadata, not semantic content)
//! - Headings (`#`, `##`) are hard section boundaries
//! - Callouts (`> [!note]`) are kept as atomic units
//! - Wikilinks (`[[...]]`) are preserved in content
//! - [`SentenceSplitter`] (UAX #29) prevents mid-sentence breaks
//!
//! # Thread Safety
//!
//! `MarkdownChunker` is `Send + Sync` and can be shared across threads.

use smallvec::SmallVec;
use uuid::Uuid;

use webfang_core::domain::DocumentChunk;
use webfang_core::error::SemanticError;

use super::sentence::SentenceSplitter;

/// Markdown chunker for Obsidian vault notes.
///
/// Chunks Markdown content into semantic segments using a heading-aware
/// approach:
/// 1. **Strip frontmatter**: Remove YAML metadata block
/// 2. **Split by headings**: `#`, `##`, etc. as hard boundaries
/// 3. **Preserve callouts**: `> [!type]` blocks stay atomic
/// 4. **Merge/split by size**: Respect min/max chunk size constraints
///
/// # Examples
///
/// ```
/// use webfang_ai::MarkdownChunker;
///
/// let chunker = MarkdownChunker::new();
/// let md = "# Title\n\nFirst section content.\n\n## Subtitle\n\nSecond section.";
/// let chunks = chunker.chunk(md).expect("chunking should succeed");
/// assert!(!chunks.is_empty());
/// ```
pub struct MarkdownChunker {
    /// Minimum chunk size in characters.
    min_chunk_size: usize,
    /// Maximum chunk size in characters.
    max_chunk_size: usize,
    /// Sentence splitter for sub-section splitting.
    sentence_splitter: SentenceSplitter,
}

impl Default for MarkdownChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownChunker {
    /// Create a new `MarkdownChunker` with default settings.
    ///
    /// # Defaults
    ///
    /// - `min_chunk_size`: 80 characters
    /// - `max_chunk_size`: 512 characters (Granite model safe zone)
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_chunk_size: 80,
            max_chunk_size: 512,
            sentence_splitter: SentenceSplitter,
        }
    }

    /// Create a new `MarkdownChunker` with custom size constraints.
    #[must_use]
    pub fn with_config(min_chunk_size: usize, max_chunk_size: usize) -> Self {
        Self {
            min_chunk_size,
            max_chunk_size,
            sentence_splitter: SentenceSplitter,
        }
    }

    /// Chunk Markdown into semantic segments.
    ///
    /// # Process
    ///
    /// 1. Strip YAML frontmatter (`---` delimited block)
    /// 2. Split by heading lines (`# ...`) as hard boundaries
    /// 3. Keep callout blocks (`> [!type]`) atomic
    /// 4. Merge small sections, split large ones via [`SentenceSplitter`]
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Tokenize`] if the input is empty after
    /// frontmatter stripping.
    pub fn chunk(&self, markdown: &str) -> Result<Vec<DocumentChunk>, SemanticError> {
        let body = Self::strip_frontmatter(markdown);

        if body.trim().is_empty() {
            return Err(SemanticError::Tokenize(
                "contenido vacío después de eliminar frontmatter".to_owned(),
            ));
        }

        // Pass 1: Split by headings into sections.
        let sections = Self::split_by_headings(body);

        // Pass 2: Process each section — protect callouts, merge/split by size.
        let mut chunks: SmallVec<[DocumentChunk; 16]> = SmallVec::new();
        for (heading, section_text) in &sections {
            let blocks = Self::extract_callout_blocks(section_text);
            for block in blocks {
                let text = block.trim();
                if text.is_empty() {
                    continue;
                }

                if text.len() <= self.max_chunk_size {
                    if text.len() >= self.min_chunk_size {
                        chunks.push(Self::make_chunk(text, heading));
                    }
                    // Small blocks are accumulated below.
                } else {
                    // Large block: split by sentences.
                    for sub in self.split_large_text(text) {
                        chunks.push(Self::make_chunk(&sub, heading));
                    }
                }
            }
        }

        // Pass 3: Merge remaining small chunks.
        let merged = self.merge_small_chunks(chunks);

        Ok(merged.into_iter().collect())
    }

    /// Strip YAML frontmatter (delimited by `---` at start of file).
    ///
    /// Returns the body after the closing `---`. If no frontmatter is
    /// present, returns the input unchanged.
    fn strip_frontmatter(md: &str) -> &str {
        let trimmed = md.trim_start();
        if !trimmed.starts_with("---") {
            return md;
        }
        // Find the closing `---` after the opening one.
        if let Some(end) = trimmed[3..].find("\n---") {
            let after = &trimmed[3 + end + 4..]; // skip past "\n---"
            // Skip the rest of the closing line.
            after.strip_prefix('\n').unwrap_or(after)
        } else {
            md // Malformed frontmatter — treat as regular content.
        }
    }

    /// Split Markdown by heading lines into `(heading_text, section_body)` pairs.
    ///
    /// Content before the first heading gets an empty heading string.
    fn split_by_headings(body: &str) -> Vec<(String, String)> {
        let mut sections: Vec<(String, String)> = Vec::new();
        let mut current_heading = String::new();
        let mut current_lines: Vec<&str> = Vec::new();

        for line in body.lines() {
            if Self::is_heading(line) {
                // Flush previous section.
                if !current_lines.is_empty() || !current_heading.is_empty() {
                    sections.push((
                        current_heading.clone(),
                        current_lines.join("\n"),
                    ));
                    current_lines.clear();
                }
                current_heading = line.trim_start_matches('#').trim().to_owned();
            } else {
                current_lines.push(line);
            }
        }

        // Flush last section.
        if !current_lines.is_empty() || !current_heading.is_empty() {
            sections.push((current_heading, current_lines.join("\n")));
        }

        sections
    }

    /// Check if a line is a Markdown heading (`# ...`).
    fn is_heading(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with('#')
            && trimmed
                .chars()
                .nth(1)
                .is_some_and(|c| c == '#' || c == ' ')
    }

    /// Extract callout blocks (`> [!type]`) as atomic units.
    ///
    /// Non-callout content is returned as separate blocks. Callout blocks
    /// include all consecutive `>` lines following the `> [!type]` marker.
    fn extract_callout_blocks(text: &str) -> Vec<String> {
        let mut blocks: Vec<String> = Vec::new();
        let mut normal_lines: Vec<&str> = Vec::new();
        let mut callout_lines: Vec<&str> = Vec::new();
        let mut in_callout = false;

        for line in text.lines() {
            let trimmed = line.trim();
            let is_blockquote = trimmed.starts_with('>');
            let is_callout_start =
                is_blockquote && trimmed.len() > 2 && trimmed[2..].trim_start().starts_with("[!");

            if is_callout_start {
                // Flush accumulated normal lines.
                if !normal_lines.is_empty() {
                    blocks.push(normal_lines.join("\n"));
                    normal_lines.clear();
                }
                in_callout = true;
                callout_lines.push(line);
            } else if in_callout && is_blockquote {
                // Continue callout block.
                callout_lines.push(line);
            } else {
                // End callout if active.
                if in_callout {
                    blocks.push(callout_lines.join("\n"));
                    callout_lines.clear();
                    in_callout = false;
                }
                normal_lines.push(line);
            }
        }

        // Flush remaining.
        if in_callout && !callout_lines.is_empty() {
            blocks.push(callout_lines.join("\n"));
        }
        if !normal_lines.is_empty() {
            blocks.push(normal_lines.join("\n"));
        }

        blocks
    }

    /// Split large text by sentences, respecting `max_chunk_size`.
    fn split_large_text(&self, text: &str) -> Vec<String> {
        let sentences = self.sentence_splitter.split_trimmed(text);
        let mut result: Vec<String> = Vec::new();
        let mut current = String::new();

        for sentence in sentences {
            if current.len() + sentence.len() + 1 > self.max_chunk_size && !current.is_empty() {
                result.push(current.trim().to_owned());
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&sentence);
        }

        if !current.trim().is_empty() {
            result.push(current.trim().to_owned());
        }

        result
    }

    /// Create a `DocumentChunk` with heading context in metadata.
    fn make_chunk(content: &str, heading: &str) -> DocumentChunk {
        let mut chunk = DocumentChunk::new(
            Uuid::new_v4(),
            String::new(), // url — filled by caller (vault path)
            String::new(), // title — filled by caller (note title)
            content.to_owned(),
        );
        if !heading.is_empty() {
            chunk
                .metadata
                .insert("heading".to_owned(), heading.to_owned());
        }
        chunk
    }

    /// Merge chunks below `min_chunk_size` with adjacent chunks.
    fn merge_small_chunks(&self, chunks: SmallVec<[DocumentChunk; 16]>) -> Vec<DocumentChunk> {
        if chunks.is_empty() {
            return Vec::new();
        }

        let mut result: Vec<DocumentChunk> = Vec::new();
        let mut pending: Option<DocumentChunk> = None;

        for chunk in chunks {
            match pending.take() {
                Some(mut prev) => {
                    if prev.content.len() < self.min_chunk_size {
                        // Merge with current chunk.
                        prev.content.push_str("\n\n");
                        prev.content.push_str(&chunk.content);
                        if prev.content.len() <= self.max_chunk_size {
                            pending = Some(prev);
                        } else {
                            // Merged result is too large — split it.
                            for sub in self.split_large_text(&prev.content) {
                                result.push(Self::make_chunk(
                                    &sub,
                                    prev.metadata.get("heading").map_or("", |s| s),
                                ));
                            }
                        }
                    } else {
                        result.push(prev);
                        pending = Some(chunk);
                    }
                }
                None => {
                    pending = Some(chunk);
                }
            }
        }

        if let Some(last) = pending {
            result.push(last);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_frontmatter() {
        let md = "---\ntitle: Test\ntags: [rust]\n---\n# Hello\n\nContent here.";
        let body = MarkdownChunker::strip_frontmatter(md);
        assert!(body.starts_with("# Hello"), "got: {body}");
        assert!(!body.contains("title:"), "frontmatter must be stripped");
    }

    #[test]
    fn test_strip_frontmatter_absent() {
        let md = "# Hello\n\nNo frontmatter here.";
        let body = MarkdownChunker::strip_frontmatter(md);
        assert_eq!(body, md, "no frontmatter — input unchanged");
    }

    #[test]
    fn test_is_heading() {
        assert!(MarkdownChunker::is_heading("# Title"));
        assert!(MarkdownChunker::is_heading("## Subtitle"));
        assert!(MarkdownChunker::is_heading("### Deep"));
        assert!(MarkdownChunker::is_heading("  # Indented"));
        assert!(!MarkdownChunker::is_heading("Not a heading"));
        assert!(!MarkdownChunker::is_heading("#hashtag"));
    }

    #[test]
    fn test_split_by_headings() {
        let md = "Intro text\n\n# Section 1\n\nContent 1\n\n## Sub 1.1\n\nContent 1.1";
        let sections = MarkdownChunker::split_by_headings(md);
        assert_eq!(sections.len(), 3, "intro + 2 headings");
        assert_eq!(sections[0].0, "", "intro has empty heading");
        assert_eq!(sections[1].0, "Section 1");
        assert_eq!(sections[2].0, "Sub 1.1");
    }

    #[test]
    fn test_callout_extraction() {
        let text = "Before\n\n> [!note] Title\n> Callout line 1\n> Callout line 2\n\nAfter";
        let blocks = MarkdownChunker::extract_callout_blocks(text);
        assert!(blocks.len() >= 2, "at least callout + surrounding text");
        let callout = blocks.iter().find(|b| b.contains("[!note]"));
        assert!(callout.is_some(), "callout block must be preserved");
        let callout_text = callout.unwrap();
        assert!(
            callout_text.contains("Callout line 2"),
            "callout must include continuation lines"
        );
    }

    #[test]
    fn test_chunk_basic_markdown() {
        let chunker = MarkdownChunker::with_config(20, 200);
        let md = "# Rust\n\nRust is a systems programming language focused on safety.\n\n## Ownership\n\nOwnership is Rust's most unique feature.";
        let chunks = chunker.chunk(md).expect("chunking should succeed");
        assert!(!chunks.is_empty(), "must produce at least one chunk");
        // Check heading metadata is preserved.
        let has_heading = chunks
            .iter()
            .any(|c| c.metadata.contains_key("heading"));
        assert!(has_heading, "at least one chunk must have heading metadata");
    }

    #[test]
    fn test_chunk_empty_after_frontmatter() {
        let chunker = MarkdownChunker::new();
        let md = "---\ntitle: Empty\n---\n";
        let result = chunker.chunk(md);
        assert!(result.is_err(), "empty content must error");
    }

    #[test]
    fn test_chunk_preserves_wikilinks() {
        let chunker = MarkdownChunker::with_config(10, 500);
        let md = "# Notes\n\nSee [[Other Note]] for details about [[Rust Patterns]].";
        let chunks = chunker.chunk(md).expect("chunking should succeed");
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert!(
            all_content.contains("[[Other Note]]"),
            "wikilinks must be preserved"
        );
    }

    #[test]
    fn test_chunker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MarkdownChunker>();
    }
}
