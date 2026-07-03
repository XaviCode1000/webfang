//! Pipeline processing stages.
//!
//! Each stage implements [`PipelineStage`] and performs a single,
//! well-defined transformation or validation on [`ScrapedItem`]s.

mod clean;
pub mod jsonl_output;
pub mod multi_sink;
pub mod output;
mod validate;

pub use clean::CleanStage;
pub use jsonl_output::JsonlOutputStage;
pub use multi_sink::MultiSinkOutput;
pub use output::{OutputError, OutputStage};
pub use validate::ValidateStage;
