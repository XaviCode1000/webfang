//! HTML chunker
//!
//! Implements size-based chunking with structural boundaries:
//! - Two-pass approach: structural boundaries → size-based merge/split
//! - SmallVec optimization for small collections (`mem-smallvec`)
//!
//! # Thread Safety
//!
//! `HtmlChunker` is `Send + Sync` and can be shared across threads.

use smallvec::SmallVec;
use uuid::Uuid;

use webfang_core::domain::DocumentChunk;
use webfang_core::error::SemanticError;

use super::sentence::SentenceSplitter;

/// Block-level HTML elements whose tag edges are real structural boundaries.
///
/// Every other tag (inline `a`, `em`, `code`, `sup`, ...) sits INSIDE prose:
/// its edges must not emit any boundary, otherwise a wrapped source line plus
/// an inline tag edge fabricates a fake paragraph break mid-sentence (#1313).
const BLOCK_TAGS: &[&str] = &[
    "address",
    "area",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "br",
    "caption",
    "center",
    "col",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "iframe",
    "isindex",
    "legend",
    "li",
    "link",
    "main",
    "marquee",
    "menu",
    "meta",
    "nav",
    "noframes",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "param",
    "plaintext",
    "pre",
    "script",
    "search",
    "section",
    "select",
    "source",
    "style",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
    "wbr",
];

