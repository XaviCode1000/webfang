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
    /// Underlying I/O failure while reading or writing the trace JSONL file.
    #[error("trace jsonl io error: {0}")]
    Io(#[from] std::io::Error),

    /// A trace line failed to parse as JSON; records the 1-based line and the
    /// underlying `serde_json` error.
    #[error("line {line}: invalid json ({source})")]
    Jsonl {
        /// 1-based line number in the trace file where parsing failed.
        line: usize,
        /// Underlying JSON deserialization error.
        source: serde_json::Error,
    },

    /// A trace record had an unexpected shape (schema drift in engine output);
    /// records the offending line and a human-readable detail.
    #[error("line {line}: unexpected record shape: {detail}")]
    Shape {
        /// 1-based line number of the malformed record.
        line: usize,
        /// Human-readable description of what was unexpected.
        detail: String,
    },

    /// The engine never emitted its expected end-of-crawl summary line.
    #[error("missing engine summary line ('crawl completed') in {path}")]
    MissingSummary {
        /// Path to the trace file that lacked the summary marker.
        path: String,
    },

    /// The engine reported a completed crawl with zero attempted pages.
    #[error("summary reports zero attempted pages")]
    EmptyCrawl,

    /// Summary claims crawled pages but no `span_close` samples exist —
    /// inconsistent trace (e.g. off-thread span emission swallowed spans).
    #[error("summary reports {total_pages} crawled pages but no span_close samples in {path}")]
    MissingSpans {
        /// Number of crawled pages claimed by the summary.
        total_pages: u64,
        /// Path to the trace file that lacked span_close samples.
        path: String,
    },

    /// The local corpus mock server failed to start or respond.
    #[error("corpus server failed: {0}")]
    Corpus(String),

    /// The target engine or its fetch router returned a failure.
    #[error("engine or fetch-router failure: {0}")]
    Engine(String),

    /// A cost-model configuration value was invalid.
    #[error("cost config invalid: {0}")]
    CostConfig(String),

    /// The final Markdown/HTML report could not be rendered.
    #[error("report render failed: {0}")]
    Render(String),

    /// Tier B live-run gate refusal (NFR-4, fail-closed): a live competitor
    /// run requires BOTH a non-empty provider API key in the environment
    /// AND an explicit CLI opt-in flag. Nothing was executed.
    #[error(
    "live competitor run not enabled in this build/session; provide {env_var} and pass --i-understand-costs to explicitly opt in ({provider})"
        )]
    LiveDisabled {
        /// Human-readable name of the live provider that was refused.
        provider: &'static str,
        /// Environment variable that must carry the provider API key.
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
        /// Human-readable name of the provider whose budget was exceeded.
        provider: &'static str,
        /// Projected credit spend at planning time.
        projected_credits: f64,
        /// Configured credit budget that was exceeded.
        budget: u32,
    },
}

/// Crate-wide result alias over [`BenchmarkError`].
pub type Result<T> = std::result::Result<T, BenchmarkError>;
