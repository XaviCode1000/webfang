//! Rust Scraper Library
//!
//! Modern web scraper for RAG datasets with clean content extraction.

pub mod config;
pub mod error;
pub mod scraper;
pub mod url_path;

// Asset detection and download modules
pub mod detector;
pub mod downloader;
pub mod extractor;

pub use clap::{Parser, ValueEnum};
pub use error::{Result, ScraperError};
pub use scraper::{
    create_http_client, save_results, scrape_with_config, scrape_with_readability, DownloadedAsset,
    ScrapedContent, ValidUrl,
};
pub use std::path::PathBuf;
pub use url_path::{Domain, OutputPath, UrlPath};

// Re-export OutputFormat for convenience
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Markdown format (recomendado para RAG)
    Markdown,
    /// Plain text sin formato
    Text,
    /// JSON estructurado
    Json,
}

/// Configuration for asset downloading
#[derive(Debug, Clone, Default)]
pub struct ScraperConfig {
    /// Enable image downloading
    pub download_images: bool,
    /// Enable document downloading (PDF, DOCX, XLSX, etc.)
    pub download_documents: bool,
    /// Output directory for downloaded assets
    pub output_dir: PathBuf,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: Option<u64>,
}

impl ScraperConfig {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable image downloading
    pub fn with_images(mut self) -> Self {
        self.download_images = true;
        self
    }

    /// Enable document downloading
    pub fn with_documents(mut self) -> Self {
        self.download_documents = true;
        self
    }

    /// Set custom output directory
    pub fn with_output_dir(mut self, dir: PathBuf) -> Self {
        self.output_dir = dir;
        self
    }

    /// Check if any download is enabled
    pub fn has_downloads(&self) -> bool {
        self.download_images || self.download_documents
    }
}

/// CLI Arguments - URL es OBLIGATORIA, no hay default
#[derive(Parser, Debug)]
#[command(name = "rust-scraper")]
#[command(about = "Modern web scraper for RAG datasets with clean content extraction", long_about = None)]
pub struct Args {
    /// URL objetivo a scrapear (OBLIGATORIA)
    /// Ejemplo: https://example.com/article
    #[arg(short, long, required = true, help = "URL to scrape (required)")]
    pub url: String,

    /// Selector CSS opcional para extraer contenido específico
    /// Si no se especifica, extrae todo el contenido legible
    #[arg(short, long, default_value = "body", help = "CSS selector (optional)")]
    pub selector: String,

    /// Directorio de salida para los archivos generados
    #[arg(short, long, default_value = "output", help = "Output directory")]
    pub output: PathBuf,

    /// Formato de salida
    #[arg(short, long, default_value = "markdown", value_enum)]
    pub format: OutputFormat,

    /// Delay entre requests (en milisegundos)
    #[arg(long, default_value = "1000", help = "Delay between requests (ms)")]
    pub delay_ms: u64,

    /// Máximo de páginas a scrapear
    #[arg(long, default_value = "10", help = "Maximum pages to scrape")]
    pub max_pages: usize,

    /// Descargar imágenes encontradas en la página
    #[arg(long, default_value = "false", help = "Download images from the page")]
    pub download_images: bool,

    /// Descargar documentos encontrados en la página (PDF, DOCX, XLSX, etc.)
    #[arg(
        long,
        default_value = "false",
        help = "Download documents from the page"
    )]
    pub download_documents: bool,

    /// Verbosity del logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// Valida y parsea una URL - retorna error claro si es inválida
pub fn validate_and_parse_url(url: &str) -> Result<url::Url> {
    if url.is_empty() {
        return Err(ScraperError::invalid_url("URL cannot be empty"));
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ScraperError::invalid_url(
            "URL must start with http:// or https://",
        ));
    }

    let parsed = url::Url::parse(url)
        .map_err(|e| ScraperError::invalid_url(format!("Failed to parse URL: {}", e)))?;

    if parsed.host_str().is_none() {
        return Err(ScraperError::invalid_url("URL must have a valid host"));
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // Tests: validate_and_parse_url - Happy Path
    // ==========================================================================

    #[test]
    fn test_validate_https_url_success() {
        // Arrange
        let url = "https://example.com/article";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("example.com"));
    }

    #[test]
    fn test_validate_http_url_success() {
        // Arrange
        let url = "http://example.com";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("example.com"));
    }

    #[test]
    fn test_validate_url_with_path_success() {
        // Arrange
        let url = "https://example.com/blog/post-123";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.path(), "/blog/post-123");
    }

    #[test]
    fn test_validate_url_with_query_params_success() {
        // Arrange
        let url = "https://example.com/search?q=rust&page=1";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert!(parsed.query().is_some());
        assert_eq!(parsed.query_pairs().count(), 2);
    }

    #[test]
    fn test_validate_url_with_port_success() {
        // Arrange
        let url = "http://localhost:8080/api/data";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.host_str(), Some("localhost"));
        assert_eq!(parsed.port(), Some(8080));
    }

    #[test]
    fn test_validate_url_subdomain_success() {
        // Arrange
        let url = "https://blog.example.com/posts/tech";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.host_str(), Some("blog.example.com"));
    }

    // ==========================================================================
    // Tests: validate_and_parse_url - Edge Cases (Invalid URLs)
    // ==========================================================================

    #[test]
    fn test_validate_empty_url_fails() {
        // Arrange
        let url = "";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("empty"),
            "Error should mention empty"
        );
    }

    #[test]
    fn test_validate_url_missing_scheme_fails() {
        // Arrange
        let url = "example.com";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("http://"),
            "Error should mention http:// or https://"
        );
    }

    #[test]
    fn test_validate_url_ftp_scheme_fails() {
        // Arrange
        let url = "ftp://example.com";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_invalid_format_fails() {
        // Arrange
        let url = "not-a-valid-url";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_only_slashes_fails() {
        // Arrange
        let url = "http://";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_err());
        let err = result.unwrap_err();
        // The url crate returns "Failed to parse URL" for this case
        let error_str = err.to_string();
        assert!(
            !error_str.is_empty(),
            "Error should not be empty, got: {}",
            error_str
        );
    }

    #[test]
    fn test_validate_url_ip_localhost_success() {
        // Arrange
        let url = "http://127.0.0.1:3000/api";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), Some(3000));
    }

    #[test]
    fn test_validate_url_ip_v4_success() {
        // Arrange
        let url = "http://192.168.1.1/admin";

        // Act
        let result = validate_and_parse_url(url);

        // Assert
        assert!(result.is_ok());
    }
}
