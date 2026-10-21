//! AI flag group (ADR-002 slice 5a): mirrors `cli::args::AiArgs`
//! field-by-field. **No entry in this group is feature-gated** (ADR-0004,
//! slice C): the semantic cleaner no longer needs ONNX to exist, so every
//! AI flag describes behavior that needs no ONNX build — a remote embedding
//! endpoint, or nothing at all when the flag is simply off.
//!
//! A flag that renders only under the feature it configures is a flag that
//! answers "unexpected argument" (exit 64) on the other build, naming
//! neither the capability nor how to get it. So `--clean-ai`,
//! `--threshold` and `--offline` render in EVERY cargo
//! configuration. Post-ADR-0004 paso 6 there is no local-ONNX build any
//! more: a requested `--clean-ai` whose cleaner wiring failed surfaces at
//! run time as the export flow's Spanish ConfigError naming
//! `--embedding-provider` (exit 78).
//!
//! `threshold` is HONESTLY DEFERRED to a hand-built `clap::Arg` in
//! `spec_command::ai_args` because its parser rejects out-of-range `f32`
//! values (range 0.0..=1.0, error message "fuera de rango (rango válido:
//! 0.0 a 1.0)"). The spec SSOT does not carry a `ValueKind::Float` yet;
//! modeling it would need both a new kind AND `{value}` substitution
//! inside `below_min_message`, breaking the existing `Uint` policy
//! contract. The spec entry below records the identity (id/long/env/
//! default/help/heading) and the defer reason; the bound
//! and parser live in `cli::args::ai::parse_threshold`, the binding in
//! `cli::spec_command::ai_args`'s `AiSlot::Manual` arm.
use super::{DefaultValue, NumericPolicy, OptionSpec, ValueKind};

/// The shipped default relevance threshold.
///
/// **Single source of truth for the NUMBER**, the same role
/// [`DEFAULT_CHARS_PER_TOKEN`] and [`DEFAULT_MAX_CHARS`] play for the rest of
/// this group. Two independent `0.3` literals would let the warning ADR-0004
/// requires ("this default is calibrated for Granite and nothing else is")
/// name a different number than the one the pipeline actually runs with — the
/// one failure mode a qualification warning cannot survive.
///
/// # Why the spec entry still spells `"0.3"` as a string
///
/// [`THRESHOLD`].default is a [`DefaultValue::Str`] because the spec models
/// textual defaults as `&'static str` (there is no `ValueKind::Float` yet —
/// see the module docs), and `format!` is not available in a `const`
/// initializer, so the rendered `default_value` cannot be derived from an
/// `f32` constant. The two spellings are therefore pinned against each other
/// by `the_announced_default_threshold_is_the_number_the_runtime_actually_defaults_to`
/// (behavioral) and `threshold_default_matches_the_numeric_constant` (unit) —
/// neither can drift alone.
pub const DEFAULT_THRESHOLD: f32 = 0.3;

/// `--threshold <THRESHOLD>` (env `WEBFANG_THRESHOLD`, f32 0.0..=1.0).
///
/// HONEST DEFER (see module docs): parser + range + error messages live
/// in `cli::args::ai::parse_threshold`. The `ValueKind::Text` placeholder
/// here only records that the spec does not currently model f32 parsing;
/// the entry is intentionally NOT routed through `build_arg` — the
/// `ai_args` builder uses its dedicated `AiSlot::Manual` slot so the
/// custom parser, `allow_negative_numbers = true`, and the verbatim
/// Spanish range message all stay intact.
///
/// UNGATED: the relevance filter it configures is the cleaner's own logic
/// and computes over vectors from whatever `EmbeddingPort` the run built.
///
/// # The default is orphaned (ADR-0004, `Umbral por defecto huérfano`)
///
/// [`DEFAULT_THRESHOLD`] is calibrated for Granite — the local ONNX model
/// that this ADR removes. Any remote endpoint retains differently under the
/// same number, so the CLI warns while the threshold is still the default and
/// no model is calibrated (see
/// [`crate::cli::args::ai::orphan_threshold_notice`]).
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
    feature_gate: None,
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

/// Characters per token: the ratio between a model's token budget and the
/// character budget derived from it.
///
/// **Single source of truth.** The domain's model profiles need it as the
/// default of their own `chars_per_token` field, and the chunk-size guard's
/// default (`DEFAULT_MAX_CHARS`) is derived from it. Two independent `3.0`
/// literals would be exactly the kind of drift that becomes a silent budget
/// bug (a profile would advertise a different budget than the guard uses,
/// and neither number would say so).
pub const DEFAULT_CHARS_PER_TOKEN: f32 = 3.0;

/// Default chunk budget in characters, derived from Granite's max sequence
/// length (32 768 tokens) times [`DEFAULT_CHARS_PER_TOKEN`].
///
/// Single source of truth for the same reason as [`DEFAULT_CHARS_PER_TOKEN`]:
/// `AiConfig::default` and the cleaner implementation each carry their own
/// `max_chars` default, and two independent `98_304` literals would let the
/// CLI default and the library default drift apart — a silent budget bug that
/// only shows up as "the flag I did not pass rejected my chunk".
pub const DEFAULT_MAX_CHARS: usize = 32_768 * 3;

/// `--offline` (env `WEBFANG_OFFLINE`, bool SetTrue).
///
/// UNGATED by decision (ADR-0004, slice C), but its effect is NOT local-ONNX:
/// since ADR-0004 step 6 there is no local engine and no hf_hub resolver. The
/// flag is read by the embeddings preflight, which rejects EVERY resolved
/// embeddings provider — remote or loopback — with exit 78 (#2166). It gates
/// the semantic-cleaning providers only; it does not restrict the crawler's
/// network.
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
    help: "Reject every embeddings provider (remote or loopback) with exit 78: no local model since 3.0.0",
    heading: Some("AI Settings"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
    value_delimiter: None,
};

/// All AI-group options, in `AiArgs` field-declaration order. The
/// spec entry for `threshold` is included for parity-table completeness
/// (so the equivalence test can iterate `GROUP`); the runtime builder
/// substitutes it with a hand-built arg carrying the `parse_threshold`
/// validator.
///
/// **No member of this group is feature-gated** (ADR-0004, slice C) — see
/// the module docs. `webfang_mcp`'s parity test pins that by asserting the
/// ungated set equals the WHOLE group, so a future re-gating of any entry
/// fails there rather than shipping a flag that vanishes on one build.
pub const GROUP: &[OptionSpec] = &[THRESHOLD, MAX_CHARS, OFFLINE];

#[cfg(test)]
mod tests {
    //! The two spellings of the default threshold — the numeric SSOT
    //! ([`DEFAULT_THRESHOLD`]) and the `&'static str` the spec renders as
    //! clap's `default_value` — cannot reference each other (`format!` is not
    //! `const`). This test is the bridge: if either moves alone, it fails.

    use super::{DEFAULT_THRESHOLD, THRESHOLD};

    #[test]
    fn threshold_default_matches_the_numeric_constant() {
        let rendered = THRESHOLD
            .default
            .as_ref()
            .map(ToString::to_string)
            .expect("the threshold entry must declare a default");
        assert_eq!(
            rendered,
            DEFAULT_THRESHOLD.to_string(),
            "the spec's rendered default_value and DEFAULT_THRESHOLD must agree: \
             a warning that names one number while the CLI defaults to another \
             is a lie about the value it exists to qualify"
        );
    }
}
