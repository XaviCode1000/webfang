//! AI-powered semantic cleaning arguments (ADR-002 slice 5a).
//!
//! Parsing stays derive-driven (`FromArgMatches`); command assembly is
//! spec-built (ADR-002 slice 3); see `cli::spec_command`.

/// Validate `--threshold`: must parse as `f32` in `0.0..=1.0`. The
/// out-of-range check is a hard requirement (#759) — a `> 1.0` threshold
/// would silently swallow every chunk, defeating the relevance filter.
/// Parsing stays in production code (NOT the OptionsSpec SSOT) because
/// the spec does not yet model `f32` ranges; the value parser is bound
/// by the hand-built `manual_threshold` slot in `cli::spec_command::ai_args`.
///
/// NOT feature-gated: the flag is ungated by decision (ADR-0004, slice C), and
/// the parser is pure `f32` range logic with no ONNX dependency, so every
/// build both parses and validates it identically.
pub(crate) fn parse_threshold(s: &str) -> Result<f32, String> {
    let val: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido"))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!(
            "'{s}' está fuera de rango (rango válido: 0.0 a 1.0)"
        ));
    }
    Ok(val)
}

/// Validate `--max-chars`: `>= 1` (ADR-0004, replaces `--max-tokens`).
///
/// This is the semantic cleaner's chunk-size guard, and it is the ONLY place
/// that bound is enforced. Bounds and messages come from the OptionsSpec
/// (`options_spec::ai::MAX_CHARS`), the single validation source — this
/// function only converts the spec's `u64` into the `usize` the field holds,
/// mirroring `args::crawler::parse_download_concurrency`.
///
/// NOT feature-gated: the flag is ungated by decision, so every binary still
/// parses (and validates) it.
///
/// # Why `0` must be rejected (Zero Silent Loss)
///
/// `SemanticCleanerImpl::clean` rejects a chunk whose character count exceeds
/// the budget. With `max_chars == 0` that predicate is true for every
/// non-empty chunk, so the run accepted the flag, built a config, and then
/// failed every chunk of every page — a total denial of the AI path that the
/// operator could not connect to the flag that caused it. Same defect class as
/// `--download-concurrency 0` (D1 deadlock) and `--timeout-secs 0` (every
/// request times out instantly).
///
/// # Why there is no ceiling
///
/// The retired 32 768-token ceiling was ONE backend's Max Sequence Length
/// encoded as policy. Against a remote embedding endpoint the real limit is
/// the provider's context window, which it enforces per request (HTTP 400/413)
/// and the pipeline degrades on; see
/// [`crate::domain::options_spec::ai::MAX_CHARS`].
///
/// # Errors
///
/// Returns the Spanish below-min message from the spec policy for `0`, and the
/// canonical parse-failure message for anything that is not a number. clap
/// renders it as a usage error (exit 64) before any network I/O.
pub(crate) fn parse_max_chars(s: &str) -> Result<usize, String> {
    let value = crate::domain::options_spec::ai::MAX_CHARS
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| {
        crate::domain::options_spec::ai::MAX_CHARS
            .parse_error(s)
            .to_string()
    })
}

/// Validate `--max-tokens`: `>= 1` — the DEPRECATED shim (ADR-0004 phase 1).
///
/// The flag and its env var keep WORKING in the announcement release; the hard
/// rejection ships with the removal. Bounds and messages come from the
/// OptionsSpec (`options_spec::ai::MAX_TOKENS`), the single validation source,
/// routed through the same `numeric_binding` → `value_parser` path as every
/// other numeric flag. Not feature-gated: a deprecation that only answers when
/// the feature it replaces is on is not a deprecation.
///
/// The returned value is a TOKEN budget. Translating it to the character
/// budget the cleaner actually enforces is
/// [`options_spec::ai::max_tokens_to_max_chars`](crate::domain::options_spec::ai::max_tokens_to_max_chars), and the precedence between
/// the two flags lives in [`build_ai_config`](crate::cli::args).
///
/// # Why `0` is still rejected (unchanged)
///
/// A budget of `0` rejects every non-empty chunk: the run would accept the
/// flag, build a config, and then fail every chunk of every page — a total
/// denial of the AI path the operator could not connect to the flag that
/// caused it. Same zero-silent-loss rule as `--max-chars`.
///
/// # Errors
///
/// Returns the Spanish below-min message for `0` and the canonical
/// parse-failure message for a non-number. clap renders both as usage errors
/// (exit 64) before any network I/O.
pub(crate) fn parse_max_tokens(s: &str) -> Result<usize, String> {
    let value = crate::domain::options_spec::ai::MAX_TOKENS
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| {
        crate::domain::options_spec::ai::MAX_TOKENS
            .parse_error(s)
            .to_string()
    })
}

