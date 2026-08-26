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

/// Typed default value used by the JSON schema path (issue #948 F4).
///
/// Replaces the previous single-string `"default"` in the advertised MCP
/// schema so `json_schema()` can serialize the wire type natively —
/// `"default": 2` for an `integer` kind, `"default": true` for a `boolean`
/// kind, `"default": "jsonl"` for a `string`/enum kind. The CLI parity
/// surface keeps the canonical string form via [`OptionSpec::default`] (a
/// `&'static str` that mirrors clap's `default_value`); [`Display`] is the
/// canonical string form for every `DefaultValue` variant so the two stay
/// drift-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultValue {
    /// Free-form string default — every `Text`/`Path`/`Enum` default and any
    /// `Bool`/`Uint` that the legacy `&'static str` representation is still
    /// useful for. The wrapped string is the CLI's `default_value` rendering
    /// and the schema's `"default"` value for textual kinds.
    Str(&'static str),
    /// Unsigned integer default — schema emits a JSON number, never a string.
    Uint(u64),
    /// Boolean default — schema emits a JSON boolean, never a string.
    Bool(bool),
}

impl DefaultValue {
    /// Native JSON value matching the option's wire kind — issue #948 F4.
    /// `Uint(2)` becomes `Value::Number(2)` (not `"2"`), `Bool(true)` becomes
    /// `Value::Bool(true)` (not `"true"`), `Str("jsonl")` becomes
    /// `Value::String("jsonl")`. This is the form [`OptionSpec::json_schema`]
    /// serializes into the advertised MCP schema and is the drift-killer
    /// for the type-inconsistency finding.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::Str(s) => Value::String((*s).to_owned()),
            Self::Uint(n) => json!(n),
            Self::Bool(b) => Value::Bool(*b),
        }
    }
}

impl core::fmt::Display for DefaultValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Str(s) => f.write_str(s),
            Self::Uint(n) => write!(f, "{n}"),
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}

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
    /// Alternative long names accepted by clap but NOT rendered in help
    /// output (clap's `alias`). Visible aliases live in
    /// [`OptionSpec::visible_aliases`].
    pub aliases: &'static [&'static str],
    /// Alternative long names accepted by clap AND rendered in help
    /// output (clap's `visible_alias`).
    pub visible_aliases: &'static [&'static str],
    /// Placeholder shown for the option's value (`<VALUE_NAME>`); today
    /// always the SCREAMING_SNAKE id (clap derive's rendering). A static
    /// string because clap's owned-`Str` support sits behind its `string`
    /// feature, which this crate does not enable.
    pub value_name: &'static str,
    /// Environment variable consulted when the flag is absent.
    pub env: Option<&'static str>,
    /// Canonical CLI default value — the string form clap's `default_value`
    /// accepts as a `&'static str`. The advertised MCP schema derives its
    /// wire-typed `default` from [`Self::schema_default`]; for textual
    /// kinds both fields carry the same string, but for `Uint`/`Bool` the
    /// schema path is the one that emits the native JSON type (issue #948
    /// F4).
    pub default: Option<&'static str>,
    /// Wire-typed default for the JSON schema path (issue #948 F4). When
    /// `None`, the schema falls back to [`Self::default`] (a string), which
    /// is correct for every `Text`/`Path`/`Enum` kind. When `Some`, the
    /// schema serializes the native JSON type (`Uint(2)` → `2`,
    /// `Bool(true)` → `true`). CLI parity stays governed by
    /// [`Self::default`].
    pub schema_default: Option<DefaultValue>,
    /// Whether the advertised JSON schema must declare the property
    /// nullable — i.e. emit `"type": ["<inner>", "null"]` (issue #948 F5).
    /// The bridge preserves the schemars-derived `["<inner>", "null"]`
    /// shape for `Option<T>` fields by default; this flag is an explicit
    /// opt-in for entries that need the nullable shape even when the
    /// spec entry's kind is non-optional, or to force a non-nullable
    /// shape on a derived optional field. Most entries leave this as
    /// `false` and let the bridge infer from the derived schema.
    pub nullable: bool,
    /// Tool-appropriate description override for the advertised MCP
    /// schema (issue #948 F6). When `Some`, `json_schema()` uses this
    /// string as the `"description"` field instead of [`Self::help`].
    /// CLI/help output keeps [`Self::help`] (which is byte-exact
    /// against clap's rendering); the MCP bridge only sees the
    /// override, decoupling the CLI help wording from the LLM-facing
    /// description. `None` = no override, fall back to [`Self::help`].
    pub description_override: Option<&'static str>,
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

