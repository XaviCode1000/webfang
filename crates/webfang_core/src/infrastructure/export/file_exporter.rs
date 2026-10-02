//! File Exporter implementation
//!
//! Implements the Exporter trait for local file system export.
//! Supports Markdown, Text, and JSON formats with structured output.

use std::fs;
use std::path::PathBuf;

#[cfg(test)]
use tracing_subscriber::layer::SubscriberExt;

use crate::domain::config::OutputFormat;
use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::entities::DocumentChunkValidated;
use crate::domain::exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};

/// File-based exporter implementing the Exporter trait
///
/// This is the infrastructure adapter that bridges the domain's Exporter trait
/// with the local file system. It replaces the legacy functions in file_saver.rs.
#[derive(Debug)]
pub struct FileExporter {
    config: ExporterConfig,
}

impl FileExporter {
    /// Create a new FileExporter with the given configuration
    #[must_use]
    pub fn new(config: ExporterConfig) -> Self {
        Self { config }
    }

    /// Create from output directory and format
    #[must_use]
    pub fn new_with_path(
        output_dir: PathBuf,
        format: OutputFormat,
        filename: impl Into<String>,
    ) -> Self {
        // Map OutputFormat to ExportFormat
        let export_format = match format {
            OutputFormat::Markdown => crate::domain::entities::ExportFormat::Jsonl, // Use Jsonl for file export
            OutputFormat::Text => crate::domain::entities::ExportFormat::Jsonl,
            OutputFormat::Json => crate::domain::entities::ExportFormat::Jsonl,
        };

        let config = ExporterConfig::new(output_dir, export_format, filename);
        Self::new(config)
    }

    /// Export a single document as Markdown
    #[allow(dead_code)]
    fn save_md(&self, doc: &DocumentChunkValidated) -> ExportResult<()> {
        let path = self.output_path(doc, "md");

        // Build markdown content with YAML frontmatter
        let content = format!(
            "---\n\
             title: {}\n\
             url: {}\n\
             date: {}\n\
             ---\n\n\
             {}",
            doc.title,
            doc.url,
            doc.timestamp.format("%Y-%m-%d"),
            doc.content
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        }

        fs::write(&path, content).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        tracing::info!("💾 Saved: {}", path.display());
        Ok(())
    }

    /// Export a single document as structured Text
    #[allow(dead_code)]
    fn save_txt(&self, doc: &DocumentChunkValidated) -> ExportResult<()> {
        let path = self.output_path(doc, "txt");

        // Extract metadata as formatted string
        let metadata = doc
            .metadata
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");

        let content = format!(
            "========================================\n\
             TITLE: {}\n\
             URL: {}\n\
             TIMESTAMP: {}\n\
             ----------------------------------------\n\
             METADATA:\n\
             {}\n\
             ----------------------------------------\n\
             CONTENT:\n\
             {}\n\
             ========================================",
            doc.title,
            doc.url,
            doc.timestamp.format("%Y-%m-%d %H:%M:%S"),
            if metadata.is_empty() {
                "N/A".to_string()
            } else {
                metadata
            },
            doc.content
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        }

        fs::write(&path, content).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        tracing::info!("💾 Saved: {}", path.display());
        Ok(())
    }

    /// Export a single document as JSON
    fn save_json(&self, doc: &DocumentChunkValidated) -> ExportResult<()> {
        let path = self.output_path(doc, "json");

        let json = serde_json::to_string_pretty(doc).map_err(ExporterError::Serialization)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        }

        fs::write(&path, json).map_err(|e| ExporterError::WriteError(e.to_string()))?;
        tracing::info!("💾 Saved: {}", path.display());
        Ok(())
    }

    /// Export every document one at a time.
    ///
    /// The write loop of [`Exporter::export_batch`], split out so the
    /// instrumented wrapper can record the batch outcome on its span
    /// (#1610, OBS-H6) without touching this loop's control flow.
    fn export_batch_documents(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        // Default: export one by one
        for doc in documents {
            self.export(doc.clone())?;
        }
        Ok(())
    }

    /// Generate output path for a document.
    ///
    /// Both the domain directory and the file stem are confined to single
    /// safe components at join time (#1125): the flattening below is kept
    /// for stable names, and [`confine_filename_component`] closes the
    /// remaining holes (backslash traversal on Windows, `..` segments,
    /// control characters) that ad-hoc `replace` chains cannot cover.
    fn output_path(&self, doc: &DocumentChunkValidated, ext: &str) -> PathBuf {
        let output_dir = &self.config.output_dir;

        // Extract domain from URL
        let domain = url::Url::parse(&doc.url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| "unknown".to_string());
        let domain = confine_filename_component(&domain, "unknown");

        // Generate filename from URL path
        #[allow(clippy::collapsible_str_replace)]
        let filename = doc
            .url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .replace('/', "-")
            .replace('?', "_")
            .replace('&', "_")
            .replace(':', "_");

        let stem = if filename.is_empty() || filename.ends_with('-') {
            "index".to_string()
        } else {
            confine_filename_component(&filename, "index")
        };

        output_dir.join(domain).join(format!("{stem}.{ext}"))
    }
}

