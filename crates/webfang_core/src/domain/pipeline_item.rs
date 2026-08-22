use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use super::entities::ScrapedContent;
use super::error::ErrorClass;

/// A scraped item flowing through the processing pipeline.
///
/// Represents raw or partially-processed data from a web page.
/// Each [`PipelineStage`] receives and optionally transforms a `ScrapedItem`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrapedItem {
    /// The URL that was scraped
    #[serde(default)]
    pub url: String,
    /// Raw HTML as received from the HTTP client
    #[serde(default)]
    pub raw_html: String,
    /// Extracted text content (may be populated by a cleaning stage)
    #[serde(default)]
    pub text_content: Option<String>,
    /// Arbitrary metadata accumulated by pipeline stages
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// HTTP status code from the fetch
    #[serde(default)]
    pub status_code: u16,
    /// Optional embedding vector (populated by an AI stage)
    #[serde(default)]
    pub embeddings: Option<Vec<f32>>,
}

impl fmt::Display for ScrapedItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text_len = self.text_content.as_ref().map_or(0, |t| t.len());
        let meta_count = self.metadata.len();
        let has_embeddings = self.embeddings.is_some();
        write!(
            f,
            "ScrapedItem {{ url: {}, status: {}, text: {}B, meta: {}, embeddings: {} }}",
            self.url, self.status_code, text_len, meta_count, has_embeddings
        )
    }
}

impl From<ScrapedContent> for ScrapedItem {
    fn from(content: ScrapedContent) -> Self {
        let mut metadata = std::collections::HashMap::new();
        if let Some(ref author) = content.author {
            metadata.insert("author".into(), author.clone());
        }
        if let Some(ref date) = content.date {
            metadata.insert("date".into(), date.clone());
        }
        if let Some(ref excerpt) = content.excerpt {
            metadata.insert("excerpt".into(), excerpt.clone());
        }
        metadata.insert("title".into(), content.title.clone());

        Self {
            url: content.url.to_string(),
            raw_html: content.html.unwrap_or_default(),
            text_content: Some(content.content),
            metadata,
            status_code: 200,
            embeddings: None,
        }
    }
}

/// Reason why an item was filtered out as non-content.
///
/// Filtered items are silently skipped without error reporting.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterReason {
    /// The URL path corresponds to a non-content resource such as robots.txt or sitemap.xml.
    NonContentPath,
}

impl fmt::Display for FilterReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonContentPath => write!(f, "non-content path"),
        }
    }
}

/// Reason why an item was rejected during validation.
///
/// Each variant carries typed context so callers can render
/// precise diagnostics without string parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum RejectReason {
    /// URL string is empty.
    EmptyUrl,
    /// URL string cannot be parsed as a valid URL.
    InvalidUrl {
        /// The original URL string that failed to parse.
        url: String,
    },
    /// URL scheme is not http or https.
    UnsupportedScheme {
        /// The scheme that was found (e.g. "ftp").
        scheme: String,
    },
    /// URL has no host component.
    MissingHost,
    /// HTTP status code indicates an error (outside 200..=399).
    HttpStatus {
        /// The status code that triggered rejection.
        code: u16,
    },
    /// Content (raw_html) is empty after trimming.
    EmptyContent,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyUrl => write!(f, "URL is empty"),
            Self::InvalidUrl { url } => write!(f, "URL is not valid: {url}"),
            Self::UnsupportedScheme { scheme } => write!(
                f,
                "URL scheme '{scheme}' is not supported (requires http or https)"
            ),
            Self::MissingHost => write!(f, "URL has no host"),
            Self::HttpStatus { code } => write!(f, "HTTP status {code} indicates an error"),
            Self::EmptyContent => write!(f, "Content is empty"),
        }
    }
}

/// Outcome returned by a [`PipelineStage`] after processing an item.
///
/// Controls whether the pipeline continues or short-circuits.
/// `Filtered`, `Rejected`, and `Failed` all short-circuit the pipeline and
/// return verbatim without running later stages. `Failed` carries an
/// [`ErrorClass`] so the RUNNER (application layer) can decide retry/abort
/// policy from the class — the domain only reports the classification.
#[derive(Debug, Clone, PartialEq)]
pub enum StageOutcome {
    /// Pipeline continues with the (possibly modified) item.
    Continue(ScrapedItem),
    /// Item was filtered as non-content — pipeline stops processing it silently.
    Filtered {
        /// Why the item was filtered.
        reason: FilterReason,
    },
    /// Item was rejected — pipeline stops processing it.
    Rejected {
        /// Why the item was rejected.
        reason: RejectReason,
    },
    /// Item processing failed — short-circuits like [`Self::Rejected`] but
    /// carries an operational [`ErrorClass`] for the runner to decide policy.
    Failed {
        /// Operational classification of the failure.
        class: ErrorClass,
    },
}

/// A single processing step in the item pipeline.
///
/// Implementors transform a [`ScrapedItem`] and return a [`StageOutcome`]
/// indicating whether processing should continue, skip, or reject.
///
/// # Requirements
///
/// * `Send + Sync` — stages must be safe to share across async tasks.
/// * Use native `async fn` in traits (Rust 1.75+).
pub trait PipelineStage: Send + Sync {
    /// Human-readable name for logging/diagnostics.
    fn name(&self) -> &str;

    /// Process an item and decide the next pipeline action.
    fn process(&self, item: ScrapedItem)
        -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scraped_item_default() {
        let item = ScrapedItem::default();
        assert!(item.url.is_empty());
        assert!(item.raw_html.is_empty());
        assert!(item.text_content.is_none());
        assert!(item.metadata.is_empty());
        assert_eq!(item.status_code, 0);
        assert!(item.embeddings.is_none());
    }

