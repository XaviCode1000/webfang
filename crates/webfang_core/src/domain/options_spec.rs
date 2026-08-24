//! OptionsSpec — single source of truth for user-facing options (ADR-002).
//!
//! Every user-facing option is described exactly once as declarative data:
//! identifiers, value kind, default, environment variable, numeric bounds,
//! help text, help heading, feature gate, and clap surface (long/short/
//! aliases). From this description the project derives, slice by slice:
//!
//! 1. clap argument definitions,
//! 2. JSON Schema for MCP tool input validation,
//! 3. shared validators with identical bounds.
//!
//! Slice 1 migrates ONLY the export flag group ([`export`], mirroring
//! `cli::args::ExportArgs`); every other option group keeps its hand-written
//! definitions until later slices. The clap derive stays in place as the
//! parsing engine — byte-identical help/error output is the acceptance bar —
//! while its value parsers and the parity tests route through the spec so
//! bounds and messages have exactly one home.

use serde_json::{json, Map, Value};

/// Declarative description of ONE user-facing option (ADR-002).
///
/// Pure data: no clap types, no infrastructure types — this is domain-layer
/// SSOT that both the CLI bridge and (in later slices) the MCP schema
/// generator consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    /// Stable identifier — clap arg id today, MCP property name tomorrow.
    pub id: &'static str,
    /// Long flag name without leading dashes (kebab-case).
    pub long: &'static str,
    /// Single-dash flag, if any.
    pub short: Option<char>,
    /// Alternative long names accepted by clap.
    pub aliases: &'static [&'static str],
    /// Environment variable consulted when the flag is absent.
    pub env: Option<&'static str>,
    /// Canonical default value as rendered in help and schema.
    pub default: Option<&'static str>,
    /// Help text (`--help` long form); must match the clap derive's rendering.
    pub help: &'static str,
    /// Help heading/group under which clap lists this option.
    pub heading: Option<&'static str>,
    /// Value domain plus validation policy (bounds live HERE only).
    pub kind: ValueKind,
    /// Cargo feature required for this option to exist (`None` = always).
    /// No export-group option is gated; the seam serves later slices.
    pub feature_gate: Option<&'static str>,
}

/// Validation policy for numeric options: the inclusive lower bound and the
/// exact user-facing messages raised when it is violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumericPolicy {
    /// Inclusive lower bound (`1` ⇒ zero is rejected).
    pub min: u64,
    /// Completes "`{raw}` no es …" when parsing fails.
    pub parse_failure_detail: &'static str,
    /// Complete message raised when the value is below [`NumericPolicy::min`].
    pub below_min_message: &'static str,
}

/// Value domain of an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// Free-form string.
    Text,
    /// Filesystem path.
    Path,
    /// Boolean switch parsed from `"true"`/`"false"`.
    Bool,
    /// Closed set of accepted canonical (kebab-case) values.
    Enum {
        /// Accepted values in declaration order.
        variants: &'static [&'static str],
    },
    /// Unsigned integer with a [`NumericPolicy`] bound.
    Uint {
        /// Shared parse + bound policy.
        policy: NumericPolicy,
    },
    /// Memory size with binary-suffix support (`8GB`, `2048MB`, plain bytes).
    /// Suffix parsing lives in `infrastructure::autotuning`; the bound lives
    /// here via [`NumericPolicy`].
    MemorySize {
        /// Shared bound policy for externally-parsed byte counts.
        policy: NumericPolicy,
    },
}

/// Error raised by spec-driven validators.
///
/// Messages are user-facing (Spanish) and must render byte-identically to
/// the pre-migration clap parsers — pinned by the parity tests in
/// `cli::args::export`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OptionSpecError {
    /// Raw input cannot be parsed into the option's value kind.
    #[error("`{raw}` no es {detail}")]
    Parse {
        /// The offending raw input.
        raw: String,
        /// Completes the Spanish message ("un número entero válido", …).
        detail: &'static str,
    },
    /// Parsed value violates the option's inclusive lower bound.
    #[error("{0}")]
    Bound(#[source] BoxedBoundMessage),
    /// A validator was called on an option whose kind does not support it.
    #[error("option `{id}` does not support this validator")]
    UnsupportedKind {
        /// Identifier of the misused option.
        id: &'static str,
    },
}

/// Owned wrapper so `OptionSpecError` stays `Clone + PartialEq` while the
/// message remains a cheap `&'static str` inside.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BoxedBoundMessage {
    /// Exact user-facing bound-violation message.
    pub message: &'static str,
}

impl OptionSpec {
    /// Whether this option participates in the current build given its
    /// feature gate. Slice 1 has no gated options.
    #[must_use]
    pub const fn active(&self) -> bool {
        self.feature_gate.is_none()
    }

