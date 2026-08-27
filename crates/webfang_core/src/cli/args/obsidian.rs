//! Obsidian vault integration arguments (ADR-002 slice 5a).
//!
//! Parsing stays derive-driven (`FromArgMatches`); command assembly is
//! spec-built (ADR-002 slice 3); see `cli::spec_command`.

/// Obsidian vault integration arguments.
#[derive(Debug, Default)]
pub struct ObsidianArgs {
    /// Convert same-domain links to Obsidian [[wiki-link]] syntax
    pub obsidian_wiki_links: bool,

    /// Tags to include in YAML frontmatter (comma-separated)
    pub obsidian_tags: Option<Vec<String>>,

    /// Rewrite downloaded asset paths as relative to the .md file
    pub obsidian_relative_assets: bool,

    /// Path to Obsidian vault (auto-detects if not provided).
    ///
    /// When provided explicitly, the vault becomes the output base: Markdown,
    /// downloaded assets and the RAG export are written inside it — no need
    /// to duplicate the path in `-o` (which then must stay at its default).
    /// Auto-detected or config-file vaults do NOT redirect output (#762).
    pub vault: Option<std::path::PathBuf>,

    /// Quick-save mode: save directly to vault _inbox folder
    pub quick_save: bool,

    /// Add rich metadata to frontmatter
    pub obsidian_rich_metadata: bool,
}

impl clap::FromArgMatches for ObsidianArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            obsidian_wiki_links: m.get_flag("obsidian_wiki_links"),
            obsidian_tags: extract::opt_many::<String>(m, "obsidian_tags"),
            obsidian_relative_assets: m.get_flag("obsidian_relative_assets"),
            vault: extract::opt(m, "vault"),
            quick_save: m.get_flag("quick_save"),
            obsidian_rich_metadata: m.get_flag("obsidian_rich_metadata"),
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for ObsidianArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::obsidian_args(
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
    //! [`ObsidianArgs`] must stay in lockstep with the OptionsSpec Obsidian
    //! group. Written FIRST against the (post-migration) spec-built command
    //! so any future spec drift fails here first.
    use super::*;
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `ObsidianArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        ObsidianArgs::augment_args(clap::Command::new("webfang-obsidian"))
            .get_arguments()
            .cloned()
            .collect()
    }

    fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from ObsidianArgs command"))
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
                spec::obsidian::GROUP.iter().any(|s| s.id == arg.get_id()),
                "clap arg `{}` has no OptionsSpec entry — spec is out of sync",
                arg.get_id()
            );
        }
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        let args = command_args();
        for s in spec::obsidian::GROUP {
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
        for s in spec::obsidian::GROUP {
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
        for s in spec::obsidian::GROUP {
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
        // Defaults (hermetic: ambient WEBFANG_OBSIDIAN_* must not leak).
        let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
        assert!(!defaults.obsidian.obsidian_wiki_links);
        assert!(defaults.obsidian.obsidian_tags.is_none());
        assert!(!defaults.obsidian.obsidian_relative_assets);
        assert!(defaults.obsidian.vault.is_none());
        assert!(!defaults.obsidian.quick_save);
        assert!(!defaults.obsidian.obsidian_rich_metadata);

        // Explicit values: bool flags + the comma-delimited `obsidian_tags`
        // (single invocation) + the path-valued `vault`.
        let parsed = parse_args(&[
            "--obsidian-wiki-links",
            "--obsidian-tags",
            "rust,cargo,docs",
            "--obsidian-relative-assets",
            "--vault",
            "/tmp/vault",
            "--quick-save",
            "--obsidian-rich-metadata",
        ])
        .expect("representative obsidian flags must parse");
        assert!(parsed.obsidian.obsidian_wiki_links);
        assert_eq!(
            parsed.obsidian.obsidian_tags,
            Some(vec!["rust".to_string(), "cargo".to_string(), "docs".to_string()])
        );
        assert!(parsed.obsidian.obsidian_relative_assets);
        assert_eq!(
            parsed.obsidian.vault,
            Some(std::path::PathBuf::from("/tmp/vault"))
        );
        assert!(parsed.obsidian.quick_save);
        assert!(parsed.obsidian.obsidian_rich_metadata);
    }

    /// Slice 5a pin: structural clap surface the spec-driven builder must
    /// reproduce byte-for-byte. `obsidian_tags` is the first entry that
    /// uses `value_delimiter = ','`, so its delimiter must round-trip
    /// through the spec.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        let args = command_args();
        for s in spec::obsidian::GROUP {
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
