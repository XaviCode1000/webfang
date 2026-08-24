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
//! Slice 1 migrated the export flag group (`export`, mirroring
//! `cli::args::ExportArgs`); slice 2 migrates the crawler flag group
//! (`crawler`, mirroring `cli::args::CrawlerArgs`). Remaining option groups
//! keep their hand-written definitions until later slices. The clap derive
//! stays in place as the parsing engine — byte-identical help/error output is
//! the acceptance bar — while its value parsers and the parity tests route
//! through the spec so bounds and messages have exactly one home.

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
    /// Verbatim parse-failure message overriding the default
    /// `` `{raw}` no es {parse_failure_detail} `` rendering, with `{value}`
    /// substituted by the raw input. Present ONLY where pre-migration legacy
    /// wording cannot be expressed by the default template — byte-identical
    /// errors outrank uniformity (ADR-002 acceptance bar). When set, the
    /// default template and `parse_failure_detail` are unused for parsing.
    pub parse_failure_template: Option<&'static str>,
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
    /// Raw input cannot be parsed; the option's policy supplied a verbatim
    /// template whose fully rendered message (with `{value}` substituted) is
    /// carried here. Used when legacy wording cannot fit the [`Self::Parse`]
    /// shape.
    #[error("{0}")]
    ParseVerbatim(#[source] OwnedParseMessage),
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

/// Owned message produced by substituting `{value}` into a policy's verbatim
/// parse-failure template ([`NumericPolicy::parse_failure_template`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OwnedParseMessage {
    /// Fully rendered user-facing parse-failure message.
    pub message: String,
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
                let value: u64 = raw
                    .parse()
                    .map_err(|_| self.parse_failure_error(raw, policy))?;
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
        match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                self.parse_failure_error(raw, policy)
            },
            _ => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail: "un valor válido",
            },
        }
    }

    /// Render the parse-failure error per the policy: verbatim template with
    /// `{value}` substitution when present, default `` `{raw}` no es … ``
    /// shape otherwise.
    fn parse_failure_error(&self, raw: &str, policy: NumericPolicy) -> OptionSpecError {
        match policy.parse_failure_template {
            Some(template) => OptionSpecError::ParseVerbatim(OwnedParseMessage {
                message: template.replace("{value}", raw),
            }),
            None => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail: policy.parse_failure_detail,
            },
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
                parse_failure_template: None,
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
                parse_failure_template: None,
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
                parse_failure_template: None,
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

/// Crawler flag group (ADR-002 slice 2): mirrors `cli::args::CrawlerArgs`
/// field-by-field for every option whose surface is spec-compatible. The
/// parity tests in that module enforce lockstep.
///
/// Deferred this slice (structurally unsuitable or out of scope, recorded in
/// the PR): `concurrency` (custom `ConcurrencyConfig` FromStr with auto
/// detection), `rate_limit_burst` (raw-string staging validated in preflight,
/// warn-and-default semantics — not a clap bound), `include_patterns`,
/// `exclude_patterns`, `headers`, `cookies` (`Vec` args with value
/// delimiters), and the feature-gated pair `clean_ai` / `adaptive_selectors`
/// (cfg duplication must be mirrored in the spec before lockstep assertions
/// are meaningful across feature combinations).
pub mod crawler {
    use super::{NumericPolicy, OptionSpec, ValueKind};

    /// `--url <URL>` (short `-u`)
    pub const URL: OptionSpec = OptionSpec {
        id: "url",
        long: "url",
        short: Some('u'),
        aliases: &[],
        env: Some("WEBFANG_URL"),
        default: None,
        help: "URL to scrape (required unless using a subcommand)",
        heading: Some("Target"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `-s, --selector <SELECTOR>`
    pub const SELECTOR: OptionSpec = OptionSpec {
        id: "selector",
        long: "selector",
        short: Some('s'),
        aliases: &[],
        env: Some("WEBFANG_SELECTOR"),
        default: Some("body"),
        help: "CSS selector for content extraction",
        heading: Some("Target"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--delay-ms <DELAY_MS>` — metadata-only entry: parsing stays with
    /// clap's built-in `u64` parser (its exact error strings are English and
    /// must not change); `min: 0` records that no bound exists today.
    pub const DELAY_MS: OptionSpec = OptionSpec {
        id: "delay_ms",
        long: "delay-ms",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DELAY_MS"),
        default: Some("1000"),
        help: "Delay between requests in milliseconds",
        heading: Some("Discovery"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "delay-ms debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--max-pages <MAX_PAGES>` — FULLY migrated: bound enforced through
    /// [`OptionSpec::parse_uint`] with verbatim legacy messages (#780).
    pub const MAX_PAGES: OptionSpec = OptionSpec {
        id: "max_pages",
        long: "max-pages",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_MAX_PAGES"),
        default: Some("10"),
        help: "Maximum pages to scrape",
        heading: Some("Discovery"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un número entero válido",
                below_min_message: "--max-pages debe ser >= 1 (0 no deja páginas para scrapear)",
                parse_failure_template: Some("'{value}' no es un número válido para --max-pages"),
            },
        },
        feature_gate: None,
    };

    /// `--use-sitemap`
    pub const USE_SITEMAP: OptionSpec = OptionSpec {
        id: "use_sitemap",
        long: "use-sitemap",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_USE_SITEMAP"),
        default: Some("false"),
        help: "Use sitemap for URL discovery NOTE: HTTP redirects (301/302) are resolved at scrape-time, not parse-time. This avoids redundant HEAD requests during sitemap parsing for better performance",
        heading: Some("Discovery"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--sitemap-url <SITEMAP_URL>`
    pub const SITEMAP_URL: OptionSpec = OptionSpec {
        id: "sitemap_url",
        long: "sitemap-url",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_SITEMAP_URL"),
        default: None,
        help: "Explicit sitemap URL",
        heading: Some("Discovery"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--single-page`
    pub const SINGLE_PAGE: OptionSpec = OptionSpec {
        id: "single_page",
        long: "single-page",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_SINGLE_PAGE"),
        default: Some("false"),
        help: "Scrape only the seed URL without discovery or crawling",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--resume`
    pub const RESUME: OptionSpec = OptionSpec {
        id: "resume",
        long: "resume",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_RESUME"),
        default: Some("false"),
        help: "Resume mode - skip URLs already processed",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--state-dir <STATE_DIR>`
    pub const STATE_DIR: OptionSpec = OptionSpec {
        id: "state_dir",
        long: "state-dir",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_STATE_DIR"),
        default: None,
        help: "Custom state directory for resume mode",
        heading: Some("Behavior"),
        kind: ValueKind::Path,
        feature_gate: None,
    };

    /// `--download-images`
    pub const DOWNLOAD_IMAGES: OptionSpec = OptionSpec {
        id: "download_images",
        long: "download-images",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOWNLOAD_IMAGES"),
        default: Some("false"),
        help: "Download images from the page",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--download-documents`
    pub const DOWNLOAD_DOCUMENTS: OptionSpec = OptionSpec {
        id: "download_documents",
        long: "download-documents",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOWNLOAD_DOCUMENTS"),
        default: Some("false"),
        help: "Download documents from the page",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--download-assets`
    pub const DOWNLOAD_ASSETS: OptionSpec = OptionSpec {
        id: "download_assets",
        long: "download-assets",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOWNLOAD_ASSETS"),
        default: Some("false"),
        help: "Download all assets (images + documents) from the page",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--extraction-fingerprint`
    pub const EXTRACTION_FINGERPRINT: OptionSpec = OptionSpec {
        id: "extraction_fingerprint",
        long: "extraction-fingerprint",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_EXTRACTION_FINGERPRINT"),
        default: Some("false"),
        // Byte-exact transcription of clap's rendering of the multi-line doc
        // comment (lines joined with spaces, trailing period stripped).
        help: "Record extraction failure fingerprints in SQLite and attach them to low-quality extraction hints (#792) Repeated low-score extractions on the same site/selector pair accumulate a failure count surfaced in the hint, instead of degrading silently",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `-v, --verbose` — count action; metadata-only (`u8` count has no
    /// bound).
    pub const VERBOSE: OptionSpec = OptionSpec {
        id: "verbose",
        long: "verbose",
        short: Some('v'),
        aliases: &[],
        env: Some("WEBFANG_VERBOSE"),
        default: None,
        help: "Verbosity level: -v (INFO), -vv (DEBUG), -vvv (TRACE)",
        heading: Some("Display"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "verbose debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `-q, --quiet`
    pub const QUIET: OptionSpec = OptionSpec {
        id: "quiet",
        long: "quiet",
        short: Some('q'),
        aliases: &[],
        env: Some("WEBFANG_QUIET"),
        default: Some("false"),
        help: "Quiet mode — suppress info/debug output",
        heading: Some("Display"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `-n, --dry-run`
    pub const DRY_RUN: OptionSpec = OptionSpec {
        id: "dry_run",
        long: "dry-run",
        short: Some('n'),
        aliases: &[],
        env: Some("WEBFANG_DRY_RUN"),
        default: Some("false"),
        help: "Dry-run mode — discover URLs and print without scraping",
        heading: Some("Display"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--trace-file <TRACE_FILE>`
    pub const TRACE_FILE: OptionSpec = OptionSpec {
        id: "trace_file",
        long: "trace-file",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_TRACE_FILE"),
        default: None,
        help: "Path to write OTel spans as JSONL for offline debugging",
        heading: Some("Display"),
        kind: ValueKind::Path,
        feature_gate: None,
    };

    /// `--max-depth <MAX_DEPTH>` — metadata-only: 0 is meaningful ("only seed
    /// URL"), so no bound exists today.
    pub const MAX_DEPTH: OptionSpec = OptionSpec {
        id: "max_depth",
        long: "max-depth",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_MAX_DEPTH"),
        default: Some("2"),
        help: "Maximum depth to crawl (0 = only seed URL)",
        heading: Some("Crawler Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "max-depth debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--timeout-secs <TIMEOUT_SECS>` — FULLY migrated: bound enforced
    /// through [`OptionSpec::parse_uint`] with verbatim legacy messages.
    pub const TIMEOUT_SECS: OptionSpec = OptionSpec {
        id: "timeout_secs",
        long: "timeout-secs",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_TIMEOUT_SECS"),
        default: Some("30"),
        help: "Request timeout in seconds",
        heading: Some("Crawler Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un número entero válido",
                below_min_message:
                    "--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)",
                parse_failure_template: Some(
                    "'{value}' no es un número válido para --timeout-secs",
                ),
            },
        },
        feature_gate: None,
    };

    /// `--asset-naming <ASSET_NAMING>`
    pub const ASSET_NAMING: OptionSpec = OptionSpec {
        id: "asset_naming",
        long: "asset-naming",
        short: None,
        aliases: &[],
        env: None,
        default: Some("hash"),
        help: "Estrategia de nombre de archivo para assets descargados: hash (default), slug, content-disposition",
        heading: None,
        kind: ValueKind::Enum {
            variants: &["hash", "slug", "content-disposition"],
        },
        feature_gate: None,
    };

    /// `--download-concurrency <DOWNLOAD_CONCURRENCY>` — FULLY migrated:
    /// bound enforced through [`OptionSpec::parse_uint`] with verbatim legacy
    /// messages (D1 deadlock guard).
    pub const DOWNLOAD_CONCURRENCY: OptionSpec = OptionSpec {
        id: "download_concurrency",
        long: "download-concurrency",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOWNLOAD_CONCURRENCY"),
        default: None,
        // Explicit `help = ...` attribute overrides the doc comment.
        help: "Máximo de descargas de assets concurrentes por página (mínimo 1)",
        heading: None,
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 1,
                parse_failure_detail: "un número entero válido",
                below_min_message:
                    "--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)",
                parse_failure_template: Some(
                    "'{value}' no es un número válido para --download-concurrency",
                ),
            },
        },
        feature_gate: None,
    };

    /// `--max-retries <MAX_RETRIES>` — metadata-only (no bound today).
    pub const MAX_RETRIES: OptionSpec = OptionSpec {
        id: "max_retries",
        long: "max-retries",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_MAX_RETRIES"),
        default: Some("3"),
        help: "Maximum number of retry attempts",
        heading: Some("HTTP Client Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "max-retries debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--backoff-base-ms <BACKOFF_BASE_MS>` — metadata-only (no bound
    /// today).
    pub const BACKOFF_BASE_MS: OptionSpec = OptionSpec {
        id: "backoff_base_ms",
        long: "backoff-base-ms",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_BACKOFF_BASE_MS"),
        default: Some("1000"),
        help: "Base delay for exponential backoff (ms)",
        heading: Some("HTTP Client Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "backoff-base-ms debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--backoff-max-ms <BACKOFF_MAX_MS>` — metadata-only (no bound today).
    pub const BACKOFF_MAX_MS: OptionSpec = OptionSpec {
        id: "backoff_max_ms",
        long: "backoff-max-ms",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_BACKOFF_MAX_MS"),
        default: Some("10000"),
        help: "Maximum delay for exponential backoff (ms)",
        heading: Some("HTTP Client Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "backoff-max-ms debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--accept-language <ACCEPT_LANGUAGE>`
    pub const ACCEPT_LANGUAGE: OptionSpec = OptionSpec {
        id: "accept_language",
        long: "accept-language",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_ACCEPT_LANGUAGE"),
        default: Some("en-US,en;q=0.9"),
        help: "Accept-Language header value",
        heading: Some("HTTP Client Settings"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--user-agent <USER_AGENT>`
    pub const USER_AGENT: OptionSpec = OptionSpec {
        id: "user_agent",
        long: "user-agent",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_USER_AGENT"),
        default: None,
        help: "Custom User-Agent header value (overrides Chrome 145 default)",
        heading: Some("HTTP Client Settings"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--max-file-size <MAX_FILE_SIZE>` — metadata-only (no bound today).
    pub const MAX_FILE_SIZE: OptionSpec = OptionSpec {
        id: "max_file_size",
        long: "max-file-size",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_MAX_FILE_SIZE"),
        default: Some("52428800"),
        help: "Maximum file size to download in bytes (default: 50MB)",
        heading: Some("Download Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "max-file-size debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--download-timeout <DOWNLOAD_TIMEOUT>` — metadata-only (no bound
    /// today).
    pub const DOWNLOAD_TIMEOUT: OptionSpec = OptionSpec {
        id: "download_timeout",
        long: "download-timeout",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOWNLOAD_TIMEOUT"),
        default: Some("30"),
        help: "Timeout for individual asset downloads in seconds",
        heading: Some("Download Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "download-timeout debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--sitemap-depth <SITEMAP_DEPTH>` — metadata-only (no bound today).
    pub const SITEMAP_DEPTH: OptionSpec = OptionSpec {
        id: "sitemap_depth",
        long: "sitemap-depth",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_SITEMAP_DEPTH"),
        default: Some("3"),
        help: "Maximum recursion depth for sitemap indexes",
        heading: Some("Sitemap Settings"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "sitemap-depth debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--checkpoint-interval <CHECKPOINT_INTERVAL>` — metadata-only (0 =
    /// disabled by design, no bound today).
    pub const CHECKPOINT_INTERVAL: OptionSpec = OptionSpec {
        id: "checkpoint_interval",
        long: "checkpoint-interval",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_CHECKPOINT_INTERVAL"),
        default: Some("100"),
        help: "Pages between automatic checkpoint saves (0 = disabled) NOTE: Checkpoint is for programmatic use (Engine API) only. CLI --resume uses StateStore instead of checkpoints",
        heading: Some("Competitive Features"),
        kind: ValueKind::Uint {
            policy: NumericPolicy {
                min: 0,
                parse_failure_detail: "un número entero válido",
                below_min_message: "checkpoint-interval debe ser >= 0",
                parse_failure_template: None,
            },
        },
        feature_gate: None,
    };

    /// `--no-checkpoint`
    pub const NO_CHECKPOINT: OptionSpec = OptionSpec {
        id: "no_checkpoint",
        long: "no-checkpoint",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_NO_CHECKPOINT"),
        default: Some("false"),
        help: "Disable checkpoint persistence entirely NOTE: Checkpoint is for programmatic use (Engine API) only. CLI --resume uses StateStore instead of checkpoints",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--ignore-robots`
    pub const IGNORE_ROBOTS: OptionSpec = OptionSpec {
        id: "ignore_robots",
        long: "ignore-robots",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_IGNORE_ROBOTS"),
        default: Some("false"),
        help: "Skip robots.txt enforcement",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--ignore-waf`
    pub const IGNORE_WAF: OptionSpec = OptionSpec {
        id: "ignore_waf",
        long: "ignore-waf",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_IGNORE_WAF"),
        default: Some("false"),
        help: "Bypass WAF/CAPTCHA detection entirely (never block on challenge markers)",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--autoscale`
    pub const AUTOSCALE: OptionSpec = OptionSpec {
        id: "autoscale",
        long: "autoscale",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_AUTOSCALE"),
        default: Some("false"),
        help: "Enable autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--no-session-health`
    pub const NO_SESSION_HEALTH: OptionSpec = OptionSpec {
        id: "no_session_health",
        long: "no-session-health",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_NO_SESSION_HEALTH"),
        default: Some("false"),
        help: "Disable session pool health checks",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// `--h2-profile <H2_PROFILE>`
    pub const H2_PROFILE: OptionSpec = OptionSpec {
        id: "h2_profile",
        long: "h2-profile",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_H2_PROFILE"),
        default: Some("Chrome145"),
        help: "TLS/HTTP2 profile name (default: Chrome145)",
        heading: Some("Competitive Features"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--js-strategy <JS_STRATEGY>`
    pub const JS_STRATEGY: OptionSpec = OptionSpec {
        id: "js_strategy",
        long: "js-strategy",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_JS_STRATEGY"),
        default: Some("static"),
        help: "JavaScript rendering strategy: static (wreq only), hybrid (3-layer), full (Chromiumoxide only)",
        heading: Some("JS Rendering"),
        kind: ValueKind::Enum {
            variants: &["static", "hybrid", "full"],
        },
        feature_gate: None,
    };

    /// `--obscura-binary <OBSCURA_BINARY>`
    pub const OBSCURA_BINARY: OptionSpec = OptionSpec {
        id: "obscura_binary",
        long: "obscura-binary",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_OBSCURA_BINARY"),
        default: Some("obscura"),
        help: "Path to the obscura binary (default: \"obscura\")",
        heading: Some("JS Rendering"),
        kind: ValueKind::Text,
        feature_gate: None,
    };

    /// `--dom-preprune` — optional-value bool (`num_args(0..=1)`); the arity
    /// nuance stays in the derive, the spec carries identity/metadata.
    pub const DOM_PREPRUNE: OptionSpec = OptionSpec {
        id: "dom_preprune",
        long: "dom-preprune",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOM_PREPRUNE"),
        default: Some("true"),
        help: "Enable DOM pre-pruning before Readability (removes invisible/empty wrappers). Default: enabled (true). Set to false via --dom-preprune=false or WEBFANG_DOM_PREPRUNE=false",
        heading: Some("Cleanup"),
        kind: ValueKind::Bool,
        feature_gate: None,
    };

    /// All crawler-group options, in `CrawlerArgs` field-declaration order
    /// (deferred fields omitted; see the module documentation).
    pub const GROUP: &[OptionSpec] = &[
        URL,
        SELECTOR,
        DELAY_MS,
        MAX_PAGES,
        USE_SITEMAP,
        SITEMAP_URL,
        SINGLE_PAGE,
        RESUME,
        STATE_DIR,
        DOWNLOAD_IMAGES,
        DOWNLOAD_DOCUMENTS,
        DOWNLOAD_ASSETS,
        EXTRACTION_FINGERPRINT,
        VERBOSE,
        QUIET,
        DRY_RUN,
        TRACE_FILE,
        MAX_DEPTH,
        TIMEOUT_SECS,
        ASSET_NAMING,
        DOWNLOAD_CONCURRENCY,
        MAX_RETRIES,
        BACKOFF_BASE_MS,
        BACKOFF_MAX_MS,
        ACCEPT_LANGUAGE,
        USER_AGENT,
        MAX_FILE_SIZE,
        DOWNLOAD_TIMEOUT,
        SITEMAP_DEPTH,
        CHECKPOINT_INTERVAL,
        NO_CHECKPOINT,
        IGNORE_ROBOTS,
        IGNORE_WAF,
        AUTOSCALE,
        NO_SESSION_HEALTH,
        H2_PROFILE,
        JS_STRATEGY,
        OBSCURA_BINARY,
        DOM_PREPRUNE,
    ];
}

#[cfg(test)]
mod tests {
    use super::{crawler, export, schema_object, OptionSpecError};
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
    fn crawler_group_covers_every_non_deferred_crawler_args_field() {
        assert_eq!(crawler::GROUP.len(), 39);
        for opt in crawler::GROUP {
            assert!(
                opt.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "id `{}` must be snake_case",
                opt.id
            );
        }
    }

    #[test]
    fn verbatim_template_renders_byte_exactly_for_migrated_crawler_flags() {
        let parse_err = crawler::MAX_PAGES
            .parse_uint("abc")
            .expect_err("must reject text");
        assert_eq!(
            parse_err.to_string(),
            "'abc' no es un número válido para --max-pages"
        );

        let bound_err = crawler::MAX_PAGES
            .parse_uint("0")
            .expect_err("zero pages is invalid");
        assert_eq!(
            bound_err.to_string(),
            "--max-pages debe ser >= 1 (0 no deja páginas para scrapear)"
        );

        let timeout_parse = crawler::TIMEOUT_SECS.parse_error("xyz");
        assert_eq!(
            timeout_parse.to_string(),
            "'xyz' no es un número válido para --timeout-secs"
        );
    }

    #[test]
    fn metadata_only_numeric_entries_accept_zero_and_any_value() {
        // min: 0 entries record "no bound today"; the spec must not invent one.
        assert_eq!(crawler::DELAY_MS.parse_uint("0").expect("unbounded"), 0);
        assert_eq!(
            crawler::MAX_DEPTH.parse_uint("255").expect("u8 domain"),
            255
        );
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