    /// Validate and parse a raw textual value as an unsigned integer.
    ///
    /// Only meaningful for [`ValueKind::Uint`]; other kinds yield
    /// [`OptionSpecError::UnsupportedKind`]. Bounds come exclusively from the
    /// spec — this is THE single enforcement point for migrated paths.
    ///
    /// # Errors
    ///
    /// [`OptionSpecError::Parse`] on malformed input;
    /// [`OptionSpecError::Bound`] below the inclusive minimum;
    /// [`OptionSpecError::UnsupportedKind`] for non-integer kinds.
    pub fn parse_uint(&self, raw: &str) -> Result<u64, OptionSpecError> {
        match self.kind {
            ValueKind::Uint { policy } => {
                let value: u64 = raw.parse().map_err(|_| OptionSpecError::Parse {
                    raw: raw.to_string(),
                    detail: policy.parse_failure_detail,
                })?;
                if value < policy.min {
                    return Err(OptionSpecError::Bound(BoxedBoundMessage {
                        message: policy.below_min_message,
                    }));
                }
                Ok(value)
            },
            _ => Err(self.unsupported_kind()),
        }
    }

    /// Enforce the inclusive lower bound on a value parsed elsewhere
    /// ([`ValueKind::MemorySize`], whose suffix parsing belongs to
    /// infrastructure). Returns the input unchanged on success.
    ///
    /// # Errors
    ///
    /// [`OptionSpecError::Bound`] when below the inclusive minimum;
    /// [`OptionSpecError::UnsupportedKind`] for kinds without bounds.
    pub fn check_bound(&self, value: u64) -> Result<u64, OptionSpecError> {
        match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                if value < policy.min {
                    Err(OptionSpecError::Bound(BoxedBoundMessage {
                        message: policy.below_min_message,
                    }))
                } else {
                    Ok(value)
                }
            },
            _ => Err(self.unsupported_kind()),
        }
    }

    /// Build the parse-failure error for this option's kind with the exact
    /// stored message. Used by bridges whose parsing engine is external
    /// (e.g. memory-size suffix parsing).
    #[must_use]
    pub fn parse_error(&self, raw: &str) -> OptionSpecError {
        let detail = match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                policy.parse_failure_detail
            },
            _ => "un valor válido",
        };
        OptionSpecError::Parse {
            raw: raw.to_string(),
            detail,
        }
    }

    fn unsupported_kind(&self) -> OptionSpecError {
        OptionSpecError::UnsupportedKind { id: self.id }
    }

    /// JSON Schema fragment describing this option (ADR-002 seam #2).
    ///
    /// Not wired into MCP yet — consumed by tests in slice 1; the MCP tool
    /// schema generation is a later slice.
    #[must_use]
    pub fn json_schema(&self) -> Value {
        let mut schema = Map::new();
        schema.insert("description".into(), json!(self.help));
        match self.kind {
            ValueKind::Text | ValueKind::Path => {
                schema.insert("type".into(), json!("string"));
            },
            ValueKind::Bool => {
                schema.insert("type".into(), json!("boolean"));
            },
            ValueKind::Enum { variants } => {
                schema.insert("type".into(), json!("string"));
                schema.insert("enum".into(), json!(variants));
            },
            ValueKind::Uint { policy } => {
                schema.insert("type".into(), json!("integer"));
                schema.insert("minimum".into(), json!(policy.min));
            },
            ValueKind::MemorySize { policy } => {
                // CLI/env input is textual (suffixes allowed); the byte floor
                // travels alongside until the MCP slice picks a final shape.
                schema.insert("type".into(), json!("string"));
                schema.insert("format".into(), json!("byte-size"));
                schema.insert("minimumBytes".into(), json!(policy.min));
            },
        }
        if let Some(default) = self.default {
            schema.insert("default".into(), json!(default));
        }
        if let Some(gate) = self.feature_gate {
            schema.insert("x-feature-gate".into(), json!(gate));
        }
        Value::Object(schema)
    }
}

/// Aggregate per-option JSON Schemas into a `{id: schema}` object.
///
/// Key layout follows serde_json's map ordering (alphabetical), not slice
/// order — consumers must key by id.
#[must_use]
pub fn schema_object(options: &[OptionSpec]) -> Value {
    Value::Object(
        options
            .iter()
            .map(|opt| (opt.id.to_owned(), opt.json_schema()))
            .collect::<Map<String, Value>>(),
    )
}

/// Export flag group (ADR-002 slice 1): mirrors `cli::args::ExportArgs`
/// field-by-field. The parity tests in that module enforce lockstep.
pub mod export {
    use super::{NumericPolicy, OptionSpec, ValueKind};

