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

fn default_version() -> u32 {
    1
}

/// Wire shape accepted by `ExportState`'s `Deserialize` boundary (#1132).
///
/// The legacy `total_exported` counter is accepted for compatibility with
/// v1 state files but is NEVER stored: it is checked against
/// `processed_urls.len()` at parse time, so a desynchronized resume file
/// (`total: 999` with 0 URLs) is rejected instead of silently corrupting
/// the resume decision. The domain must be non-empty — `ExportState::new`
/// never produced `""`, and now deserialization cannot either.
#[derive(Deserialize)]
struct RawExportState {
    #[serde(default = "default_version")]
    version: u32,
    domain: String,
    processed_urls: Vec<String>,
    last_export: Option<DateTime<Utc>>,
    #[serde(default)]
    total_exported: Option<u64>,
}

impl TryFrom<RawExportState> for ExportState {
    type Error = crate::ScraperError;

    fn try_from(raw: RawExportState) -> Result<Self, Self::Error> {
        if raw.domain.trim().is_empty() {
            return Err(crate::ScraperError::Config(
                "el dominio del estado de exportación no puede estar vacío".to_string(),
            ));
        }
        if let Some(total) = raw.total_exported {
            if total != raw.processed_urls.len() as u64 {
                return Err(crate::ScraperError::Config(format!(
                    "estado de exportación inconsistente: total_exported {total} != {} URLs procesadas",
                    raw.processed_urls.len()
                )));
            }
        }
        Ok(Self {
            version: raw.version,
            domain: raw.domain,
            processed_urls: raw.processed_urls,
            last_export: raw.last_export,
        })
    }
}

/// Metadata for the export state file
///
/// Stored at `~/.cache/webfang/state/<domain>.json`
/// Tracks which URLs have been processed for a given domain
/// to support incremental exports and resume capability.
///
/// # Type-state (#1132)
///
/// * `domain` is private and non-empty: the only construction paths are
///   [`ExportState::new`] (validated) and deserialization (validated via
///   `#[serde(try_from)]`). `Default` was removed because it produced an
///   empty domain.
/// * `total_exported` is DERIVED from `processed_urls` — the stored counter
///   could desync (hand-edited or corrupt resume files); now the roundtrip
///   invariant `total_exported() == processed_urls.len()` holds by
///   construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "RawExportState")]
pub struct ExportState {
    /// Schema version for forward-compatible evolution. Current: 1.
    pub version: u32,
    /// Domain this state belongs to (e.g., "example.com"). Non-empty by
    /// construction; read via [`domain()`](Self::domain).
    domain: String,
    /// URLs that have been successfully exported
    pub processed_urls: Vec<String>,
    /// Last export timestamp
    pub last_export: Option<DateTime<Utc>>,
}

impl ExportState {
    /// Create a new ExportState for a domain.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`](crate::ScraperError::Config) when
    /// `domain` is empty or whitespace-only — an empty domain is not a
    /// valid state owner (#1132).
    pub fn new(domain: impl Into<String>) -> crate::error::Result<Self> {
        let domain = domain.into();
        if domain.trim().is_empty() {
            return Err(crate::ScraperError::Config(
                "el dominio del estado de exportación no puede estar vacío".to_string(),
            ));
        }
        Ok(Self {
            domain,
            version: 1,
            processed_urls: Vec::new(),
            last_export: None,
        })
    }

    /// The (non-empty) domain this state belongs to.
    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Total documents exported — DERIVED from `processed_urls`, never a
    /// stored counter that can desync (#1132).
    #[must_use]
    pub fn total_exported(&self) -> u64 {
        self.processed_urls.len() as u64
    }

