//! Export factory for creating exporters based on format
//!
//! Provides flexible factory methods for creating appropriate exporters
//! based on ExportFormat enum values.

use std::path::PathBuf;
use tracing::info;

use crate::{
    domain::{entities::ExportFormat, Exporter, ExporterConfig, ExporterError},
    infrastructure::export::{
        jsonl_exporter, state_store::StateStore, zvec_exporter::ZvecExporter,
    },
};

/// Create exporter based on output format
pub fn create_exporter(
    output_dir: PathBuf,
    filename: &str,
    format: ExportFormat,
) -> Result<Box<dyn Exporter>, ExporterError> {
    let config = ExporterConfig::new(output_dir.clone(), format, filename);

    match format {
        ExportFormat::Jsonl => {
            info!("Creating JSONL exporter: {:?}", config.output_path());
            let exporter = jsonl_exporter::JsonlExporter::new(config);
            Ok(Box::new(exporter))
        }
        ExportFormat::Zvec => {
            if ZvecExporter::is_available() {
                info!("Creating Zvec exporter: {:?}", config.output_path());
                let exporter = ZvecExporter::new(config);
                Ok(Box::new(exporter))
            } else {
                Err(ExporterError::InvalidConfig(
                    "Zvec format requires zvec feature enabled".to_string(),
                ))
            }
        }
        ExportFormat::Auto => {
            // Auto-detect: checks if export.jsonl exists, then uses Jsonl
            info!("Auto-detecting format...");

            let jsonl_path = output_dir.join(format!("{}.jsonl", filename));
            if jsonl_path.exists() {
                info!("Detected JSONL format - {:?} exists", jsonl_path);
                let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename);
                let exporter = jsonl_exporter::JsonlExporter::new(config);
                Ok(Box::new(exporter))
            } else {
                // Fallback to default Jsonl
                info!("No existing export, using default Jsonl format");
                let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename);
                let exporter = jsonl_exporter::JsonlExporter::new(config);
                Ok(Box::new(exporter))
            }
        }
        _ => Err(ExporterError::InvalidConfig(format!(
            "Format {:?} not yet implemented in Exporter trait",
            format
        ))),
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
    use crate::domain::entities::DocumentChunk;

    info!("Processing {} results for export", results.len());

    // Create exporter
    let exporter = create_exporter(output_dir, filename, format)?;

    // Convert results to DocumentChunk and export
    let mut processed_urls = Vec::new();

    // Load or create export state if resume mode is enabled
    let mut export_state = if resume_mode && state_store.is_some() {
        Some(state_store.unwrap().load_or_default()?)
    } else {
        None
    };

    for result in results {
        let chunk = DocumentChunk::from_scraped_content(result);

        // Export the chunk
        exporter.export(chunk)?;

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
        "✅ Export completed: {} documents processed",
        processed_urls.len()
    );
    Ok(processed_urls)
}

/// Get domain from URL
pub fn domain_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|p| p.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}
