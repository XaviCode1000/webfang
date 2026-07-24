//! Error handling module for webfang.
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

    /// Internal error from domain layer
    #[error("error interno: {0}")]
    Internal(String),

    /// Crawl limit exceeded (max depth, max pages)
    #[error("{0}")]
    CrawlLimit(String),

    /// Sitemap not found during auto-discovery
    #[error("sitemap not found for {0}")]
    SitemapNotFound(String),
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
/// use webfang::SemanticError;
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
    /// the model can handle (32768 tokens for Granite-97M).
    ///
    /// # Fields
    ///
    /// * `chunk_id` - Identifier of the problematic chunk
    /// * `tokens` - Actual token count
    /// * `max` - Maximum allowed tokens
    #[error("Chunk {chunk_id} excede límite de tokens: {tokens} > {max} (modelo: IBM Granite)")]
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

    /// Classify this error by operational severity.
    ///
    /// Used by retry logic and observability systems to decide retry/alert
    /// behavior without matching on specific variants or string-matching on
    /// display text.
    pub fn classify(&self) -> ErrorClass {
        match self {
            // Network/Download: transient if the wrapped source is a known-retryable
            // io::Error kind (timeout, connection reset, etc.)
            Self::Network(e) | Self::Download(e) if is_transient_network(e.as_ref()) => {
                ErrorClass::TransientRetriable
            },
            Self::Http { status, .. } if *status >= 500 => ErrorClass::TransientRetriable,
            Self::Http { status, .. } if *status == 429 => ErrorClass::TransientBackoff,
            Self::GlobalTimeout => ErrorClass::TransientBackoff,
            Self::SlowlorisTimeout => ErrorClass::TransientBackoff,
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            Self::WafBlocked { .. } => ErrorClass::PermanentFatal,
            Self::Readability(_) => ErrorClass::PermanentFatal,
            Self::Extraction(_) => ErrorClass::PermanentFatal,
            Self::ExtractionFailed { .. } => ErrorClass::PermanentFatal,
            Self::Validation(_) => ErrorClass::PermanentFatal,
            Self::FeatureGated(_) => ErrorClass::PermanentFatal,
            Self::Config(_) => ErrorClass::PermanentFatal,
            Self::PayloadTooLarge => ErrorClass::PermanentFatal,
            Self::Http { .. } => ErrorClass::PermanentFatal,
            Self::SemaphoreInanition => ErrorClass::InternalFatal,
            Self::Io(_) => ErrorClass::InternalFatal,
            Self::Serialization(_) => ErrorClass::InternalFatal,
            Self::Yaml(_) => ErrorClass::InternalFatal,
            Self::UrlParse(_) => ErrorClass::InternalFatal,
            Self::Persistence(_) => ErrorClass::InternalFatal,
            Self::Ingestion(_) => ErrorClass::InternalFatal,
            Self::Middleware(_) => ErrorClass::InternalFatal,
            Self::H2Config(_) => ErrorClass::InternalFatal,
            Self::Export(_) => ErrorClass::InternalFatal,
            Self::ExportBatch(_) => ErrorClass::InternalFatal,
            Self::Conversion(_) => ErrorClass::InternalFatal,
            Self::Semantic(_) => ErrorClass::InternalFatal,
            Self::Internal(_) => ErrorClass::InternalFatal,
            Self::CrawlLimit(_) => ErrorClass::PermanentFatal,
            Self::SitemapNotFound(_) => ErrorClass::PermanentFatal,
            // Non-transient Network/Download (e.g. DNS resolution, TLS errors)
            Self::Network(_) | Self::Download(_) => ErrorClass::InternalFatal,
        }
    }
}

/// Check if a boxed error represents a transient network failure.
///
/// This is a heuristic — `io::ErrorKind` is the primary signal,
/// but some wreq errors may also be transient.
fn is_transient_network(e: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
        return matches!(
            io_err.kind(),
            std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::BrokenPipe
        );
    }
    if e.downcast_ref::<WreqError>().is_some() {
        let msg = e.to_string().to_ascii_lowercase();
        return msg.contains("timeout")
            || msg.contains("timed out")
            || msg.contains("connection refused")
            || msg.contains("connection reset")
            || msg.contains("broken pipe")
            || msg.contains("connection aborted");
    }
    false
}