/// HTML chunker
///
/// Chunks HTML content into semantic segments using a two-pass approach:
/// 1. **Structural boundaries**: Split by paragraphs and HTML elements
/// 2. **Size-based merge/split**: Merge small chunks, split large ones
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "ai")]
/// # fn example() -> anyhow::Result<()> {
/// use webfang_ai::HtmlChunker;
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
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_chunk_size: 100,
            max_chunk_size: 512,
            sentence_splitter: SentenceSplitter,
        }
    }

    /// Create a new HtmlChunker with custom settings
    ///
    /// # Arguments
    ///
    /// * `min_chunk_size` - Minimum characters per chunk
    /// * `max_chunk_size` - Maximum characters per chunk
    ///
    /// # Returns
    ///
    /// A new HtmlChunker instance
    #[must_use]
    pub fn with_config(min_chunk_size: usize, max_chunk_size: usize) -> Self {
        Self {
            min_chunk_size,
            max_chunk_size,
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
    /// use webfang_ai::HtmlChunker;
    ///
    /// let chunker = HtmlChunker::new();
    /// let html = "<article><p>Hello World</p><p>Second paragraph</p></article>";
    /// let chunks = chunker.chunk(html)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn chunk(&self, html: &str) -> Result<Vec<DocumentChunk>, SemanticError> {
        // Pass 1: Structural boundaries (strip HTML and split by block edges).
        // `strip_html_tags` reserves '\n' exclusively for block boundaries, so
        // every piece here is a real HTML block, never a mid-sentence fragment
        // caused by an inline-link line wrap (#1313).
        let text = self.strip_html_tags(html);
        let paragraphs: Vec<&str> = text.split('\n').filter(|p| !p.trim().is_empty()).collect();

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
    /// Block-level tag edges emit a single `'\n'` — the only paragraph
    /// boundary marker in the returned text. Inline tag edges emit nothing,
    /// and source newlines become plain spaces, so a soft-wrapped source line
    /// around an inline link can no longer fabricate a break inside a
    /// sentence (#1313).
    ///
    /// # Arguments
    ///
    /// * `html` - HTML content
    ///
    /// # Returns
    ///
    /// Plain text with HTML tags removed; `'\n'` marks block boundaries
    fn strip_html_tags(&self, html: &str) -> String {
        let mut result = String::with_capacity(html.len());
        let mut in_tag = false;
        // Lowercase tag name collected until the first name delimiter
        // (whitespace, '/', '>'); empty for comments/doctype/PI, which are
        // never block boundaries.
        let mut tag_name = String::new();
        let mut name_complete = false;

        for ch in html.chars() {
            if in_tag {
                if ch == '>' {
                    in_tag = false;
                    if BLOCK_TAGS.contains(&tag_name.as_str()) {
                        result.push('\n');
                    }
                } else if !name_complete {
                    if ch.is_ascii_alphanumeric() {
                        tag_name.push(ch.to_ascii_lowercase());
                    } else {
                        name_complete = true;
                    }
                }
            } else if ch == '<' {
                in_tag = true;
                tag_name.clear();
                name_complete = false;
            } else if ch == '\n' {
                // In HTML, a source newline is collapsible whitespace, never a
                // paragraph boundary (#1313).
                result.push(' ');
            } else {
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
    }

    #[test]
    fn test_chunker_with_config() {
        let chunker = HtmlChunker::with_config(50, 300);
        assert_eq!(chunker.min_chunk_size(), 50);
        assert_eq!(chunker.max_chunk_size(), 300);
    }

    #[test]
    fn test_chunker_builder_pattern() {
        let chunker = HtmlChunker::new()
            .with_min_chunk_size(80)
            .with_max_chunk_size(400);

        assert_eq!(chunker.min_chunk_size(), 80);
        assert_eq!(chunker.max_chunk_size(), 400);
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

/// One-off measurement harness for #1368 (phase 1: quantify the text lost by
/// the pass-1 `< min_chunk_size` drop in [`HtmlChunker::chunk`], before
/// `merge_small_chunks` ever sees it). It replicates production's exact
/// enumeration — `strip_html_tags` → `split('\n')` → non-empty-trim filter →
/// `trim()` — and measures byte length with `str::len()` (the same metric as
/// production). It adds zero behaviour: it only reads crate-internal items
/// that are already visible to this descendant module, and the test is
/// `#[ignore]`d, so it stays dormant in CI.
#[cfg(all(test, feature = "ai"))]
mod short_para_measurement {
    use super::HtmlChunker;
    use crate::infrastructure_ai::content_pruner::{ContentPruner, LegibleContentPruner};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Dropped-paragraph length buckets under the 100-byte threshold.
    const BUCKETS: [(&str, usize, usize); 6] = [
        ("1-9", 1, 9),
        ("10-29", 10, 29),
        ("30-49", 30, 49),
        ("50-69", 50, 69),
        ("70-84", 70, 84),
        ("85-99", 85, 99),
    ];

    /// Literal maximum length of a reported sample (chars, not bytes).
    const SAMPLE_MAX_CHARS: usize = 90;

    /// Literal samples reported for the post-prune 1-9-bucket composition
    /// (#1368 phase 2 — the datum F is decided from).
    const B19_SAMPLE_CAP: usize = 10;

    struct Dropped {
        len: usize,
        text: String,
    }

    struct PageOutcome {
        file: String,
        url: String,
        paras: usize,
        chars_total: usize,
        /// Block structure of the RAW html (only counted in prune mode; the
        /// pruned input is never apples-to-apples with raw and the JSON must
        /// say so explicitly).
        paras_raw: usize,
        chars_total_raw: usize,
        dropped: Vec<Dropped>,
    }

    /// True when `NAME=1` in the environment.
    fn env_flag(name: &str) -> bool {
        std::env::var(name).is_ok_and(|v| v == "1")
    }

    /// Words that make a short block chrome when EVERY alphabetic word of the
    /// block is in this list (nav/labels observed in the phase-1 and phase-2
    /// sample review). Numbers never count as words.
    const CHROME_WORDS: &[&str] = &[
        "menu",
        "search",
        "page",
        "edit",
        "editar",
        "edición",
        "edits",
        "source",
        "history",
        "views",
        "read",
        "talk",
        "contributions",
        "login",
        "logout",
        "sign",
        "up",
        "in",
        "out",
        "help",
        "about",
        "contact",
        "donate",
        "tools",
        "language",
        "languages",
        "next",
        "prev",
        "previous",
        "top",
        "home",
        "index",
        "contents",
        "category",
        "categories",
        "tag",
        "tags",
        "share",
        "report",
        "bug",
        "show",
        "hide",
        "more",
        "less",
        "close",
        "submit",
        "skip",
        "main",
        "site",
        "navigation",
        "sidebar",
        "footer",
        "header",
        "nav",
        "article",
        "articles",
        "document",
        "docs",
        "version",
        "versions",
        "stable",
        "beta",
        "nightly",
        "note",
        "notes",
        "warning",
        "see",
        "also",
        "here",
        "new",
        "news",
        "best",
        "ask",
        "reply",
        "replies",
        "vote",
        "votes",
        "by",
        "action",
        "actions",
    ];

    /// Suffixes that turn a single-token block into a file/domain chrome token
    /// (site titles, repo names like `tokio.rs`).
    const DOMAIN_SUFFIXES: &[&str] = &[
        "rs", "py", "js", "ts", "go", "rb", "sh", "md", "html", "htm", "css", "xml", "json", "com",
        "org", "net", "io", "dev", "app", "edu", "gov", "wiki", "xyz", "me", "tv", "cc",
    ];

    /// Deterministic class for a 1-9 byte block: UI chrome vs likely content.
    /// Reviewed against the literal samples this module prints.
    fn classify_1_9(text: &str) -> &'static str {
        let t = text.trim();
        // Pure punctuation (bullets, slashes, em-dashes): chrome.
        if !t.chars().any(char::is_alphanumeric) {
            return "boilerplate";
        }
        let lower = t.to_lowercase();
        // Single-token file/domain (e.g. `tokio.rs`): chrome.
        if !lower.contains(' ') {
            if let Some(dot) = lower.rfind('.') {
                let head = &lower[..dot];
                let tail = &lower[dot + 1..];
                let head_ok = !head.is_empty()
                    && !head.ends_with('.')
                    && head
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
                let tail_ok = !tail.is_empty()
                    && tail.len() <= 6
                    && tail.chars().all(|c| c.is_ascii_alphabetic())
                    && DOMAIN_SUFFIXES.contains(&tail);
                if head_ok && tail_ok {
                    return "boilerplate";
                }
            }
        }
        // Chrome label: every alphabetic word is a known nav word.
        let mut saw_word = false;
        for word in lower.split(|c: char| !c.is_alphabetic()) {
            if word.is_empty() {
                continue;
            }
            saw_word = true;
            if !CHROME_WORDS.contains(&word) {
                return "legit";
            }
        }
        if saw_word {
            "boilerplate"
        } else {
            // No letters at all but has digits (e.g. `2006`, `[1]`): treated as
            // content unless review says otherwise.
            "legit"
        }
    }

    impl PageOutcome {
        fn chars_dropped(&self) -> usize {
            self.dropped.iter().map(|d| d.len).sum()
        }
        fn chars_lost_frac(&self) -> f64 {
            safe_frac(self.chars_dropped(), self.chars_total)
        }
    }

    fn safe_frac(num: usize, den: usize) -> f64 {
        if den == 0 {
            0.0
        } else {
            num as f64 / den as f64
        }
    }

    fn list_html(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "html"))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(std::io::Error::other(format!("no *.html files under {dir:?}")).into());
        }
        Ok(files)
    }

    fn load_urls(dir: &Path) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        let Ok(manifest) = fs::read_to_string(dir.join("manifest.tsv")) else {
            return map;
        };
        for line in manifest.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() >= 5 {
                map.insert(cols[0].to_string(), cols[4].to_string());
            }
        }
        map
    }

    /// Replicates `chunk()` pass-1 exactly, keeping the dropped pieces.
    ///
    /// With a pruner, feeds the pass-1 replica the EXACT input production
    /// feeds it: `LegibleContentPruner::prune(html)`, falling back to the raw
    /// html when the prune result is empty (same guard as
    /// `SemanticCleanerImpl::clean` step 0, #1368 phase 2).
    fn measure_page(
        chunker: &HtmlChunker,
        pruner: Option<&LegibleContentPruner>,
        path: &Path,
        urls: &BTreeMap<String, String>,
    ) -> anyhow::Result<PageOutcome> {
        let raw = fs::read_to_string(path)?;
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let min = chunker.min_chunk_size();
        let pruned = pruner.map(|p| {
            let out = p.prune(&raw);
            if out.is_empty() {
                raw.clone()
            } else {
                out
            }
        });
        let html = pruned.as_deref().unwrap_or(&raw);
        let mut outcome = PageOutcome {
            url: urls.get(&file).cloned().unwrap_or_default(),
            file,
            paras: 0,
            chars_total: 0,
            paras_raw: 0,
            chars_total_raw: 0,
            dropped: Vec::new(),
        };
        if pruner.is_some() {
            for paragraph in chunker
                .strip_html_tags(&raw)
                .split('\n')
                .filter(|p| !p.trim().is_empty())
            {
                outcome.paras_raw += 1;
                outcome.chars_total_raw += paragraph.trim().len();
            }
        }
        for paragraph in chunker
            .strip_html_tags(html)
            .split('\n')
            .filter(|p| !p.trim().is_empty())
        {
            let trimmed = paragraph.trim();
            outcome.paras += 1;
            outcome.chars_total += trimmed.len();
            if trimmed.len() < min {
                outcome.dropped.push(Dropped {
                    len: trimmed.len(),
                    text: trimmed.to_string(),
                });
            }
        }
        Ok(outcome)
    }

    fn dropped_in_bucket(outcomes: &[PageOutcome], (lo, hi): (usize, usize)) -> Vec<&Dropped> {
        outcomes
            .iter()
            .flat_map(|p| p.dropped.iter())
            .filter(|d| d.len >= lo && d.len <= hi)
            .collect()
    }

    /// Deterministic even spread of `want` picks over `items` (endpoints
    /// included); takes all items when fewer than `want`.
    fn take_evenly<'a, T>(items: &[&'a T], want: usize) -> Vec<&'a T> {
        let n = items.len().min(want);
        if n == 0 {
            return Vec::new();
        }
        if n == 1 || items.len() == n {
            return items.iter().copied().take(n).collect();
        }
        (0..n)
            .map(|k| items[k * (items.len() - 1) / (n - 1)])
            .collect()
    }

    fn truncate_chars(text: &str, max_chars: usize) -> String {
        text.chars().take(max_chars).collect()
    }

    fn json_escape(out: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
    }

    fn json_per_page(out: &mut String, outcomes: &[PageOutcome]) {
        out.push_str("  \"per_page\": [\n");
        for (i, p) in outcomes.iter().enumerate() {
            out.push_str("    {\"file\": \"");
            json_escape(out, &p.file);
            out.push_str("\", \"url\": \"");
            json_escape(out, &p.url);
            out.push_str(&format!(
                "\", \"paras\": {}, \"paras_dropped\": {}, \"paras_dropped_frac\": {:.6}, \
                 \"chars_total\": {}, \"chars_dropped\": {}, \"chars_lost_frac\": {:.6}}}",
                p.paras,
                p.dropped.len(),
                safe_frac(p.dropped.len(), p.paras),
                p.chars_total,
                p.chars_dropped(),
                p.chars_lost_frac(),
            ));
            if i + 1 < outcomes.len() {
                out.push_str(",\n");
            } else {
                out.push('\n');
            }
        }
        out.push_str("  ],\n");
    }

    fn json_global(out: &mut String, outcomes: &[PageOutcome]) {
        let paras_total: usize = outcomes.iter().map(|p| p.paras).sum();
        let paras_dropped: usize = outcomes.iter().map(|p| p.dropped.len()).sum();
        let chars_total: usize = outcomes.iter().map(|p| p.chars_total).sum();
        let chars_dropped: usize = outcomes.iter().map(PageOutcome::chars_dropped).sum();
        out.push_str(&format!(
            "  \"global\": {{\"pages_ok\": {}, \"paras_total\": {}, \"paras_dropped\": {}, \
             \"paras_dropped_frac\": {:.6}, \"chars_total\": {}, \"chars_dropped\": {}, \
             \"chars_dropped_frac\": {:.6}}},\n",
            outcomes.len(),
            paras_total,
            paras_dropped,
            safe_frac(paras_dropped, paras_total),
            chars_total,
            chars_dropped,
            safe_frac(chars_dropped, chars_total),
        ));
    }

    /// Raw-vs-pruned block structure comparison — pruned measurements are NOT
    /// comparable to raw ones, and the JSON must state that (prune mode only).
    fn json_prune_structure(out: &mut String, outcomes: &[PageOutcome]) {
        let blocks_raw: usize = outcomes.iter().map(|p| p.paras_raw).sum();
        let blocks_pruned: usize = outcomes.iter().map(|p| p.paras).sum();
        let chars_raw: usize = outcomes.iter().map(|p| p.chars_total_raw).sum();
        let chars_pruned: usize = outcomes.iter().map(|p| p.chars_total).sum();
        out.push_str(&format!(
            "  \"prune_structure\": {{\"blocks_raw\": {blocks_raw}, \"blocks_pruned\": {blocks_pruned}, \
             \"chars_raw\": {chars_raw}, \"chars_pruned\": {chars_pruned}, \
             \"note\": \"prune() rewrites the block structure (chrome blocks removed and text truncated); \
              pruned numbers are not comparable to the raw-corpus measurement\"}},\n",
        ));
    }

    /// Post-prune composition of the 1-9 byte bucket: the datum F is decided
    /// from (phase 2 counts the classes over EVERY block in the bucket, and
    /// reports up to B19_SAMPLE_CAP literal samples). Final JSON section —
    /// emits no trailing comma.
    fn json_bucket_1_9(out: &mut String, outcomes: &[PageOutcome]) {
        let items = dropped_in_bucket(outcomes, (1, 9));
        let chars_total: usize = outcomes.iter().map(|p| p.chars_total).sum();
        let chars: usize = items.iter().map(|d| d.len).sum();
        let mut boiler = 0_usize;
        let mut boiler_chars = 0_usize;
        for d in &items {
            if classify_1_9(&d.text) == "boilerplate" {
                boiler += 1;
                boiler_chars += d.len;
            }
        }
        out.push_str("  \"bucket_1_9_pruned\": {\"count\": ");
        out.push_str(&format!("{}", items.len()));
        out.push_str(", \"chars\": ");
        out.push_str(&format!("{chars}"));
        out.push_str(&format!(
            ", \"frac_of_pruned_chars\": {:.6}",
            safe_frac(chars, chars_total)
        ));
        out.push_str(", \"classes\": {\"boilerplate\": {\"count\": ");
        out.push_str(&format!("{boiler}"));
        out.push_str(&format!(", \"chars\": {boiler_chars}}}"));
        out.push_str(", \"legit\": {\"count\": ");
        out.push_str(&format!("{}", items.len() - boiler));
        out.push_str(", \"chars\": ");
        out.push_str(&format!("{}", chars - boiler_chars));
        out.push_str("}}");
        out.push_str(",\n    \"samples\": [\n");
        let shown = items.len().min(B19_SAMPLE_CAP);
        for (i, d) in take_evenly(&items, B19_SAMPLE_CAP).into_iter().enumerate() {
            out.push_str("      {\"text\": \"");
            json_escape(out, &truncate_chars(&d.text, SAMPLE_MAX_CHARS));
            out.push_str(&format!(
                "\", \"len_bytes\": {}, \"class\": \"{}\"}}",
                d.len,
                classify_1_9(&d.text)
            ));
            if i + 1 < shown {
                out.push_str(",\n");
            } else {
                out.push('\n');
            }
        }
        out.push_str("    ]}\n");
    }

    fn json_histogram(out: &mut String, outcomes: &[PageOutcome]) {
        out.push_str("  \"histogram\": {");
        for (i, (name, lo, hi)) in BUCKETS.iter().enumerate() {
            let items = dropped_in_bucket(outcomes, (*lo, *hi));
            let chars: usize = items.iter().map(|d| d.len).sum();
            out.push_str(&format!(
                "\"{name}\": {{\"count\": {}, \"chars\": {}}}",
                items.len(),
                chars
            ));
            if i + 1 < BUCKETS.len() {
                out.push_str(", ");
            }
        }
        out.push_str("},\n");
    }

    fn json_worst5(out: &mut String, outcomes: &[PageOutcome]) {
        let mut ranked: Vec<&PageOutcome> = outcomes.iter().collect();
        ranked.sort_by(|a, b| b.chars_lost_frac().total_cmp(&a.chars_lost_frac()));
        out.push_str("  \"worst5\": [\n");
        for (i, p) in ranked.iter().take(5).enumerate() {
            out.push_str("    {\"file\": \"");
            json_escape(out, &p.file);
            out.push_str("\", \"url\": \"");
            json_escape(out, &p.url);
            out.push_str(&format!(
                "\", \"chars_lost_frac\": {:.6}}}",
                p.chars_lost_frac()
            ));
            if i + 1 < ranked.len().min(5) {
                out.push_str(",\n");
            } else {
                out.push('\n');
            }
        }
        out.push_str("  ],\n");
    }

    fn json_samples(out: &mut String, outcomes: &[PageOutcome], trailing_comma: bool) {
        out.push_str("  \"samples\": [\n");
        let mut all: Vec<(&str, &Dropped)> = Vec::new();
        for bucket in ["1-9", "50-69", "85-99"] {
            let (lo, hi) = BUCKETS
                .iter()
                .find(|(name, _, _)| *name == bucket)
                .map(|&(_, lo, hi)| (lo, hi))
                .unwrap_or((0, 0));
            for d in take_evenly(&dropped_in_bucket(outcomes, (lo, hi)), 5) {
                all.push((bucket, d));
            }
        }
        for (i, (bucket, d)) in all.iter().enumerate() {
            out.push_str("    {\"text\": \"");
            json_escape(out, &truncate_chars(&d.text, SAMPLE_MAX_CHARS));
            out.push_str(&format!(
                "\", \"len_bytes\": {}, \"bucket\": \"{bucket}\", \"class\": \"pending_manual\"}}",
                d.len
            ));
            if i + 1 < all.len() {
                out.push_str(",\n");
            } else {
                out.push('\n');
            }
        }
        if trailing_comma {
            out.push_str("  ],\n");
        } else {
            out.push_str("  ]\n");
        }
    }

    fn render_json(
        outcomes: &[PageOutcome],
        dir: &Path,
        min_chunk_size: usize,
        prune: bool,
    ) -> String {
        let mut out = String::from("{\n");
        out.push_str("  \"corpus_dir\": \"");
        json_escape(&mut out, &dir.display().to_string());
        out.push_str(&format!(
            "\",\n  \"min_chunk_size\": {min_chunk_size},\n  \"prune\": {prune},\n"
        ));
        if prune {
            json_prune_structure(&mut out, outcomes);
        }
        json_per_page(&mut out, outcomes);
        json_global(&mut out, outcomes);
        json_histogram(&mut out, outcomes);
        json_worst5(&mut out, outcomes);
        json_samples(&mut out, outcomes, prune);
        if prune {
            json_bucket_1_9(&mut out, outcomes);
        }
        out.push('}');
        out
    }

    /// Phase-2 review aid: with prune mode on, print EVERY post-prune 1-9 byte
    /// block with its automatic class so the sample review stays auditable.
    fn dump_bucket_1_9(outcomes: &[PageOutcome]) {
        for d in dropped_in_bucket(outcomes, (1, 9)) {
            println!(
                "[dump19] class={} len={} text={:?}",
                classify_1_9(&d.text),
                d.len,
                d.text
            );
        }
    }

    #[test]
    #[ignore = "one-off quantification #1368; needs WEBFANG_1368_CORPUS dir"]
    fn measure_short_paragraph_loss_on_real_corpus() -> anyhow::Result<()> {
        let dir = PathBuf::from(std::env::var("WEBFANG_1368_CORPUS")?);
        let prune = env_flag("WEBFANG_1368_PRUNE");
        let urls = load_urls(&dir);
        let chunker = HtmlChunker::new();
        let min = chunker.min_chunk_size();
        let pruner = prune.then(|| LegibleContentPruner::standard());
        let mut outcomes = Vec::new();
        for path in list_html(&dir)? {
            outcomes.push(measure_page(&chunker, pruner.as_ref(), &path, &urls)?);
        }
        if prune {
            dump_bucket_1_9(&outcomes);
        }
        let json = render_json(&outcomes, &dir, min, prune);
        println!("{json}");
        if let Ok(target) = std::env::var("WEBFANG_1368_OUT") {
            let target = PathBuf::from(target);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, json.as_bytes())?;
            println!("wrote {target:?}");
        }
        // Sanity: the harness must at least never lose more than it counts.
        for p in &outcomes {
            assert!(
                p.chars_dropped() <= p.chars_total,
                "inconsistent page {}",
                p.file
            );
        }
        Ok(())
    }
}
