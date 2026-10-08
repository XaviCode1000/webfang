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
#[cfg(feature = "ai")]
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
/// NOT feature-gated: the flag is ungated by decision, so a binary built
/// without `ai` still parses (and validates) it.
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
/// [`options_spec::ai::max_tokens_to_max_chars`], and the precedence between
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
/// `max_chars` and the deprecated `max_tokens` are UNGATED (ADR-0004): both are
/// emitted by `ai_args` in every cargo configuration, so the fields and their
/// reads are too. Every other field is `#[cfg(feature = "ai")]` — mirroring the
/// pre-migration derive's behavior of producing zero args and a zero-field
/// struct when the cargo feature is off. `From<Args> for CrawlOptions`
/// reflects that.
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

    /// Relevance threshold for AI semantic filtering (0.0-1.0)
    #[cfg(feature = "ai")]
    pub threshold: f32,

    /// Run AI model in offline mode
    #[cfg(feature = "ai")]
    pub offline: bool,

    // Raw string on purpose (#827): validation is deferred to the AI init
    // path (`build_ai_cleaner`) so a poisoned AI_MODEL_ID env var cannot
    // make unrelated CLI invocations fail at parse time.
    /// AI model to use: granite-97m (default, fast) or granite-311m (higher quality). Env WEBFANG_AI_MODEL_ID wins; legacy AI_MODEL_ID still accepted but deprecated for removal in v3.0
    #[cfg(feature = "ai")]
    pub ai_model: Option<String>,
}

/// Did an operator set `--max-chars` (or `WEBFANG_MAX_CHARS`) rather than
/// leaving the spec default in place?
///
/// Both `CommandLine` and `EnvVariable` count: an operator who exported
/// `WEBFANG_MAX_CHARS` made the same deliberate choice as one who typed the
/// flag. Only `DefaultValue` (or absent) means "nobody chose".
fn max_chars_was_chosen(m: &clap::ArgMatches) -> bool {
    matches!(
        m.value_source("max_chars"),
        Some(clap::parser::ValueSource::CommandLine) | Some(clap::parser::ValueSource::EnvVariable)
    )
}

#[cfg(feature = "ai")]
impl clap::FromArgMatches for AiArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            max_chars: extract::value::<usize>(m, "max_chars")?,
            max_chars_explicit: max_chars_was_chosen(m),
            max_tokens: extract::opt::<usize>(m, "max_tokens"),
            threshold: extract::value::<f32>(m, "threshold")?,
            offline: m.get_flag("offline"),
            ai_model: extract::opt::<String>(m, "ai_model"),
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

/// `cfg(not(feature = "ai"))` counterpart: only the ungated `max_chars` is
/// read, the rest of the struct is gone. `FromArgMatches` still reads
/// `max_chars` because the flag renders in this configuration too.
#[cfg(not(feature = "ai"))]
impl clap::FromArgMatches for AiArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            max_chars: extract::value::<usize>(m, "max_chars")?,
            max_chars_explicit: max_chars_was_chosen(m),
            max_tokens: extract::opt::<usize>(m, "max_tokens"),
        })
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

#[cfg(all(test, feature = "ai"))]
mod spec_parity_tests {
    //! ADR-002 equivalence proof (slice 5a, `cfg(feature = "ai")` only):
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