/// AI-powered semantic cleaning arguments.
///
/// EVERY field is UNGATED (ADR-0004, slice C).
///
/// This struct used to carry a cargo-feature gate on `threshold`,
/// `offline` and `ai_model`, with `max_chars` and the deprecated
/// `max_tokens` ungated. That split is gone: the semantic cleaner no longer
/// needs ONNX to exist, so every flag here describes behavior every build
/// can honor — the cleaner runs over whatever
/// `EmbeddingPort` the run built, remote included. `ai_args` renders the same
/// surface in every configuration, and this struct matches it field for
/// field. `From<Args> for CrawlOptions` reflects that.
#[derive(Debug, Default)]
pub struct AiArgs {
    /// Maximum characters per chunk before rejection (a chunk-size guard, not a context-window setting; the effective ceiling is the embedding backend's own limit, which a remote endpoint enforces server-side as HTTP 400/413)
    pub max_chars: usize,

    /// Whether `max_chars` came from an operator (argv or `WEBFANG_MAX_CHARS`)
    /// rather than from the spec's default.
    ///
    /// Carried because precedence cannot be decided from the VALUE alone: a
    /// user who typed `--max-chars 98304` must beat `--max-tokens 4096` with
    /// no warning, and a user who passed neither must fall back to the default
    /// with no warning. `clap::ArgMatches::value_source` is the only place
    /// that distinction exists.
    pub max_chars_explicit: bool,

    /// Deprecated token budget, `None` when the flag and its env var were not
    /// used. Still accepted (ADR-0004 phase 1): it is converted to
    /// [`max_chars`](Self::max_chars) by `cli::args::resolve_max_chars`, which
    /// documents the precedence against `max_chars_explicit`.
    pub max_tokens: Option<usize>,

    /// Relevance threshold for AI semantic filtering (0.0-1.0).
    ///
    /// UNGATED (ADR-0004, slice C): the relevance filter is the cleaner's own
    /// logic and computes over vectors from whatever `EmbeddingPort` the run
    /// built, so every build can honor this value.
    pub threshold: f32,

    /// Whether the operator chose `--threshold`, as distinct from letting it
    /// fall to the spec default. Same `value_source` distinction as
    /// [`max_chars_explicit`](Self::max_chars_explicit): `show` must not claim
    /// a value is "the spec default" when the operator typed it.
    pub threshold_explicit: bool,

    /// Reject every embeddings provider (remote or loopback) with exit 78: no local model since 3.0.0
    ///
    /// UNGATED. Since ADR-0004 paso 6 there is no local backend to constrain:
    /// the flag is read by the embeddings preflight, which rejects EVERY
    /// embeddings provider — loopback included — with exit 78 (#2166).
    pub offline: bool,

    /// ADR-0004 step-5 announcements this run owes, decided at parse time.
    ///
    /// NOT a flag and not part of the OptionsSpec parity table — it is the
    /// parse-time VERDICT of `pending_notices`, carried here because parse
    /// time is the only place the trigger can be observed and `CrawlOptions`
    /// (which the binary keeps past `init_logging_dual`) is not where a
    /// diagnostic belongs. `webfang_cli::main` reads it and renders the two
    /// `warn!` events; see [`PendingNotices`].
    pub pending_notices: PendingNotices,

    // Raw string on purpose (#827): validation is deferred to the AI init
    // path (`build_ai_cleaner`) so a poisoned AI_MODEL_ID env var cannot
    // make unrelated CLI invocations fail at parse time.
    /// AI model to use: granite-97m (default, fast) or granite-311m (higher quality). Env WEBFANG_AI_MODEL_ID wins; legacy AI_MODEL_ID still accepted but deprecated for removal in v3.0
    ///
    /// UNGATED, and the raw-string policy is what makes that safe rather than
    /// merely convenient: an unparseable value is recorded here and rejected by
    /// the AI init path (which exists only on a local-ONNX build), instead of
    /// failing at parse time on a build that has no local model to configure.
    pub ai_model: Option<String>,
}

