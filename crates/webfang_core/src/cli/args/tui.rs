//! Terminal UI configuration arguments (ADR-002 slice 5a).
//!
//! Parsing stays derive-driven (`FromArgMatches`); command assembly is
//! spec-built (ADR-002 slice 3); see `cli::spec_command`.

/// Terminal UI configuration arguments.
#[derive(Debug, Default)]
pub struct TuiArgs {
    /// Unified TUI mode: config form (collapsible sections) → URL selector → scraping
    pub tui: bool,
}

impl clap::FromArgMatches for TuiArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            tui: m.get_flag("tui"),
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for TuiArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::tui_args(
            crate::cli::spec_command::Headings::Applied,
        ))
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

#[cfg(test)]
mod spec_parity_tests {
    //! ADR-002 equivalence proof (slice 5a): the hand-derived clap surface of
    //! [`TuiArgs`] must stay in lockstep with the OptionsSpec TUI group.
    //! Written FIRST against the (post-migration) spec-built command so any
    //! future spec drift fails here first.
    use super::*;
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `TuiArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        TuiArgs::augment_args(clap::Command::new("webfang-tui"))
            .get_arguments()
            .cloned()
            .collect()
    }

    fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from TuiArgs command"))
    }

    fn parse_args(extra: &[&str]) -> Result<crate::Args, String> {
        let mut argv = vec!["webfang"];
        argv.extend_from_slice(extra);
        clap::Parser::try_parse_from(argv).map_err(|e| e.to_string())
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        let args = command_args();
        for arg in &args {
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            assert!(
                spec::tui::GROUP.iter().any(|s| s.id == arg.get_id()),
                "clap arg `{}` has no OptionsSpec entry — spec is out of sync",
                arg.get_id()
            );
        }
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        let args = command_args();
        for s in spec::tui::GROUP {
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
        for s in spec::tui::GROUP {
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
        for s in spec::tui::GROUP {
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
        // Defaults (hermetic: ambient WEBFANG_TUI must not leak into the
        // bare parse — issue #926).
        let defaults = crate::cli::args::test_support::with_clap_env_cleared(|| parse_args(&[]))
            .expect("bare invocation must parse");
        assert!(!defaults.tui.tui, "--tui defaults to false");

        // Explicit form.
        let parsed = parse_args(&["--tui"]).expect("--tui must parse");
        assert!(parsed.tui.tui, "--tui sets the flag");
    }

    /// Slice 5a pin: structural clap surface the spec-driven builder must
    /// reproduce byte-for-byte.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        let args = command_args();
        for s in spec::tui::GROUP {
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
            let possible: Vec<String> = arg
                .get_possible_values()
                .into_iter()
                .map(|v| v.get_name().to_string())
                .collect();
            if let spec::ValueKind::Enum { variants } = s.kind {
                assert_eq!(possible, variants, "possible values for `{}`", s.id);
            } else if !matches!(s.kind, spec::ValueKind::Bool) {
                assert!(
                    possible.is_empty(),
                    "`{}` must have no possible values",
                    s.id
                );
            }
            assert!(
                arg.get_long_help().is_none(),
                "`{}` must not carry long help",
                s.id
            );
        }
    }
}