    /// Mark a URL as processed
    pub fn mark_processed(&mut self, url: &str) {
        if !self.processed_urls.contains(&url.to_string()) {
            self.processed_urls.push(url.to_string());
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
        let mut state = ExportState::new("example.com").expect("valid domain");
        assert_eq!(state.total_exported(), 0);

        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported(), 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_no_duplicate() {
        let mut state = ExportState::new("example.com").expect("valid domain");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported(), 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_multiple_urls() {
        let mut state = ExportState::new("example.com").expect("valid domain");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page2");
        state.mark_processed("https://example.com/page3");
        assert_eq!(state.total_exported(), 3);
        assert!(state.is_processed("https://example.com/page1"));
        assert!(state.is_processed("https://example.com/page2"));
        assert!(!state.is_processed("https://example.com/other"));
    }

    #[test]
    fn test_export_state_update_timestamp() {
        let mut state = ExportState::new("example.com").expect("valid domain");
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
        let mut state = ExportState::new("example.com").expect("valid domain");
        state.mark_processed("https://example.com/page1");
        state.update_timestamp();

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ExportState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.domain(), "example.com");
        assert_eq!(deserialized.total_exported(), 1);
        assert!(deserialized.is_processed("https://example.com/page1"));
        assert!(deserialized.last_export.is_some());
    }

    /// #1132 reproduction: the invalid states that the raw-primitive shape
    /// accepted are now rejected at the boundary. Before this change the
    /// same JSON deserialized to `Ok` (a `total: 999` counter with zero
    /// processed URLs, and an empty domain, were representable).
    #[test]
    fn issue_1132_desynced_and_empty_domain_state_rejected_at_boundary() {
        let desynced = r#"{"version":1,"domain":"example.com","processed_urls":[],"last_export":null,"total_exported":999}"#;
        let err = serde_json::from_str::<ExportState>(desynced)
            .expect_err("total_exported != len(processed_urls) must be rejected");
        assert!(
            err.to_string().contains("inconsistente"),
            "rejection must name the desync, got: {err}"
        );

        let empty_domain = r#"{"version":1,"domain":"","processed_urls":[],"last_export":null,"total_exported":0}"#;
        let err = serde_json::from_str::<ExportState>(empty_domain)
            .expect_err("empty domain must be rejected at the boundary");
        assert!(
            err.to_string().contains("vacío"),
            "rejection must name the empty domain, got: {err}"
        );

        // Construction rejects it too — `new()` can no longer produce it.
        assert!(
            ExportState::new("").is_err(),
            "empty domain must fail new()"
        );
        assert!(
            ExportState::new("   ").is_err(),
            "whitespace-only domain must fail new()"
        );
    }

    /// #1132: the counter is derived, so the roundtrip invariant
    /// `total_exported == len(processed_urls)` holds by construction — the
    /// legacy field is accepted on the wire but never stored desynced.
    #[test]
    fn issue_1132_roundtrip_total_equals_processed_len() {
        let mut state = ExportState::new("example.com").expect("valid domain");
        for i in 0..3 {
            state.mark_processed(&format!("https://example.com/p{i}"));
        }
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(
            !json.contains("total_exported"),
            "derived counter must not be stored, got: {json}"
        );
        let back: ExportState = serde_json::from_str(&json).expect("valid state");
        assert_eq!(back.total_exported(), 3);
        assert_eq!(back.total_exported(), back.processed_urls.len() as u64);
        assert_eq!(back.domain(), "example.com");
    }

    #[test]
    fn test_export_state_is_processed_empty() {
        let state = ExportState::new("test.com").expect("valid domain");
        assert!(!state.is_processed("https://test.com/anything"));
    }

    #[test]
    fn test_export_state_mark_many_urls() {
        let mut state = ExportState::new("example.com").expect("valid domain");
        for i in 0..100 {
            state.mark_processed(&format!("https://example.com/page{i}"));
        }
        assert_eq!(state.total_exported(), 100);
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

    // --- Sprint 0 Gate 0: StateStore version contract (RED before GREEN) ---

    #[test]
    fn test_export_state_new_version_is_one() {
        let state = ExportState::new("example.com").expect("valid domain");
        assert_eq!(state.version, 1);
    }

    #[test]
    fn test_export_state_legacy_json_missing_version_defaults_to_one() {
        let legacy_json = r#"{"domain":"example.com","processed_urls":[],"total_exported":0}"#;
        let state: ExportState = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(
            state.version, 1,
            "legacy JSON without version must default to 1 via default_version()"
        );
    }

    #[test]
    fn test_export_state_roundtrip_preserves_version_one() {
        let state = ExportState::new("example.com").expect("valid domain");
        assert_eq!(state.version, 1);
        let json = serde_json::to_string(&state).unwrap();
        assert!(
            json.contains("\"version\":1"),
            "serialized JSON must contain version:1, got: {json}"
        );
        let deserialized: ExportState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, 1);
        assert_eq!(deserialized.domain(), "example.com");
    }

    #[test]
    fn test_export_state_explicit_version_zero_deserializes() {
        let json_zero =
            r#"{"domain":"example.com","processed_urls":[],"total_exported":0,"version":0}"#;
        let state: ExportState = serde_json::from_str(json_zero).unwrap();
        assert_eq!(
            state.version, 0,
            "explicit version:0 must be preserved for mismatch discard path"
        );
    }
}
