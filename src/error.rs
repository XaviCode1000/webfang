//! Error handling module for rust_scraper.
//!
//! Uses thiserror for library error types (err-thiserror-lib).
//! This provides type-safe, structured error handling instead of anyhow.

use thiserror::Error;

/// Main error type for the scraper library.
///
/// Each variant represents a specific failure mode, making it easy to:
/// - Handle specific errors in calling code
/// - Convert to/from other error types
/// - Provide meaningful error messages to users
#[derive(Error, Debug)]
pub enum ScraperError {
    /// URL is invalid or cannot be parsed
    #[error("URL inválida: {0}")]
    InvalidUrl(String),

    /// HTTP request failed with a status code
    #[error("http error {status} al acceder a {url}")]
    Http {
        /// The HTTP status code
        status: u16,
        /// The URL that was being accessed
        url: String,
    },

    /// Content extraction failed (Readability algorithm)
    #[error("Error de legibilidad: {0}")]
    Readability(String),

    /// I/O error (file system, etc.)
    #[error("Error de I/O: {0}")]
    Io(#[from] std::io::Error),

    /// Network error (connection failed, timeout, etc.)
    #[error("error de red: {0}")]
    Network(String),

    /// Middleware error (from reqwest-middleware, e.g., retry failures)
    #[error("error de middleware: {0}")]
    Middleware(String),

    /// Serialization/Deserialization error (JSON, YAML, etc.)
    #[error("Error de serialización: {0}")]
    Serialization(#[from] serde_json::Error),

    /// YAML serialization error
    #[error("Error de YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// URL parse error
    #[error("Error de parseo de URL: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Asset extraction failed
    #[error("Error de extracción: {0}")]
    Extraction(String),

    /// Asset download failed
    #[error("Error de descarga: {0}")]
    Download(String),

    /// Configuration error
    #[error("Error de configuración: {0}")]
    Config(String),

    /// WAF/CAPTCHA challenge detected in HTTP 200 response
    #[error("WAF/CAPTCHA detectado en {url}: {provider}")]
    WafBlocked {
        /// URL that was blocked
        url: String,
        /// Detected WAF provider (e.g., "Cloudflare", "DataDome", "reCAPTCHA")
        provider: String,
    },

    /// URL validation failed
    #[error("Validación de URL falló: {0}")]
    Validation(String),

    /// Conversion error (HTML to Markdown, YAML, etc.)
    #[error("Error de conversión: {0}")]
    Conversion(String),

    /// Export operation failed
    #[error("Error de exportación: {0}")]
    Export(String),

    /// Batch export failed (partial success)
    #[error("Error de exportación en batch: {0}")]
    ExportBatch(String),

    /// Semantic cleaning error (AI-powered content processing)
    #[cfg(feature = "ai")]
    #[error("Error de limpieza semántica: {0}")]
    Semantic(#[from] SemanticError),
}

/// Semantic cleaning errors (AI/ML operations)
///
/// These errors occur during AI-powered semantic cleaning operations:
/// - Model loading from cache
/// - Tokenization of input text
/// - ONNX inference
/// - Model download from HuggingFace Hub
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::SemanticError;
/// use std::io;
///
/// let io_err = io::Error::new(io::ErrorKind::NotFound, "model file missing");
/// let semantic_err = SemanticError::ModelLoad(io_err);
/// assert!(semantic_err.to_string().contains("modelo"));
/// # }
/// ```
#[cfg(feature = "ai")]
#[derive(Error, Debug)]
pub enum SemanticError {
    /// Failed to load ONNX model from cache
    ///
    /// This occurs when:
    /// - Model file doesn't exist in cache
    /// - Model file is corrupted
    /// - Memory mapping failed (disk full, permissions)
    #[error("Error cargando modelo ONNX: {0}")]
    ModelLoad(#[from] std::io::Error),

    /// Tokenization failed
    ///
    /// This occurs when:
    /// - Input text contains invalid UTF-8
    /// - Text exceeds model's maximum token limit
    /// - Special characters break tokenizer
    /// - Tokenizer file not found
    #[error("Error tokenizando texto: {0}")]
    Tokenize(String),

    /// ONNX inference failed
    ///
    /// This occurs when:
    /// - Model graph execution failed
    /// - Input tensor shape mismatch
    /// - Output tensor extraction failed
    /// - Tensor creation failed
    #[error("Error ejecutando inferencia ONNX: {0}")]
    Inference(String),

    /// Content chunk exceeds model's token limit
    ///
    /// This occurs when a single chunk of content has more tokens than
    /// the model can handle (512 tokens for all-MiniLM-L6-v2).
    ///
    /// # Fields
    ///
    /// * `chunk_id` - Identifier of the problematic chunk
    /// * `tokens` - Actual token count
    /// * `max` - Maximum allowed tokens
    #[error(
        "Chunk {chunk_id} excede límite de tokens: {tokens} > {max} (modelo: all-MiniLM-L6-v2)"
    )]
    ChunkTooLarge {
        /// Identifier of the chunk (UUID or index)
        chunk_id: String,
        /// Actual token count
        tokens: usize,
        /// Maximum allowed tokens for this model
        max: usize,
    },

    /// Model download failed from HuggingFace Hub
    ///
    /// This occurs when:
    /// - Network error during download
    /// - Repository doesn't exist or is private
    /// - Authentication required
    /// - SHA256 validation failed after download
    #[error("Error descargando modelo '{repo}': {cause}")]
    Download {
        /// HuggingFace repository (e.g., "sentence-transformers/all-MiniLM-L6-v2")
        repo: String,
        /// Underlying error cause
        cause: String,
    },

    /// Cache validation failed (SHA256 mismatch)
    ///
    /// This occurs when:
    /// - Downloaded file is corrupted
    /// - Cache was modified externally
    /// - Incomplete download
    #[error("Validación de caché falló para '{repo}': SHA256 inválido (esperado: {expected}, obtenido: {actual})")]
    CacheValidation {
        /// HuggingFace repository
        repo: String,
        /// Expected SHA256 hash
        expected: String,
        /// Actual SHA256 hash of downloaded file
        actual: String,
    },

    /// Offline mode: model not available in cache
    ///
    /// This occurs when:
    /// - Application is in offline mode
    /// - Model is not cached
    /// - Cannot download from HuggingFace Hub
    #[error("Modo offline: modelo '{repo}' no está en caché")]
    OfflineMode {
        /// HuggingFace repository
        repo: String,
    },
}

/// Result type alias using ScraperError as the error type.
pub type Result<T> = std::result::Result<T, ScraperError>;

impl ScraperError {
    /// Create an InvalidUrl error
    #[must_use]
    pub fn invalid_url(msg: impl Into<String>) -> Self {
        Self::InvalidUrl(msg.into())
    }

    /// Create an Http error
    #[must_use]
    pub fn http(status: u16, url: &str) -> Self {
        Self::Http {
            status,
            url: url.to_string(),
        }
    }

    /// Create a WafBlocked error
    #[must_use]
    pub fn waf_blocked(url: impl Into<String>, provider: impl Into<String>) -> Self {
        Self::WafBlocked {
            url: url.into(),
            provider: provider.into(),
        }
    }

    /// Create a Readability error
    #[must_use]
    pub fn readability(msg: impl Into<String>) -> Self {
        Self::Readability(msg.into())
    }

    /// Create an Extraction error
    #[must_use]
    pub fn extraction(msg: impl Into<String>) -> Self {
        Self::Extraction(msg.into())
    }

    /// Create a Download error
    #[must_use]
    pub fn download(msg: impl Into<String>) -> Self {
        Self::Download(msg.into())
    }

    /// Create a Conversion error
    #[must_use]
    pub fn conversion(msg: impl Into<String>) -> Self {
        Self::Conversion(msg.into())
    }

    /// Create an Export error
    #[must_use]
    pub fn export(msg: impl Into<String>) -> Self {
        Self::Export(msg.into())
    }

    /// Create an ExportBatch error
    #[must_use]
    pub fn export_batch(msg: impl Into<String>) -> Self {
        Self::ExportBatch(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_url_error() {
        let err = ScraperError::invalid_url("URL vacía");
        assert_eq!(err.to_string(), "URL inválida: URL vacía");
    }

    #[test]
    fn test_http_error() {
        let err = ScraperError::http(404, "https://example.com");
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("example.com"));
    }

    #[test]
    fn test_readability_error() {
        let err = ScraperError::readability("Failed to parse HTML");
        assert_eq!(
            err.to_string(),
            "Error de legibilidad: Failed to parse HTML"
        );
    }

    #[test]
    fn test_io_error_from_std() {
        let std_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ScraperError = std_err.into();
        assert!(err.to_string().contains("file not found"));
    }

    #[cfg(feature = "ai")]
    #[test]
    fn test_semantic_error_model_load() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "model file missing");
        let err = SemanticError::ModelLoad(io_err);
        assert!(err.to_string().contains("cargando modelo"));
    }

    #[cfg(feature = "ai")]
    #[test]
    fn test_semantic_error_chunk_too_large() {
        let err = SemanticError::ChunkTooLarge {
            chunk_id: "chunk-123".to_string(),
            tokens: 600,
            max: 512,
        };
        assert!(err.to_string().contains("chunk-123"));
        assert!(err.to_string().contains("600 > 512"));
    }

    #[cfg(feature = "ai")]
    #[test]
    fn test_semantic_error_download() {
        let err = SemanticError::Download {
            repo: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            cause: "network timeout".to_string(),
        };
        assert!(err.to_string().contains("all-MiniLM-L6-v2"));
        assert!(err.to_string().contains("network timeout"));
    }

    #[cfg(feature = "ai")]
    #[test]
    fn test_scraper_error_from_semantic() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "model missing");
        let semantic_err = SemanticError::ModelLoad(io_err);
        let scraper_err: ScraperError = semantic_err.into();
        assert!(scraper_err.to_string().contains("limpieza semántica"));
    }
}
