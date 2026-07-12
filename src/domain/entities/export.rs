//! Export-related entities

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Export format variants for RAG pipeline
///
/// Defines the supported output formats when exporting scraped content
/// for use in retrieval-augmented generation systems.
///
/// These formats are designed for RAG/embedding pipelines, NOT for
/// individual file output (see OutputFormat for that).
///
/// | Format | Extension | Use Case |
/// |--------|-----------|----------|
/// | Jsonl | .jsonl | One JSON object per line, optimal for RAG |
/// | Auto | .auto | Auto-detect from existing files |
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum, Default,
)]
pub enum ExportFormat {
    /// JSONL format (JSON Lines - one JSON object per line)
    /// Optimal for RAG pipelines and vector database ingestion
    #[default]
    Jsonl,
    /// Vector format (JSON with metadata header)
    /// Supports embeddings and cosine similarity
    Vector,
    /// Auto-detect format from existing export files
    Auto,
}

impl ExportFormat {
    /// Parse from string (case-insensitive).
    /// Note: Named `parse_str` to avoid confusion with `FromStr::from_str`.
    pub fn parse_str(s: &str) -> Result<Self, &'static str> {
        match s.to_lowercase().as_str() {
            "jsonl" => Ok(ExportFormat::Jsonl),
            "vector" => Ok(ExportFormat::Vector),
            "auto" => Ok(ExportFormat::Auto),
            _ => Err("Invalid export format. Use 'jsonl', 'vector', or 'auto'"),
        }
    }
    /// Returns the file extension for this format
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Vector => "json",
            Self::Auto => "auto",
        }
    }

    /// Returns a human-readable name for this format
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Jsonl => "JSONL",
            Self::Vector => "Vector",
            Self::Auto => "Auto",
        }
    }
}

/// Metadata for the export state file
///
/// Stored at ~/.cache/rust_scraper/state/<domain>.json
/// Tracks which URLs have been processed for a given domain
/// to support incremental exports and resume capability.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportState {
    /// Domain this state belongs to (e.g., "example.com")
    pub domain: String,
    /// URLs that have been successfully exported
    pub processed_urls: Vec<String>,
    /// Last export timestamp
    pub last_export: Option<DateTime<Utc>>,
    /// Total documents exported
    pub total_exported: u64,
}

impl ExportState {
    /// Create a new ExportState for a domain
    #[must_use]
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            processed_urls: Vec::new(),
            last_export: None,
            total_exported: 0,
        }
    }

    /// Mark a URL as processed
    pub fn mark_processed(&mut self, url: &str) {
        if !self.processed_urls.contains(&url.to_string()) {
            self.processed_urls.push(url.to_string());
            self.total_exported += 1;
        }
    }

    /// Check if a URL has been processed
    #[must_use]
    pub fn is_processed(&self, url: &str) -> bool {
        self.processed_urls.contains(&url.to_string())
    }

    /// Update last export timestamp
    pub fn update_timestamp(&mut self) {
        self.last_export = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_format_vector_extension() {
        assert_eq!(ExportFormat::Vector.extension(), "json");
    }

    #[test]
    fn test_export_format_vector_name() {
        assert_eq!(ExportFormat::Vector.name(), "Vector");
    }

    #[test]
    fn test_export_format_parse_str_all_variants() {
        assert_eq!(ExportFormat::parse_str("jsonl"), Ok(ExportFormat::Jsonl));
        assert_eq!(ExportFormat::parse_str("vector"), Ok(ExportFormat::Vector));
        assert_eq!(ExportFormat::parse_str("auto"), Ok(ExportFormat::Auto));
    }

    #[test]
    fn test_export_format_parse_str_case_insensitive() {
        assert_eq!(ExportFormat::parse_str("JSONL"), Ok(ExportFormat::Jsonl));
        assert_eq!(ExportFormat::parse_str("Vector"), Ok(ExportFormat::Vector));
        assert_eq!(ExportFormat::parse_str("AUTO"), Ok(ExportFormat::Auto));
    }

    #[test]
    fn test_export_format_parse_str_invalid_returns_error() {
        assert!(ExportFormat::parse_str("bogus").is_err());
        assert!(ExportFormat::parse_str("json").is_err());
        assert!(ExportFormat::parse_str("markdown").is_err());
        assert!(ExportFormat::parse_str("").is_err());
    }

    #[test]
    fn test_export_state_mark_processed_increments_counter() {
        let mut state = ExportState::new("example.com");
        assert_eq!(state.total_exported, 0);

        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported, 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_no_duplicate() {
        let mut state = ExportState::new("example.com");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported, 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_multiple_urls() {
        let mut state = ExportState::new("example.com");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page2");
        state.mark_processed("https://example.com/page3");
        assert_eq!(state.total_exported, 3);
        assert!(state.is_processed("https://example.com/page1"));
        assert!(state.is_processed("https://example.com/page2"));
        assert!(!state.is_processed("https://example.com/other"));
    }

    #[test]
    fn test_export_state_update_timestamp() {
        let mut state = ExportState::new("example.com");
        assert!(state.last_export.is_none());

        state.update_timestamp();
        assert!(state.last_export.is_some());

        let ts1 = state.last_export.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        state.update_timestamp();
        let ts2 = state.last_export.unwrap();
        assert!(ts2 >= ts1);
    }

    #[test]
    fn test_export_state_serde_roundtrip() {
        let mut state = ExportState::new("example.com");
        state.mark_processed("https://example.com/page1");
        state.update_timestamp();

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ExportState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.domain, "example.com");
        assert_eq!(deserialized.total_exported, 1);
        assert!(deserialized.is_processed("https://example.com/page1"));
        assert!(deserialized.last_export.is_some());
    }

    #[test]
    fn test_export_state_default() {
        let state = ExportState::default();
        assert!(state.domain.is_empty());
        assert!(state.processed_urls.is_empty());
        assert!(state.last_export.is_none());
        assert_eq!(state.total_exported, 0);
    }

    #[test]
    fn test_export_state_is_processed_empty() {
        let state = ExportState::new("test.com");
        assert!(!state.is_processed("https://test.com/anything"));
    }

    #[test]
    fn test_export_state_mark_many_urls() {
        let mut state = ExportState::new("example.com");
        for i in 0..100 {
            state.mark_processed(&format!("https://example.com/page{i}"));
        }
        assert_eq!(state.total_exported, 100);
        assert_eq!(state.processed_urls.len(), 100);
        assert!(state.is_processed("https://example.com/page0"));
        assert!(state.is_processed("https://example.com/page99"));
    }

    #[test]
    fn test_export_format_jsonl_extension_and_name() {
        assert_eq!(ExportFormat::Jsonl.extension(), "jsonl");
        assert_eq!(ExportFormat::Jsonl.name(), "JSONL");
    }

    #[test]
    fn test_export_format_auto_extension_and_name() {
        assert_eq!(ExportFormat::Auto.extension(), "auto");
        assert_eq!(ExportFormat::Auto.name(), "Auto");
    }

    #[test]
    fn test_export_format_default_is_jsonl() {
        assert_eq!(ExportFormat::default(), ExportFormat::Jsonl);
    }

    #[test]
    fn test_export_format_serde_roundtrip() {
        for fmt in [
            ExportFormat::Jsonl,
            ExportFormat::Vector,
            ExportFormat::Auto,
        ] {
            let json = serde_json::to_string(&fmt).unwrap();
            let deserialized: ExportFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(fmt, deserialized);
        }
    }
}
