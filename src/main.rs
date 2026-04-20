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
//!     |
//!     ├─→ Args::try_parse()           ← CLI parsing
//!     ├─→ handle_completions()        ← Subcommand handling
//!     ├─→ run_config_tui()             ← Config TUI (if --config-tui)
//!     ├─→ ConfigDefaults::load()      ← TOML config
//!     ├─→ preflight::apply_config_defaults() ← Config merge
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     └─→ orchestrator::run()         ← Full pipeline
//! ```
//!
//! **Golden Rule:** Application layer NEVER imports ratatui/crossterm/indicatif.

mod orchestrator;

use std::env;
use std::io::{self, IsTerminal};

use clap::Parser;
use crossterm::event;
use inquire::Text;
use rust_scraper::adapters::tui::{restore_terminal, setup_terminal, ConfigFormState};
use rust_scraper::cli::config::ConfigDefaults;
use rust_scraper::cli::error::CliExit;
use rust_scraper::{init_logging_dual, is_no_color, Args, Commands};
use rust_scraper::cli::preflight;

/// Check if running in CI environment.
fn is_ci() -> bool {
    env::var("CI").is_ok()
}

/// Check if stdin is a terminal.
fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

/// Run the configuration TUI and return the submitted config values.
///
/// Returns `Ok(Some(values))` if form was submitted,
/// `Ok(None)` if cancelled, or `Err` if TTY not available.
fn run_config_tui() -> Result<Option<serde_json::Value>, CliExit> {
    // Check if stdout is a TTY
    if !io::stdout().is_terminal() {
        eprintln!("Error: --config-tui requires a terminal");
        return Err(CliExit::UsageError(
            "--config-tui requires interactive terminal".into(),
        ));
    }

    // Setup terminal
    let mut terminal = match setup_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: Failed to setup terminal: {}", e);
            return Err(CliExit::UsageError(format!(
                "Terminal setup failed: {}",
                e
            )));
        },
    };

    // Create config form state
    let mut config_state = ConfigFormState::new_default();

    // Run the event loop for the config form
    let result = loop {
        // Render the form
        let _ = terminal.draw(|f| {
            config_state.render(f, f.area());
        });

        // Check if we're done
        if config_state.is_done() {
            break if config_state.submitted {
                Some(config_state.data())
            } else {
                None
            };
        }

        // Poll for events with timeout
        if let Ok(true) = event::poll(std::time::Duration::from_millis(50)) {
            if let Ok(event::Event::Key(key)) = event::read() {
                config_state.handle_input(key);
            }
        }
    };

    // Restore terminal
    let _ = restore_terminal();

    Ok(result)
}

/// Prompt for URL using inquire (interactive mode).
fn prompt_for_url() -> Result<String, CliExit> {
    use inquire::validator::Validation;

    Text::new("Enter the URL to scrape:")
        .with_help_message("Example: https://example.com")
        .with_validator(|input: &str| {
            if input.is_empty() {
                Err("URL cannot be empty".into())
            } else if !input.starts_with("http://") && !input.starts_with("https://") {
                Err("URL must start with http:// or https://".into())
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()
        .map_err(|e| {
            eprintln!("Error prompting for URL: {}", e);
            CliExit::UsageError("interactive prompt failed".into())
        })
}

#[tokio::main]
async fn main() -> CliExit {
    // =========================================================================
    // 1. Parse CLI arguments
    // =========================================================================
    let mut args = match Args::try_parse() {
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
    // 3. Config TUI (if --config-tui flag is set)
    // =========================================================================
    if args.config_tui {
        // Run config TUI and get submitted values
        let config_result = run_config_tui();
        match config_result {
            Ok(Some(config_values)) => {
                // Apply TUI config values to args (overrides CLI values)
                args = preflight::apply_tui_config(args, &config_values);
                println!("Config applied from TUI form.");
            },
            Ok(None) => {
                println!("Config TUI cancelled.");
                return CliExit::Success;
            },
            Err(e) => {
                // Error already printed in run_config_tui
                return e;
            },
        }
    }

    // =========================================================================
    // 4. URL handling with interactive wizard
    // =========================================================================

    // If no URL provided, check for interactive mode
    if args.url.is_none() {
        // CI environment always requires --url
        if is_ci() {
            eprintln!("Error: --url is required for scraping (CI mode)");
            return CliExit::UsageError("--url is required".into());
        }

        // Try interactive prompt only if stdin is a TTY
        if stdin_is_tty() {
            match prompt_for_url() {
                Ok(url) => {
                    args.url = Some(url);
                },
                Err(_e) => {
                    // Prompt failed (e.g., non-interactive), fall through to error
                    eprintln!("Error: --url is required for scraping");
                    return CliExit::UsageError("--url is required".into());
                },
            }
        } else {
            // Not a TTY and no URL provided
            eprintln!("Error: --url is required for scraping");
            return CliExit::UsageError("--url is required".into());
        }
    }

    // =========================================================================
    // 5. Load config file (graceful: missing file = defaults)
    // =========================================================================
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rust-scraper")
        .join("config.toml");
    let config_defaults = ConfigDefaults::load(&config_path);

    // =========================================================================
    // 6. Apply config file defaults where CLI args are at default values
    // =========================================================================
    let args = preflight::apply_config_defaults(args, &config_defaults);

    // =========================================================================
    // 7. Initialize logging (stderr-only, respects quiet + NO_COLOR)
    // =========================================================================
    let no_color = is_no_color();
    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    init_logging_dual(log_level, args.quiet, no_color);

    // =========================================================================
    // 8. Delegate to orchestrator
    // =========================================================================
    orchestrator::run(args).await
}
