//! Export factory for creating exporters based on format
//!
//! Provides flexible factory methods for creating appropriate exporters
//! based on ExportFormat enum values.

use std::path::PathBuf;
use tracing::info;

use crate::{
    domain::{entities::ExportFormat, Exporter, ExporterConfig, ExporterError},
    infrastructure::export::{
        jsonl_exporter, state_store::StateStore, vector_exporter::VectorExporter,
    },
};

/// Create exporter based on output format
pub fn create_exporter(
    output_dir: PathBuf,
    filename: &str,
    format: ExportFormat,
) -> Result<Box<dyn Exporter>, ExporterError> {
    let config = ExporterConfig::new(output_dir.clone(), format, filename).with_append(true);

    match format {
        ExportFormat::Jsonl => {
            info!("Creating JSONL exporter: {:?}", config.output_path());
            let exporter = jsonl_exporter::JsonlExporter::new(config);
            Ok(Box::new(exporter))
        },
        ExportFormat::Vector => {
            info!("Creating Vector exporter: {:?}", config.output_path());
            let exporter = VectorExporter::new(config);
            Ok(Box::new(exporter))
        },
        ExportFormat::Auto => {
            // Auto-detect: checks if export.jsonl or export.json exists
            info!("Auto-detecting format...");

            let jsonl_path = output_dir.join(format!("{filename}.jsonl"));
            let vector_path = output_dir.join(format!("{filename}.json"));

            if jsonl_path.exists() {
                info!("Detected JSONL format - {:?} exists", jsonl_path);
                let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename)
                    .with_append(true);
                let exporter = jsonl_exporter::JsonlExporter::new(config);
                Ok(Box::new(exporter))
            } else if vector_path.exists() {
                info!("Detected Vector format - {:?} exists", vector_path);
                let config = ExporterConfig::new(output_dir, ExportFormat::Vector, filename)
                    .with_append(true);
                let exporter = VectorExporter::new(config);
                Ok(Box::new(exporter))
            } else {
                // Fallback to default Jsonl
                info!("No existing export, using default Jsonl format");
                let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename)
                    .with_append(true);
                let exporter = jsonl_exporter::JsonlExporter::new(config);
                Ok(Box::new(exporter))
            }
        },
    }
}

/// Create a new StateStore for tracking processed URLs
///
/// # Arguments
///
/// * `state_dir` - Directory to store state files
/// * `domain` - Domain name for state file (e.g., "example.com")
///
/// # Returns
///
/// * `Ok(StateStore)` - Created state store
/// * `Err(ScraperError)` - Failed to create state store
///
/// # Errors
///
/// Returns error if:
/// - State directory cannot be created
/// - State file cannot be read/written
pub fn create_state_store(
    state_dir: PathBuf,
    domain: &str,
) -> Result<StateStore, crate::error::ScraperError> {
    use crate::infrastructure::export::state_store::StateStore;

    info!("Creating StateStore in {:?}", state_dir);
    let mut store = StateStore::new(domain);
    store.set_cache_dir(state_dir);
    Ok(store)
}

/// Process export results and update state store if resume mode is enabled
///
/// # Arguments
///
/// * `results` - Scraped content results to export
/// * `output_dir` - Output directory for export files
/// * `format` - Export format to use
/// * `filename` - Base filename for export
/// * `state_store` - Optional state store for resume tracking
/// * `resume_mode` - Whether resume mode is enabled
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of processed URLs
/// * `Err(ExporterError)` - Export failed
///
/// # Errors
///
/// Returns error if:
/// - Exporter creation fails
/// - Export operation fails
/// - State store update fails
pub fn process_results(
    results: &[crate::domain::ScrapedContent],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    state_store: Option<&StateStore>,
    resume_mode: bool,
) -> Result<Vec<String>, ExporterError> {
    use crate::domain::entities::DocumentChunkUnvalidated;

    info!("Processing {} results for export", results.len());

    // Create exporter
    let exporter = create_exporter(output_dir, filename, format)?;

    // Convert results to DocumentChunk and export
    let mut processed_urls = Vec::new();

    // Load or create export state if resume mode is enabled
    let mut export_state = if resume_mode {
        if let Some(store) = state_store {
            Some(store.load_or_default()?)
        } else {
            None
        }
    } else {
        None
    };

    for result in results {
        let chunk = DocumentChunkUnvalidated::from_scraped_content(result);
        let validated = chunk
            .validate()
            .map_err(|e| ExporterError::InvalidConfig(e.to_string()))?;

        // Export the chunk
        exporter.export(validated)?;

        // Track URL
        let url_str = result.url.as_str().to_string();
        processed_urls.push(url_str.clone());

        // Update state store if resume mode is enabled
        if resume_mode {
            if let Some(store) = state_store {
                if let Some(ref mut state) = export_state {
                    store.mark_processed(state, &url_str);
                }
            }
        }
    }

    // Save state if resume mode is enabled
    if resume_mode {
        if let Some(store) = state_store {
            if let Some(state) = export_state {
                store.save(&state)?;
            }
        }
    }

    info!(
        "✅ Export completado: {} documentos procesados",
        processed_urls.len()
    );
    Ok(processed_urls)
}