/// Did an operator set `--max-chars` (or `WEBFANG_MAX_CHARS`) rather than
/// leaving the spec default in place?
///
/// Both `CommandLine` and `EnvVariable` count: an operator who exported
/// `WEBFANG_MAX_CHARS` made the same deliberate choice as one who typed the
/// flag. Only `DefaultValue` (or absent) means "nobody chose".
fn max_chars_was_chosen(m: &clap::ArgMatches) -> bool {
    chosen_by_operator(m, "max_chars")
}

/// Whether the operator (or their `WEBFANG_*` env) chose `--threshold`, as
/// distinct from accepting the spec default.
///
/// The hand-built `--threshold` slot DOES route its env var through clap:
/// `spec_command::manual_threshold` declares `.env("WEBFANG_THRESHOLD")`
/// (pinned by `manual_threshold_surface_is_pinned` in this file), so
/// `EnvVariable` appears today whenever the operator exported the variable,
/// and `CommandLine` when they typed the flag. The predicate covers both;
/// only `DefaultValue` (or absent) means "nobody chose".
fn threshold_was_chosen(m: &clap::ArgMatches) -> bool {
    chosen_by_operator(m, "threshold")
}

/// Distinctive substring of [`ONNX_DEPRECATION_NOTICE`], used as the count
/// anchor by the behavioral tests.
///
/// Counted, not merely searched, so "announced exactly once per run" is a real
/// assertion. It is also a guard on THIS module: if the anchor appeared twice
/// inside the notice itself, every count would report 2 and read as a
/// regression that does not exist.
pub const ONNX_DEPRECATION_NOTICE_MARKER: &str = "ONNX local se retiró";

/// Distinctive substring of [`orphan_threshold_notice`]. Same counting
/// rationale as [`ONNX_DEPRECATION_NOTICE_MARKER`].
pub const ORPHAN_THRESHOLD_NOTICE_MARKER: &str = "calibrado para Granite";

/// ADR-0004 paso 6: the announced break, already shipped (spec T4 / S3).
///
/// Four facts in a fixed order, per ADR-0004 `Compatibilidad`:
///
/// 1. what disappeared in 3.0.0 — the `ai` Cargo feature and ONNX local, and
///    that `cargo build --features ai` fails at RESOLUTION (Cargo features
///    resolve at compile time, so no shim could keep the flag alive);
/// 2. what survives un-gated — `--clean-ai` (now resolving its endpoint
///    through `--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER`) and
///    `--offline` (which rejects EVERY embeddings provider, remote or
///    loopback, with exit 78 — the guard fires before the endpoint is
///    inspected, so "loopback allowed" was never true; #2166);
/// 3. the shims that keep working until 3.2.0 —
///    `--ai-model`, `--max-tokens`, `WEBFANG_MAX_TOKENS`,
///    `AI_MODEL_ID` / `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`;
/// 4. where to migrate — `webfang connect` / `webfang doctor` /
///    `webfang show`.
///
/// User-facing prose is Spanish with every flag and env name left verbatim.
/// The facts travel AGAIN as English structured fields on the `warn!` event
/// that carries this text — but the TEXT itself goes in the message only, once:
/// the compact formatter appends fields after the message, so a field carrying
/// the same prose would print ~1200 characters per line and make the
/// announcement unreadable.
///
/// `webfang_cli` emits this; it is not emitted from here. See
/// [`PendingNotices`] for why the decision cannot be made at the emit site.
pub const ONNX_DEPRECATION_NOTICE: &str = concat!(
    "ONNX local se retiró en 3.0.0: la feature Cargo `ai` y la inferencia ONNX ya no existen, ",
    "y `cargo build --features ai` falla en resolución de features. ",
    "Sin gate desde entonces: `--clean-ai` (resuelve su endpoint por `--embedding-provider` / ",
    "`WEBFANG_EMBEDDING_PROVIDER`; sin proveedor falla con ConfigError, exit 78) y `--offline` ",
    "(rechaza cualquier provider de embeddings, remoto o loopback, con exit 78). ",
    "Estos shims siguen funcionando hasta 3.2.0: ",
    "`--ai-model`, `--max-tokens`, `WEBFANG_MAX_TOKENS`, ",
    "`AI_MODEL_ID` / `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`. ",
    "Migración: `webfang connect`, `webfang doctor`, `webfang show`."
);

