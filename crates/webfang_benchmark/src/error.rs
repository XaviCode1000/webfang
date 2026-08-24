//! Single typed-error surface for the benchmark harness (ADR-B7).
//!
//! One enum for the whole leaf crate: no downstream consumers need granular
//! matching yet, and repo discipline forbids `anyhow` and `.unwrap()`/
//! `.expect()` outside tests in non-test code.

/// Errors produced anywhere in the harness pipeline.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkError {
    #[error("trace jsonl io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("line {line}: invalid json ({source})")]
    Jsonl {
        line: usize,
        source: serde_json::Error,
    },

    #[error("line {line}: unexpected record shape: {detail}")]
    Shape { line: usize, detail: String },

    #[error("missing engine summary line ('crawl completed') in {path}")]
    MissingSummary { path: String },

    #[error("summary reports zero attempted pages")]
    EmptyCrawl,

    #[error("corpus server failed: {0}")]
    Corpus(String),

    #[error("cost config invalid: {0}")]
    CostConfig(String),

    #[error("report render failed: {0}")]
    Render(String),
}

/// Crate-wide result alias over [`BenchmarkError`].
pub type Result<T> = std::result::Result<T, BenchmarkError>;
