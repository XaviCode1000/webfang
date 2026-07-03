//! Item processing pipeline.
//!
//! Provides [`PipelineStage`] trait and [`PipelineExecutor`] for composing
//! sequential processing steps on [`ScrapedItem`]s.

mod executor;

pub use executor::PipelineExecutor;

// Re-export domain types used by the pipeline API
pub use crate::domain::pipeline_item::{PipelineStage, ScrapedItem, StageOutcome};
