//! Single typed-error surface for the benchmark harness (ADR-B7).
//!
//! One enum for the whole leaf crate: no downstream consumers need granular
//! matching yet, and repo discipline forbids `anyhow` and `.unwrap()`/
//! `.expect()` outside tests in non-test code.
//!
//! Error text in this crate is deliberately written in English:
//! `webfang_benchmark` is internal developer tooling, not product surface.
//! The AGENTS.md rule "user-facing errors in Spanish" applies to the product
//! CLI surface (`webfang_cli`), not to this harness.

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

    /// Summary claims crawled pages but no `span_close` samples exist —
    /// inconsistent trace (e.g. off-thread span emission swallowed spans).
    #[error("summary reports {total_pages} crawled pages but no span_close samples in {path}")]
    MissingSpans { total_pages: u64, path: String },

    #[error("corpus server failed: {0}")]
    Corpus(String),

    #[error("engine or fetch-router failure: {0}")]
    Engine(String),

    #[error("cost config invalid: {0}")]
    CostConfig(String),

    #[error("report render failed: {0}")]
    Render(String),

    /// Tier B live-run gate refusal (NFR-4, fail-closed): a live competitor
    /// run requires BOTH a non-empty provider API key in the environment
    /// AND an explicit CLI opt-in flag. Nothing was executed.
    #[error(
    "live competitor run not enabled in this build/session; provide {env_var} and pass --i-understand-costs to explicitly opt in ({provider})"
        )]
    LiveDisabled {
        provider: &'static str,
        env_var: &'static str,
    },

    /// Projected credit spend exceeds the configured budget guard. The
    /// refusal happens during PLANNING — before any request is prepared or
    /// sent.
    #[error(
            "projected credit spend {projected_credits:.0} exceeds the budget guard of {budget} credits ({provider}); \
             refused before any request was prepared or sent"
        )]
    BudgetExceeded {
        provider: &'static str,
        projected_credits: f64,
        budget: u32,
    },
}

/// Crate-wide result alias over [`BenchmarkError`].
pub type Result<T> = std::result::Result<T, BenchmarkError>;