/// Operational classification of errors for observability and retry logic.
///
/// Partitions ScraperError variants by severity and recoverability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// Transient errors that should be retried immediately (e.g., connection reset, 5xx)
    TransientRetriable,
    /// Transient errors that require backoff before retry (e.g., rate limit, slowloris)
    TransientBackoff,
    /// Permanent errors that cannot be recovered by retry (e.g., 4xx, invalid URL, WAF)
    PermanentFatal,
    /// Internal errors that indicate a bug (e.g., integer overflow, semaphore exhaustion)
    InternalFatal,
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
// Error Map V2: From impls for layer-specific errors
// ============================================================================

impl From<crate::domain::error::CrawlError> for ScraperError {
    fn from(e: crate::domain::error::CrawlError) -> Self {
        use crate::domain::error::CrawlError;
        match e {
            CrawlError::Network {
                message,
                status_code,
            } => {
                if let Some(status) = status_code {
                    ScraperError::Http {
                        status,
                        url: message,
                    }
                } else {
                    ScraperError::Internal(format!("network: {message}"))
                }
            },
            CrawlError::Http { status, url } => ScraperError::Http { status, url },
            CrawlError::InvalidUrl(msg) => ScraperError::InvalidUrl(msg),
            CrawlError::Io(e) => ScraperError::Io(e),
            CrawlError::WafChallenge { provider, url, .. } => {
                ScraperError::WafBlocked { url, provider }
            },
            CrawlError::Internal(msg) => ScraperError::Internal(msg),
            CrawlError::Download(e) => ScraperError::Download(e),
            CrawlError::SitemapNotFound(url) => ScraperError::SitemapNotFound(url),
            CrawlError::Parse(msg) => ScraperError::Internal(format!("parse: {msg}")),
            CrawlError::MaxDepthExceeded { current, max } => {
                ScraperError::CrawlLimit(format!("maximum depth {max} exceeded at depth {current}"))
            },
            CrawlError::MaxPagesExceeded { max } => {
                ScraperError::CrawlLimit(format!("maximum pages {max} exceeded"))
            },
            CrawlError::UrlExcluded(url) => ScraperError::Internal(format!("URL excluded: {url}")),
            CrawlError::InvalidContentType(ct) => {
                ScraperError::Internal(format!("invalid content type: {ct}"))
            },
            CrawlError::Storage(msg) => ScraperError::Internal(format!("storage: {msg}")),
            CrawlError::Checkpoint(msg) => ScraperError::Internal(format!("checkpoint: {msg}")),
            CrawlError::SessionPool(msg) => ScraperError::Internal(format!("session pool: {msg}")),
            CrawlError::Discovery(msg) => ScraperError::Internal(format!("discovery: {msg}")),
            CrawlError::RetryExhausted { url, attempts } => ScraperError::Internal(format!(
                "retry exhausted for {url} after {attempts} attempts"
            )),
            CrawlError::TransientHttp { status, url } => ScraperError::Http { status, url },
            // NOTE: retry_after embedded in message. If downstream needs to
            // pattern-match on retry_after, consider adding ScraperError::RateLimited(u64).
            CrawlError::RateLimited(retry_after) => {
                ScraperError::Internal(format!("rate limited, retry after {retry_after}s"))
            },
            CrawlError::Timeout => ScraperError::Internal("request timeout".to_string()),
            CrawlError::Connection(msg) => ScraperError::Internal(format!("connection: {msg}")),
            CrawlError::ResourceExhausted {
                resource,
                limit,
                actual,
            } => ScraperError::Internal(format!(
                "resource exhausted: {resource:?} limit={limit} actual={actual}"
            )),
            CrawlError::SitemapEmpty => ScraperError::SitemapNotFound("empty sitemap".to_string()),
            CrawlError::SitemapDepthExceeded => {
                ScraperError::CrawlLimit("sitemap depth exceeded".to_string())
            },
            CrawlError::SemaphoreInanition => ScraperError::SemaphoreInanition,
        }
    }
}

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
            crate::infrastructure::error::InfraError::Network(e) => ScraperError::Network(e),
            crate::infrastructure::error::InfraError::Middleware(msg) => {
                ScraperError::Middleware(msg)
            },
            crate::infrastructure::error::InfraError::WafBlocked { url, provider } => {
                ScraperError::WafBlocked { url, provider }
            },
            crate::infrastructure::error::InfraError::Download(e) => ScraperError::Download(e),
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

