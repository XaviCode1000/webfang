//! Error handling module for rust_scraper.
//!
//! Uses thiserror for library error types (err-thiserror-lib).
//! This provides type-safe, structured error handling instead of anyhow.

use thiserror::Error;
use wreq::Error as WreqError;

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
    ///
    /// Carries the underlying error as `#[source]` so the root-cause chain
    /// (e.g. `wreq::Error` → I/O → timeout) is preserved for `Error::source()`
    /// traversal instead of being flattened to a `String` (D4).
    #[error("error de red: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

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
    ///
    /// Carries the underlying error as `#[source]` to preserve the root-cause
    /// chain (D4). Previously flattened to a `String`.
    #[error("Error de descarga: {0}")]
    Download(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Configuration error
    #[error("Error de configuración: {0}")]
    Config(String),

    /// Feature not yet implemented (gated for future release)
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

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

    /// Content extraction failed (poor fallback content)
    #[error("extracción falló para {url}: {reason}")]
    ExtractionFailed {
        /// URL that failed
        url: String,
        /// Reason for failure
        reason: String,
    },

    /// Batch export failed (partial success)
    #[error("Error de exportación en batch: {0}")]
    ExportBatch(String),

    /// Global download timeout (30s)
    #[error("descarga superó tiempo global de 30 segundos")]
    GlobalTimeout,

    /// Slowloris attack detected (per-chunk timeout)
    #[error("descarga superó timeout de inactividad de 5 segundos por chunk")]
    SlowlorisTimeout,

    /// Payload exceeded 25MB limit
    #[error("recurso superó límite de 25 MB")]
    PayloadTooLarge,

    /// Semaphore exhausted (backpressure)
    #[error("semáforo agotado: no hay permisos disponibles")]
    SemaphoreInanition,

    /// Persistence error (SQLite storage layer — resources/chunks CRUD, pool).
    ///
    /// Holds the underlying error rendered to a string so it uniformly covers
    /// both `rusqlite::Error` and `deadpool_sqlite` pool errors (which are
    /// different types) without forcing two `#[from]` variants.
    #[error("Error de persistencia: {0}")]
    Persistence(String),

    /// Elastic ingestion pipeline error (orchestration / dispatch failures).
    #[error("Error de ingestión: {0}")]
    Ingestion(String),

    /// Semantic cleaning error (AI-powered content processing)
    #[error("Error de limpieza semántica: {0}")]
    Semantic(#[from] SemanticError),

    /// HTTP/2 configuration error (ALPN, settings, or handshake failure)
    #[error("Error de configuración HTTP/2: {0}")]
    H2Config(String),
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

    /// Create a Download error, preserving the underlying error as the cause
    /// chain (`#[source]`) so `Error::source()` traversal works (D4).
    #[must_use]
    pub fn download(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Download(e)
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

    /// Create a Persistence error from anything displayable.
    ///
    /// Used to uniformly convert `rusqlite::Error` and `deadpool_sqlite` pool
    /// errors into `ScraperError::Persistence` without `#[from]` ambiguity.
    #[must_use]
    pub fn persistence(err: impl std::fmt::Display) -> Self {
        Self::Persistence(err.to_string())
    }

    /// Create an Ingestion error from anything displayable.
    #[must_use]
    pub fn ingestion(err: impl std::fmt::Display) -> Self {
        Self::Ingestion(err.to_string())
    }
}

impl From<WreqError> for ScraperError {
    /// Convert a `wreq::Error` into the single network variant, preserving the
    /// underlying cause chain (D4). Consolidates the former duplicate
    /// `NetworkFailure` variant (#153).
    fn from(e: WreqError) -> Self {
        ScraperError::Network(Box::new(e))
    }
}

// ============================================================================
// Phase 2: Error Stratification — From impls for layer-specific errors
// ============================================================================

impl From<crate::domain::error::DomainError> for ScraperError {
    fn from(e: crate::domain::error::DomainError) -> Self {
        match e {
            crate::domain::error::DomainError::InvalidUrl(msg) => ScraperError::InvalidUrl(msg),
            crate::domain::error::DomainError::Readability(msg) => ScraperError::Readability(msg),
            crate::domain::error::DomainError::Extraction(msg) => ScraperError::Extraction(msg),
            crate::domain::error::DomainError::ExtractionFailed { url, reason } => {
                ScraperError::ExtractionFailed { url, reason }
            },
            crate::domain::error::DomainError::Validation(msg) => ScraperError::Validation(msg),
            crate::domain::error::DomainError::FeatureGated(msg) => ScraperError::FeatureGated(msg),
            crate::domain::error::DomainError::Conversion(msg) => ScraperError::Conversion(msg),
        }
    }
}

impl From<crate::infrastructure::error::InfraError> for ScraperError {
    fn from(e: crate::infrastructure::error::InfraError) -> Self {
        match e {
            crate::infrastructure::error::InfraError::Http { status, url } => {
                ScraperError::Http { status, url }
            },
            crate::infrastructure::error::InfraError::Network(msg) => {
                ScraperError::Network(Box::new(std::io::Error::other(msg)))
            },
            crate::infrastructure::error::InfraError::Middleware(msg) => {
                ScraperError::Middleware(msg)
            },
            crate::infrastructure::error::InfraError::WafBlocked { url, provider } => {
                ScraperError::WafBlocked { url, provider }
            },
            crate::infrastructure::error::InfraError::Download(msg) => {
                ScraperError::Download(Box::new(std::io::Error::other(msg)))
            },
            crate::infrastructure::error::InfraError::GlobalTimeout => ScraperError::GlobalTimeout,
            crate::infrastructure::error::InfraError::SlowlorisTimeout => {
                ScraperError::SlowlorisTimeout
            },
            crate::infrastructure::error::InfraError::PayloadTooLarge => {
                ScraperError::PayloadTooLarge
            },
            crate::infrastructure::error::InfraError::SemaphoreInanition => {
                ScraperError::SemaphoreInanition
            },
            crate::infrastructure::error::InfraError::Persistence(msg) => {
                ScraperError::Persistence(msg)
            },
            crate::infrastructure::error::InfraError::Ingestion(msg) => {
                ScraperError::Ingestion(msg)
            },
            crate::infrastructure::error::InfraError::H2Config(msg) => ScraperError::H2Config(msg),
            crate::infrastructure::error::InfraError::UrlParse(e) => ScraperError::UrlParse(e),
            crate::infrastructure::error::InfraError::Io(e) => ScraperError::Io(e),
        }
    }
}

impl From<crate::application::error::AppError> for ScraperError {
    fn from(e: crate::application::error::AppError) -> Self {
        match e {
            crate::application::error::AppError::Config(msg) => ScraperError::Config(msg),
            crate::application::error::AppError::Export(msg) => ScraperError::Export(msg),
            crate::application::error::AppError::ExportBatch(msg) => ScraperError::ExportBatch(msg),
            crate::application::error::AppError::FeatureGated(msg) => {
                ScraperError::FeatureGated(msg)
            },
            crate::application::error::AppError::GlobalTimeout => ScraperError::GlobalTimeout,
            crate::application::error::AppError::SlowlorisTimeout => ScraperError::SlowlorisTimeout,
            crate::application::error::AppError::PayloadTooLarge => ScraperError::PayloadTooLarge,
            crate::application::error::AppError::SemaphoreInanition => {
                ScraperError::SemaphoreInanition
            },
        }
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

    #[test]
    fn test_persistence_error_message() {
        let err = ScraperError::persistence("disco lleno");
        assert_eq!(err.to_string(), "Error de persistencia: disco lleno");
    }

    #[test]
    fn test_ingestion_error_message() {
        let err = ScraperError::ingestion("pipeline abortó");
        assert_eq!(err.to_string(), "Error de ingestión: pipeline abortó");
    }

    // `rusqlite` is only linked under the `persistence` feature; this triangulation
    // test must be gated to keep the default (core) build dependency-free.
    #[cfg(all(feature = "persistence", not(miri)))]
    #[test]
    fn test_persistence_error_from_rusqlite() {
        // Triangulation: the Display-based helper must carry the real rusqlite
        // error text (proves it converts a genuine DB error, not a hardcoded value).
        let db = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
        let rusqlite_err = db
            .prepare("SELECT * FROM tabla_inexistente")
            .expect_err("expected error for missing table");
        let scraper_err = ScraperError::persistence(&rusqlite_err);
        let msg = scraper_err.to_string();
        assert!(
            msg.contains("persistencia"),
            "missing Spanish prefix: {msg}"
        );
        assert!(
            msg.contains("no such table"),
            "missing rusqlite detail: {msg}"
        );
    }

    #[test]
    fn test_semantic_error_model_load() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "model file missing");
        let err = SemanticError::ModelLoad(io_err);
        assert!(err.to_string().contains("cargando modelo"));
    }

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

    #[test]
    fn test_semantic_error_download() {
        let err = SemanticError::Download {
            repo: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            cause: "network timeout".to_string(),
        };
        assert!(err.to_string().contains("all-MiniLM-L6-v2"));
        assert!(err.to_string().contains("network timeout"));
    }

    #[test]
    fn test_scraper_error_from_semantic() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "model missing");
        let semantic_err = SemanticError::ModelLoad(io_err);
        let scraper_err: ScraperError = semantic_err.into();
        assert!(scraper_err.to_string().contains("limpieza semántica"));
    }

    #[test]
    fn test_scraper_error_h2_config() {
        let err = ScraperError::H2Config("ALPN negotiation failed".to_string());
        assert!(err.to_string().contains("Error de configuración HTTP/2"));
        assert!(err.to_string().contains("ALPN negotiation failed"));
    }

    // ========================================================================
    // Phase 2: Error Stratification — DomainError → ScraperError From tests
    // ========================================================================

    #[test]
    fn test_domain_error_invalid_url_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::InvalidUrl("bad url".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert_eq!(
            scraper_err.to_string(),
            "URL inválida: bad url",
            "Spanish Display message must be preserved through From conversion"
        );
    }

    #[test]
    fn test_domain_error_readability_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Readability("parse failed".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("parse failed"));
        assert!(scraper_err.to_string().contains("legibilidad"));
    }

    #[test]
    fn test_domain_error_extraction_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Extraction("no content".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("no content"));
        assert!(scraper_err.to_string().contains("extracción"));
    }

    #[test]
    fn test_domain_error_extraction_failed_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::ExtractionFailed {
            url: "https://example.com".to_string(),
            reason: "empty body".to_string(),
        };
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("example.com"));
        assert!(scraper_err.to_string().contains("empty body"));
    }

    #[test]
    fn test_domain_error_validation_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Validation("bad pattern".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("bad pattern"));
        assert!(scraper_err.to_string().contains("Validación"));
    }

    #[test]
    fn test_domain_error_feature_gated_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::FeatureGated("AI module".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("AI module"));
        assert!(scraper_err
            .to_string()
            .contains("funcionalidad no disponible"));
    }

    #[test]
    fn test_domain_error_conversion_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Conversion("YAML parse".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(scraper_err.to_string().contains("YAML parse"));
        assert!(scraper_err.to_string().contains("conversión"));
    }

    #[test]
    fn test_domain_error_question_mark_operator() {
        fn inner() -> std::result::Result<(), crate::domain::error::DomainError> {
            Err(crate::domain::error::DomainError::InvalidUrl(
                "test".to_string(),
            ))
        }

        fn outer() -> std::result::Result<(), ScraperError> {
            inner().map_err(ScraperError::from)?;
            Ok(())
        }

        let err = outer().unwrap_err();
        assert!(err.to_string().contains("URL inválida"));
    }

    // ========================================================================
    // Phase 2: Error Stratification — InfraError → ScraperError From tests
    // ========================================================================

    #[test]
    fn test_infra_error_http_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::Http {
            status: 500,
            url: "https://example.com".to_string(),
        };
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            scraper_err.to_string().contains("500"),
            "Status code must be preserved"
        );
        assert!(
            scraper_err.to_string().contains("example.com"),
            "URL must be preserved"
        );
    }

    #[test]
    fn test_infra_error_network_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::Network("connection refused".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("connection refused"));
        assert!(scraper_err.to_string().contains("red"));
    }

    #[test]
    fn test_infra_error_download_wraps_to_scraper_download_variant() {
        // Regression: download failures must reach `ScraperError::Download`,
        // NOT be silently misrouted into `ScraperError::Network` (arch-remediation).
        let infra_err =
            crate::infrastructure::error::InfraError::Download("checksum mismatch".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Download(_)),
            "InfraError::Download must map to ScraperError::Download, got: {scraper_err}"
        );
        assert!(scraper_err.to_string().contains("descarga"));
        assert!(scraper_err.to_string().contains("checksum mismatch"));
        // Must NOT be classified as a Network error.
        assert!(!matches!(scraper_err, ScraperError::Network(_)));
    }

    #[test]
    fn test_infra_error_waf_blocked_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::WafBlocked {
            url: "https://example.com".to_string(),
            provider: "Cloudflare".to_string(),
        };
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("example.com"));
        assert!(scraper_err.to_string().contains("Cloudflare"));
    }

    #[test]
    fn test_infra_error_persistence_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::Persistence("disk full".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("disk full"));
        assert!(scraper_err.to_string().contains("persistencia"));
    }

    #[test]
    fn test_infra_error_ingestion_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::Ingestion("pipeline failed".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("pipeline failed"));
        assert!(scraper_err.to_string().contains("ingestión"));
    }

    #[test]
    fn test_infra_error_global_timeout_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::GlobalTimeout;
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("30 segundos"));
    }

    #[test]
    fn test_infra_error_slowloris_timeout_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::SlowlorisTimeout;
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("5 segundos"));
    }

    #[test]
    fn test_infra_error_payload_too_large_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::PayloadTooLarge;
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("25 MB"));
    }

    #[test]
    fn test_infra_error_semaphore_inanition_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::SemaphoreInanition;
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("semáforo agotado"));
    }

    #[test]
    fn test_infra_error_h2_config_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::H2Config("ALPN failed".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("ALPN failed"));
        assert!(scraper_err.to_string().contains("HTTP/2"));
    }

    #[test]
    fn test_infra_error_url_parse_wraps_to_scraper() {
        let url_err = url::ParseError::EmptyHost;
        let infra_err = crate::infrastructure::error::InfraError::UrlParse(url_err);
        let scraper_err: ScraperError = infra_err.into();
        assert!(scraper_err.to_string().contains("URL"));
    }

    // ========================================================================
    // Phase 2: Error Stratification — AppError → ScraperError From tests
    // ========================================================================

    #[test]
    fn test_app_error_config_wraps_to_scraper() {
        let app_err = crate::application::error::AppError::Config("invalid port".to_string());
        let scraper_err: ScraperError = app_err.into();
        assert!(scraper_err.to_string().contains("invalid port"));
        assert!(scraper_err.to_string().contains("configuración"));
    }

    #[test]
    fn test_app_error_export_wraps_to_scraper() {
        let app_err = crate::application::error::AppError::Export("write failed".to_string());
        let scraper_err: ScraperError = app_err.into();
        assert!(scraper_err.to_string().contains("write failed"));
        assert!(scraper_err.to_string().contains("exportación"));
    }

    #[test]
    fn test_app_error_export_batch_wraps_to_scraper() {
        let app_err =
            crate::application::error::AppError::ExportBatch("partial success".to_string());
        let scraper_err: ScraperError = app_err.into();
        assert!(scraper_err.to_string().contains("partial success"));
        assert!(scraper_err.to_string().contains("batch"));
    }

    #[test]
    fn test_app_error_feature_gated_wraps_to_scraper() {
        let app_err = crate::application::error::AppError::FeatureGated("AI module".to_string());
        let scraper_err: ScraperError = app_err.into();
        assert!(scraper_err.to_string().contains("AI module"));
    }

    #[test]
    fn test_app_error_question_mark_operator() {
        fn inner() -> std::result::Result<(), crate::application::error::AppError> {
            Err(crate::application::error::AppError::Config(
                "test".to_string(),
            ))
        }

        fn outer() -> std::result::Result<(), ScraperError> {
            inner().map_err(ScraperError::from)?;
            Ok(())
        }

        let err = outer().unwrap_err();
        assert!(err.to_string().contains("configuración"));
    }
}
