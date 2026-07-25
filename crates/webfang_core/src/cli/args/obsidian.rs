use clap::Args;

/// Obsidian vault integration arguments.
#[derive(Args, Debug, Default)]
pub struct ObsidianArgs {
    /// Convert same-domain links to Obsidian [[wiki-link]] syntax
    #[arg(long, default_value = "false", env = "WEBFANG_OBSIDIAN_WIKI_LINKS")]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_wiki_links: bool,

    /// Tags to include in YAML frontmatter (comma-separated)
    #[arg(long, env = "WEBFANG_OBSIDIAN_TAGS", value_delimiter = ',')]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_tags: Option<Vec<String>>,

    /// Rewrite downloaded asset paths as relative to the .md file
    #[arg(
        long,
        default_value = "false",
        env = "WEBFANG_OBSIDIAN_RELATIVE_ASSETS"
    )]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_relative_assets: bool,

    /// Path to Obsidian vault (auto-detects if not provided)
    #[arg(long, env = "WEBFANG_OBSIDIAN_VAULT")]
    #[clap(next_help_heading = "Obsidian")]
    pub vault: Option<std::path::PathBuf>,

    /// Quick-save mode: save directly to vault _inbox folder
    #[arg(long, default_value = "false", env = "WEBFANG_OBSIDIAN_QUICK_SAVE")]
    #[clap(next_help_heading = "Obsidian")]
    pub quick_save: bool,

    /// Add rich metadata to frontmatter
    #[arg(long, default_value = "false", env = "WEBFANG_OBSIDIAN_RICH_METADATA")]
    #[clap(next_help_heading = "Obsidian")]
    pub obsidian_rich_metadata: bool,
}
