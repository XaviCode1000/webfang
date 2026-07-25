//! Pipeline processing stages.
//!
//! Each stage implements [`crate::domain::pipeline_item::PipelineStage`] and performs a single,
//! well-defined transformation or validation on [`crate::domain::pipeline_item::ScrapedItem`]s.

mod clean;
pub mod jsonl_output;
pub mod multi_sink;
pub mod output;
mod validate;

pub use clean::CleanStage;
pub(crate) use output::OutputStage;
pub use validate::ValidateStage;