impl Exporter for FileExporter {
    fn export(&self, document: DocumentChunkValidated) -> ExportResult<()> {
        // Use config's format to determine export method
        let format = self.config.format;

        // Map to save methods - we use config's format field for format selection
        match format {
            crate::domain::entities::ExportFormat::Jsonl => {
                // JSONL: append mode
                let json =
                    serde_json::to_string(&document).map_err(ExporterError::Serialization)?;

                let path = self.config.output_path();
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| ExporterError::WriteError(e.to_string()))?;
                }

                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|e| ExporterError::WriteError(e.to_string()))?;

                use std::io::Write;
                writeln!(file, "{json}").map_err(|e| ExporterError::WriteError(e.to_string()))?;
                Ok(())
            },
            crate::domain::entities::ExportFormat::Vector => {
                // Vector format: save as JSON
                self.save_json(&document)
            },
            crate::domain::entities::ExportFormat::Auto => {
                // Default to JSON
                self.save_json(&document)
            },
        }
    }

    // `payload_bytes` semantics (shared by all three exporters, #1610): the
    // serialized document bytes THIS batch handed to its writer. The file
    // exporter's Jsonl branch serializes inside `export`, and its Auto/Vector
    // branches delegate to `save_json`, neither of which reports a size — so
    // the field is left unrecorded here rather than filled with an invented
    // number. Absence is the honest reading; the jsonl and vector exporters
    // document their own (known) figures on the same key.
    #[tracing::instrument(
        skip(self, documents),
        fields(
            exporter = "file",
            documents = documents.len(),
            outcome = tracing::field::Empty,
            payload_bytes = tracing::field::Empty
        )
    )]
    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        let outcome = self.export_batch_documents(documents);
        match &outcome {
            Ok(()) => record_export_outcome("ok", None),
            Err(_) => record_export_outcome("error", None),
        }
        outcome
    }

    fn config(&self) -> &ExporterConfig {
        &self.config
    }
}

// ============================================================================
// Conversion from ScrapedContent
// ============================================================================

/// Record how an `export_batch` call ended on its own span (#1610, OBS-H6).
///
/// A span that only knows "N documents, M seconds" cannot answer "did the
/// export land?", so the outcome is declared `Empty` and recorded here.
/// `payload_bytes` is `None` for exporters whose writes are delegated to a
/// helper that reports no size: an absent field is the honest reading, while
/// a made-up number would be indistinguishable from a real one offline.
fn record_export_outcome(outcome: &str, payload_bytes: Option<u64>) {
    let span = tracing::Span::current();
    span.record("outcome", outcome);
    if let Some(bytes) = payload_bytes {
        span.record("payload_bytes", bytes);
    }
}

// NOTE: From<ScrapedContent> for DocumentChunk<Draft> is implemented in entities.rs