/// ADR-0004 step 5: the orphaned default relevance threshold (spec T5 / S4).
///
/// The shipped [`DEFAULT_THRESHOLD`](crate::domain::options_spec::ai::DEFAULT_THRESHOLD)
/// was calibrated for Granite, and Granite is the local ONNX model this ADR
/// removes: no model left in the box is calibrated for it, so a remote
/// endpoint retains differently under the same number. ADR-0004 makes the
/// mitigation mandatory with an exit criterion — warn while the threshold is
/// still the default, capped at 2 minor releases.
///
/// `threshold` is interpolated rather than restated as a literal so the warning
/// cannot describe a number the run is not using.
#[must_use]
pub fn orphan_threshold_notice(threshold: f32) -> String {
    format!(
        concat!(
            "El umbral de relevancia por defecto ({threshold}) está calibrado para Granite ",
            "(ONNX local). Sin modelo local, un endpoint remoto retiene distinto con el ",
            "mismo número: elegí `--threshold` (o `WEBFANG_THRESHOLD`) según tu modelo. ",
            "Referencia: ADR-0004, sección «Umbral por defecto huérfano»."
        ),
        threshold = threshold
    )
}

/// Which ADR-0004 step-5 announcements THIS run owes the operator.
///
/// Produced at parse time by `AiArgs::from_matches` and consumed by
/// `webfang_cli::main` AFTER `init_logging_dual`. Both halves are forced:
///
/// - **The decision cannot be made at the emit site.** Two of the five trigger
///   doors (`AI_MODEL_ID`, `WEBFANG_AI_ENGINE`) are read below clap by
///   `webfang_ai`, which `webfang_core` cannot see, and "did the operator
///   choose `--threshold`" is a `value_source` distinction that does not
///   survive into `AiConfig` — the value alone cannot tell an explicit `0.3`
///   from the default. Only parse time knows.
/// - **The emission cannot happen at parse time.** argv normalization runs
///   before the tracing subscriber exists (#796), so a `warn!` raised there is
///   silently dropped.
///
/// `webfang_cli` therefore owns the `warn!` events — the same shape as the
/// `--max-tokens` deprecation — and this type is only the verdict. It is `Copy`
/// and carries no allocation: the notices themselves are const text rendered on
/// demand at emit time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PendingNotices {
    /// The run touched the AI surface: emit [`ONNX_DEPRECATION_NOTICE`].
    pub onnx_deprecation: bool,
    /// The threshold is still the default, so emit
    /// [`orphan_threshold_notice`] with this value. `None` when the operator
    /// chose `--threshold` / `WEBFANG_THRESHOLD` themselves.
    pub orphan_threshold: Option<f32>,
}

/// Whether this value came from an operator decision rather than the spec's
/// default, for any arg id.
///
/// THE provenance predicate for this module: every "did somebody choose this?"
/// question in `AiArgs` — `--max-chars`, `--threshold`, and the two args
/// `touches_ai_surface` reads from the root matches — is answered here, so
/// the meaning of "chosen" cannot drift between them.
fn chosen_by_operator(m: &clap::ArgMatches, id: &str) -> bool {
    matches!(
        m.value_source(id),
        Some(clap::parser::ValueSource::CommandLine) | Some(clap::parser::ValueSource::EnvVariable)
    )
}

/// Legacy (unprefixed) AI model env var.
///
/// NOT clap-bound: the [`AI_MODEL`](crate::domain::options_spec::ai::AI_MODEL)
/// spec entry routes `WEBFANG_AI_MODEL_ID`, and `AI_MODEL_ID` survives only as
/// a HIDDEN LONG ALIAS of `--ai-model` (#827/#1587). The bare env var is read
/// further down the stack by `webfang_ai`'s
/// `compat::read_ai_model_id_with`, which `webfang_core` cannot see — so from
/// argv it is invisible, and an operator whose only AI input is a stale
/// `AI_MODEL_ID` in a systemd unit still needs the deprecation notice.
///
/// PUBLIC since ADR-0004 paso 6: `webfang_ai` is gone, so the CLI is now the
/// only reader left, and it must spell the name the same way this module
/// does.
pub const LEGACY_AI_MODEL_ENV: &str = "AI_MODEL_ID";