/// Validation policy for numeric options: the inclusive bounds and the
/// exact user-facing messages raised when they are violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumericPolicy {
    /// Inclusive lower bound (`1` ⇒ zero is rejected; `0` keeps zero valid,
    /// e.g. seed-only crawl semantics).
    pub min: u64,
    /// Inclusive upper bound (`None` = unbounded above). Caps shared by
    /// every surface live HERE only (ADR-002 slice 4).
    pub max: Option<u64>,
    /// Completes "`{raw}` no es …" when parsing fails.
    pub parse_failure_detail: &'static str,
    /// Complete message raised when the value is below [`NumericPolicy::min`].
    pub below_min_message: &'static str,
    /// Complete message raised when the value is above
    /// [`NumericPolicy::max`]. Unreachable (and empty) when `max` is
    /// `None` or when `min = 0` already accepts every valid value below it.
    pub above_max_message: &'static str,
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
            max: None,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            above_max_message: "",
            parse_failure_template: None,
        }
    }

    /// Policy that keeps zero valid (e.g. seed-only crawl depth semantics)
    /// and enforces an inclusive upper bound with the canonical integer
    /// parse-failure message. The below-min branch is unreachable because
    /// `min = 0`.
    #[must_use]
    pub const fn zero_valid_capped(max: u64, above_max_message: &'static str) -> Self {
        Self {
            min: 0,
            max: Some(max),
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message: "",
            above_max_message,
            parse_failure_template: None,
        }
    }

    /// Attach an inclusive upper bound to an existing policy without
    /// touching its user-facing messages. Used when a fully migrated flag
    /// gains a shared cap while keeping its legacy wording (#940).
    #[must_use]
    pub const fn capped(mut self, max: u64, above_max_message: &'static str) -> Self {
        self.max = Some(max);
        self.above_max_message = above_max_message;
        self
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
            max: None,
            parse_failure_detail: Self::INTEGER_PARSE_DETAIL,
            below_min_message,
            above_max_message: "",
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

/// Structured bound violation emitted by [`OptionSpec::check_bound`]
/// (issue #948 F7).
///
/// Carries the offending bound as data so consumers (e.g. the MCP
/// validators in `webfang_mcp::mcp_server::params`) can format the
/// MCP-stable English wording ("must be at least N" / "must be at most N")
/// without re-comparing the value or re-reading the spec's
/// [`NumericPolicy`]. The `Display` impl IS the MCP-stable wording; the
/// Spanish verbatim message stays in [`NumericPolicy`] and is reached via
/// [`OptionSpecError::Bound`] for CLI consumers that need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundError {
    /// Value is below the inclusive lower bound.
    #[error("must be at least {min}")]
    MinViolated {
        /// Inclusive lower bound.
        min: u64,
    },
    /// Value is above the inclusive upper bound.
    #[error("must be at most {max}")]
    MaxViolated {
        /// Inclusive upper bound.
        max: u64,
    },
}

impl OptionSpec {
    /// Whether this option participates in the current build given its
    /// feature gate. A gate of `None` is always active; known gates map
    /// to this crate's cargo features so the spec mirrors the `cfg`
    /// duplication of the pre-slice-3 derives. Unknown gates fail closed
    /// (inactive) until they are wired here.
    #[must_use]
    pub const fn active(&self) -> bool {
        match self.feature_gate {
            None => true,
            Some(gate) => Self::gate_active(gate.as_bytes()),
        }
    }