    /// `--output <OUTPUT>` (short `-o`)
    pub const OUTPUT: OptionSpec = OptionSpec {
        id: "output",
        long: "output",
        short: Some('o'),
        aliases: &[],
        env: Some("WEBFANG_OUTPUT"),
        default: Some("output"),
        help: "Output directory for scraped content",
        heading: Some("Output"),
        kind: ValueKind::Path,
        feature_gate: None,
    };

    /// `-f, --format <FORMAT>`
    pub const FORMAT: OptionSpec = OptionSpec {
        id: "format",
        long: "format",
        short: Some('f'),
        aliases: &[],
        env: Some("WEBFANG_FORMAT"),
        default: Some("markdown"),
        // Byte-exact transcription of clap's rendering of the two-paragraph
        // doc comment (lines are joined with spaces).
        help: "Output format for individual files (markdown, text, json) NOTE: For RAG pipeline export, use --export-format instead",
        heading: Some("Output"),
        kind: ValueKind::Enum {
            variants: &["markdown", "json", "text"],
        },
        feature_gate: None,
    };

    /// `--export-format <EXPORT_FORMAT>` (alias `--export`)
    pub const EXPORT_FORMAT: OptionSpec = OptionSpec {
        id: "export_format",
        long: "export-format",
        short: None,
        aliases: &["export"],
        env: Some("WEBFANG_EXPORT_FORMAT"),
        default: Some("jsonl"),
        help: "Export format for RAG pipeline (jsonl, vector, auto) NOTE: Use --format for output file format (markdown, text, json)",
        heading: Some("Output"),
        kind: ValueKind::Enum {
            variants: &["jsonl", "vector", "auto"],
        },
        feature_gate: None,
    };

