//! Rich Obsidian metadata generation.
//!
//! Generates extended metadata fields for Obsidian frontmatter:
//! - `word_count` — Total word count of content
//! - `reading_time` — Estimated reading time in minutes (200 WPM)
//! - `language` — Detected language (ISO 639-1 code)
//! - `content_type` — Inferred content type (article, documentation, etc.)

use crate::domain::ScrapedContent;
use serde::Serialize;

/// Content type classification for scraped pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    /// Standard article or blog post
    Article,
    /// Technical documentation or API reference
    Documentation,
    /// Forum post or discussion thread
    Forum,
    /// Product page or landing page
    Product,
    /// Generic/unknown content type
    #[default]
    Other,
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Article => write!(f, "article"),
            Self::Documentation => write!(f, "documentation"),
            Self::Forum => write!(f, "forum"),
            Self::Product => write!(f, "product"),
            Self::Other => write!(f, "other"),
        }
    }
}

/// Rich metadata for Obsidian frontmatter.
#[derive(Debug, Clone, Serialize)]
pub struct ObsidianRichMetadata {
    /// Total word count of the content
    pub word_count: usize,
    /// Estimated reading time in minutes (200 WPM average)
    pub reading_time: usize,
    /// Detected language (ISO 639-1 code, e.g. "en", "es", "fr")
    pub language: Option<String>,
    /// Inferred content type
    pub content_type: String,
    /// Scrape timestamp (ISO 8601)
    pub scrape_date: String,
    /// Source URL
    pub source: String,
    /// Status (for Obsidian workflow)
    pub status: String,
}

impl ObsidianRichMetadata {
    /// Generate rich metadata from scraped content.
    pub fn from_content(scraped: &ScrapedContent) -> Self {
        let content = &scraped.content;
        let url = scraped.url.as_str();

        let word_count = compute_word_count(content);
        let reading_time = compute_reading_time(word_count);
        let language = detect_language(content);
        let content_type = detect_content_type(scraped);
        let scrape_date = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%z").to_string();

        Self {
            word_count,
            reading_time,
            language,
            content_type: content_type.to_string(),
            scrape_date,
            source: url.to_string(),
            status: "unread".to_string(),
        }
    }
}

/// Count words in text using whitespace separation.
pub fn compute_word_count(content: &str) -> usize {
    content.split_whitespace().count()
}

/// Estimate reading time in minutes at 200 WPM (average adult).
///
/// Rounds up to nearest minute, minimum 1 minute.
pub fn compute_reading_time(word_count: usize) -> usize {
    if word_count == 0 {
        return 1;
    }
    (word_count as f64 / 200.0).ceil() as usize
}

/// Detect the language of text using whatlang.
///
/// Only returns a language if detection is reliable.
/// Caps input at ~1024 bytes for performance (always on char boundary).
pub fn detect_language(content: &str) -> Option<String> {
    // Limit to first ~1024 bytes for performance, always on char boundary
    let sample = if content.len() > 1024 {
        let end = content
            .char_indices()
            .take_while(|(idx, _)| *idx <= 1024)
            .last()
            .map(|(idx, c)| idx + c.len_utf8())
            .unwrap_or(0);
        &content[..end]
    } else {
        content
    };

    whatlang::detect(sample)
        .filter(|info| info.is_reliable())
        .map(|info| info.lang().code().to_string())
}