/// The name of the Cargo feature ADR-0004 removes.
///
/// ONE definition, because the removal is announced in two places that must
/// not drift: the deprecation notice names it in prose, and the CLI's `warn!`
/// event carries it as a structured field. Keeping the literal here (and not
/// at the call sites) is what makes the announcement and the queryable event
/// the same fact.
pub const REMOVED_AI_FEATURE: &str = "ai";

/// Engine-selection env var (`single` | `pool:<N>`), with no CLI flag at all.
///
/// Read by `webfang_ai`'s `EngineConfig::from_env` (ADR-0004 lists it among
/// the shims that must survive the announcement), so argv cannot see it.
const AI_ENGINE_ENV: &str = "WEBFANG_AI_ENGINE";

/// Did this run actually TOUCH the AI surface?
///
/// The five doors, in the order ADR-0004 lists them: `--clean-ai`,
/// `--ai-model` (with `WEBFANG_AI_MODEL_ID`), the legacy `AI_MODEL_ID`, and
/// `WEBFANG_AI_ENGINE`. `--clean-ai` comes from the crawler group rather than
/// this one, but `ArgMatches` is the whole command's matches, so both ids
/// resolve here.
///
/// # Why the notice is CONDITIONAL and not on every run (decided, do not
/// re-open)
///
/// A warning printed on every invocation is a warning nobody reads: it puts
/// the same bytes in front of an operator scraping a single page with no AI
/// involvement at all, and the trained response to a constant stderr line is to
/// stop reading stderr — which would swallow the run that genuinely needs the
/// notice. The general announcement is the release notes' job (ADR-0004
/// `Compatibilidad`, plus the `BREAKING CHANGE:` trailer that release-plz
/// renders from the commit); THIS notice is for the operator who actually has
/// something to migrate.
///
/// The discipline is deliberately the same one the existing `--max-tokens`
/// shim warning follows: it speaks only when the deprecated thing was used.
/// `ai_engine_env_alone_reaches_the_deprecation_notice` and
/// `onnx_deprecation_notice_does_not_fire_when_the_ai_surface_is_untouched`
/// pin both directions.
fn touches_ai_surface(m: &clap::ArgMatches) -> bool {
    chosen_by_operator(m, "clean_ai")
        || chosen_by_operator(m, "ai_model")
        || std::env::var_os(LEGACY_AI_MODEL_ENV).is_some()
        || std::env::var_os(AI_ENGINE_ENV).is_some()
}

/// Decide which ADR-0004 step-5 announcements this run owes (T4 + T5).
///
/// Returns the verdict; it emits nothing. `webfang_cli::main` renders it after
/// `init_logging_dual` — see [`PendingNotices`] for why both halves must live
/// where they do.
///
/// Called from `AiArgs::from_matches`, the single reader every build goes
/// through, so the notices are feature-independent by construction: an operator
/// on a non-`ai` binary is exactly the one who most needs to hear the feature
/// is going away.
///
/// The trigger is deliberately NOT narrowed to the shims now that the removal
/// shipped. `--clean-ai` still fires the notice, and that is bounded rather
/// than permanent: the notice is deleted together with the shims in 3.2.0.
/// Narrowing it now would change a contract the behavioral tests pin
/// (`onnx_deprecation_notice_fires_once_when_clean_ai_is_used`) inside the
/// change that has to declare a breaking release, and the notice is true for a
/// `--clean-ai` operator either way.
fn pending_notices(
    m: &clap::ArgMatches,
    threshold: f32,
    threshold_explicit: bool,
) -> PendingNotices {
    if !touches_ai_surface(m) {
        return PendingNotices::default();
    }
    // The orphan-threshold mitigation has an exit criterion (ADR-0004: capped
    // at 2 minor releases) and its precondition is precisely "the threshold is
    // still the default AND no model is calibrated". Granite was the only
    // calibrated model and it is what this ADR removes, so the default value
    // alone carries the "uncalibrated" half. An operator who typed a threshold
    // has taken the calibration into their own hands: warning them would be
    // noise about a decision they already made.
    PendingNotices {
        onnx_deprecation: true,
        orphan_threshold: if threshold_explicit {
            None
        } else {
            Some(threshold)
        },
    }
}

