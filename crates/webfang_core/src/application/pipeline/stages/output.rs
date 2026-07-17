//! Output stage trait for pipeline sinks.
//!
//! Output stages write [`ScrapedItem`]s to arbitrary backends (files, databases,
//! network). They are called AFTER [`PipelineExecutor`] completes, not as part
//! of the stage chain.

use std::future::Future;
use std::pin::Pin;

use crate::domain::pipeline_item::ScrapedItem;

/// Errors that can occur when writing to an output sink.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("backend error: {0}")]
    Backend(String),
}

/// A sink that receives [`ScrapedItem`]s after pipeline processing.
///
/// Output stages are separate from [`PipelineStage`](crate::domain::pipeline_item::PipelineStage).
/// Pipeline stages transform items; output stages persist them.
pub trait OutputStage: Send + Sync {
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