    /// `--cpu-cores <CPU_CORES>`
    pub const CPU_CORES: OptionSpec = OptionSpec {
        id: "cpu_cores",
        long: "cpu-cores",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_CPU_CORES"),
        default: None,
        help: "CPU core override for the elastic ingestion Rayon pool (else auto-detect)",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un número entero válido",
                below_min_message: "cpu-cores debe ser > 0",
            },
        },
        feature_gate: None,
    };

    /// `--ram-budget <RAM_BUDGET>`
    pub const RAM_BUDGET: OptionSpec = OptionSpec {
        id: "ram_budget",
        long: "ram-budget",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_RAM_BUDGET"),
        default: None,
        help: "RAM budget override for the byte-weighted semaphore (`8GB`, `2048MB`, or bytes)",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::MemorySize {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un tamaño de memoria válido",
                below_min_message: "ram-budget debe ser > 0",
            },
        },
        feature_gate: None,
    };

    /// `--db-path <DB_PATH>`
    pub const DB_PATH: OptionSpec = OptionSpec {
        id: "db_path",
        long: "db-path",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DB_PATH"),
        default: None,
        help: "SQLite database path override for persisted resources/chunks",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::Path,
        feature_gate: None,
    };

    /// `--elastic`
    pub const ELASTIC: OptionSpec = OptionSpec {
        id: "elastic",
        long: "elastic",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_ELASTIC"),
        default: Some("false"),
        help: "Enable elastic ingestion pipeline (streaming, SQLite dedup, Rayon CPU bridge)",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--output-vectors <OUTPUT_VECTORS>`
    pub const OUTPUT_VECTORS: OptionSpec = OptionSpec {
        id: "output_vectors",
        long: "output-vectors",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_OUTPUT_VECTORS"),
        default: None,
        // Trailing period stripped by clap's doc-comment rendering.
        help: "Write extracted vectors to a JSONL file for RAG pipelines. Use `-` for stdout. No SQLite dependency — available in every build (core binary too)",
        heading: Some("Elastic Ingestion"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--batch`
    pub const BATCH: OptionSpec = OptionSpec {
        id: "batch",
        long: "batch",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_BATCH"),
        default: Some("false"),
        help: "Enable batch mode — read URLs from stdin (one per line)",
        heading: Some("Batch Processing"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--batch-file <BATCH_FILE>`
    pub const BATCH_FILE: OptionSpec = OptionSpec {
        id: "batch_file",
        long: "batch-file",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_BATCH_FILE"),
        default: None,
        help: "Path to a file containing URLs to crawl (one per line)",
        heading: Some("Batch Processing"),
        kind: ValueKind::Path,
        feature_gate: None,
    };

    /// `--batch-concurrency <BATCH_CONCURRENCY>`
    pub const BATCH_CONCURRENCY: OptionSpec = OptionSpec {
        id: "batch_concurrency",
        long: "batch-concurrency",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_BATCH_CONCURRENCY"),
        default: None,
        help: "Maximum concurrent URLs in batch mode (omit = auto from budget model)",
        heading: Some("Batch Processing"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un número entero válido",
                below_min_message: "batch-concurrency debe ser > 0",
            },
        },
        feature_gate: None,
    };

    /// `--pipeline`
    pub const PIPELINE: OptionSpec = OptionSpec {
        id: "pipeline",
        long: "pipeline",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_PIPELINE"),
        default: Some("false"),
        help: "Enable item pipeline processing (validate → clean → output)",
        heading: Some("Item Pipeline"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--pipeline-output <PIPELINE_OUTPUT>`
    pub const PIPELINE_OUTPUT: OptionSpec = OptionSpec {
        id: "pipeline_output",
        long: "pipeline-output",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_PIPELINE_OUTPUT"),
        default: Some("jsonl"),
        help: "Pipeline output format: jsonl (default), none",
        heading: Some("Item Pipeline"),
        kind: ValueKind::Enum {
            variants: &["jsonl", "none"],
        },
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
}

#[cfg(test)]
mod tests {
    use super::{export, schema_object, OptionSpecError};
    use serde_json::json;

    #[test]
    fn every_export_option_has_a_wellformed_identity() {
        for opt in export::GROUP {
            assert!(
                opt.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "id `{}` must be snake_case (clap arg id / MCP property)",
                opt.id
            );
            assert!(
                opt.long
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "long `{}` must be kebab-case",
                opt.long
            );
        }
        assert_eq!(
            export::GROUP.len(),
            13,
            "export group must cover all ExportArgs fields"
        );
    }

    #[test]
    fn parse_uint_rejects_non_numeric_input_with_exact_message() {
        let err = export::CPU_CORES
            .parse_uint("abc")
            .expect_err("must reject text");
        assert_eq!(err.to_string(), "`abc` no es un número entero válido");
    }

    #[test]
    fn parse_uint_rejects_zero_with_exact_message() {
        let err = export::CPU_CORES
            .parse_uint("0")
            .expect_err("zero cores is invalid");
        assert_eq!(err.to_string(), "cpu-cores debe ser > 0");
    }

    #[test]
    fn parse_uint_accepts_positive_values() {
        assert_eq!(export::CPU_CORES.parse_uint("8").expect("valid"), 8);
        assert_eq!(
            export::BATCH_CONCURRENCY.parse_uint("16").expect("valid"),
            16
        );
    }

    #[test]
    fn memory_size_bound_rejects_zero_with_exact_message() {
        let err = export::RAM_BUDGET
            .check_bound(0)
            .expect_err("zero budget is invalid");
        assert_eq!(err.to_string(), "ram-budget debe ser > 0");
        assert_eq!(export::RAM_BUDGET.check_bound(2048).expect("valid"), 2048);
    }

    #[test]
    fn non_integer_kinds_have_no_integer_parser() {
        let err = export::OUTPUT
            .parse_uint("output")
            .expect_err("Path kind has no uint parser");
        assert!(matches!(err, OptionSpecError::UnsupportedKind { .. }));
    }

    #[test]
    fn json_schema_describes_enum_options_with_canonical_variants() {
        let schema = export::EXPORT_FORMAT.json_schema();
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["enum"], json!(["jsonl", "vector", "auto"]));
        assert_eq!(schema["default"], "jsonl");
        assert!(schema["description"].as_str().is_some());
    }

    #[test]
    fn json_schema_encodes_numeric_bounds_from_the_spec() {
        let schema = export::CPU_CORES.json_schema();
        assert_eq!(schema["type"], "integer");
        assert_eq!(schema["minimum"], 1);

        let ram = export::RAM_BUDGET.json_schema();
        assert_eq!(ram["format"], "byte-size");
        assert_eq!(ram["minimumBytes"], 1);
    }

    #[test]
    fn json_schema_covers_boolean_path_and_text_kinds() {
        let elastic = export::ELASTIC.json_schema();
        assert_eq!(elastic["type"], "boolean");
        assert_eq!(elastic["default"], "false");

        let output = export::OUTPUT.json_schema();
        assert_eq!(output["type"], "string");

        let vectors = export::OUTPUT_VECTORS.json_schema();
        assert_eq!(vectors["type"], "string");
    }

    #[test]
    fn schema_object_aggregates_the_whole_group_by_id() {
        let obj = schema_object(export::GROUP);
        let entries = obj.as_object().expect("aggregate must be an object");
        assert_eq!(entries.len(), 13);
        for opt in export::GROUP {
            assert!(
                entries.contains_key(opt.id),
                "missing `{}` in aggregate",
                opt.id
            );
        }
        assert_eq!(
            obj["pipeline_output"]["enum"],
            json!(["jsonl", "none"]),
            "PipelineOutputFormat variants must match clap's kebab-case rendering"
        );
        assert_eq!(
            obj["format"]["enum"],
            json!(["markdown", "json", "text"]),
            "OutputFormat variants must match clap's kebab-case rendering"
        );
    }
}
