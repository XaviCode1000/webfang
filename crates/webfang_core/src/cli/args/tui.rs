use clap::Args;

/// Terminal UI configuration arguments.
#[derive(Args, Debug, Default)]
pub struct TuiArgs {
    /// Unified TUI mode: config form (collapsible sections) → URL selector → scraping
    #[arg(long, env = "WEBFANG_TUI")]
    #[clap(next_help_heading = "Behavior")]
    pub tui: bool,

    /// `[DEPRECATED]` Use --tui instead. Interactive mode with TUI URL selector
    #[arg(long, env = "WEBFANG_INTERACTIVE", hide = true)]
    #[clap(next_help_heading = "Behavior")]
    pub interactive: bool,

    /// `[DEPRECATED]` Use --tui instead. Open configuration TUI
    #[arg(long, env = "WEBFANG_CONFIG_TUI", hide = true)]
    #[clap(next_help_heading = "Behavior")]
    pub config_tui: bool,
}
