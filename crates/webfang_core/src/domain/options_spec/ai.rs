//! AI flag group (ADR-002 slice 5a): mirrors `cli::args::AiArgs`
//! field-by-field. All entries are feature-gated by `ai`; the runtime
//! builder (`cli::spec_command::ai_args`) emits the spec args only when
//! the cargo feature is on — matching the pre-migration derive's
//! `#[cfg(feature = "ai")]` behavior of producing zero args when the
//! feature is off (NOT the slice-3 hidden-placeholder pattern used by
//! `clean_ai` / `adaptive_selectors`, which has a different legacy).
//!
//! `threshold` is HONESTLY DEFERRED to a hand-built `clap::Arg` in
//! `spec_command::ai_args` because its parser rejects out-of-range `f32`
//! values (range 0.0..=1.0, error message "fuera de rango (rango válido:
//! 0.0 a 1.0)"). The spec SSOT does not carry a `ValueKind::Float` yet;
//! modeling it would need both a new kind AND `{value}` substitution
//! inside `below_min_message`, breaking the existing `Uint` policy
//! contract. The spec entry below records the identity (id/long/env/
//! default/help/heading/feature_gate) and the defer reason; the bound
//! and parser live in `cli::args::ai::parse_threshold`, the binding in
//! `cli::spec_command::ai_args`'s `AiSlot::Manual` arm.
use super::{DefaultValue, OptionSpec, ValueKind};

/// `--threshold <THRESHOLD>` (env `WEBFANG_THRESHOLD`, f32 0.0..=1.0).
///
/// HONEST DEFER (see module docs): parser + range + error messages live
/// in `cli::args::ai::parse_threshold`. The `ValueKind::Text` placeholder
/// here only records that the spec does not currently model f32 parsing;
/// the entry is intentionally NOT routed through `build_arg` — the
/// `ai_args` builder uses its dedicated `AiSlot::Manual` slot so the
/// custom parser, `allow_negative_numbers = true`, and the verbatim
/// Spanish range message all stay intact.
pub const THRESHOLD: OptionSpec = OptionSpec {
    id: "threshold",
    value_name: "THRESHOLD",
    long: "threshold",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_THRESHOLD"),
    default: Some(DefaultValue::Str("0.3")),
    help: "Relevance threshold for AI semantic filtering (0.0-1.0)",
    heading: Some("AI Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: Some("ai"),
    value_delimiter: None,
};

/// `--max-tokens <MAX_TOKENS>` (env `WEBFANG_MAX_TOKENS`, usize).
pub const MAX_TOKENS: OptionSpec = OptionSpec {
    id: "max_tokens",
    value_name: "MAX_TOKENS",
    long: "max-tokens",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_TOKENS"),
    default: Some(DefaultValue::Uint(32768)),
    nullable: false,
    description_override: None,
    help: "Maximum tokens per chunk before rejection (a chunk-size guard, not a context-window setting; chunks exceeding this fail)",
    heading: Some("AI Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: Some("ai"),
    value_delimiter: None,
};

/// `--offline` (env `WEBFANG_OFFLINE`, bool SetTrue).
pub const OFFLINE: OptionSpec = OptionSpec {
    id: "offline",
    value_name: "OFFLINE",
    long: "offline",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OFFLINE"),
    default: Some(DefaultValue::Bool(false)),
    nullable: false,
    description_override: None,
    help: "Run AI model in offline mode",
    heading: Some("AI Settings"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: Some("ai"),
    value_delimiter: None,
};

/// `--ai-model <AI_MODEL>` (env `AI_MODEL_ID`, `Option<String>`).
///
/// Raw string on purpose (#827): validation is deferred to the AI init
/// path (`build_ai_cleaner`) so a poisoned `AI_MODEL_ID` env var cannot
/// make unrelated CLI invocations fail at parse time. The env var name
/// is intentionally NOT `WEBFANG_AI_MODEL` — the rename is sub-slice 5b.
pub const AI_MODEL: OptionSpec = OptionSpec {
    id: "ai_model",
    value_name: "AI_MODEL",
    long: "ai-model",
    short: None,
    aliases: &[],
    env: Some("AI_MODEL_ID"),
    default: None,
    nullable: false,
    description_override: None,
    help: "AI model to use: granite-97m (default, fast) or granite-311m (higher quality)",
    heading: Some("AI Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: Some("ai"),
    value_delimiter: None,
};

/// All AI-group options, in `AiArgs` field-declaration order. The
/// spec entry for `threshold` is included for parity-table completeness
/// (so the equivalence test can iterate `GROUP`); the runtime builder
/// substitutes it with a hand-built arg carrying the `parse_threshold`
/// validator.
pub const GROUP: &[OptionSpec] = &[THRESHOLD, MAX_TOKENS, OFFLINE, AI_MODEL];
