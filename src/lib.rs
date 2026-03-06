//! Rust Scraper Library
//!
//! Modern web scraper for RAG datasets with clean content extraction.

pub mod config;
pub mod markdown;
pub mod scraper;

pub use clap::{Parser, ValueEnum};
pub use scraper::{create_http_client, save_results, scrape_with_readability, ScrapedContent};
pub use std::path::PathBuf;

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

    /// Verbosity del logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            url: String::new(),
            selector: "body".to_string(),
            output: PathBuf::from("output"),
            format: OutputFormat::Markdown,
            delay_ms: 1000,
            max_pages: 10,
            verbose: 0,
        }
    }
}

/// Valida y parsea una URL - retorna error claro si es inválida
pub fn validate_and_parse_url(url: &str) -> anyhow::Result<url::Url> {
    use anyhow::Context;

    if url.is_empty() {
        anyhow::bail!("URL cannot be empty");
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        anyhow::bail!("URL must start with http:// or https://");
    }

    let parsed = url::Url::parse(url).context("Failed to parse URL - check format")?;

    if parsed.host_str().is_none() {
        anyhow::bail!("URL must have a valid host");
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
