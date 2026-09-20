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
    use crate::cli::args::test_support::{
        assert_defaults, assert_help, assert_long_short_alias_env_heading, assert_structural,
        assert_surface_covered, collect_args, parse_args_hermetic,
    };
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `ObsidianArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        collect_args(ObsidianArgs::augment_args(clap::Command::new(
            "webfang-obsidian",
        )))
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        assert_surface_covered(&command_args(), spec::obsidian::GROUP);
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        assert_long_short_alias_env_heading(&command_args(), spec::obsidian::GROUP);
    }

    #[test]
    fn defaults_match_the_spec() {
        assert_defaults(&command_args(), spec::obsidian::GROUP);
    }

    #[test]
    fn help_text_matches_the_spec() {
        assert_help(&command_args(), spec::obsidian::GROUP);
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
        let parsed = parse_args_hermetic(&[
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
            Some(vec![
                "rust".to_string(),
                "cargo".to_string(),
                "docs".to_string()
            ])
        );
        assert!(parsed.obsidian.obsidian_relative_assets);
        assert_eq!(
            parsed.obsidian.vault,
            Some(std::path::PathBuf::from("/tmp/vault"))
        );
        assert!(parsed.obsidian.quick_save);
        assert!(parsed.obsidian.obsidian_rich_metadata);
    }

    /// End-to-end coverage for `ValueKind::TextList` (`obsidian_tags`).
    /// The structural pin (line ~221) only asserts the action is `Append`,
    /// not that the append actually appends: a future contributor who
    /// correctly sets the action but breaks the value-name binding or
    /// silently drops the `value_delimiter` would slip through. This test
    /// closes that class of gap (the same one that allowed the original
    /// `Set` vs `Append` bug to escape the spec↔clap self-compare).
    #[test]
    fn text_list_repeated_invocations_append_end_to_end() {
        // Two repeated single-token invocations: each occurrence must
        // append, not overwrite. Under the broken `Set` action this would
        // collapse to `Some(vec!["b".to_string()])`.
        let parsed = parse_args_hermetic(&["--obsidian-tags", "a", "--obsidian-tags", "b"])
            .expect("repeated `--obsidian-tags` must parse");
        assert_eq!(
            parsed.obsidian.obsidian_tags,
            Some(vec!["a".to_string(), "b".to_string()]),
            "ArgAction::Append must keep both occurrences"
        );

        // Three repeated invocations, all single tokens.
        let parsed = parse_args_hermetic(&[
            "--obsidian-tags",
            "a",
            "--obsidian-tags",
            "b",
            "--obsidian-tags",
            "c",
        ])
        .expect("three repeated `--obsidian-tags` must parse");
        assert_eq!(
            parsed.obsidian.obsidian_tags,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );

        // Single comma-delimited invocation: the `value_delimiter = ','`
        // splits it. (This is the path the existing
        // `representative_values_parse_identically_through_clap` covers;
        // included here so the test fails loudly if either side of the
        // append/delimiter contract drifts.)
        let parsed = parse_args_hermetic(&["--obsidian-tags", "x,y,z"])
            .expect("comma-delimited `--obsidian-tags` must parse");
        assert_eq!(
            parsed.obsidian.obsidian_tags,
            Some(vec!["x".to_string(), "y".to_string(), "z".to_string()])
        );
    }

    /// Slice 5a pin: structural clap surface the spec-driven builder must
    /// reproduce byte-for-byte. `obsidian_tags` is the first entry that
    /// uses `value_delimiter = ','`, so its delimiter must round-trip
    /// through the spec.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        assert_structural(&command_args(), spec::obsidian::GROUP);
    }
}
