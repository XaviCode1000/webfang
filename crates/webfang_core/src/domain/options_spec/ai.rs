//! AI flag group (ADR-002 slice 5a): mirrors `cli::args::AiArgs`
//! field-by-field. Most entries are feature-gated by `ai`; the runtime
//! builder (`cli::spec_command::ai_args`) emits the gated spec args only when
//! the cargo feature is on — matching the pre-migration derive's
//! `#[cfg(feature = "ai")]` behavior of producing zero args when the
//! feature is off (NOT the slice-3 hidden-placeholder pattern used by
//! `clean_ai` / `adaptive_selectors`, which has a different legacy).
//!
//! The UNGATED exceptions are `max_chars` and the `max_tokens` deprecation
//! shim (ADR-0004): both are emitted in every configuration so the migration
//! is visible in `--help` and answers with a migration message instead of
//! "unexpected argument".
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
use super::{DefaultValue, NumericPolicy, OptionSpec, ValueKind};

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

/// `--max-chars <MAX_CHARS>` (env `WEBFANG_MAX_CHARS`, usize).
///
/// Bound enforced through [`OptionSpec::parse_uint`] via
/// `super::args::ai::parse_max_chars`, which
/// `cli::spec_command::numeric_binding` binds as the clap `value_parser`.
///
/// # Why characters and not tokens (ADR-0004)
///
/// The semantic cleaner no longer tokenizes anything: it embeds each chunk
/// through the domain [`EmbeddingPort`](crate::domain::embedding_port::EmbeddingPort),
/// which may be a local ONNX model or a remote HTTP endpoint. A token count
/// would be a number the CLI could not compute for either backend, so the knob
/// counts what every backend can be told about — characters.
///
/// # Why there is NO ceiling
///
/// The retired 32 768-token ceiling was Granite's Max Sequence Length
/// encoded as policy. That number is a property of ONE backend: a remote
/// provider's real limit is its own context window, which it reports per
/// request as HTTP 400/413 and the pipeline degrades on. A cap here would be
/// a claim about every future backend, made by none of them.
///
/// # Why the lower bound is 1
///
/// The guard rejects a chunk whose character count exceeds the budget. With
/// `0` that predicate is true for every non-empty chunk — a guard that
/// rejects everything while looking configured (zero silent loss).
pub const MAX_CHARS: OptionSpec = OptionSpec {
    id: "max_chars",
    value_name: "MAX_CHARS",
    long: "max-chars",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_CHARS"),
    // `DefaultValue::Uint` carries u64 while the config fields carry usize;
    // the widening is lossless on every supported target.
    default: Some(DefaultValue::Uint(DEFAULT_MAX_CHARS as u64)),
    nullable: false,
    description_override: None,
    help: "Maximum characters per chunk before rejection, >= 1 (a chunk-size guard, not a context-window setting; the effective ceiling is the embedding backend's own limit, which a remote endpoint enforces server-side as HTTP 400/413)",
    heading: Some("AI Settings"),
    kind: ValueKind::uint(NumericPolicy::positive(
        "--max-chars debe ser >= 1 (0 rechazaría todos los chunks)",
    )),
    visible_aliases: &[],
    feature_gate: None,
    value_delimiter: None,
};

/// Characters per token: the conversion factor from the retired
/// `--max-tokens` budget to the `--max-chars` budget that replaced it.
///
/// **Single source of truth.** `webfang_core` needs it to translate a legacy
/// `--max-tokens` value at the argv boundary (`cli::args::build_ai_config`),
/// and `webfang_ai`'s `ModelConfig::default()` needs it as the default of its
/// own `chars_per_token` field. `webfang_core` cannot see `webfang_ai`, so the
/// constant lives here and `webfang_ai` reads it — two independent `3.0`
/// literals would be exactly the kind of drift that becomes a silent budget
/// bug (the legacy flag would map to a different budget than the default
/// advertises).
pub const DEFAULT_CHARS_PER_TOKEN: f32 = 3.0;

/// Default chunk budget in characters, derived from Granite's max sequence
/// length (32 768 tokens) times [`DEFAULT_CHARS_PER_TOKEN`].
///
/// Single source of truth for the same reason as [`DEFAULT_CHARS_PER_TOKEN`]:
/// `AiConfig::default` and `webfang_ai`'s `ModelConfig::default` each carry
/// their own `max_chars` default, and two independent `98_304` literals would
/// let the CLI default and the library default drift apart — a silent budget
/// bug that only shows up as "the flag I did not pass rejected my chunk".
pub const DEFAULT_MAX_CHARS: usize = 32_768 * 3;

