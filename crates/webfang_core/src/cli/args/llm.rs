//! LLM provider arguments — `--extract-with-llm` and provider selection.
//!
//! Parsing stays derive-driven (`FromArgMatches`); command assembly is
//! spec-built (ADR-002 slice 3); see `cli::spec_command`.
//!
//! This group is **not** feature-gated: the remote provider port lives in
//! `webfang_core` unconditionally, and ADR-0004 paso 6 removed the local ONNX
//! stack that the old `ai` feature used to gate.

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
    /// capability (config order).
    pub embedding_provider: Option<String>,

    /// Provider id to use for the Tier 2 semantic inspector embedding slot (ADR-0004).
    ///
    /// `None` leaves the slot empty: Tier 2 does not fall back to the
    /// vault-search provider, and there is no local pool left to serve it
    /// (ADR-0004 paso 6).
    /// Separate from `embedding_provider` on purpose: vault search and
    /// Tier 2 repair are distinct privacy domains (issue #1977).
    pub tier2_embedding_provider: Option<String>,

    /// Explicit consent for Tier 2 remote embeddings egress (ADR-0004).
    ///
    /// Required when `--tier2-embedding-provider` is set: every resolved
    /// Tier 2 embedding provider is remote. A hash of `provider_id|base_url`
    /// is logged at consent time for auditability.
    pub tier2_consent: bool,
}

impl clap::FromArgMatches for LlmArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            extract_with_llm: m.get_flag("extract_with_llm"),
            llm_provider: extract::opt::<String>(m, "llm_provider"),
            embedding_provider: extract::opt::<String>(m, "embedding_provider"),
            tier2_embedding_provider: extract::opt::<String>(m, "tier2_embedding_provider"),
            tier2_consent: m.get_flag("tier2_consent"),
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
        assert!(defaults.llm.tier2_embedding_provider.is_none());
        assert!(!defaults.llm.tier2_consent);

        // Explicit values.
        let parsed = parse_args_hermetic(&["--extract-with-llm", "--llm-provider", "openai"])
            .expect("representative llm flags must parse");
        assert!(parsed.llm.extract_with_llm);
        assert_eq!(parsed.llm.llm_provider.as_deref(), Some("openai"));

        let parsed = parse_args_hermetic(&["--embedding-provider", "ollama"])
            .expect("embedding provider flag must parse");
        assert_eq!(parsed.llm.embedding_provider.as_deref(), Some("ollama"));

        let parsed = parse_args_hermetic(&["--tier2-embedding-provider", "ollama"])
            .expect("tier2 embedding provider flag must parse");
        assert_eq!(
            parsed.llm.tier2_embedding_provider.as_deref(),
            Some("ollama")
        );

        let parsed =
            parse_args_hermetic(&["--tier2-consent"]).expect("tier2 consent flag must parse");
        assert!(parsed.llm.tier2_consent);
    }

    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        assert_structural(&command_args(), spec::llm::GROUP);
    }
}
