//! Application layer errors
//!
//! Errors for configuration, export, and application-level concerns.

use thiserror::Error;

/// Application-level errors representing orchestration and configuration failures.
///
/// These errors represent failures in the application layer that are
/// independent of domain logic or infrastructure concerns.
#[derive(Error, Debug)]
pub enum AppError {
    /// Configuration error
    #[error("Error de configuración: {0}")]
    Config(String),

    /// Export operation failed
    #[error("Error de exportación: {0}")]
    Export(String),

    /// Batch export failed (partial success)
    #[error("Error de exportación en batch: {0}")]
    ExportBatch(String),

    /// Feature not yet implemented (gated for future release)
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_config() {
        let err = AppError::Config("invalid port".to_string());
        assert!(err.to_string().contains("invalid port"));
        assert!(err.to_string().contains("configuración"));
    }

    #[test]
    fn test_app_error_export() {
        let err = AppError::Export("write failed".to_string());
        assert!(err.to_string().contains("write failed"));
        assert!(err.to_string().contains("exportación"));
    }

    #[test]
    fn test_app_error_export_batch() {
        let err = AppError::ExportBatch("partial success".to_string());
        assert!(err.to_string().contains("partial success"));
        assert!(err.to_string().contains("batch"));
    }

    #[test]
    fn test_app_error_is_std_error() {
        let err = AppError::Config("test".to_string());
        let _: &dyn std::error::Error = &err;
    }
}
