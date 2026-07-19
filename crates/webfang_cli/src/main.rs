//! WebFang - Production-ready web scraper with Clean Architecture
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

use webfang_core::cli::orchestrator;

use std::env;
use std::io::{self, IsTerminal};
use std::panic;

use clap::Parser;
#[cfg(feature = "ui")]
use inquire::Text;
#[cfg(feature = "ai")]
use std::sync::Arc;
#[cfg(feature = "ai")]
use webfang_ai::{ModelConfig, SemanticCleanerImpl};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::config::ConfigDefaults;
use webfang_core::cli::error::CliExit;
use webfang_core::cli::preflight;
#[cfg(feature = "ai")]
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
use webfang_core::{init_logging_dual, is_no_color, Args, Commands};
#[cfg(feature = "ui")]
use webfang_tui::tui::modal::HelpModal;
#[cfg(feature = "ui")]
use webfang_tui::tui::{App, AppMode, AppResult, CollapsibleConfig, Header, StatusBar};

/// Check if running in CI environment.
fn is_ci() -> bool {
    env::var("CI").is_ok()
}

/// Check if stdin is a terminal.
fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

/// Run the unified TUI with collapsible config sections.
///
/// Phase 1: Config form with 8 collapsible sections (39 fields)
/// Phase 2: URL selector (after config submitted)
///
/// Returns `Ok(Some(values))` if both phases completed,
/// `Ok(None)` if cancelled at any point, or `Err` if TTY not available.
#[cfg(feature = "ui")]
async fn run_unified_tui() -> Result<Option<serde_json::Value>, CliExit> {
    // Check if stdout is a TTY
    if !io::stdout().is_terminal() {
        eprintln!("Error: --tui requiere un terminal interactivo");
        return Err(CliExit::UsageError(
            "--tui requiere un terminal interactivo".into(),
        ));
    }

    // =========================================================================
    // Phase 1: Configuration Form
    // =========================================================================
    let mut config_app = match App::new(AppMode::Config) {
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
    .with_component(CollapsibleConfig::new())
    .with_component(StatusBar::new().with_items(vec![
        ("↑↓", "Navegar"),
        ("Enter", "Expandir"),
        ("←", "Colapsar"),
        ("Ctrl+S", "Enviar"),
        ("q", "Salir"),
    ]))
    .with_modal(HelpModal::new(
        "Ayuda — Configuración".into(),
        vec![
            ("↑↓".into(), "Navegar secciones".into()),
            ("Enter/→".into(), "Expandir sección".into()),
            ("←".into(), "Colapsar sección".into()),
            ("Space".into(), "Toggle expand/collapse".into()),
            ("Tab".into(), "Mover a campos".into()),
            ("Ctrl+S".into(), "Enviar formulario".into()),
            ("?".into(), "Mostrar ayuda".into()),
            ("q".into(), "Salir".into()),
        ],
    ));

    let config_values = match config_app.run().await {
        Ok(AppResult::Config(values)) => values,
        Ok(AppResult::None) => return Ok(None), // User cancelled
        Ok(_) => return Ok(None),
        Err(e) => {
            eprintln!("Error en TUI de configuración: {}", e);
            return Ok(None);
        },
    };

    // If config was cancelled or empty, return None
    let config_values = match config_values {
        Some(v) => v,
        None => return Ok(None),
    };

    // =========================================================================
    // Phase 2: URL Selection (using config values)
    // =========================================================================
    // The URL will be extracted from config and used for discovery.
    // For now, return the config values. The orchestrator will handle
    // URL discovery and selection based on the config.
    Ok(Some(config_values))
}

/// Prompt for URL using inquire (interactive mode).
#[cfg(feature = "ui")]
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
pub async fn main() -> CliExit {
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
            // clap returns DisplayHelp/DisplayVersion for --help/--version
            // These are NOT errors — print and exit 0
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                e.print().ok();
                return CliExit::Success;
            }
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
    // 3. Unified TUI mode (if --tui flag is set)
    // =========================================================================
    #[cfg(feature = "ui")]
    {
        if args.tui {
            // Run unified TUI: config form → URL selector → scraping
            let tui_result = run_unified_tui().await;
            match tui_result {
                Ok(Some(config_values)) => {
                    // Apply TUI config values to args (overrides CLI values)
                    args = preflight::apply_tui_config_args(args, &config_values);
                    println!("Config applied from TUI.");
                },
                Ok(None) => {
                    println!("TUI cancelled.");
                    return CliExit::Success;
                },
                Err(e) => {
                    return e;
                },
            }
        } else if args.config_tui {
            // [DEPRECATED] Legacy config TUI — redirects to unified TUI
            eprintln!("Warning: --config-tui is deprecated, use --tui instead");
            let tui_result = run_unified_tui().await;
            match tui_result {
                Ok(Some(config_values)) => {
                    args = preflight::apply_tui_config_args(args, &config_values);
                },
                Ok(None) => return CliExit::Success,
                Err(e) => return e,
            }
        } else if args.interactive {
            // [DEPRECATED] Legacy interactive — redirects to unified TUI
            eprintln!("Warning: --interactive is deprecated, use --tui instead");
            let tui_result = run_unified_tui().await;
            match tui_result {
                Ok(Some(config_values)) => {
                    args = preflight::apply_tui_config_args(args, &config_values);
                },
                Ok(None) => return CliExit::Success,
                Err(e) => return e,
            }
        }
    }
    // When `ui` is OFF, any TUI flag triggers a graceful Spanish error (spec S2.2).
    #[cfg(not(feature = "ui"))]
    if args.tui || args.config_tui || args.interactive {
        eprintln!("TUI no disponible: compilar con --features ui");
        return CliExit::UsageError("TUI no disponible: compilar con --features ui".into());
    }

    // =========================================================================
    // 4. URL handling with interactive wizard
    // =========================================================================

    // Batch mode reads URLs from stdin/file — --url is not required
    let is_batch = args.batch || args.batch_file.is_some();

    // If no URL provided, check for interactive mode
    if args.url.is_none() && !is_batch {
        // CI environment always requires --url
        if is_ci() {
            eprintln!("Error: --url is required for scraping (CI mode)");
            return CliExit::UsageError("--url is required".into());
        }

        // Try interactive prompt only if stdin is a TTY
        if stdin_is_tty() {
            #[cfg(feature = "ui")]
            {
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
            }
            #[cfg(not(feature = "ui"))]
            {
                // No inquire prompt available in headless builds — require --url explicitly.
                eprintln!(
                    "Error: --url is required for scraping (interactive prompt requires --features ui)"
                );
                return CliExit::UsageError("--url is required".into());
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
        .join("webfang")
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
    // 6. Extract trace_file and ai_model before args is moved into CrawlOptions
    // =========================================================================
    let trace_file = args.trace_file.take();
    #[cfg(feature = "ai")]
    let ai_model_arg = args.ai_model.take();

    // =========================================================================
    // 6b. Convert Args → CrawlOptions and apply config file defaults
    // =========================================================================
    let opts = CrawlOptions::from(args);
    let opts = preflight::apply_config_defaults(opts, &config_defaults);

    // =========================================================================
    // 7. Initialize logging (stderr-only, respects quiet + NO_COLOR)
    // =========================================================================
    let no_color = is_no_color();
    // L3 FIX: Map verbose count to tracing levels (0=WARN, 1=INFO, 2=DEBUG, 3+=TRACE)
    let log_level = match opts.verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    // Create FileTraceLayer when --trace-file is present (always available, no feature gate)
    let file_trace_layer = trace_file.and_then(|path| {
        match webfang_core::infrastructure::observability::FileTraceLayer::new(path) {
            Ok(layer) => Some(layer),
            Err(e) => {
                eprintln!("Error: no se pudo crear archivo de trazas: {e}");
                None
            },
        }
    });

    // OpenTelemetry tracing + metrics (feature-gated)
    #[cfg(feature = "otel-metrics")]
    let _otel_guard = {
        let config = webfang_core::infrastructure::observability::otel::OtelConfig::from_env();
        match webfang_core::infrastructure::observability::otel::init_otel_metrics(config) {
            Ok((_meter, guard, layer)) => {
                init_logging_dual(
                    log_level,
                    opts.export.quiet,
                    no_color,
                    file_trace_layer,
                    Some(layer),
                );
                Some(guard)
            },
            Err(e) => {
                eprintln!("Warning: OTel metrics init failed: {e}");
                init_logging_dual(
                    log_level,
                    opts.export.quiet,
                    no_color,
                    file_trace_layer,
                    None,
                );
                None
            },
        }
    };
    #[cfg(all(feature = "otel", not(feature = "otel-metrics")))]
    let _otel_guard = {
        let config = webfang_core::infrastructure::observability::otel::OtelConfig::from_env();
        match webfang_core::infrastructure::observability::otel::init_otel_tracing(config) {
            Ok((guard, layer)) => {
                init_logging_dual(
                    log_level,
                    opts.export.quiet,
                    no_color,
                    file_trace_layer,
                    Some(layer),
                );
                Some(guard)
            },
            Err(e) => {
                eprintln!("Warning: OTel tracing init failed: {e}");
                init_logging_dual(
                    log_level,
                    opts.export.quiet,
                    no_color,
                    file_trace_layer,
                    None,
                );
                None
            },
        }
    };
    #[cfg(not(feature = "otel"))]
    #[allow(clippy::let_unit_value)]
    let _guard = init_logging_dual(log_level, opts.export.quiet, no_color, file_trace_layer);

    // =========================================================================
    // 8. Delegate to orchestrator
    // =========================================================================
    #[cfg(feature = "ai")]
    let result = {
        let ai_cleaner = if opts.ai {
            // Resolve model variant: CLI flag takes precedence over AI_MODEL_ID env var
            let model_variant = match &ai_model_arg {
                Some(model_str) => match model_str.parse::<webfang_ai::AiModel>() {
                    Ok(variant) => variant,
                    Err(e) => {
                        tracing::warn!("Parsing error para --ai-model: {}", e);
                        webfang_ai::AiModel::from_env_or_default()
                    },
                },
                None => webfang_ai::AiModel::from_env_or_default(),
            };

            match SemanticCleanerImpl::new(
                ModelConfig::default()
                    .with_model_variant(model_variant)
                    .with_relevance_threshold(0.3)
                    .with_max_tokens(32768)
                    .with_offline_mode(false),
            )
            .await
            {
                Ok(cleaner) => Some(Arc::new(cleaner) as Arc<dyn SemanticCleaner>),
                Err(e) => {
                    tracing::warn!("No se pudo inicializar el limpiador semántico AI: {e}");
                    None
                },
            }
        } else {
            None
        };
        orchestrator::run(opts, ai_cleaner).await
    };
    #[cfg(not(feature = "ai"))]
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