    #[test]
    fn test_scraped_item_display() {
        let item = ScrapedItem {
            url: "https://example.com".into(),
            status_code: 200,
            text_content: Some("hello".into()),
            metadata: std::collections::HashMap::from([("key".into(), "val".into())]),
            ..Default::default()
        };
        let display = item.to_string();
        assert!(display.contains("https://example.com"));
        assert!(display.contains("200"));
        assert!(display.contains("5B"));
        assert!(display.contains("1"));
        assert!(display.contains("false"));
    }

    #[test]
    fn test_stage_outcome_variants() {
        let item = ScrapedItem::default();
        assert_eq!(
            StageOutcome::Continue(item.clone()),
            StageOutcome::Continue(item.clone())
        );
        assert_eq!(
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            },
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            }
        );
        assert_eq!(
            StageOutcome::Rejected {
                reason: RejectReason::EmptyUrl
            },
            StageOutcome::Rejected {
                reason: RejectReason::EmptyUrl
            }
        );
        assert_eq!(
            StageOutcome::Failed {
                class: crate::domain::error::ErrorClass::TransientBackoff
            },
            StageOutcome::Failed {
                class: crate::domain::error::ErrorClass::TransientBackoff
            }
        );
        assert_ne!(
            StageOutcome::Filtered {
                reason: FilterReason::NonContentPath
            },
            StageOutcome::Rejected {
                reason: RejectReason::EmptyUrl
            }
        );
    }

    #[test]
    fn test_filter_reason_display_non_empty() {
        assert!(!FilterReason::NonContentPath.to_string().is_empty());
    }

    #[test]
    fn test_reject_reason_display_non_empty() {
        let reasons = [
            RejectReason::EmptyUrl,
            RejectReason::InvalidUrl { url: "bad".into() },
            RejectReason::UnsupportedScheme {
                scheme: "ftp".into(),
            },
            RejectReason::MissingHost,
            RejectReason::HttpStatus { code: 404 },
            RejectReason::EmptyContent,
        ];
        for r in reasons {
            assert!(!r.to_string().is_empty(), "{r:?} rendered empty");
        }
    }

    #[test]
    fn test_scraped_item_serialization_roundtrip() {
        let item = ScrapedItem {
            url: "https://test.com".into(),
            raw_html: "<p>hi</p>".into(),
            text_content: Some("hi".into()),
            metadata: std::collections::HashMap::from([("k".into(), "v".into())]),
            status_code: 201,
            embeddings: Some(vec![0.1, 0.2]),
        };
        let json = serde_json::to_string(&item).unwrap();
        let restored: ScrapedItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item.url, restored.url);
        assert_eq!(item.raw_html, restored.raw_html);
        assert_eq!(item.text_content, restored.text_content);
        assert_eq!(item.status_code, restored.status_code);
        assert_eq!(item.embeddings, restored.embeddings);
        assert_eq!(item.metadata, restored.metadata);
    }

    #[test]
    fn test_scraped_item_json_roundtrip() {
        let item = ScrapedItem {
            url: "https://roundtrip.test".into(),
            raw_html: "<html></html>".into(),
            text_content: Some("content".into()),
            metadata: std::collections::HashMap::new(),
            status_code: 200,
            embeddings: Some(vec![1.0]),
        };
        let json = serde_json::to_string(&item).unwrap();
        let restored: ScrapedItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item.url, restored.url);
        assert_eq!(item.embeddings, restored.embeddings);
    }

    #[test]
    fn test_scraped_item_deserialize_missing_fields() {
        // Forward compat: deserialize with empty JSON
        let json = "{}";
        let item: ScrapedItem = serde_json::from_str(json).unwrap();
        assert!(item.url.is_empty());
        assert_eq!(item.status_code, 0);
    }

    #[test]
    fn test_from_scraped_content_preserves_data() {
        use super::super::entities::ScrapedContent;
        use super::super::value_objects::ValidUrl;

        let url = ValidUrl::parse("https://example.com").unwrap();
        let content = ScrapedContent {
            title: "Test Title".into(),
            content: "Test content body".into(),
            url,
            excerpt: Some("An excerpt".into()),
            author: Some("Author Name".into()),
            date: Some("2025-01-15".into()),
            html: Some("<html><body>raw</body></html>".into()),
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };

        let item: ScrapedItem = content.into();
        assert_eq!(item.url, "https://example.com/");
        assert_eq!(item.raw_html, "<html><body>raw</body></html>");
        assert_eq!(item.text_content.as_deref(), Some("Test content body"));
        assert_eq!(item.status_code, 200);
        assert!(item.embeddings.is_none());
        assert_eq!(
            item.metadata.get("title").map(String::as_str),
            Some("Test Title")
        );
        assert_eq!(
            item.metadata.get("author").map(String::as_str),
            Some("Author Name")
        );
        assert_eq!(
            item.metadata.get("date").map(String::as_str),
            Some("2025-01-15")
        );
        assert_eq!(
            item.metadata.get("excerpt").map(String::as_str),
            Some("An excerpt")
        );
    }

    #[test]
    fn test_from_scraped_content_minimal() {
        use super::super::entities::ScrapedContent;
        use super::super::value_objects::ValidUrl;

        let url = ValidUrl::parse("https://minimal.com").unwrap();
        let content = ScrapedContent {
            title: String::new(),
            content: String::new(),
            url,
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };

        let item: ScrapedItem = content.into();
        assert_eq!(item.url, "https://minimal.com/");
        assert!(item.raw_html.is_empty());
        assert_eq!(item.text_content.as_deref(), Some(""));
        assert!(item.metadata.contains_key("title"));
        assert!(!item.metadata.contains_key("author"));
    }
}