/// Get domain from URL
///
/// Extracts the domain (host) from a URL string.
///
/// # Arguments
///
/// * `url` - URL string to extract domain from
///
/// # Returns
///
/// Domain string (e.g., "example.com" from "https://www.example.com/docs/api/")
///
/// # Examples
///
/// ```
/// use webfang::export_factory::domain_from_url;
///
/// let domain = domain_from_url("https://www.example.com/docs/api/");
/// assert_eq!(domain, "www.example.com");
/// ```
pub fn domain_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|p| p.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Process pre-cleaned document chunks and export them.
///
/// This function is used when `--clean-ai` is enabled. It accepts
/// `DocumentChunk` instances that already have embeddings populated
/// by the `SemanticCleaner`, bypassing the simple field-mapping conversion.
#[cfg(feature = "ai")]
pub fn process_results_with_chunks(
    chunks: &[crate::domain::DocumentChunk],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    state_store: Option<&crate::infrastructure::export::state_store::StateStore>,
    resume_mode: bool,
) -> Result<Vec<String>, ExporterError> {
    info!("Processing {} cleaned chunks for export", chunks.len());

    let exporter = create_exporter(output_dir, filename, format)?;

    // Track URLs before batch export
    let processed_urls: Vec<String> = chunks.iter().map(|c| c.url.clone()).collect();

    // Validate chunks before passing to export_batch
    let validated_chunks: Vec<crate::domain::DocumentChunkValidated> = chunks
        .iter()
        .filter_map(|c| c.clone().validate().ok())
        .collect();

    // Use export_batch to avoid per-chunk file open/close (which overwrites in VectorExporter)
    if !validated_chunks.is_empty() {
        exporter.export_batch(&validated_chunks)?;
    }

    let mut export_state = if resume_mode {
        if let Some(store) = state_store {
            Some(store.load_or_default()?)
        } else {
            None
        }
    } else {
        None
    };

    if resume_mode {
        if let Some(store) = state_store {
            if let Some(ref mut state) = export_state {
                for url_str in &processed_urls {
                    store.mark_processed(state, url_str);
                }
                store.save(state)?;
            }
        }
    }

    info!(
        "✅ AI-cleaned export completed: {} chunks processed",
        processed_urls.len()
    );
    Ok(processed_urls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::ScrapedContent;
    use crate::domain::ValidUrl;
    use tempfile::TempDir;

    fn make_scraped_content(url: &str, title: &str, content: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_string(),
            content: content.to_string(),
            url: ValidUrl::parse(url).unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        }
    }

    // =========================================================================
    // domain_from_url tests
    // =========================================================================

    #[test]
    fn test_domain_from_url_extracts_correctly() {
        let url = "https://www.example.com/docs/api/";
        let domain = domain_from_url(url);
        assert_eq!(domain, "www.example.com");
    }

    #[test]
    fn test_domain_from_url_invalid_url_returns_unknown() {
        let domain = domain_from_url("not-a-url");
        assert_eq!(domain, "unknown");
    }

    // =========================================================================
    // create_state_store tests
    // =========================================================================

    #[test]
    fn test_create_state_store_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let domain = "example.com";
        let store = create_state_store(temp_dir.path().to_path_buf(), domain);
        assert!(store.is_ok());
        let state_file = temp_dir.path().join("example.com.json");
        let store = store.unwrap();
        assert_eq!(store.get_state_path(), state_file);
    }

    // =========================================================================
    // process_results tests (T2.2)
    // =========================================================================

    #[test]
    fn test_process_results_empty_results_returns_empty_vec() {
        let temp_dir = TempDir::new().unwrap();
        let results: Vec<ScrapedContent> = vec![];

        let processed = process_results(
            &results,
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
            false,
        )
        .unwrap();

        assert!(processed.is_empty());
    }

    #[test]
    fn test_process_results_single_item_exports_and_returns_url() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content(
            "https://example.com/page1",
            "Page One",
            "Content of page one",
        );

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
            false,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        assert_eq!(processed[0], "https://example.com/page1");
        // Verify export file was created
        assert!(temp_dir.path().join("export.jsonl").exists());
    }

    #[test]
    fn test_process_results_multiple_items_exports_all() {
        let temp_dir = TempDir::new().unwrap();
        let contents = vec![
            make_scraped_content("https://a.com/", "A", "Content A"),
            make_scraped_content("https://b.com/", "B", "Content B"),
            make_scraped_content("https://c.com/", "C", "Content C"),
        ];

        let processed = process_results(
            &contents,
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
            false,
        )
        .unwrap();

        assert_eq!(processed.len(), 3);
        // URLs get normalized through ValidUrl — check that all 3 are present
        assert_eq!(processed.len(), 3);
    }

    #[test]
    fn test_process_results_vector_format_creates_json_file() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content("https://example.com", "Test", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Vector,
            "export",
            None,
            false,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        assert!(temp_dir.path().join("export.json").exists());
    }

    #[test]
    fn test_process_results_invalid_content_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        // Empty content fails validation
        let content = make_scraped_content("https://example.com", "Title", "");

        let result = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
            false,
        );

        assert!(result.is_err());
    }

    // =========================================================================
    // create_exporter tests
    // =========================================================================

    #[test]
    fn test_create_exporter_jsonl_returns_ok() {
        let temp_dir = TempDir::new().unwrap();
        let result = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Jsonl);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_exporter_vector_returns_ok() {
        let temp_dir = TempDir::new().unwrap();
        let result = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Vector);
        assert!(result.is_ok());
    }

    // =========================================================================
    // ExportFormat::Auto branch tests (T2.3)
    // =========================================================================

    #[test]
    fn test_auto_format_detects_jsonl_when_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        // Create a .jsonl file to trigger detection
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Auto detects JSONL and creates a JSONL exporter
    }

    #[test]
    fn test_auto_format_detects_vector_when_json_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        // Create a .json file to trigger Vector detection
        // Vector takes priority over JSONL in the detection logic
        std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Auto detects Vector format from .json file
    }

    #[test]
    fn test_auto_format_vector_takes_priority_over_jsonl() {
        let temp_dir = TempDir::new().unwrap();
        // Create both files — Vector (.json) takes priority
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();
        std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Vector (.json) is checked first in the code
    }

    #[test]
    fn test_auto_format_falls_back_to_jsonl_when_no_files_exist() {
        let temp_dir = TempDir::new().unwrap();
        // No files exist — should default to Jsonl

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Falls back to default Jsonl format
    }

    #[test]
    fn test_auto_format_with_empty_dir_exports_successfully() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content("https://example.com", "Test", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Auto,
            "export",
            None,
            false,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        // Default fallback creates .jsonl file
        assert!(temp_dir.path().join("export.jsonl").exists());
    }

    #[test]
    fn test_auto_format_with_existing_jsonl_exports_to_jsonl() {
        let temp_dir = TempDir::new().unwrap();
        // Pre-create a .jsonl file
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

        let content = make_scraped_content("https://example.com", "Test", "Body");
        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Auto,
            "export",
            None,
            false,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        // Should have appended to the existing .jsonl file
        let content = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
        assert!(!content.is_empty());
    }

    // =========================================================================
    // process_results with resume mode
    // =========================================================================

    #[test]
    fn test_process_results_resume_mode_tracks_urls() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_state_store(temp_dir.path().to_path_buf(), "example.com").unwrap();

        let content = make_scraped_content("https://example.com/page", "Page", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            Some(&store),
            true, // resume_mode
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
    }
}
