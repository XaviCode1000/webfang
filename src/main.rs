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

use rust_scraper::cli::orchestrator;

use std::env;
use std::io::{self, IsTerminal};
use std::panic;

use clap::Parser;
use inquire::Text;
use rust_scraper::adapters::tui::modal::HelpModal;
use rust_scraper::adapters::tui::{App, AppMode, AppResult, ConfigFormState, Header, StatusBar};
use rust_scraper::application::crawl_options::CrawlOptions;
use rust_scraper::cli::config::ConfigDefaults;
use rust_scraper::cli::error::CliExit;
use rust_scraper::cli::preflight;
use rust_scraper::{init_logging_dual, is_no_color, Args, Commands};

/// Check if running in CI environment.
fn is_ci() -> bool {
    env::var("CI").is_ok()
}

/// Check if stdin is a terminal.
fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

/// Run the configuration TUI using the App + Component architecture.
///
/// Returns `Ok(Some(values))` if form was submitted,
/// `Ok(None)` if cancelled, or `Err` if TTY not available.
async fn run_config_tui() -> Result<Option<serde_json::Value>, CliExit> {
    // Check if stdout is a TTY
    if !io::stdout().is_terminal() {
        eprintln!("Error: --config-tui requiere un terminal interactivo");
        return Err(CliExit::UsageError(
            "--config-tui requiere un terminal interactivo".into(),
        ));
    }

    let mut app = match App::new(AppMode::Config) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("Error al crear la aplicación TUI: {}", e);
            return Err(CliExit::UsageError(format!(
                "Error creando la aplicación: {}",
                e
            )));
        },
    }
    .with_component(Header::new(AppMode::Config))
    .with_component(ConfigFormState::new_default())
    .with_component(StatusBar::new().with_items(vec![
        ("↑↓", "Navegar"),
        ("Enter", "Confirmar"),
        ("q", "Salir"),
    ]))
    .with_modal(HelpModal::new(
        "Ayuda — Configuración".into(),
        vec![
            ("↑↓".into(), "Navegar campos".into()),
            ("Enter".into(), "Editar campo / Confirmar".into()),
            ("?".into(), "Mostrar ayuda".into()),
            ("q".into(), "Salir".into()),
        ],
    ));

    match app.run().await {
        Ok(AppResult::Config(values)) => Ok(values),
        Ok(AppResult::None) => Ok(None),
        Ok(_) => {
            // En modo Config no deberían llegar otros resultados
            Ok(None)
        },
        Err(e) => {
            eprintln!("Error en TUI de configuración: {}", e);
            Ok(None)
        },
    }
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
    // Suppress OTel background thread panics during Tokio runtime shutdown.
    // The BatchSpanProcessor and PeriodicReader threads panic when the reactor
    // drops before they finish — this is a known SDK limitation, not our bug.
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unknown")
            .to_string();
        if thread_name.starts_with("OpenTelemetry.") {
            eprintln!("Warning: OTel background thread '{thread_name}' panicked during shutdown (safe to ignore)");
            return;
        }
        default_hook(info);
    }));

    // tokio-console: usa 'cargo install tokio-console' y corre en otra terminal
    // El runtime con tokio[unstable] ya expone el endpoint automaticamente
    __main().await
}

async fn __main() -> CliExit {
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
        let config_result = run_config_tui().await;
        match config_result {
            Ok(Some(config_values)) => {
                // Apply TUI config values to args (overrides CLI values)
                // TUI operates on Args because it needs config_tui/url fields
                args = preflight::apply_tui_config_args(args, &config_values);
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
        .join("rust_scraper")
        .join("config.toml");
    let config_defaults = ConfigDefaults::load(&config_path);

    // =========================================================================
    // 5b. Validate URL before conversion (CrawlOptions::from panics on invalid URL)
    // =========================================================================
    if let Some(ref url_str) = args.url {
        if url::Url::parse(url_str).is_err() {
            return CliExit::UsageError(format!("Invalid URL: {url_str}"));
        }
    }

    // =========================================================================
    // 6. Extract trace_file before args is moved into CrawlOptions
    // =========================================================================
    #[cfg(feature = "otel")]
    let trace_file = args.trace_file.take();

    // =========================================================================
    // 6b. Convert Args → CrawlOptions and apply config file defaults
    // =========================================================================
    let opts = CrawlOptions::from(args);
    let opts = preflight::apply_config_defaults(opts, &config_defaults);

    // =========================================================================
    // 7. Initialize logging (stderr-only, respects quiet + NO_COLOR)
    // =========================================================================
    let no_color = is_no_color();
    let log_level = match opts.verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    // OpenTelemetry tracing + metrics (feature-gated)
    #[cfg(feature = "otel-metrics")]
    let _otel_guard = {
        let mut config = rust_scraper::infrastructure::observability::otel::OtelConfig::from_env();
        if let Some(path) = trace_file {
            config = config.with_trace_file(path);
        }
        match rust_scraper::infrastructure::observability::otel::init_otel_metrics(config) {
            Ok((_meter, guard, layer)) => {
                init_logging_dual(log_level, opts.export.quiet, no_color, Some(layer));
                Some(guard)
            },
            Err(e) => {
                eprintln!("Warning: OTel metrics init failed: {e}");
                init_logging_dual(log_level, opts.export.quiet, no_color, None);
                None
            },
        }
    };
    #[cfg(all(feature = "otel", not(feature = "otel-metrics")))]
    let _otel_guard = {
        let mut config = rust_scraper::infrastructure::observability::otel::OtelConfig::from_env();
        if let Some(path) = trace_file {
            config = config.with_trace_file(path);
        }
        match rust_scraper::infrastructure::observability::otel::init_otel_tracing(config) {
            Ok((guard, layer)) => {
                init_logging_dual(log_level, opts.export.quiet, no_color, Some(layer));
                Some(guard)
            },
            Err(e) => {
                eprintln!("Warning: OTel tracing init failed: {e}");
                init_logging_dual(log_level, opts.export.quiet, no_color, None);
                None
            },
        }
    };
    #[cfg(not(feature = "otel"))]
    #[allow(clippy::let_unit_value)]
    let _guard = init_logging_dual(log_level, opts.export.quiet, no_color);

    // =========================================================================
    // 8. Delegate to orchestrator
    // =========================================================================
    let result = orchestrator::run(opts).await;

    // Flush OpenTelemetry while the Tokio runtime is still alive.
    // The batch processor and periodic reader tasks need a live reactor
    // to drain their buffers — if we rely on Drop, the runtime may already
    // be gone, causing "there is no reactor running" panics.
    #[cfg(feature = "otel-metrics")]
    if let Some(ref guard) = _otel_guard {
        guard.flush().await;
    }
    #[cfg(all(feature = "otel", not(feature = "otel-metrics")))]
    if let Some(ref guard) = _otel_guard {
        guard.flush().await;
    }

    result
}
