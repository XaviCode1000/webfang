//! Rust Scraper - Production-ready web scraper with Clean Architecture
//!
//! Extracts clean, structured content from web pages using readability algorithm.
//!
//! # Architecture
//!
//! Following Clean Architecture with TUI support:
//!
//! ```text
//! main.rs (thin entry point) -> orchestrator::run()
//!     │
//!     ├─→ Args::try_parse()           ← CLI parsing
//!     ├─→ handle_completions()        ← Subcommand handling
//!     ├─→ ConfigDefaults::load()      ← TOML config
//!     ├─→ preflight::apply_config_defaults() ← Config merge
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     └─→ orchestrator::run()         ← Full pipeline
//! ```
//!
//! **Golden Rule:** Application layer NEVER imports ratatui/crossterm/indicatif.

mod export_flow;
mod orchestrator;
mod preflight;

use clap::Parser;
use rust_scraper::cli::config::ConfigDefaults;
use rust_scraper::cli::error::CliExit;
use rust_scraper::{init_logging_dual, is_no_color, Args, Commands};

#[tokio::main]
async fn main() -> CliExit {
    // =========================================================================
    // 1. Parse CLI arguments
    // =========================================================================
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{}", e);
            return CliExit::UsageError("invalid arguments".into());
        },
    };

    // =========================================================================
    // 2. Handle subcommands (completions)
    // =========================================================================
    if let Some(Commands::Completions { shell }) = args.subcommand {
        return orchestrator::handle_completions(shell);
    }

    // =========================================================================
    // 3. URL is required for scraping
    // =========================================================================
    if args.url.is_none() {
        eprintln!("Error: --url is required for scraping");
        return CliExit::UsageError("--url is required".into());
    }

    // =========================================================================
    // 4. Load config file (graceful: missing file = defaults)
    // =========================================================================
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rust-scraper")
        .join("config.toml");
    let config_defaults = ConfigDefaults::load(&config_path);

    // =========================================================================
    // 5. Apply config file defaults where CLI args are at default values
    // =========================================================================
    let args = preflight::apply_config_defaults(args, &config_defaults);

    // =========================================================================
    // 6. Initialize logging (stderr-only, respects quiet + NO_COLOR)
    // =========================================================================
    let no_color = is_no_color();
    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    init_logging_dual(log_level, args.quiet, no_color);

    // =========================================================================
    // 7. Delegate to orchestrator
    // =========================================================================
    orchestrator::run(args).await
}