#[cfg(test)]
#[allow(clippy::io_other_error)]
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
        assert!(
            matches!(err, ScraperError::Http { status: 404, .. }),
            "HTTP variant with status 404 must be preserved"
        );
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
        assert!(
            matches!(err, ScraperError::Io(_)),
            "std::io::Error must convert to ScraperError::Io"
        );
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
        // Display contract: triangulation test — verify real rusqlite error text appears
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
        assert!(
            matches!(err, SemanticError::ModelLoad(_)),
            "ModelLoad variant must be preserved"
        );
    }

    #[test]
    fn test_semantic_error_chunk_too_large() {
        let err = SemanticError::ChunkTooLarge {
            chunk_id: "chunk-123".to_string(),
            tokens: 600,
            max: 512,
        };
        assert!(
            matches!(
                err,
                SemanticError::ChunkTooLarge {
                    chunk_id,
                    tokens: 600,
                    max: 512,
                }
                if chunk_id == "chunk-123"
            ),
            "ChunkTooLarge fields must be preserved"
        );
    }

    #[test]
    fn test_semantic_error_download() {
        let err = SemanticError::Download {
            repo: "sentence-transformers/all-MiniLM-L6-v2".to_string(),
            cause: "network timeout".to_string(),
        };
        assert!(
            matches!(
                err,
                SemanticError::Download { repo, cause }
                if repo == "sentence-transformers/all-MiniLM-L6-v2" && cause == "network timeout"
            ),
            "Download fields must be preserved"
        );
    }

    #[test]
    fn test_scraper_error_from_semantic() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "model missing");
        let semantic_err = SemanticError::ModelLoad(io_err);
        let scraper_err: ScraperError = semantic_err.into();
        assert!(
            matches!(
                scraper_err,
                ScraperError::Semantic(SemanticError::ModelLoad(_))
            ),
            "SemanticError::ModelLoad must convert to ScraperError::Semantic(ModelLoad)"
        );
    }

    #[test]
    fn test_scraper_error_h2_config() {
        let err = ScraperError::H2Config("ALPN negotiation failed".to_string());
        assert!(
            matches!(err, ScraperError::H2Config(s) if s == "ALPN negotiation failed"),
            "H2Config variant with message must be preserved"
        );
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
        assert!(
            matches!(scraper_err, ScraperError::Readability(s) if s == "parse failed"),
            "DomainError::Readability must convert to ScraperError::Readability"
        );
    }

    #[test]
    fn test_domain_error_extraction_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Extraction("no content".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Extraction(s) if s == "no content"),
            "DomainError::Extraction must convert to ScraperError::Extraction"
        );
    }

    #[test]
    fn test_domain_error_extraction_failed_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::ExtractionFailed {
            url: "https://example.com".to_string(),
            reason: "empty body".to_string(),
        };
        let scraper_err: ScraperError = domain_err.into();
        assert!(
            matches!(
                scraper_err,
                ScraperError::ExtractionFailed { url, reason }
                if url == "https://example.com" && reason == "empty body"
            ),
            "DomainError::ExtractionFailed fields must be preserved"
        );
    }

    #[test]
    fn test_domain_error_validation_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Validation("bad pattern".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Validation(s) if s == "bad pattern"),
            "DomainError::Validation must convert to ScraperError::Validation"
        );
    }

    #[test]
    fn test_domain_error_feature_gated_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::FeatureGated("AI module".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(
            matches!(scraper_err, ScraperError::FeatureGated(s) if s == "AI module"),
            "DomainError::FeatureGated must convert to ScraperError::FeatureGated"
        );
    }

    #[test]
    fn test_domain_error_conversion_wraps_to_scraper() {
        let domain_err = crate::domain::error::DomainError::Conversion("YAML parse".to_string());
        let scraper_err: ScraperError = domain_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Conversion(s) if s == "YAML parse"),
            "DomainError::Conversion must convert to ScraperError::Conversion"
        );
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
        assert!(
            matches!(err, ScraperError::InvalidUrl(s) if s == "test"),
            "? operator must preserve InvalidUrl variant and message"
        );
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
            matches!(scraper_err, ScraperError::Http { status: 500, .. }),
            "Status code must be preserved"
        );
        assert!(
            matches!(
                scraper_err,
                ScraperError::Http { url, .. }
                if url == "https://example.com"
            ),
            "URL must be preserved"
        );
    }

    #[test]
    fn test_infra_error_network_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::Network(Box::new(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"),
        ));
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Network(_)),
            "InfraError::Network must convert to ScraperError::Network"
        );
    }

    #[test]
    fn test_infra_error_download_wraps_to_scraper_download_variant() {
        // Regression: download failures must reach `ScraperError::Download`,
        // NOT be silently misrouted into `ScraperError::Network` (arch-remediation).
        let infra_err = crate::infrastructure::error::InfraError::Download(Box::new(
            std::io::Error::new(std::io::ErrorKind::Other, "checksum mismatch"),
        ));
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Download(_)),
            "InfraError::Download must map to ScraperError::Download, got: {scraper_err}"
        );
        // Display contract: Download variant must preserve Spanish prefix and inner message
        assert_eq!(
            scraper_err.to_string(),
            "Error de descarga: checksum mismatch"
        );
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
        assert!(
            matches!(
                scraper_err,
                ScraperError::WafBlocked { url, provider }
                if url == "https://example.com" && provider == "Cloudflare"
            ),
            "WafBlocked fields must be preserved"
        );
    }

    #[test]
    fn test_infra_error_persistence_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::Persistence("disk full".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Persistence(s) if s == "disk full"),
            "InfraError::Persistence must convert to ScraperError::Persistence"
        );
    }

    #[test]
    fn test_infra_error_ingestion_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::Ingestion("pipeline failed".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::Ingestion(s) if s == "pipeline failed"),
            "InfraError::Ingestion must convert to ScraperError::Ingestion"
        );
    }

    #[test]
    fn test_infra_error_global_timeout_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::GlobalTimeout;
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::GlobalTimeout),
            "InfraError::GlobalTimeout must convert to ScraperError::GlobalTimeout"
        );
    }

    #[test]
    fn test_infra_error_slowloris_timeout_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::SlowlorisTimeout;
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::SlowlorisTimeout),
            "InfraError::SlowlorisTimeout must convert to ScraperError::SlowlorisTimeout"
        );
    }

    #[test]
    fn test_infra_error_payload_too_large_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::PayloadTooLarge;
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::PayloadTooLarge),
            "InfraError::PayloadTooLarge must convert to ScraperError::PayloadTooLarge"
        );
    }

    #[test]
    fn test_infra_error_semaphore_inanition_wraps_to_scraper() {
        let infra_err = crate::infrastructure::error::InfraError::SemaphoreInanition;
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::SemaphoreInanition),
            "InfraError::SemaphoreInanition must convert to ScraperError::SemaphoreInanition"
        );
    }

    #[test]
    fn test_infra_error_h2_config_wraps_to_scraper() {
        let infra_err =
            crate::infrastructure::error::InfraError::H2Config("ALPN failed".to_string());
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::H2Config(s) if s == "ALPN failed"),
            "InfraError::H2Config must convert to ScraperError::H2Config"
        );
    }

    #[test]
    fn test_infra_error_url_parse_wraps_to_scraper() {
        let url_err = url::ParseError::EmptyHost;
        let infra_err = crate::infrastructure::error::InfraError::UrlParse(url_err);
        let scraper_err: ScraperError = infra_err.into();
        assert!(
            matches!(scraper_err, ScraperError::UrlParse(_)),
            "InfraError::UrlParse must convert to ScraperError::UrlParse"
        );
    }

    // ========================================================================
    // ErrorMap V2: ErrorClass classify() tests
    // ========================================================================

    #[test]
    fn test_classify_transient_retriable_connection_reset() {
        let err = ScraperError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        assert_eq!(err.classify(), ErrorClass::TransientRetriable);
    }

    #[test]
    fn test_classify_transient_retriable_5xx() {
        let err = ScraperError::http(503, "https://example.com");
        assert_eq!(err.classify(), ErrorClass::TransientRetriable);
    }

    #[test]
    fn test_classify_transient_backoff_timeout() {
        let err = ScraperError::GlobalTimeout;
        assert_eq!(err.classify(), ErrorClass::TransientBackoff);
    }

    #[test]
    fn test_classify_transient_backoff_slowloris() {
        let err = ScraperError::SlowlorisTimeout;
        assert_eq!(err.classify(), ErrorClass::TransientBackoff);
    }

    #[test]
    fn test_classify_transient_backoff_429() {
        let err = ScraperError::http(429, "https://example.com");
        assert_eq!(err.classify(), ErrorClass::TransientBackoff);
    }

    #[test]
    fn test_classify_permanent_fatal_invalid_url() {
        let err = ScraperError::invalid_url("bad url");
        assert_eq!(err.classify(), ErrorClass::PermanentFatal);
    }

    #[test]
    fn test_classify_permanent_fatal_4xx() {
        let err = ScraperError::http(404, "https://example.com");
        assert_eq!(err.classify(), ErrorClass::PermanentFatal);
    }

    #[test]
    fn test_classify_permanent_fatal_waf() {
        let err = ScraperError::waf_blocked("https://example.com", "Cloudflare");
        assert_eq!(err.classify(), ErrorClass::PermanentFatal);
    }

    #[test]
    fn test_classify_internal_fatal_semaphone() {
        let err = ScraperError::SemaphoreInanition;
        assert_eq!(err.classify(), ErrorClass::InternalFatal);
    }

    #[test]
    fn test_classify_internal_fatal_io() {
        let err: ScraperError = std::io::Error::new(std::io::ErrorKind::Other, "test").into();
        assert_eq!(err.classify(), ErrorClass::InternalFatal);
    }

    #[test]
    fn test_classify_internal_fatal_persistence() {
        let err = ScraperError::persistence("disk full");
        assert_eq!(err.classify(), ErrorClass::InternalFatal);
    }

    // ========================================================================
    // B-05: CrawlError → ScraperError → classify() integration tests
    // ========================================================================

    #[test]
    fn test_crawl_rate_limited_classifies_as_internal_fatal() {
        let crawl_err = crate::domain::error::CrawlError::RateLimited(60);
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::InternalFatal,
            "RateLimited maps to ScraperError::Internal → InternalFatal"
        );
    }

    #[test]
    fn test_crawl_timeout_classifies_as_internal_fatal() {
        let crawl_err = crate::domain::error::CrawlError::Timeout;
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::InternalFatal,
            "Timeout maps to ScraperError::Internal → InternalFatal"
        );
    }

    #[test]
    fn test_crawl_connection_classifies_as_internal_fatal() {
        let crawl_err = crate::domain::error::CrawlError::Connection("refused".into());
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::InternalFatal,
            "Connection maps to ScraperError::Internal → InternalFatal"
        );
    }

    #[test]
    fn test_crawl_http_403_classifies_as_permanent_fatal() {
        let crawl_err = crate::domain::error::CrawlError::Http {
            status: 403,
            url: "https://example.com".to_string(),
        };
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::PermanentFatal,
            "HTTP 403 should be PermanentFatal"
        );
    }

    #[test]
    fn test_crawl_http_503_classifies_as_transient_retriable() {
        let crawl_err = crate::domain::error::CrawlError::Http {
            status: 503,
            url: "https://example.com".to_string(),
        };
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::TransientRetriable,
            "HTTP 503 should be TransientRetriable"
        );
    }

    #[test]
    fn test_crawl_http_429_classifies_as_transient_backoff() {
        let crawl_err = crate::domain::error::CrawlError::Http {
            status: 429,
            url: "https://example.com".to_string(),
        };
        let scraper_err: ScraperError = crawl_err.into();
        assert_eq!(
            scraper_err.classify(),
            ErrorClass::TransientBackoff,
            "HTTP 429 should be TransientBackoff"
        );
    }

    #[test]
    fn test_is_transient_network_broken_pipe() {
        let err = ScraperError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "broken pipe",
        )));
        assert_eq!(err.classify(), ErrorClass::TransientRetriable);
    }
}
