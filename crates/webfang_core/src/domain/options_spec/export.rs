//! Export flag group (ADR-002 slice 1): mirrors `cli::args::ExportArgs`
//! field-by-field. The parity tests in that module enforce lockstep.
use super::{DefaultValue, NumericPolicy, OptionSpec, ValueKind};

/// `--output <OUTPUT>` (short `-o`)
pub const OUTPUT: OptionSpec = OptionSpec {
    id: "output",
    value_name: "OUTPUT",
    long: "output",
    short: Some('o'),
    aliases: &[],
    env: Some("WEBFANG_OUTPUT"),
    default: Some(DefaultValue::Str("output")),
    help: "Output directory for scraped content",
    heading: Some("Output"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `-f, --format <FORMAT>`
pub const FORMAT: OptionSpec = OptionSpec {
        id: "format",
        value_name: "FORMAT",
        long: "format",
        short: Some('f'),
        aliases: &[],
        env: Some("WEBFANG_FORMAT"),
        default: Some(DefaultValue::Str("markdown")),
        // Byte-exact transcription of clap's rendering of the two-paragraph
        // doc comment (lines are joined with spaces).
        help: "Output format for individual files (markdown, text, json) NOTE: For RAG pipeline export, use --export-format instead",
        heading: Some("Output"),
        kind: ValueKind::Enum {
            variants: &["markdown", "json", "text"],
        },
        visible_aliases: &[],
        nullable: false,
        description_override: None,
        feature_gate: None,
    };

/// `--export-format <EXPORT_FORMAT>` (alias `--export`)
pub const EXPORT_FORMAT: OptionSpec = OptionSpec {
        id: "export_format",
        value_name: "EXPORT_FORMAT",
        long: "export-format",
        short: None,
        aliases: &["export"],
        env: Some("WEBFANG_EXPORT_FORMAT"),
        default: Some(DefaultValue::Str("jsonl")),
        help: "Export format for RAG pipeline (jsonl, vector, auto) NOTE: Use --format for output file format (markdown, text, json)",
        heading: Some("Output"),
        kind: ValueKind::Enum {
            variants: &["jsonl", "vector", "auto"],
        },
        visible_aliases: &[],
        nullable: false,
        description_override: None,
        feature_gate: None,
    };

/// `--cpu-cores <CPU_CORES>`
pub const CPU_CORES: OptionSpec = OptionSpec {
    id: "cpu_cores",
    value_name: "CPU_CORES",
    long: "cpu-cores",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_CPU_CORES"),
    default: None,
    help: "CPU core override for the elastic ingestion Rayon pool (else auto-detect)",
    heading: Some("Elastic Ingestion"),
    kind: ValueKind::uint(NumericPolicy::positive("cpu-cores debe ser > 0")),
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--ram-budget <RAM_BUDGET>`
pub const RAM_BUDGET: OptionSpec = OptionSpec {
    id: "ram_budget",
    value_name: "RAM_BUDGET",
    long: "ram-budget",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_RAM_BUDGET"),
    default: None,
    help: "RAM budget override for the byte-weighted semaphore (`8GB`, `2048MB`, or bytes)",
    heading: Some("Elastic Ingestion"),
    kind: ValueKind::MemorySize {
        policy: Some(NumericPolicy {
            min: 1,
            max: None,
            parse_failure_detail: "un tamaño de memoria válido",
            below_min_message: "ram-budget debe ser > 0",
            above_max_message: "",
            parse_failure_template: None,
        }),
    },
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--db-path <DB_PATH>`
pub const DB_PATH: OptionSpec = OptionSpec {
    id: "db_path",
    value_name: "DB_PATH",
    long: "db-path",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DB_PATH"),
    default: None,
    help: "SQLite database path override for persisted resources/chunks",
    heading: Some("Elastic Ingestion"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--elastic`
pub const ELASTIC: OptionSpec = OptionSpec {
    id: "elastic",
    value_name: "ELASTIC",
    long: "elastic",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_ELASTIC"),
    default: Some(DefaultValue::Bool(false)),
    help: "Enable elastic ingestion pipeline (streaming, SQLite dedup, Rayon CPU bridge)",
    heading: Some("Elastic Ingestion"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--output-vectors <OUTPUT_VECTORS>`
pub const OUTPUT_VECTORS: OptionSpec = OptionSpec {
        id: "output_vectors",
        value_name: "OUTPUT_VECTORS",
        long: "output-vectors",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_OUTPUT_VECTORS"),
        default: None,
        // Trailing period stripped by clap's doc-comment rendering.
        help: "Write extracted vectors to a JSONL file for RAG pipelines. Use `-` for stdout. No SQLite dependency — available in every build (core binary too)",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::Text,
        visible_aliases: &[],
        nullable: false,
        description_override: None,
        feature_gate: None,
    };

/// `--batch`
pub const BATCH: OptionSpec = OptionSpec {
    id: "batch",
    value_name: "BATCH",
    long: "batch",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BATCH"),
    default: Some(DefaultValue::Bool(false)),
    help: "Enable batch mode — read URLs from stdin (one per line)",
    heading: Some("Batch Processing"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--batch-file <BATCH_FILE>`
pub const BATCH_FILE: OptionSpec = OptionSpec {
    id: "batch_file",
    value_name: "BATCH_FILE",
    long: "batch-file",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BATCH_FILE"),
    default: None,
    help: "Path to a file containing URLs to crawl (one per line)",
    heading: Some("Batch Processing"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--batch-concurrency <BATCH_CONCURRENCY>`
pub const BATCH_CONCURRENCY: OptionSpec = OptionSpec {
    id: "batch_concurrency",
    value_name: "BATCH_CONCURRENCY",
    long: "batch-concurrency",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BATCH_CONCURRENCY"),
    default: None,
    help: "Maximum concurrent URLs in batch mode (omit = auto from budget model)",
    heading: Some("Batch Processing"),
    kind: ValueKind::uint(NumericPolicy::positive("batch-concurrency debe ser > 0")),
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--pipeline`
pub const PIPELINE: OptionSpec = OptionSpec {
    id: "pipeline",
    value_name: "PIPELINE",
    long: "pipeline",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_PIPELINE"),
    default: Some(DefaultValue::Bool(false)),
    help: "Enable item pipeline processing (validate → clean → output)",
    heading: Some("Item Pipeline"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// `--pipeline-output <PIPELINE_OUTPUT>`
pub const PIPELINE_OUTPUT: OptionSpec = OptionSpec {
    id: "pipeline_output",
    value_name: "PIPELINE_OUTPUT",
    long: "pipeline-output",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_PIPELINE_OUTPUT"),
    default: Some(DefaultValue::Str("jsonl")),
    help: "Pipeline output format: jsonl (default), none",
    heading: Some("Item Pipeline"),
    kind: ValueKind::Enum {
        variants: &["jsonl", "none"],
    },
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
};

/// All export-group options, in `ExportArgs` field-declaration order.
pub const GROUP: &[OptionSpec] = &[
    OUTPUT,
    FORMAT,
    EXPORT_FORMAT,
    CPU_CORES,
    RAM_BUDGET,
    DB_PATH,
    ELASTIC,
    OUTPUT_VECTORS,
    BATCH,
    BATCH_FILE,
    BATCH_CONCURRENCY,
    PIPELINE,
    PIPELINE_OUTPUT,
];