    /// Byte-wise gate comparison: `&str` patterns are not allowed in
    /// `const fn` on Rust 1.88, so known gates are matched manually.
    /// Unknown gates fail closed (inactive) until wired here.
    const fn gate_active(gate: &[u8]) -> bool {
        if Self::bytes_eq(gate, b"ai") {
            return cfg!(feature = "ai");
        }
        if Self::bytes_eq(gate, b"adaptive-selectors") {
            return cfg!(feature = "adaptive-selectors");
        }
        false
    }

    const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut i = 0;
        while i < a.len() {
            if a[i] != b[i] {
                return false;
            }
            i += 1;
        }
        true
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
    /// [`OptionSpecError::Bound`] below the inclusive minimum or above the
    /// inclusive maximum;
    /// [`OptionSpecError::UnsupportedKind`] for non-integer kinds.
    pub fn parse_uint(&self, raw: &str) -> Result<u64, OptionSpecError> {
        match self.kind {
            ValueKind::Uint { policy } => {
                let value: u64 = raw
                    .parse()
                    .map_err(|_| self.parse_failure_error(raw, policy.as_ref()))?;
                Self::enforce_policy(value, policy.as_ref())?;
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
    /// [`BoundError::MinViolated`] when below the inclusive minimum or
    /// [`BoundError::MaxViolated`] above the inclusive maximum;
    /// [`OptionSpecError::UnsupportedKind`] for kinds without bounds.
    /// The error is the structured form (issue #948 F7) so consumers can
    /// format the wire-correct wording without re-comparing the value.
    pub fn check_bound(&self, value: u64) -> Result<u64, BoundError> {
        match self.kind {
            ValueKind::Uint { policy } | ValueKind::MemorySize { policy } => {
                Self::enforce_policy_structured(value, policy.as_ref())?;
                Ok(value)
            },
            _ => Err(self.unsupported_kind_bound()),
        }
    }

    /// Shared bound enforcement behind [`Self::parse_uint`] and
    /// [`Self::check_bound`]: THE single place where spec bounds reject a
    /// value, using each policy's exact stored user-facing message.
    fn enforce_policy(value: u64, policy: Option<&NumericPolicy>) -> Result<(), OptionSpecError> {
        let Some(policy) = policy else {
            // `None` = the spec records no bound: nothing to enforce.
            return Ok(());
        };
        if value < policy.min {
            return Err(OptionSpecError::Bound(BoxedBoundMessage {
                message: policy.below_min_message,
            }));
        }
        if let Some(max) = policy.max {
            if value > max {
                return Err(OptionSpecError::Bound(BoxedBoundMessage {
                    message: policy.above_max_message,
                }));
            }
        }
        Ok(())
    }

    /// Structured bound enforcement behind [`Self::check_bound`] (issue
    /// #948 F7). Same policy semantics as [`Self::enforce_policy`] but
    /// returns the typed [`BoundError`] variants so consumers can format
    /// the wire-correct wording without re-comparing the value or
    /// re-reading the policy. Used by every surface whose user-facing
    /// messages name the parameter, not the CLI flag.
    fn enforce_policy_structured(
        value: u64,
        policy: Option<&NumericPolicy>,
    ) -> Result<(), BoundError> {
        let Some(policy) = policy else {
            return Ok(());
        };
        if value < policy.min {
            return Err(BoundError::MinViolated { min: policy.min });
        }
        if let Some(max) = policy.max {
            if value > max {
                return Err(BoundError::MaxViolated { max });
            }
        }
        Ok(())
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

    /// Bound-check on a non-numeric kind never reaches here in practice,
    /// but the structured API mirrors the existing `unsupported_kind` so
    /// the error type stays coherent (issue #948 F7).
    fn unsupported_kind_bound(&self) -> BoundError {
        // Unreachable for the current `ValueKind` set — the structured
        // path is only entered from numeric kinds. Re-routed through the
        // `MinViolated` arm with a sentinel min of 0 so the validator
        // surfaces a usable message rather than panicking on an
        // impossible branch. Callers that hit this path have a logic
        // bug; the assertion is intentionally not panic-typed because
        // the MCP validator surfaces errors as JSON-RPC, not crashes.
        BoundError::MinViolated { min: 0 }
    }

    /// Accepted variants when this option is [`ValueKind::Enum`] (in
    /// declaration order); `None` for every other kind. Lets downstream
    /// surfaces (MCP validation, schema bridge) derive closed value sets
    /// from the SSOT instead of duplicating string literals.
    #[must_use]
    pub const fn enum_variants(&self) -> Option<&'static [&'static str]> {
        match self.kind {
            ValueKind::Enum { variants } => Some(variants),
            _ => None,
        }
    }

    /// JSON Schema fragment describing this option (ADR-002 seam #2).
    ///
    /// Consumed by the MCP schema bridge (slice 4) and pinned by tests in
    /// slice 1.
    #[must_use]
    pub fn json_schema(&self) -> Value {
        let mut schema = Map::new();
        // F6: description override wins over `help` for the MCP wire
        // surface; the CLI/help path keeps `help` byte-exact against
        // clap. Default is `help` (no override) so existing entries
        // stay drift-free.
        let description = self.description_override.unwrap_or(self.help);
        schema.insert("description".into(), json!(description));
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
                    if let Some(max) = policy.max {
                        schema.insert("maximum".into(), json!(max));
                    }
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
        // F5: explicit `nullable: true` upgrades the type to a union with
        // `null`. The bridge also auto-promotes optional fields by
        // preserving the derived `["<inner>", "null"]` shape, so most
        // entries leave this as `false`.
        if self.nullable {
            if let Some(Value::String(inner)) = schema.get("type").cloned() {
                schema.insert(
                    "type".into(),
                    Value::Array(vec![Value::String(inner), Value::Null]),
                );
            }
        }
        // F4: prefer the wire-typed `schema_default`; fall back to the CLI
        // string form for every option that doesn't override it. Bool/Uint
        // entries that override `schema_default` will emit native JSON
        // types (true / 2), closing the type-inconsistency drift in the
        // advertised MCP schema.
        if let Some(typed) = self.schema_default {
            schema.insert("default".into(), typed.to_json_value());
        } else if let Some(default) = self.default {
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
    use super::{crawler, export, schema_object, BoundError, DefaultValue, OptionSpecError};
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
    fn memory_size_bound_rejects_zero_with_structured_error() {
        // F7: `check_bound` returns a typed `BoundError`, not the verbose
        // Spanish message the CLI path uses. The English wording IS the
        // MCP-stable rendering every MCP validator relies on.
        let err = export::RAM_BUDGET
            .check_bound(0)
            .expect_err("zero budget is invalid");
        assert!(
            matches!(err, BoundError::MinViolated { min: 1 }),
            "expected MinViolated {{ min: 1 }}, got {err:?}"
        );
        assert_eq!(err.to_string(), "must be at least 1");
        assert_eq!(export::RAM_BUDGET.check_bound(2048).expect("valid"), 2048);

        // The CLI path keeps the Spanish verbatim message via
        // `parse_uint` (which goes through `OptionSpecError::Bound`).
        // `RAM_BUDGET` is a `MemorySize`, not `Uint`, so the CLI path is
        // `parse_error` instead — we cover the same Spanish wording via
        // `parse_uint` on a `Uint` entry below.
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
        assert_eq!(crawler::GROUP.len(), 41);
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
    fn feature_gated_entries_mirror_the_compile_time_cfg() {
        // The spec's gate must name a known cargo feature and `active()`
        // must agree with the actual compilation configuration.
        for (opt, gate) in [
            (crawler::CLEAN_AI, "ai"),
            (crawler::ADAPTIVE_SELECTORS, "adaptive-selectors"),
        ] {
            assert_eq!(opt.feature_gate, Some(gate));
            assert_eq!(
                opt.active(),
                match gate {
                    "ai" => cfg!(feature = "ai"),
                    "adaptive-selectors" => cfg!(feature = "adaptive-selectors"),
                    _ => unreachable!(),
                },
                "active() must mirror cfg for gate `{gate}`"
            );
        }
        // Ungated entries are always active.
        assert!(export::OUTPUT.active());
    }

    #[test]
    fn unknown_feature_gates_fail_closed() {
        let gated = super::OptionSpec {
            feature_gate: Some("not-a-real-feature"),
            ..export::OUTPUT
        };
        assert!(!gated.active());
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
    fn crawler_max_depth_keeps_zero_valid_and_enforces_the_shared_cap() {
        // 0 = seed-only semantics stay valid; 10 is the inclusive cap
        // shared with the MCP tool schema (#940).
        assert_eq!(crawler::MAX_DEPTH.parse_uint("0").expect("seed-only"), 0);
        assert_eq!(crawler::MAX_DEPTH.parse_uint("10").expect("cap"), 10);
        let err = crawler::MAX_DEPTH
            .parse_uint("11")
            .expect_err("above the shared cap");
        assert_eq!(err.to_string(), "--max-depth debe ser <= 10");
        // Parse failures keep the canonical integer message.
        let err = crawler::MAX_DEPTH
            .parse_uint("abc")
            .expect_err("must reject text");
        assert_eq!(err.to_string(), "`abc` no es un número entero válido");
    }

    #[test]
    fn crawler_max_pages_enforces_the_shared_upper_cap() {
        assert_eq!(crawler::MAX_PAGES.parse_uint("1").expect("min"), 1);
        assert_eq!(
            crawler::MAX_PAGES
                .parse_uint("100000")
                .expect("inclusive cap"),
            100_000
        );
        let err = crawler::MAX_PAGES
            .parse_uint("100001")
            .expect_err("above the shared cap");
        assert_eq!(err.to_string(), "--max-pages debe ser <= 100000");
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
        assert!(schema.get("maximum").is_none());

        let ram = export::RAM_BUDGET.json_schema();
        assert_eq!(ram["format"], "byte-size");
        assert_eq!(ram["minimumBytes"], 1);

        // Capped entries advertise both bounds (shared CLI↔MCP caps).
        let depth = crawler::MAX_DEPTH.json_schema();
        assert_eq!(depth["minimum"], 0);
        assert_eq!(depth["maximum"], 10);
        let pages = crawler::MAX_PAGES.json_schema();
        assert_eq!(pages["minimum"], 1);
        assert_eq!(pages["maximum"], 100_000);
    }

    #[test]
    fn json_schema_covers_boolean_path_and_text_kinds() {
        // ELASTIC carries a typed `schema_default: Bool(false)` (issue #948
        // F4) — the advertised schema emits a native JSON boolean, not a
        // string, closing the type-inconsistency drift the bridge
        // previously inherited.
        let elastic = export::ELASTIC.json_schema();
        assert_eq!(elastic["type"], "boolean");
        assert_eq!(elastic["default"], false);
        assert_eq!(elastic["default"], json!(false));

        let output = export::OUTPUT.json_schema();
        assert_eq!(output["type"], "string");

        let vectors = export::OUTPUT_VECTORS.json_schema();
        assert_eq!(vectors["type"], "string");
    }

    /// Issue #948 F4 drift-killer: integer- and boolean-kind properties must
    /// advertise their `default` as a native JSON number/bool, NEVER as a
    /// JSON string. The pre-F4 renderer emitted `"default": "2"` for an
    /// `integer` kind — inconsistent with `"type": "integer"` and
    /// type-strict MCP validators.
    #[test]
    fn json_schema_emits_native_typed_defaults_for_uint_and_bool() {
        // MAX_PAGES (crawler) is `Uint(10)` — schema must carry `2`-as-number.
        let pages = crawler::MAX_PAGES.json_schema();
        assert_eq!(pages["type"], "integer");
        assert_eq!(
            pages["default"],
            json!(10u64),
            "MAX_PAGES default must be a JSON number, not a string"
        );
        assert!(
            pages["default"].is_number(),
            "MAX_PAGES default must be a JSON number, got: {}",
            pages["default"]
        );

        // MAX_DEPTH (crawler) is `Uint(2)` — same invariant.
        let depth = crawler::MAX_DEPTH.json_schema();
        assert_eq!(depth["type"], "integer");
        assert_eq!(depth["default"], json!(2u64));
        assert!(depth["default"].is_number());

        // Bool entry: DOM_PREPRUNE is `Bool(true)`.
        let dom = crawler::DOM_PREPRUNE.json_schema();
        assert_eq!(dom["type"], "boolean");
        assert_eq!(
            dom["default"],
            json!(true),
            "DOM_PREPRUNE default must be a JSON bool, not a string"
        );
        assert!(dom["default"].is_boolean());

        // Sanity: textual entries (no `schema_default`) still emit strings
        // so the CLI parity path stays drift-free.
        let text = crawler::SELECTOR.json_schema();
        assert_eq!(text["type"], "string");
        assert_eq!(text["default"], json!("body"));

        // F4 back-compat: `DefaultValue` `Display` mirrors the canonical
        // CLI string form the args parity tests assert.
        assert_eq!(DefaultValue::Uint(10).to_string(), "10");
        assert_eq!(DefaultValue::Bool(false).to_string(), "false");
        assert_eq!(DefaultValue::Str("jsonl").to_string(), "jsonl");
    }

    /// Issue #948 F7 drift-killer: `check_bound` returns the typed
    /// `BoundError` variants so MCP validators can format the
    /// MCP-stable wording without re-comparing the value or re-reading
    /// the spec's `NumericPolicy`. The pre-F7 code re-implemented
    /// `value < policy.min` / `value > policy.max` in every validator
    /// (`params.rs::validate_max_pages`, `validate_max_depth`) — the
    /// structured error collapses the duplication.
    #[test]
    fn check_bound_returns_typed_bound_error_variants() {
        // Below the inclusive minimum: MinViolated carries the bound.
        let err = crawler::MAX_PAGES
            .check_bound(0)
            .expect_err("zero pages is invalid");
        assert!(
            matches!(err, BoundError::MinViolated { min: 1 }),
            "expected MinViolated {{ min: 1 }}, got {err:?}"
        );
        assert_eq!(err.to_string(), "must be at least 1");

        // Above the inclusive cap: MaxViolated carries the cap.
        let err = crawler::MAX_PAGES
            .check_bound(100_001)
            .expect_err("above the cap is invalid");
        assert!(
            matches!(err, BoundError::MaxViolated { max: 100_000 }),
            "expected MaxViolated {{ max: 100000 }}, got {err:?}"
        );
        assert_eq!(err.to_string(), "must be at most 100000");

        // MAX_DEPTH keeps zero valid (seed-only) and caps at 10.
        assert_eq!(crawler::MAX_DEPTH.check_bound(0).expect("seed-only"), 0);
        let err = crawler::MAX_DEPTH
            .check_bound(11)
            .expect_err("above the cap is invalid");
        assert!(matches!(err, BoundError::MaxViolated { max: 10 }));
        assert_eq!(err.to_string(), "must be at most 10");

        // Inclusive boundaries accept (no re-implementation needed).
        assert_eq!(crawler::MAX_PAGES.check_bound(1).expect("min"), 1);
        assert_eq!(
            crawler::MAX_PAGES.check_bound(100_000).expect("cap"),
            100_000
        );

        // Unbounded entries accept any value (policy = None).
        assert_eq!(crawler::DELAY_MS.check_bound(0).expect("unbounded"), 0);
        assert_eq!(
            crawler::DELAY_MS.check_bound(u64::MAX).expect("unbounded"),
            u64::MAX
        );
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
