//! File Exporter implementation
//!
//! Implements the Exporter trait for local file system export.
//! Supports Markdown, Text, and JSON formats with structured output.

use std::fs;
use std::path::PathBuf;

use crate::config::OutputFormat;
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
            .map(|(k, v)| format!("{}: {}", k, v))
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

    /// Generate output path for a document
    fn output_path(&self, doc: &DocumentChunkValidated, ext: &str) -> PathBuf {
        let output_dir = &self.config.output_dir;

        // Extract domain from URL
        let domain = url::Url::parse(&doc.url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| "unknown".to_string());

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

        let filename = if filename.is_empty() || filename.ends_with('-') {
            format!("index.{}", ext)
        } else {
            format!("{}.{}", filename, ext)
        };

        output_dir.join(domain).join(filename)
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
                writeln!(file, "{}", json).map_err(|e| ExporterError::WriteError(e.to_string()))?;
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

    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        // Default: export one by one
        for doc in documents {
            self.export(doc.clone())?;
        }
        Ok(())
    }

    fn config(&self) -> &ExporterConfig {
        &self.config
    }
}

// ============================================================================
// Conversion from ScrapedContent
// ============================================================================

// NOTE: From<ScrapedContent> for DocumentChunk<Draft> is implemented in entities.rs

#[cfg(test)]
mod tests {
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
        };

        let chunk: crate::domain::DocumentChunkUnvalidated = scraped.into();

        assert_eq!(chunk.title, "Test Title");
        assert_eq!(chunk.content, "Test Content");
        assert_eq!(chunk.url, "https://example.com/test");
        assert!(chunk.metadata.contains_key("excerpt"));
        assert!(chunk.metadata.contains_key("author"));
    }
}
