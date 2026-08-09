use crate::domain::config::{ExportFormat, OutputFormat, PipelineOutputFormat};
use clap::Args;

/// Export format and output configuration arguments.
#[derive(Args, Debug, Default)]
pub struct ExportArgs {
    // ========== Output ==========
    /// Output directory for scraped content
    #[arg(short, long, default_value = "output", env = "WEBFANG_OUTPUT")]
    #[clap(next_help_heading = "Output")]
    pub output: std::path::PathBuf,

    /// Output format for individual files (markdown, text, json)
    /// NOTE: For RAG pipeline export, use --export-format instead
    #[arg(
        short = 'f',
        long,
        default_value = "markdown",
        value_enum,
        env = "WEBFANG_FORMAT"
    )]
    #[clap(next_help_heading = "Output")]
    pub format: OutputFormat,

    /// Export format for RAG pipeline (jsonl, vector, auto)
    /// NOTE: Use --format for output file format (markdown, text, json)
    #[arg(
        long = "export-format",
        alias = "export",
        default_value = "jsonl",
        value_enum,
        env = "WEBFANG_EXPORT_FORMAT"
    )]
    #[clap(next_help_heading = "Output")]
    pub export_format: ExportFormat,

    // ========== Elastic Ingestion (Issue #51, PR5) ==========
    /// CPU core override for the elastic ingestion Rayon pool (else auto-detect)
    #[arg(long, env = "WEBFANG_CPU_CORES")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub cpu_cores: Option<usize>,

    /// RAM budget override for the byte-weighted semaphore (`8GB`, `2048MB`, or bytes)
    #[arg(long, env = "WEBFANG_RAM_BUDGET")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub ram_budget: Option<String>,

    /// SQLite database path override for persisted resources/chunks
    #[arg(long, env = "WEBFANG_DB_PATH")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub db_path: Option<std::path::PathBuf>,

    /// Enable elastic ingestion pipeline (streaming, SQLite dedup, Rayon CPU bridge)
    #[arg(long, default_value = "false", env = "WEBFANG_ELASTIC")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub elastic: bool,

    /// Write extracted vectors to a JSONL file for RAG pipelines. Use `-` for
    /// stdout. No SQLite dependency — available in every build (core binary too).
    #[arg(long, env = "WEBFANG_OUTPUT_VECTORS")]
    #[clap(next_help_heading = "Elastic Ingestion")]
    pub output_vectors: Option<String>,

    // ========== Batch Processing ==========
    /// Enable batch mode — read URLs from stdin (one per line)
    #[arg(long, default_value = "false", env = "WEBFANG_BATCH")]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch: bool,

    /// Path to a file containing URLs to crawl (one per line)
    #[arg(long, env = "WEBFANG_BATCH_FILE")]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch_file: Option<std::path::PathBuf>,

    /// Maximum concurrent URLs in batch mode
    #[arg(long, default_value = "5", env = "WEBFANG_BATCH_CONCURRENCY", value_parser = parse_batch_concurrency)]
    #[clap(next_help_heading = "Batch Processing")]
    pub batch_concurrency: usize,

    // ========== Item Pipeline ==========
    /// Enable item pipeline processing (validate → clean → output)
    #[arg(long, default_value = "false", env = "WEBFANG_PIPELINE")]
    #[clap(next_help_heading = "Item Pipeline")]
    pub pipeline: bool,

    /// Pipeline output format: jsonl (default), none
    #[arg(
        long,
        default_value = "jsonl",
        value_enum,
        env = "WEBFANG_PIPELINE_OUTPUT"
    )]
    #[clap(next_help_heading = "Item Pipeline")]
    pub pipeline_output: PipelineOutputFormat,
}

/// Validate `--batch-concurrency` is greater than zero.
///
/// Clap's `value_parser!(usize)` does not expose `.range()` in the derive API,
/// so a custom parser enforces the invariant at the system boundary (#640).
fn parse_batch_concurrency(s: &str) -> Result<usize, String> {
    let value: usize = s
        .parse()
        .map_err(|_| format!("`{s}` no es un número entero válido"))?;
    if value == 0 {
        Err("batch-concurrency debe ser > 0".to_string())
    } else {
        Ok(value)
    }
}