#[cfg(test)]
mod deprecation_notice_tests {
    //! The two notice texts and the counting anchors the behavioral tests use.
    //!
    //! [`orphan_threshold_notice`]'s interpolation is otherwise UNOBSERVABLE
    //! from the CLI: the warning only fires when the threshold is the default,
    //! so no reachable run can ever show it quoting anything but `0.3`. A
    //! literal pasted into the message would pass every behavioral test and
    //! still be wrong the moment the default moves — this is its only witness.

    use super::{
        orphan_threshold_notice, ONNX_DEPRECATION_NOTICE, ONNX_DEPRECATION_NOTICE_MARKER,
        ORPHAN_THRESHOLD_NOTICE_MARKER,
    };

    /// The behavioral tests assert a count of exactly 1. That is only
    /// meaningful if each anchor appears once INSIDE its own text; an anchor
    /// repeated in the prose would make every count report 2 and read as a
    /// regression that does not exist.
    #[test]
    fn each_anchor_occurs_once_inside_its_own_text() {
        assert_eq!(
            ONNX_DEPRECATION_NOTICE
                .matches(ONNX_DEPRECATION_NOTICE_MARKER)
                .count(),
            1,
            "the ONNX notice must carry its own counting anchor exactly once"
        );
        assert_eq!(
            orphan_threshold_notice(0.3)
                .matches(ORPHAN_THRESHOLD_NOTICE_MARKER)
                .count(),
            1,
            "the orphan-threshold notice must carry its own counting anchor \
             exactly once"
        );
        // Neither anchor may appear in the other text: counting one must never
        // be able to satisfy the other notice's assertion.
        assert!(!ONNX_DEPRECATION_NOTICE.contains(ORPHAN_THRESHOLD_NOTICE_MARKER));
        assert!(!orphan_threshold_notice(0.3).contains(ONNX_DEPRECATION_NOTICE_MARKER));
    }

    /// The message quotes the value it was HANDED, so it can never describe a
    /// threshold the run is not using.
    #[test]
    fn the_orphan_notice_quotes_the_threshold_it_was_given() {
        assert!(
            orphan_threshold_notice(0.55).contains("(0.55)"),
            "the notice must interpolate the value, never a hardcoded literal"
        );
        assert!(
            orphan_threshold_notice(crate::domain::options_spec::ai::DEFAULT_THRESHOLD)
                .contains("(0.3)"),
            "the shipped default must render as `0.3`, not as an f32 artifact"
        );
    }
}

/// The reads that exist in EVERY build, as `(max_chars, max_chars_explicit,
/// max_tokens)`.
///
/// ADR-0004 makes `--max-chars` and the `--max-tokens` shim ungated, so both
/// `FromArgMatches` arms below read the same three values. Extracting them here
/// keeps the two arms from drifting: a fourth ungated field must be added in
/// one place, and a reviewer reading either arm sees only what that build adds.
fn ungated_from_matches(m: &clap::ArgMatches) -> Result<(usize, bool, Option<usize>), clap::Error> {
    use crate::cli::spec_command::extract;
    Ok((
        extract::value::<usize>(m, "max_chars")?,
        max_chars_was_chosen(m),
        extract::opt::<usize>(m, "max_tokens"),
    ))
}

impl AiArgs {
    /// The single reader for both builds.
    ///
    /// The whole struct is ungated, so this reads EVERY field unconditionally.
    /// Concentrating the read HERE means one `FromArgMatches` impl serves every
    /// build: two `cfg`-gated impls once duplicated the trait methods verbatim,
    /// and any field a build could not read would have needed editing in two
    /// places with nothing to catch the miss.
    fn from_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let (max_chars, max_chars_explicit, max_tokens) = ungated_from_matches(m)?;
        use crate::cli::spec_command::extract;
        let threshold = extract::value::<f32>(m, "threshold")?;
        let threshold_explicit = threshold_was_chosen(m);
        // The AI surface is known only here — every door is either in this
        // group or readable from the same root `ArgMatches` — so this is the
        // one place the ADR-0004 announcements can be decided.
        let pending_notices = pending_notices(m, threshold, threshold_explicit);
        Ok(Self {
            max_chars,
            max_chars_explicit,
            max_tokens,
            threshold,
            threshold_explicit,
            offline: m.get_flag("offline"),
            ai_model: extract::opt::<String>(m, "ai_model"),
            pending_notices,
        })
    }
}