/// Run `body` on its own current-thread runtime under a real
/// `FileTraceLayer` and parse the emitted JSONL (#1610).
///
/// Canonical home of the exporter span-capture harness: the jsonl and vector
/// exporter test modules reuse it so all three prove the same contract
/// (recorded span fields reach the trace file) with the same parser.
#[cfg(test)]
pub(crate) fn capture_export_trace<F, Fut>(body: F) -> Vec<serde_json::Value>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("trace.jsonl");
    let layer = crate::infrastructure::observability::FileTraceLayer::new(path.clone())
        .expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()));
    drop(runtime);
    // Flush before reading: the layer buffers and only drains on drop.
    drop(dispatch);
    std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::capture_export_trace;
    use super::*;
    use std::env::temp_dir;

    fn make_test_doc() -> DocumentChunkValidated {
        use crate::domain::DocumentChunkUnvalidated;
        let unvalidated = DocumentChunkUnvalidated {
            id: uuid::Uuid::new_v4(),
            url: "https://example.com/page".to_string(),
            title: "Test Page".to_string(),
            content: "This is test content.".to_string(),
            metadata: [("author".to_string(), "Test Author".to_string())]
                .into_iter()
                .collect(),
            timestamp: chrono::Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: std::marker::PhantomData,
        };
        // Validate before export
        unvalidated.validate().unwrap()
    }

    #[test]
    fn test_file_exporter_json() {
        let dir = temp_dir().join("exporter_test_json");
        let format = crate::domain::entities::ExportFormat::Jsonl;
        let config = ExporterConfig::new(dir.clone(), format, "test");
        let exporter = FileExporter::new(config);

        let doc = make_test_doc();
        let result = exporter.export(doc);

        assert!(result.is_ok());

        // Cleanup
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hostile_document_url_is_confined_inside_output_dir() {
        // Issue #1125: hostile URL paths, backslashes and queries must
        // collapse so the written file always stays exactly two levels deep:
        // `<output_dir>/<domain>/<file>`.
        let dir = temp_dir().join("exporter_test_traversal");
        let format = crate::domain::entities::ExportFormat::Jsonl;
        let exporter = FileExporter::new(ExporterConfig::new(dir.clone(), format, "test"));

        for hostile_url in [
            "https://example.com/../escape",
            "https://example.com/a\\..\\escape",
            "https://example.com/?q=..\\..\\escape",
        ] {
            let mut doc = make_test_doc();
            doc.url = hostile_url.to_string();
            let path = exporter.output_path(&doc, "md");
            let relative = path
                .strip_prefix(&dir)
                .expect("must stay inside output_dir");
            assert_eq!(
                relative.components().count(),
                2,
                "{hostile_url:?} escaped: {}",
                path.display()
            );
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_conversion_from_scraped_content() {
        use crate::domain::{ScrapedContent, ValidUrl};

        let url = ValidUrl::parse("https://example.com/test").unwrap();
        let scraped = ScrapedContent {
            title: "Test Title".to_string(),
            content: "Test Content".to_string(),
            url,
            excerpt: Some("Test Excerpt".to_string()),
            author: Some("Test Author".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };

        let chunk: crate::domain::DocumentChunkUnvalidated = scraped.into();

        assert_eq!(chunk.title, "Test Title");
        assert_eq!(chunk.content, "Test Content");
        assert_eq!(chunk.url, "https://example.com/test");
        assert!(chunk.metadata.contains_key("excerpt"));
        assert!(chunk.metadata.contains_key("author"));
    }

    /// #1610 (OBS-H6): the `export_batch` span must say whether the export landed
    /// — and must NOT invent a byte count it cannot know.
    ///
    /// The file exporter's Auto/Vector branches delegate to `save_json`, which
    /// reports no size, so `payload_bytes` stays absent: an absent key is honest,
    /// a fabricated number would be indistinguishable from a measured one.
    #[test]
    fn export_batch_span_records_outcome_and_omits_unknown_bytes() {
        let dir = temp_dir().join("exporter_test_span_outcome");
        let config = ExporterConfig::new(
            dir.clone(),
            crate::domain::entities::ExportFormat::Jsonl,
            "test",
        );
        let exporter = FileExporter::new(config);

        let records = capture_export_trace(|| async {
            exporter
                .export_batch(&[make_test_doc(), make_test_doc()])
                .expect("batch export must succeed");
        });

        let closes: Vec<_> = records
            .iter()
            .filter(|r| r["record"] == "span_close" && r["span"] == "export_batch")
            .collect();
        assert_eq!(closes.len(), 1, "exactly one export_batch span");
        let fields = closes[0]["span_fields"].as_object().expect("span_fields");
        assert_eq!(fields.get("outcome").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(
            fields.get("exporter").and_then(|v| v.as_str()),
            Some("file")
        );
        assert_eq!(fields.get("documents").and_then(|v| v.as_u64()), Some(2));
        assert!(
            !fields.contains_key("payload_bytes"),
            "the file exporter cannot measure its payload — it must omit the key, not guess: {fields:?}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// #1610: the failure arm. An unwritable output directory is a batch error,
    /// and the span must say so instead of closing as a bare duration.
    #[test]
    fn export_batch_span_records_error_outcome() {
        let dir = temp_dir().join("exporter_test_span_error");
        let _ = std::fs::remove_dir_all(&dir);
        // A plain FILE where a directory is required: `create_dir_all` fails for
        // every user and on every OS, without a permissions fixture.
        std::fs::write(&dir, b"not a directory").expect("fixture file");
        let config = ExporterConfig::new(
            dir.clone(),
            crate::domain::entities::ExportFormat::Jsonl,
            "test",
        );
        let exporter = FileExporter::new(config);

        let records = capture_export_trace(|| async {
            let result = exporter.export_batch(&[make_test_doc()]);
            assert!(result.is_err(), "unwritable output must fail the batch");
        });

        let closes: Vec<_> = records
            .iter()
            .filter(|r| r["record"] == "span_close" && r["span"] == "export_batch")
            .collect();
        assert_eq!(closes.len(), 1, "exactly one export_batch span");
        let fields = closes[0]["span_fields"].as_object().expect("span_fields");
        assert_eq!(
            fields.get("outcome").and_then(|v| v.as_str()),
            Some("error")
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
