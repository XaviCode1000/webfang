use clap::Args;

/// Terminal UI configuration arguments.
#[derive(Args, Debug, Default)]
pub struct TuiArgs {
    /// Unified TUI mode: config form (collapsible sections) → URL selector → scraping
    #[arg(long, env = "WEBFANG_TUI", help_heading = "Behavior")]
    pub tui: bool,
}
