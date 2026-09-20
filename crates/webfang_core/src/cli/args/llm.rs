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
}

impl clap::FromArgMatches for LlmArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            extract_with_llm: m.get_flag("extract_with_llm"),
            llm_provider: extract::opt::<String>(m, "llm_provider"),
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
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `LlmArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        LlmArgs::augment_args(clap::Command::new("webfang-llm"))
            .get_arguments()
            .cloned()
            .collect()
    }

    fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from LlmArgs command"))
    }

    fn parse_args(extra: &[&str]) -> Result<crate::Args, String> {
        let mut argv = vec!["webfang"];
        argv.extend_from_slice(extra);
        clap::Parser::try_parse_from(argv).map_err(|e| e.to_string())
    }

    fn parse_args_hermetic(extra: &[&str]) -> Result<crate::Args, String> {
        crate::cli::args::test_support::with_clap_env_cleared(|| parse_args(extra))
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        let args = command_args();
        for arg in &args {
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            assert!(
                spec::llm::GROUP.iter().any(|s| s.id == arg.get_id()),
                "clap arg `{}` has no OptionsSpec entry — spec is out of sync",
                arg.get_id()
            );
        }
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        let args = command_args();
        for s in spec::llm::GROUP {
            let arg = arg_by_id(&args, s.id);
            assert_eq!(arg.get_long(), Some(s.long), "long mismatch for `{}`", s.id);
            assert_eq!(arg.get_short(), s.short, "short mismatch for `{}`", s.id);
            let aliases = arg.get_aliases().unwrap_or_default();
            assert_eq!(aliases, s.aliases, "alias mismatch for `{}`", s.id);
            let env = arg.get_env().map(|e| e.to_string_lossy().into_owned());
            assert_eq!(env.as_deref(), s.env, "env var mismatch for `{}`", s.id);
        }
    }

    #[test]
    fn defaults_match_the_spec() {
        let args = command_args();
        for s in spec::llm::GROUP {
            let arg = arg_by_id(&args, s.id);
            let defaults: Vec<String> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let expected: Vec<String> = s.default.map(|d| vec![d.to_string()]).unwrap_or_default();
            assert_eq!(defaults, expected, "default mismatch for `{}`", s.id);
        }
    }

    #[test]
    fn help_text_matches_the_spec() {
        let args = command_args();
        for s in spec::llm::GROUP {
            let arg = arg_by_id(&args, s.id);
            let help = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .unwrap_or_else(|| panic!("arg `{}` has no help text", s.id))
                .to_string();
            assert_eq!(
                help.trim(),
                s.help.trim(),
                "help text mismatch for `{}`",
                s.id
            );
        }
    }

    #[test]
    fn representative_values_parse_identically_through_clap() {
        // Defaults (hermetic: ambient WEBFANG_LLM_* must not leak).
        let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
        assert!(!defaults.llm.extract_with_llm);
        assert!(defaults.llm.llm_provider.is_none());

        // Explicit values.
        let parsed = parse_args_hermetic(&["--extract-with-llm", "--llm-provider", "openai"])
            .expect("representative llm flags must parse");
        assert!(parsed.llm.extract_with_llm);
        assert_eq!(parsed.llm.llm_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        let args = command_args();
        for s in spec::llm::GROUP {
            let arg = arg_by_id(&args, s.id);
            match s.kind {
                spec::ValueKind::Bool => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::SetTrue),
                        "bool `{}` must use SetTrue",
                        s.id
                    );
                },
                _ => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::Set),
                        "value option `{}` must use Set",
                        s.id
                    );
                },
            }
            let names: Vec<String> = arg
                .get_value_names()
                .unwrap_or_default()
                .iter()
                .map(|id| id.to_string())
                .collect();
            assert_eq!(
                names,
                vec![s.id.to_ascii_uppercase()],
                "value name mismatch for `{}`",
                s.id
            );
            assert_eq!(
                arg.get_value_delimiter(),
                s.value_delimiter,
                "value_delimiter mismatch for `{}`",
                s.id
            );
            assert!(
                arg.get_long_help().is_none(),
                "`{}` must not carry long help",
                s.id
            );
        }
    }
}