/// Convert a legacy token budget to the equivalent character budget.
///
/// `round` (not truncation) so the operator's budget is never silently
/// lowered by up to half a character per token, and `f64` (not `f32`) so a
/// large `tokens` does not lose integer precision before scaling. The cast
/// saturates, so a budget near `usize::MAX` cannot wrap.
#[must_use]
pub fn max_tokens_to_max_chars(tokens: usize) -> usize {
    (tokens as f64 * f64::from(DEFAULT_CHARS_PER_TOKEN)).round() as usize
}

/// `--max-tokens <MAX_TOKENS>` (env `WEBFANG_MAX_TOKENS`) — DEPRECATED SHIM.
///
/// ADR-0004's compat policy is TWO PHASED: release N announces the deprecation
/// and the flag keeps WORKING (with a warning), release N+1 removes it. This
/// release is the announcement, so the flag is accepted, parsed by the same
/// OptionsSpec policy machinery every other numeric flag uses, and translated
/// to the equivalent character budget by
/// [`max_tokens_to_max_chars`] — with ONE
/// deprecation warning, not a rejection.
///
/// The hard rejection arrives with the removal, when this entry is deleted.
/// What must NOT happen in between is silence: the flag stays in [`GROUP`] and
/// stays ungated so it renders in `--help` (ADR-0004 forbids a deprecation an
/// operator cannot see), and every use warns.
///
/// # The lower bound is unchanged
///
/// `>= 1`, exactly as before the rename, with the same Spanish message: a
/// budget of `0` rejects every non-empty chunk, so accepting it would be the
/// silent, total denial of the AI path this bound exists to prevent (zero
/// silent loss). The retired 32 768-token CEILING does not come back: it was
/// one backend's Max Sequence Length, and the effective ceiling is now the
/// embedding provider's own context window.
pub const MAX_TOKENS: OptionSpec = OptionSpec {
    id: "max_tokens",
    value_name: "MAX_TOKENS",
    long: "max-tokens",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_TOKENS"),
    default: None,
    nullable: false,
    description_override: None,
    help: "Deprecated: use --max-chars instead. Still accepted for now — the value is converted to characters (tokens x 3.0) and ignored when --max-chars is given",
    heading: Some("AI Settings"),
    kind: ValueKind::uint(NumericPolicy::positive(
        "--max-tokens debe ser >= 1 (0 rechazaría todos los chunks)",
    )),
    visible_aliases: &[],
    feature_gate: None,
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

/// `--ai-model <AI_MODEL>` (env `WEBFANG_AI_MODEL_ID`, `Option<String>`).
///
/// Raw string on purpose (#827): validation is deferred to the AI init
/// path (`build_ai_cleaner`) so a poisoned `AI_MODEL_ID` env var cannot
/// make unrelated CLI invocations fail at parse time. `AI_MODEL_ID` is
/// accepted as a hidden CLI alias for backward compatibility, deprecated
/// for removal in v3.0 (#1587).
pub const AI_MODEL: OptionSpec = OptionSpec {
    id: "ai_model",
    value_name: "AI_MODEL",
    long: "ai-model",
    short: None,
    aliases: &["AI_MODEL_ID"],
    env: Some("WEBFANG_AI_MODEL_ID"),
    default: None,
    nullable: false,
    description_override: None,
    help: "AI model to use: granite-97m (default, fast) or granite-311m (higher quality). Env WEBFANG_AI_MODEL_ID wins; legacy AI_MODEL_ID still accepted but deprecated for removal in v3.0",
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
///
/// [`MAX_CHARS`] and [`MAX_TOKENS`] are UNGATED on purpose — see
/// [`MAX_TOKENS`] — so `cli::spec_command::ai_args` emits them in every
/// cargo configuration. Adding a third ungated entry to this group is a
/// deliberate decision, not a default: `webfang_mcp`'s parity test pins the
/// ungated set by name.
pub const GROUP: &[OptionSpec] = &[THRESHOLD, MAX_CHARS, MAX_TOKENS, OFFLINE, AI_MODEL];
