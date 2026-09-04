//! Output stage trait for pipeline sinks.
//!
//! Output stages write [`crate::domain::pipeline_item::ScrapedItem`]s to arbitrary backends (files, databases,
//! network). They are called AFTER [`crate::application::pipeline::executor::PipelineExecutor`] completes, not as part
//! of the stage chain.

use std::future::Future;
use std::pin::Pin;

use crate::domain::pipeline_item::ScrapedItem;

/// Errors that can occur when writing to an output sink.
///
/// `Serialization`/`Backend` are the error vocabulary of the [`OutputStage`]
/// extension point: they are constructed by stage implementors (today only the
/// `crawl_task` test mocks — the production wiring is intentionally empty, see
/// #1114) and classified by the live `classify` below. The allow covers the
/// wired-but-empty extension point, not a dead wall item.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[allow(dead_code)] // extension-point vocabulary — see module note above
    #[error("serialization error: {0}")]
    Serialization(String),
    #[allow(dead_code)] // extension-point vocabulary — see module note above
    #[error("backend error: {0}")]
    Backend(String),
}

impl OutputError {
    /// Classify this output error per the Error Classification Matrix
    /// (`docs/error-classification-matrix.md`, #865).
    ///
    /// Io errors mirror [`CrawlError::classify`] rows 21/22 (transient for
    /// `Interrupted`/`WouldBlock`/`TimedOut`, permanent otherwise).
    /// Serialization failures are single-item data issues: the pipeline is
    /// healthy, so the class is domain-recoverable. Backend failures are
    /// indeterminate transport faults (rows 1/8 rationale): transient.
    pub(crate) fn classify(&self) -> crate::domain::error::ErrorClass {
        use crate::domain::error::ErrorClass;

        match self {
            Self::Io(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                ErrorClass::TransientRetriable
            },
            Self::Io(_) => ErrorClass::PermanentFatal,
            Self::Serialization(_) => ErrorClass::DomainRecoverable,
            Self::Backend(_) => ErrorClass::TransientRetriable,
        }
    }
}

/// A sink that receives [`ScrapedItem`]s after pipeline processing.
///
/// Output stages are separate from [`PipelineStage`](crate::domain::pipeline_item::PipelineStage).
/// Pipeline stages transform items; output stages persist them.
pub(crate) trait OutputStage: Send + Sync {
    /// Human-readable name for logging/diagnostics.
    fn name(&self) -> &str;

    /// Write an item to this output sink.
    fn write<'a>(
        &'a self,
        item: &'a ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutputError>> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_error_display() {
        let err = OutputError::Serialization("bad json".into());
        assert!(err.to_string().contains("serialization"));
    }

    #[test]
    fn test_output_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: OutputError = io_err.into();
        assert!(matches!(err, OutputError::Io(_)));
    }
}
