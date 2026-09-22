//! LLM provider arguments — `--extract-with-llm` and provider selection.
//!
//! Parsing stays derive-driven (`FromArgMatches`); command assembly is
//! spec-built (ADR-002 slice 3); see `cli::spec_command`.
//!
//! This group is **not** gated by the `ai` cargo feature: the `ai` feature is
//! the local ONNX stack, while the remote provider port lives in
//! `webfang_core` unconditionally.

/// LLM extraction arguments.
#[derive(Debug, Default)]
pub struct LlmArgs {
    /// Run structured extraction through the configured LLM provider.
    ///
    /// Trigger for the Container contract in `ai-providers-design.md` §8b: set
    /// without a configured `completion` provider, startup fails (exit 78).
    pub extract_with_llm: bool,

    /// Provider id to use for LLM extraction.
    ///
    /// `None` selects the first provider declaring the `completion`
    /// capability (config order).
    pub llm_provider: Option<String>,

    /// Provider id to use for the embedding slot (vault search, #1462).
    ///
    /// `None` selects the first provider declaring the `embedding`
    /// capability (config order); `local_onnx` serves the local pool.
    pub embedding_provider: Option<String>,
}

impl clap::FromArgMatches for LlmArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            extract_with_llm: m.get_flag("extract_with_llm"),
            llm_provider: extract::opt::<String>(m, "llm_provider"),
            embedding_provider: extract::opt::<String>(m, "embedding_provider"),
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for LlmArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::llm_args(
            crate::cli::spec_command::Headings::Applied,
        ))
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

#[cfg(test)]
mod spec_parity_tests {
    //! ADR-002 equivalence proof: the hand-derived clap surface of
    //! [`LlmArgs`] must stay in lockstep with the OptionsSpec LLM group.
    //! Written FIRST against the spec-built command so any future spec drift
    //! fails here first.
    use super::*;
    use crate::cli::args::test_support::{
        assert_defaults, assert_help, assert_long_short_alias_env_heading, assert_structural,
        assert_surface_covered, collect_args, parse_args_hermetic,
    };
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `LlmArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        collect_args(LlmArgs::augment_args(clap::Command::new("webfang-llm")))
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        assert_surface_covered(&command_args(), spec::llm::GROUP);
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        assert_long_short_alias_env_heading(&command_args(), spec::llm::GROUP);
    }

    #[test]
    fn defaults_match_the_spec() {
        assert_defaults(&command_args(), spec::llm::GROUP);
    }

    #[test]
    fn help_text_matches_the_spec() {
        assert_help(&command_args(), spec::llm::GROUP);
    }

    #[test]
    fn representative_values_parse_identically_through_clap() {
        // Defaults (hermetic: ambient WEBFANG_LLM_* must not leak).
        let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
        assert!(!defaults.llm.extract_with_llm);
        assert!(defaults.llm.llm_provider.is_none());
        assert!(defaults.llm.embedding_provider.is_none());

        // Explicit values.
        let parsed = parse_args_hermetic(&["--extract-with-llm", "--llm-provider", "openai"])
            .expect("representative llm flags must parse");
        assert!(parsed.llm.extract_with_llm);
        assert_eq!(parsed.llm.llm_provider.as_deref(), Some("openai"));

        let parsed = parse_args_hermetic(&["--embedding-provider", "ollama"])
            .expect("embedding provider flag must parse");
        assert_eq!(parsed.llm.embedding_provider.as_deref(), Some("ollama"));
    }

    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        assert_structural(&command_args(), spec::llm::GROUP);
    }
}