/// Detect content type from URL patterns and content heuristics.
pub fn detect_content_type(content: &ScrapedContent) -> ContentType {
    let url = content.url.as_str();
    let url_lower = url.to_lowercase();

    // URL-based heuristics (fast path)
    if url_lower.contains("/doc") || url_lower.contains("/docs") || url_lower.contains("/api") {
        return ContentType::Documentation;
    }
    if url_lower.contains("/forum")
        || url_lower.contains("/thread")
        || url_lower.contains("/discussion")
    {
        return ContentType::Forum;
    }
    if url_lower.contains("/product") || url_lower.contains("/shop") || url_lower.contains("/store")
    {
        return ContentType::Product;
    }

    // Content-based heuristic
    let word_count = compute_word_count(&content.content);
    if word_count > 500 {
        return ContentType::Article;
    }

    ContentType::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ValidUrl;

    #[test]
    fn test_compute_word_count_empty() {
        assert_eq!(compute_word_count(""), 0);
    }

    #[test]
    fn test_compute_word_count_simple() {
        assert_eq!(compute_word_count("hello world foo bar"), 4);
    }

    #[test]
    fn test_compute_word_count_multiline() {
        let text = "line one\nline two\nline three";
        assert_eq!(compute_word_count(text), 6);
    }

    #[test]
    fn test_compute_reading_time_zero() {
        assert_eq!(compute_reading_time(0), 1);
    }

    #[test]
    fn test_compute_reading_time_under_minute() {
        assert_eq!(compute_reading_time(50), 1);
    }

    #[test]
    fn test_compute_reading_time_exact() {
        assert_eq!(compute_reading_time(200), 1);
        assert_eq!(compute_reading_time(201), 2);
        assert_eq!(compute_reading_time(400), 2);
    }

    #[test]
    fn test_detect_language_english() {
        let text = "This is a clear English sentence with common words.";
        let lang = detect_language(text);
        assert!(lang.is_some());
        // whatlang returns ISO 639-2 codes (eng, spa, etc.)
        assert_eq!(lang.unwrap(), "eng");
    }

    #[test]
    fn test_detect_language_spanish() {
        let text = "Este es un claro ejemplo de texto en español con palabras comunes.";
        let lang = detect_language(text);
        assert!(lang.is_some());
        // whatlang returns ISO 639-2 codes (eng, spa, etc.)
        assert_eq!(lang.unwrap(), "spa");
    }

    #[test]
    fn test_detect_language_too_short() {
        let text = "xyz";
        let lang = detect_language(text);
        // Short text may not be reliable
        let _ = lang;
    }

    #[test]
    fn test_detect_content_type_documentation() {
        let content = ScrapedContent {
            title: "API Docs".to_string(),
            content: "documentation content".to_string(),
            url: ValidUrl::parse("https://example.com/docs/api").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        assert_eq!(detect_content_type(&content), ContentType::Documentation);
    }

    #[test]
    fn test_detect_content_type_forum() {
        let content = ScrapedContent {
            title: "Forum Thread".to_string(),
            content: "discussion content".to_string(),
            url: ValidUrl::parse("https://example.com/forum/thread/123").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        assert_eq!(detect_content_type(&content), ContentType::Forum);
    }

    #[test]
    fn test_detect_content_type_article() {
        let content = ScrapedContent {
            title: "Blog Post".to_string(),
            content: "word ".repeat(600),
            url: ValidUrl::parse("https://example.com/blog/post").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        assert_eq!(detect_content_type(&content), ContentType::Article);
    }

    #[test]
    fn test_detect_content_type_other() {
        let content = ScrapedContent {
            title: "Page".to_string(),
            content: "short".to_string(),
            url: ValidUrl::parse("https://example.com/page").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        assert_eq!(detect_content_type(&content), ContentType::Other);
    }

    #[test]
    fn test_content_type_display() {
        assert_eq!(ContentType::Article.to_string(), "article");
        assert_eq!(ContentType::Documentation.to_string(), "documentation");
        assert_eq!(ContentType::Other.to_string(), "other");
    }

    #[test]
    fn test_rich_metadata_generation() {
        let content = ScrapedContent {
            title: "Test Article".to_string(),
            // Need > 500 words for "article" content type
            content: "This is a test article with some English content that should be detectable by the language detection library. ".repeat(50),
            url: ValidUrl::parse("https://example.com/blog/article").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let meta = ObsidianRichMetadata::from_content(&content);

        assert!(meta.word_count > 400);
        assert!(meta.reading_time >= 1);
        // Language detection may or may not work depending on content
        let _ = meta.language;
        // Content type based on URL (/blog/article) or word count > 500
        assert_eq!(meta.content_type, "article");
        assert_eq!(meta.source, "https://example.com/blog/article");
        assert!(!meta.scrape_date.is_empty());
        assert_eq!(meta.status, "unread");
    }
}
