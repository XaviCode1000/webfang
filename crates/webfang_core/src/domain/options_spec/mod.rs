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

impl NumericPolicy {
    /// Canonical detail completing "`{raw}` no es …" for integer options
    /// whose kind carries no explicit policy (unbounded metadata entries).
    const INTEGER_PARSE_DETAIL: &'static str = "un número entero válido";

    /// Canonical positive-integer policy (`min = 1`) with the standard
    /// integer parse-failure message and no verbatim template.
    #[must_use]
    pub const fn positive(below_min_message: &'static str) -> Self {
        Self {
            min: 1,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            parse_failure_template: None,
        }
    }

    /// Policy for fully migrated flags that keep pre-migration legacy
    /// wording: canonical integer detail, an enforced inclusive minimum, and
    /// a verbatim parse-failure template with `{value}` substitution
    /// (#780 byte-exactness outranks uniformity).
    #[must_use]
    pub const fn legacy_verbatim(
        min: u64,
        below_min_message: &'static str,
        parse_failure_template: &'static str,
    ) -> Self {
        Self {
            min,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            parse_failure_template: Some(parse_failure_template),
        }
    }
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
    /// Unsigned integer. `Some(policy)` enforces the policy's inclusive
    /// lower bound with its exact messages; `None` records a metadata-only
    /// entry with NO bound today — parse failures render the canonical
    /// `` `{raw}` no es un número entero válido `` message.
    Uint {
        /// Shared parse + bound policy, absent when unbounded.
        policy: Option<NumericPolicy>,
    },
    /// Memory size with binary-suffix support (`8GB`, `2048MB`, plain bytes).
    /// Suffix parsing lives in `infrastructure::autotuning`; the byte floor,
    /// when one exists, lives here via [`NumericPolicy`].
    MemorySize {
        /// Shared bound policy for externally-parsed byte counts, absent
        /// when unbounded.
        policy: Option<NumericPolicy>,
    },
}

impl ValueKind {
    /// Metadata-only unsigned integer: parsing stays with the external
    /// engine (clap's built-in integer parser) and the spec invents no
    /// bound.
    #[must_use]
    pub const fn uint_unbounded() -> Self {
        Self::Uint { policy: None }
    }

    /// Unsigned integer whose inclusive lower bound is enforced through the
    /// spec ([`OptionSpec::parse_uint`] / [`OptionSpec::check_bound`]).
    #[must_use]
    pub const fn uint(policy: NumericPolicy) -> Self {
        Self::Uint {
            policy: Some(policy),
        }
    }
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
                    .map_err(|_| self.parse_failure_error(raw, policy.as_ref()))?;
                if let Some(policy) = policy {
                    if value < policy.min {
                        return Err(OptionSpecError::Bound(BoxedBoundMessage {
                            message: policy.below_min_message,
                        }));
                    }
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
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => match policy {
                Some(policy) if value < policy.min => {
                    Err(OptionSpecError::Bound(BoxedBoundMessage {
                        message: policy.below_min_message,
                    }))
                },
                // `None` = the spec records no bound: nothing to enforce.
                _ => Ok(value),
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
                self.parse_failure_error(raw, policy.as_ref())
            },
            _ => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail: "un valor válido",
            },
        }
    }

    /// Render the parse-failure error per the policy: verbatim template with
    /// `{value}` substitution when present, default `` `{raw}` no es … ``
    /// shape otherwise. Unbounded entries (`None`) render the canonical
    /// integer message.
    fn parse_failure_error(&self, raw: &str, policy: Option<&NumericPolicy>) -> OptionSpecError {
        let (template, detail) = match policy {
            Some(policy) => (policy.parse_failure_template, policy.parse_failure_detail),
            None => (None, NumericPolicy::INTEGER_PARSE_DETAIL),
        };
        match template {
            Some(template) => OptionSpecError::ParseVerbatim(OwnedParseMessage {
                message: template.replace("{value}", raw),
            }),
            None => OptionSpecError::Parse {
                raw: raw.to_string(),
                detail,
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
                if let Some(policy) = policy {
                    schema.insert("minimum".into(), json!(policy.min));
                }
            },
            ValueKind::MemorySize { policy } => {
                // CLI/env input is textual (suffixes allowed); the byte floor
                // travels alongside until the MCP slice picks a final shape.
                schema.insert("type".into(), json!("string"));
                schema.insert("format".into(), json!("byte-size"));
                if let Some(policy) = policy {
                    schema.insert("minimumBytes".into(), json!(policy.min));
                }
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

/// Export flag group (ADR-002 slice 1): mirrors `cli::args::ExportArgs`.
pub mod export;

/// Crawler flag group (ADR-002 slice 2): mirrors `cli::args::CrawlerArgs`.
pub mod crawler;
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
        // `policy: None` records "no bound today"; the spec must not invent
        // one, and the schema must not advertise a fabricated minimum.
        assert_eq!(crawler::DELAY_MS.parse_uint("0").expect("unbounded"), 0);
        assert_eq!(
            crawler::MAX_DEPTH.parse_uint("255").expect("u8 domain"),
            255
        );
        let schema = crawler::DELAY_MS.json_schema();
        assert_eq!(schema["type"], "integer");
        assert!(
            schema.get("minimum").is_none(),
            "unbounded entries must not fabricate a minimum"
        );
        // Parse failures keep the canonical integer message.
        let err = crawler::DELAY_MS
            .parse_uint("abc")
            .expect_err("must reject text");
        assert_eq!(err.to_string(), "`abc` no es un número entero válido");
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