impl clap::FromArgMatches for AiArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        Self::from_matches(m)
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for AiArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::ai_args(
            crate::cli::spec_command::Headings::Applied,
        ))
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

#[cfg(test)]
mod spec_parity_tests {
    //! ADR-002 equivalence proof (slice 5a, ALL configurations):
    //! the hand-derived clap surface of [`AiArgs`] must stay in lockstep
    //! with the OptionsSpec AI group. The `threshold` arg is hand-built
    //! (deferred from the spec), so its surface is pinned independently.
    use super::*;
    use crate::cli::args::test_support::{
        arg_by_id, assert_defaults, assert_help, assert_long_short_alias_env_heading,
        assert_structural, assert_surface_covered, collect_args, parse_args, parse_args_hermetic,
    };
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `AiArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        collect_args(AiArgs::augment_args(clap::Command::new("webfang-ai")))
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        // The hand-built `threshold` slot is in the spec (not routed
        // through `build_arg`); its surface is pinned separately below.
        assert_surface_covered(&command_args(), spec::ai::GROUP);
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        assert_long_short_alias_env_heading(&command_args(), spec::ai::GROUP);
    }

    #[test]
    fn defaults_match_the_spec() {
        assert_defaults(&command_args(), spec::ai::GROUP);
    }

    #[test]
    fn help_text_matches_the_spec() {
        assert_help(&command_args(), spec::ai::GROUP);
    }

    #[test]
    fn representative_values_parse_identically_through_clap() {
        // Defaults (hermetic: ambient WEBFANG_THRESHOLD / WEBFANG_MAX_CHARS
        // / WEBFANG_MAX_TOKENS / WEBFANG_OFFLINE / AI_MODEL_ID must not leak
        // — issue #926).
        let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
        assert_eq!(defaults.ai.threshold, 0.3);
        assert_eq!(defaults.ai.max_chars, 98_304);
        assert!(!defaults.ai.offline);
        assert!(defaults.ai.ai_model.is_none());

        // Explicit values, including the unprefixed `AI_MODEL_ID` env
        // (#827) and the floating-point range.
        let parsed = parse_args_hermetic(&[
            "--threshold",
            "0.5",
            "--max-chars",
            "1024",
            "--offline",
            "--ai-model",
            "granite-311m",
        ])
        .expect("representative ai flags must parse");
        assert_eq!(parsed.ai.threshold, 0.5);
        assert_eq!(parsed.ai.max_chars, 1024);
        assert!(parsed.ai.offline);
        assert_eq!(parsed.ai.ai_model.as_deref(), Some("granite-311m"));
    }

    /// `--max-tokens` is a deprecation shim in ADR-0004's PHASE 1
    /// (announcement): the flag still works, and its value is recorded so
    /// `build_ai_config` can convert it and so the CLI can warn exactly once.
    /// The hard rejection belongs to phase 2 (removal), not here.
    #[test]
    fn max_tokens_shim_still_accepts_values_and_records_them() {
        // ADR-0004 phase 1 (announcement): the flag WORKS. Rejecting it here
        // would pin the removal into the announcement release.
        for value in ["1", "1024", "32768", "999999"] {
            let parsed = parse_args(&["--max-tokens", value])
                .unwrap_or_else(|e| panic!("`--max-tokens {value}` must still parse: {e}"));
            assert_eq!(
                parsed.ai.max_tokens,
                Some(value.parse::<usize>().expect("numeric")),
                "the deprecated value must survive parsing untouched"
            );
            assert!(
                parsed.ai.max_tokens.is_some(),
                "provenance must be recorded so the CLI can warn exactly once"
            );
            assert!(
                !parsed.ai.max_chars_explicit,
                "using only the deprecated flag does not make --max-chars explicit"
            );
        }

        // Zero silent loss, unchanged by the rename: 0 would reject every chunk.
        let err = parse_args(&["--max-tokens", "0"]).expect_err("zero must be rejected");
        assert!(err.contains("0 rechazaría todos los chunks"), "got: {err}");

        // A typo is not a request.
        let err = parse_args(&["--max-tokens", "banana"]).expect_err("non-numeric rejected");
        assert!(err.contains("no es un número entero válido"), "got: {err}");

        // Absent means "nobody used it", which is what keeps the deprecation
        // silent for every run that never touched the flag.
        let parsed = parse_args(&[]).expect("bare invocation must parse");
        assert_eq!(parsed.ai.max_tokens, None);
        assert!(
            !parsed.ai.max_chars_explicit,
            "no flag means --max-chars was not chosen, only defaulted"
        );

        // Precedence input: an explicit --max-chars is marked as chosen, so
        // `build_ai_config` can let it win silently over a stale shim value.
        let parsed = parse_args(&["--max-chars", "2048", "--max-tokens", "4096"])
            .expect("both flags must parse");
        assert!(parsed.ai.max_chars_explicit);
        assert_eq!(parsed.ai.max_chars, 2048);
        assert_eq!(parsed.ai.max_tokens, Some(4096));

        // The env door is bound to the SAME parser as the flag — a stale
        // `.env` reaches the same budget, not a softer one.
        let arg = collect_args(AiArgs::augment_args(clap::Command::new("webfang-ai")))
            .into_iter()
            .find(|a| a.get_id() == "max_tokens")
            .expect("the shim must still render as an arg");
        assert_eq!(
            arg.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_MAX_TOKENS"),
            "the shim keeps its env var so a stale .env still reaches the guard"
        );
    }

    /// Slice 5a pin: structural clap surface the spec-driven builder must
    /// reproduce byte-for-byte. `threshold` is the hand-built slot, and it
    /// is built structurally identical (Set / THRESHOLD / no delimiter),
    /// so the shared sweep covers it too; everything else goes through
    /// `build_arg`.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        assert_structural(&command_args(), spec::ai::GROUP);
    }

    /// Slice 5a pin: the hand-built `threshold` slot's surface stays
    /// byte-exact against the pre-migration derive. Custom f32 parser
    /// plus range check plus verbatim Spanish error messages plus the
    /// `allow_negative_numbers` escape hatch (#759, range 0.0..=1.0).
    #[test]
    fn manual_threshold_surface_is_pinned() {
        let args = command_args();
        let t = arg_by_id(&args, "threshold", "AiArgs");
        assert_eq!(t.get_long(), Some("threshold"));
        assert_eq!(t.get_short(), None);
        assert_eq!(
            t.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_THRESHOLD")
        );
        assert!(matches!(t.get_action(), clap::ArgAction::Set));
        assert_eq!(
            t.get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["0.3"]
        );
        assert_eq!(
            t.get_help()
                .expect("threshold must carry short help")
                .to_string()
                .trim(),
            "Relevance threshold for AI semantic filtering (0.0-1.0)"
        );
        assert_eq!(
            t.get_help_heading(),
            Some("AI Settings"),
            "help_heading mismatch for threshold"
        );
        assert!(t.get_long_help().is_none());
        // The parser is custom (`parse_threshold`) — clap does not let us
        // introspect its source, but the BEHAVIOR is pinned by the
        // out_of_bounds_and_malformed_inputs_error_exactly_as_before
        // test below.
    }

    #[test]
    fn out_of_bounds_and_malformed_inputs_error_exactly_as_before() {
        // The pre-migration `parse_threshold` produced:
        //   `'{s}' no es un número válido`                  on parse failure
        //   `'{s}' está fuera de rango (rango válido: 0.0 a 1.0)` on range
        // Both messages must round-trip through the spec-built command.
        let err = parse_args(&["--threshold", "abc"]).expect_err("non-numeric rejected");
        assert!(err.contains("'abc' no es un número válido"), "got: {err}");

        let err = parse_args(&["--threshold", "1.5"]).expect_err("above range rejected");
        assert!(
            err.contains("'1.5' está fuera de rango (rango válido: 0.0 a 1.0)"),
            "got: {err}"
        );

        let err = parse_args(&["--threshold", "-0.1"]).expect_err("below range rejected");
        assert!(
            err.contains("'-0.1' está fuera de rango (rango válido: 0.0 a 1.0)"),
            "got: {err}"
        );
    }
}
