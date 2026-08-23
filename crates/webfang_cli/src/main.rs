#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
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
//!     ├─→ preflight::normalize()      ← Config merge
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     └─→ orchestrator::run()         ← Full pipeline
//! ```
//!
//! **Golden Rule:** Application layer NEVER imports ratatui/crossterm/indicatif.

use webfang_core::cli::orchestrator;

use std::env;
use std::io::{self, IsTerminal};
use std::panic;

#[cfg(feature = "ui")]
use inquire::Text;
#[cfg(any(feature = "ai", feature = "adaptive-selectors"))]
use std::sync::Arc;
#[cfg(feature = "ai")]
use webfang_ai::{ModelConfig, SemanticCleanerImpl, SemanticError};
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::{AdaptiveSelectorEngine, AdaptiveSelectorOptions};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::config::ConfigDefaults;
use webfang_core::cli::error::CliExit;
use webfang_core::cli::preflight;
use webfang_core::cli::preflight::{ArgSources, TuiOverrides};
#[cfg(feature = "ai")]
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;
use webfang_core::{init_logging_dual, is_no_color, Args, Commands};
#[cfg(feature = "ui")]
use webfang_tui::tui::modal::HelpModal;
#[cfg(feature = "ui")]
use webfang_tui::tui::{
    run_selector, App, AppMode, AppResult, CollapsibleConfig, Header, StatusBar,
};

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
        tracing::error!("--tui requires an interactive terminal");
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
            tracing::error!(error = %e, "Failed to create TUI application");
            return Err(CliExit::UsageError(format!(
                "Error creando la aplicación: {e}"
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
            tracing::error!(error = %e, "Error in configuration TUI");
            return Ok(None);
        },
    };

    // If config was cancelled or empty, return None
    let config_values = match config_values {
        Some(v) => v,
        None => return Ok(None),
    };

    // =========================================================================
    // Phase 2: Discovery + URL Selection
    // =========================================================================
    let selected_urls = run_url_selection_phase(&config_values).await?;

    let mut combined = config_values;
    combined["selected_urls"] = serde_json::to_value(&selected_urls)
        .map_err(|e| CliExit::UsageError(format!("Error serializando URLs: {e}")))?;
    Ok(Some(combined))
}

/// Run Phase 2: discover URLs from seed and run interactive selector.
#[cfg(feature = "ui")]
async fn run_url_selection_phase(
    config_values: &serde_json::Value,
) -> Result<Vec<url::Url>, CliExit> {
    use webfang_core::application::crawler::discovery::discover_urls_for_tui;

    let seed_url_str = config_values
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            tracing::error!("Missing base URL in TUI configuration");
            CliExit::UsageError("URL base es obligatoria en la TUI".into())
        })?;

    let seed_url = url::Url::parse(seed_url_str).map_err(|e| {
        tracing::error!(error = %e, "Invalid base URL");
        CliExit::UsageError(format!("URL base inválida: {e}"))
    })?;

    let crawler_config = build_crawler_config_from_json(seed_url, config_values);

    tracing::info!(url = %seed_url_str, "Starting TUI discovery phase");
    let discovered = discover_urls_for_tui(seed_url_str, &crawler_config)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "URL discovery failed");
            CliExit::UsageError(format!("Fallo en descubrimiento: {e}"))
        })?;

    if discovered.is_empty() {
        tracing::warn!("No URLs discovered");
        return Err(CliExit::UsageError(
            "No se encontraron URLs. Revise la URL base o la configuración.".into(),
        ));
    }

    let selected = run_selector(&discovered).await.map_err(|e| {
        tracing::error!(error = %e, "Error in URL selector");
        CliExit::UsageError(format!("Error en selector: {e}"))
    })?;

    if selected.is_empty() {
        return Err(CliExit::UsageError(
            "Selección cancelada por el usuario".into(),
        ));
    }

    Ok(selected)
}

/// Build CrawlerConfig from TUI JSON config (manual bridge — CrawlerConfig has no Deserialize).
#[cfg(feature = "ui")]
fn build_crawler_config_from_json(
    seed_url: url::Url,
    config_values: &serde_json::Value,
) -> webfang_core::domain::CrawlerConfig {
    use webfang_core::domain::CrawlerConfig;

    let mut builder = CrawlerConfig::builder(seed_url);

    if let Some(d) = config_values.get("max_depth").and_then(|v| v.as_u64()) {
        builder = builder.max_depth(d as u8);
    }
    if let Some(p) = config_values.get("max_pages").and_then(|v| v.as_u64()) {
        builder = builder.max_pages(p as usize);
    }
    if let Some(c) = config_values.get("concurrency").and_then(|v| v.as_u64()) {
        builder = builder.concurrency(c as usize);
    }
    if let Some(d) = config_values.get("delay_ms").and_then(|v| v.as_u64()) {
        builder = builder.delay_ms(d);
    }
    if let Some(t) = config_values.get("timeout_secs").and_then(|v| v.as_u64()) {
        builder = builder.timeout_secs(t);
    }
    if let Some(ua) = config_values.get("user_agent").and_then(|v| v.as_str()) {
        builder = builder.user_agent(ua);
    }
    if let Some(sm) = config_values.get("use_sitemap").and_then(|v| v.as_bool()) {
        builder = builder.use_sitemap(sm);
    }
    if let Some(sm_url) = config_values.get("sitemap_url").and_then(|v| v.as_str()) {
        if !sm_url.is_empty() {
            builder = builder.sitemap_url(sm_url);
        }
    }
    if let Some(ir) = config_values.get("ignore_robots").and_then(|v| v.as_bool()) {
        builder = builder.ignore_robots(ir);
    }
    if let Some(inc) = config_values.get("include").and_then(|v| v.as_array()) {
        let patterns: Vec<String> = inc
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !patterns.is_empty() {
            builder = builder.include_patterns(patterns);
        }
    }
    if let Some(exc) = config_values.get("exclude").and_then(|v| v.as_array()) {
        let patterns: Vec<String> = exc
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !patterns.is_empty() {
            builder = builder.exclude_patterns(patterns);
        }
    }

    builder.build()
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
            tracing::error!(error = %e, "Error prompting for URL");
            CliExit::UsageError("interactive prompt failed".into())
        })
}

#[tokio::main]
pub async fn main() -> CliExit {
    // D6 crash-injection harness: arm BEFORE anything else so every pinned
    // site (even the earliest pipeline stage) sees the same parsed spec.
    // Inert unless WEBFANG_CRASH_AT is set (one OnceLock write).
    webfang_core::cli::crash_points::arm_from_env();

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
    // 1. Parse CLI arguments (single pass, provenance captured)
    let (mut args, arg_sources) = match parse_args() {
        Ok(v) => v,
        Err(exit) => return exit,
    };

    // 1b. Merge positional URL into --url (positional is syntactic sugar)
    if let Some(pos_url) = args.positional_url.take() {
        args.crawler.url = Some(pos_url);
    }

    // 2. Handle subcommands (completions)
    if let Some(Commands::Completions { shell }) = args.subcommand {
        return orchestrator::handle_completions(shell);
    }

    // 3. Unified TUI mode — returns touched-only overrides
    let tui_overrides = match handle_tui_mode(&mut args).await {
        Ok(v) => v,
        Err(exit) => return exit,
    };

    // 4. URL handling with interactive wizard
    if let Err(exit) = resolve_url(&mut args).await {
        return exit;
    }

    // 5. Load config file (graceful: missing file = defaults)
    let config_path = resolve_config_path();
    let config_defaults = ConfigDefaults::load(&config_path);

    // 5b. Validate URL before conversion (CrawlOptions::from panics on invalid URL)
    if let Some(ref url_str) = args.crawler.url {
        if url::Url::parse(url_str).is_err() {
            return CliExit::UsageError(format!("Invalid URL: {url_str}"));
        }
    }

    // 6. Trace file (cloned before normalize borrows args)
    let trace_file = args.crawler.trace_file.clone();

    // 6b. Single normalization pipeline (design D3) — replaces legacy merges
    let normalized =
        match preflight::normalize(&args, &arg_sources, &config_defaults, tui_overrides) {
            Ok(n) => n,
            Err(e) => return e,
        };
    // Project contested fields; copy non-contested from Args via From
    let base = CrawlOptions::from(args);
    let projected = normalized.into_crawl_options();
    // Overwrite contested fields with provenance-correct values, keep base for the rest
    let mut opts = base;
    opts.crawl.max_pages = projected.crawl.max_pages;
    opts.crawl.max_depth = projected.crawl.max_depth;
    opts.crawl.sitemap_url = projected.crawl.sitemap_url;
    opts.crawl.use_sitemap = projected.crawl.use_sitemap;
    opts.crawl.selector = projected.crawl.selector;
    opts.network.delay_ms = projected.network.delay_ms;
    opts.network.timeout_secs = projected.network.timeout_secs;
    opts.network.concurrency = projected.network.concurrency;
    opts.export.output_dir = projected.export.output_dir;
    opts.export.output_format = projected.export.output_format;
    opts.export.export_format = projected.export.export_format;
    opts.export.obsidian_tags = projected.export.obsidian_tags;
    opts.export.obsidian_wiki_links = projected.export.obsidian_wiki_links;
    opts.export.obsidian_relative_assets = projected.export.obsidian_relative_assets;
    opts.export.obsidian_rich_metadata = projected.export.obsidian_rich_metadata;
    opts.export.obsidian_vault = projected.export.obsidian_vault;
    opts.export.quick_save = projected.export.quick_save;
    opts.crawl.ignore_waf = projected.crawl.ignore_waf;
    if opts.crawl.sitemap_url.is_some() {
        opts.crawl.use_sitemap = true;
    }

    // 6b2. Initialize logging (hoisted above the preflight gates on purpose):
    // the 6c-6f2 gates emit `warn!` diagnostics, and a subscriber is only
    // installed by `init_logging_dual` (`try_init` makes repeated calls a
    // no-op, so nothing downstream needs updating). A gate that fails fast
    // must leave its reason on stderr — otherwise the WARN would be silently
    // dropped (#796). Still stderr-only, respects quiet + NO_COLOR.
    let no_color = is_no_color();
    let log_level = resolve_log_level(opts.verbosity);
    let file_trace_layer = build_file_trace_layer(trace_file);

    // Initialize logging (stderr + optional JSONL file trace layer)
    #[allow(clippy::let_unit_value)]
    let _guard = init_logging_dual(log_level, opts.export.quiet, no_color, file_trace_layer);

    // 6c. JS strategy dependency preflight (#685): --js-strategy full needs
    // Chrome installed — fail fast with exit 78 before any crawl starts.
    if let Err(exit) = preflight::check_js_dependencies(&opts) {
        return exit;
    }

    // 6d. Elastic sink preflight (#695): --elastic without persistence nor
    // --output-vectors would wire no sink and silently no-op — exit 78.
    if let Err(exit) = preflight::check_elastic_sink(&opts) {
        return exit;
    }

    // 6d2. Orphan --db-path guard (#799): --db-path is a silent no-op without
    // --elastic, so reject it before any crawl starts (exit 78) instead of
    // letting the run persist nothing.
    if let Err(exit) = preflight::check_db_path_requires_elastic(&opts) {
        return exit;
    }

    // 6e. AI feature preflight (#761): --clean-ai on a non-AI build must
    // fail before any network request, not after a full scrape.
    if let Err(exit) = preflight::check_clean_ai_feature(&opts) {
        return exit;
    }

    // 6f. Obsidian vault/output conflict preflight (#762): an explicit
    // --vault redirect cannot coexist with a custom non-default -o/WEBFANG_OUTPUT.
    if let Err(exit) = webfang_core::cli::commands::validate_vault_output_conflict(&opts) {
        return exit;
    }

    // 6f2. Vector export preflight (#796): --export-format vector without
    // --clean-ai would write an invalid export.json (no embeddings) yet
    // report success — fail fast before any network I/O, mirroring the
    // output-vectors gate (#703). Placed here (after the #762 vault gate,
    // before the orchestrator) to match the fail-fast philosophy of the other
    // 6c-6f gates; dry-run is not exempt for the same reason.
    if let Err(exit) = preflight::check_export_format_vector(&opts) {
        return exit;
    }

    // 8. Build optional engines and delegate to orchestrator
    build_and_run(opts).await
}

/// Parse CLI arguments, translating clap's help/version/error cases into exits.
///
/// Single parse pass per design D1: `ArgMatches` built once, `Args` derived
/// via `FromArgMatches`, provenance captured via `ArgSources::capture`.
fn parse_args() -> Result<(Args, ArgSources), CliExit> {
    use clap::{CommandFactory, FromArgMatches};
    let matches = match Args::command().try_get_matches() {
        Ok(m) => m,
        Err(e) => {
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                e.print().ok();
                return Err(CliExit::Success);
            }
            eprintln!("{e}");
            return Err(CliExit::UsageError("invalid arguments".into()));
        },
    };
    let args = Args::from_arg_matches(&matches).map_err(|e| {
        if e.kind() == clap::error::ErrorKind::DisplayHelp
            || e.kind() == clap::error::ErrorKind::DisplayVersion
        {
            e.print().ok();
            CliExit::Success
        } else {
            eprintln!("{e}");
            CliExit::UsageError("invalid arguments".into())
        }
    })?;
    let sources = ArgSources::capture(&matches);
    Ok((args, sources))
}

/// Run the unified TUI (or reject it when the `ui` feature is off).
///
/// Returns the (possibly TUI-modified) args, or the `CliExit` to propagate —
/// including `CliExit::Success` when the user cancels the TUI.
#[cfg(feature = "ui")]
async fn handle_tui_mode(args: &mut Args) -> Result<Option<TuiOverrides>, CliExit> {
    if args.tui.tui {
        let tui_result = run_unified_tui().await;
        match tui_result {
            Ok(Some(config_values)) => {
                apply_selected_urls(args, &config_values);
                println!("Config applied from TUI.");
                let mut filtered = config_values.clone();
                if let serde_json::Value::Object(ref mut map) = filtered {
                    map.remove("selected_urls");
                }
                return Ok(Some(TuiOverrides::from_json(filtered)));
            },
            Ok(None) => {
                println!("TUI cancelled.");
                return Err(CliExit::Success);
            },
            Err(e) => return Err(e),
        }
    } else if args.tui.config_tui || args.tui.interactive {
        if args.tui.config_tui {
            eprintln!(
                "Warning: --config-tui is deprecated, use --tui instead. Will be removed in v0.6.0"
            );
        } else {
            eprintln!("Warning: --interactive is deprecated, use --tui instead. Will be removed in v0.6.0");
        }
        let tui_result = run_unified_tui().await;
        match tui_result {
            Ok(Some(config_values)) => {
                apply_selected_urls(args, &config_values);
                let mut filtered = config_values.clone();
                if let serde_json::Value::Object(ref mut map) = filtered {
                    map.remove("selected_urls");
                }
                return Ok(Some(TuiOverrides::from_json(filtered)));
            },
            Ok(None) => return Err(CliExit::Success),
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Apply Phase 2 selected URLs to args (multi-URL → batch file).
#[cfg(feature = "ui")]
fn apply_selected_urls(args: &mut Args, config_values: &serde_json::Value) {
    let Some(selected_urls) = config_values
        .get("selected_urls")
        .and_then(|v| v.as_array())
    else {
        return;
    };

    let urls: Vec<String> = selected_urls
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    if urls.is_empty() {
        return;
    }

    args.crawler.url = Some(urls[0].clone());

    if urls.len() > 1 {
        let batch_file =
            std::env::temp_dir().join(format!("webfang_batch_{}.txt", uuid::Uuid::now_v7()));

        match write_batch_file(&batch_file, &urls) {
            Ok(()) => {
                tracing::info!(path = %batch_file.display(), urls = urls.len(), "Batch file created from TUI selection");
                args.export.batch = true;
                args.export.batch_file = Some(batch_file);
            },
            Err(e) => {
                let msg = format!("{e:?}");
                tracing::error!(error = %msg, "Error creating temporary batch from TUI");
            },
        }
    }
}

/// Write URLs to a temp batch file (one URL per line).
#[cfg(feature = "ui")]
fn write_batch_file(path: &std::path::Path, urls: &[String]) -> Result<(), CliExit> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)
        .map_err(|e| CliExit::ConfigError(format!("Error creando archivo batch: {e}")))?;

    for url in urls {
        writeln!(file, "{url}")
            .map_err(|e| CliExit::ConfigError(format!("Error escribiendo batch: {e}")))?;
    }

    Ok(())
}

/// When `ui` is OFF, any TUI flag triggers a graceful Spanish error (spec S2.2).
#[cfg(not(feature = "ui"))]
async fn handle_tui_mode(args: &mut Args) -> Result<Option<TuiOverrides>, CliExit> {
    if args.tui.tui || args.tui.config_tui || args.tui.interactive {
        eprintln!("Error: La interfaz TUI no está disponible en esta compilación.");
        eprintln!();
        eprintln!("Para habilitarla, compile con el feature 'ui' del crate CLI");
        eprintln!("desde la raíz del workspace:");
        eprintln!("  cargo run -p webfang_cli --features ui -- --tui");
        return Err(CliExit::UsageError(
            "La TUI no está disponible: recompile con 'cargo run -p webfang_cli --features ui'"
                .into(),
        ));
    }
    Ok(None)
}

/// Ensure a URL is present, prompting interactively when stdin is a TTY.
async fn resolve_url(args: &mut Args) -> Result<(), CliExit> {
    // Batch mode reads URLs from stdin/file — --url is not required
    let is_batch = args.export.batch || args.export.batch_file.is_some();

    // If a URL is provided (or batch mode), nothing to resolve.
    if args.crawler.url.is_some() || is_batch {
        return Ok(());
    }

    // CI environment always requires --url
    if is_ci() {
        return Err(CliExit::UsageError(
            "--url is required for scraping (CI mode)".into(),
        ));
    }

    // Try interactive prompt only if stdin is a TTY
    if stdin_is_tty() {
        prompt_for_url_interactive(args)
    } else {
        // Not a TTY and no URL provided
        Err(CliExit::UsageError("--url is required for scraping".into()))
    }
}

/// Prompt for a URL when stdin is a TTY and the `ui` feature is enabled.
#[cfg(feature = "ui")]
fn prompt_for_url_interactive(args: &mut Args) -> Result<(), CliExit> {
    match prompt_for_url() {
        Ok(url) => {
            args.crawler.url = Some(url);
            Ok(())
        },
        Err(_e) => {
            // Prompt failed (e.g., non-interactive), fall through to error
            Err(CliExit::UsageError("--url is required for scraping".into()))
        },
    }
}

/// Headless builds have no inquire prompt — require --url explicitly.
#[cfg(not(feature = "ui"))]
fn prompt_for_url_interactive(_args: &mut Args) -> Result<(), CliExit> {
    Err(CliExit::UsageError(
        "--url is required for scraping (interactive prompt requires --features ui)".into(),
    ))
}

/// Resolve the webfang config file path (graceful: missing file = defaults).
fn resolve_config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("webfang")
        .join("config.toml")
}

/// Map verbosity count to tracing levels (0=WARN, 1=INFO, 2=DEBUG, 3+=TRACE).
fn resolve_log_level(verbosity: u8) -> &'static str {
    match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    }
}

/// Create FileTraceLayer when --trace-file is present (always available, no feature gate).
fn build_file_trace_layer(
    trace_file: Option<std::path::PathBuf>,
) -> Option<webfang_core::infrastructure::observability::FileTraceLayer> {
    trace_file.and_then(|path| {
        match webfang_core::infrastructure::observability::FileTraceLayer::new(path) {
            Ok(layer) => Some(layer),
            Err(e) => {
                eprintln!("Error: no se pudo crear archivo de trazas: {e}");
                None
            },
        }
    })
}

/// Tier 2 wiring regression tests (#702): the adaptive engine must expose the
/// wired semantic provider, degrade to Tier 1 (lexical) when no provider is
/// available, and resolve repairs through the semantic cascade.
#[cfg(all(test, feature = "adaptive-selectors"))]
mod wiring_tests {
    use super::*;
    use webfang_core::application::adaptive_engine::RepairStatus;
    use webfang_core::domain::dom_inspector::SelectorErrorKind;
    use webfang_core::domain::semantic_inspector::{
        BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
    };

    /// Mock semantic inspector returning a configurable match.
    struct MockSemanticInspector(Option<SemanticMatch>);

    impl SemanticInspectorPort for MockSemanticInspector {
        fn find_semantic_match<'a>(
            &'a self,
            _ctx: SemanticContext,
        ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
            let response = self.0.clone();
            Box::pin(async { Ok(response) })
        }
    }

    fn opts_with_adaptive(enabled: bool) -> CrawlOptions {
        CrawlOptions {
            adaptive_selectors: enabled,
            ..CrawlOptions::default()
        }
    }

    #[test]
    fn build_adaptive_engine_wires_semantic_provider_when_present() {
        let engine = build_adaptive_engine(
            &opts_with_adaptive(true),
            Some(Arc::new(MockSemanticInspector(None))),
            AdaptiveSelectorOptions::default(),
        )
        .expect("engine must exist when adaptive_selectors is enabled");

        assert!(
            engine.has_semantic_provider(),
            "wired semantic provider must be visible on the engine"
        );
    }

    #[test]
    fn build_adaptive_engine_degrades_to_tier1_without_semantic() {
        let engine = build_adaptive_engine(
            &opts_with_adaptive(true),
            None,
            AdaptiveSelectorOptions::default(),
        )
        .expect("engine must exist when adaptive_selectors is enabled");

        assert!(
            !engine.has_semantic_provider(),
            "no shared pool must leave a Tier-1-only engine"
        );
    }

    #[test]
    fn build_adaptive_engine_absent_when_flag_off() {
        assert!(
            build_adaptive_engine(
                &opts_with_adaptive(false),
                Some(Arc::new(MockSemanticInspector(None))),
                AdaptiveSelectorOptions::default()
            )
            .is_none(),
            "flag off must not build an engine even with a semantic provider available"
        );
    }

    /// End-to-end wiring evidence (AS-2): the engine built by the production
    /// wiring function resolves a repair through Tier 2. `trace.tier2_score`
    /// is `Some` only on the semantic path — Tier 1 outcomes always carry
    /// `None`, so this marker is equivalent to the trace field
    /// `method = "tier2_semantic"` emitted by the cascade.
    #[tokio::test]
    async fn wired_engine_resolves_repair_through_tier2() {
        let semantic_hit = Some(SemanticMatch {
            selector: ".semantic-hit".to_owned(),
            confidence: 0.9, // above semantic_threshold (0.75)
            source: TierSource::Semantic,
        });
        let engine = build_adaptive_engine(
            &opts_with_adaptive(true),
            Some(Arc::new(MockSemanticInspector(semantic_hit))),
            AdaptiveSelectorOptions::default(),
        )
        .expect("engine must exist when adaptive_selectors is enabled");

        let html = "<div class='semantic-hit'>Contenido real</div>".to_owned();
        let outcome = engine
            .select_sync_aware(html, ".missing".to_owned(), None)
            .await
            .expect("tier2 repair must succeed with a confident semantic match");

        assert_eq!(outcome.status, RepairStatus::Repaired);
        assert_eq!(
            outcome.suggestion.selector, ".semantic-hit",
            "the repaired selector must come from the semantic provider"
        );
        let trace = outcome.trace.expect("tier2 repair carries a trace");
        assert_eq!(
            trace.tier2_score,
            Some(0.9),
            "the trace must record the tier2_semantic score"
        );
    }
}

/// Build the adaptive selector engine when the feature and flag are both active.
///
/// The `semantic` parameter carries the Tier 2 provider when the `ai` feature
/// supplied one (shared ONNX pool + tokenizer); `None` degrades the engine to
/// Tier 1 (lexical) repair. `options` is the shared engine configuration —
/// the SAME instance that configured the Tier 2 threshold.
#[cfg(feature = "adaptive-selectors")]
fn build_adaptive_engine(
    opts: &CrawlOptions,
    semantic: Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>>,
    options: AdaptiveSelectorOptions,
) -> Option<Arc<AdaptiveSelectorEngine>> {
    if opts.adaptive_selectors {
        tracing::info!(
            semantic_wired = semantic.is_some(),
            "building adaptive selector engine"
        );
        Some(Arc::new(AdaptiveSelectorEngine::new(
            Arc::new(DefaultDomInspector::new()),
            semantic,
            options,
        )))
    } else {
        None
    }
}

/// Build the Tier 2 semantic inspector from the cleaner's shared ONNX assets
/// (#702).
///
/// Returns `None` when no shared pool is available (`--ai` off or dry-run),
/// degrading the adaptive engine to Tier 1 lexical repair. The threshold comes
/// from the single shared [`AdaptiveSelectorOptions`] — no duplicate constant.
#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
fn semantic_inspector(
    shared: &Option<(
        Arc<webfang_ai::InferencePool>,
        Arc<webfang_ai::MiniLmTokenizer>,
    )>,
    options: &AdaptiveSelectorOptions,
) -> Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>> {
    shared.as_ref().map(|(pool, tokenizer)| {
        let inspector = webfang_ai::GraniteDomInspector::new(
            Arc::clone(pool),
            Arc::clone(tokenizer),
            options.semantic_threshold,
        );
        Arc::new(inspector) as Arc<dyn webfang_core::domain::SemanticInspectorPort>
    })
}

/// Build the AI semantic cleaner and its vault-search ports, surfacing the
/// cleaner's shared ONNX assets (pool + tokenizer) so the adaptive engine can
/// reuse them for Tier 2 semantic repair (#702) without a second model load.
#[cfg(feature = "ai")]
async fn build_ai_cleaner(
    opts: &CrawlOptions,
) -> Result<
    (
        Option<Arc<dyn SemanticCleaner>>,
        webfang_core::application::container::VaultAiPorts,
        Option<(
            Arc<webfang_ai::InferencePool>,
            Arc<webfang_ai::MiniLmTokenizer>,
        )>,
    ),
    CliExit,
> {
    if !opts.ai || opts.export.dry_run {
        return Ok((
            None,
            webfang_core::application::container::VaultAiPorts::default(),
            None,
        ));
    }

    // Resolve model variant: CLI flag takes precedence over AI_MODEL_ID env var
    let model_variant = if opts.ai_config.model.is_empty() {
        webfang_ai::AiModel::from_env_or_default()
    } else {
        match opts.ai_config.model.parse::<webfang_ai::AiModel>() {
            Ok(variant) => variant,
            Err(e) => {
                return Err(CliExit::UsageError(format!(
                    "Modelo AI inválido para --ai-model: {e}"
                )));
            },
        }
    };

    let model_config = ModelConfig::default()
        .with_model_variant(model_variant)
        .with_relevance_threshold(opts.ai_config.threshold)
        .map(|c| {
            c.with_max_tokens(opts.ai_config.max_tokens)
                .with_offline_mode(opts.ai_config.offline)
        });

    match model_config {
        Ok(config) => match SemanticCleanerImpl::new(config).await {
            Ok(cleaner) => {
                // Share the cleaner's ONNX pool + tokenizer (#433) so the
                // vault-search embedding adapter reuses the SAME model — one
                // `resolve_model_assets` call, one `InferencePool` on `--ai`.
                // Extracted before type-erasing the cleaner behind the trait.
                let (pool, tokenizer) = cleaner.shared_inference();
                // Surface a second Arc pair for Tier 2 wiring (#702); the vault
                // ports consume the originals below.
                let shared = Some((Arc::clone(&pool), Arc::clone(&tokenizer)));
                let cleaner: Option<Arc<dyn SemanticCleaner>> = Some(Arc::new(cleaner));
                let mut ports = build_vault_ports(pool, tokenizer).await;
                ports.cleaner = cleaner.clone();
                Ok((cleaner, ports, shared))
            },
            Err(e) => {
                let msg = format!("No se pudo inicializar el limpiador semántico AI: {e}");
                Err(match e {
                    SemanticError::Download { .. } => CliExit::NetworkError(msg),
                    _ => CliExit::ConfigError(msg),
                })
            },
        },
        Err(e) => Err(CliExit::ConfigError(format!(
            "Configuración de umbral AI inválida: {e}"
        ))),
    }
}

/// Build optional engines and dispatch to the orchestrator.
///
/// The AI cleaner is built FIRST (#702): the adaptive engine needs the
/// cleaner's shared ONNX assets to wire Tier 2 semantic repair. The argument
/// list passed to the orchestrator depends on which optional features are
/// compiled in, so each combination is spelled out explicitly.
async fn build_and_run(opts: CrawlOptions) -> CliExit {
    #[cfg(feature = "ai")]
    let (ai_cleaner, vault_ports, shared) = match build_ai_cleaner(&opts).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    #[cfg(not(feature = "ai"))]
    let vault_ports = webfang_core::application::container::VaultAiPorts::default();

    #[cfg(feature = "adaptive-selectors")]
    // Inference worker bound derives from the budget model's Operation.inference
    // tier (task 2.5d); explicit operator flags arrive via BudgetOverrides.
    let engine_options = {
        let budget = webfang_core::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &webfang_core::domain::budget::detector::SystemDetector,
        );
        AdaptiveSelectorOptions {
            max_concurrent_inference: budget.inference().get(),
            ..AdaptiveSelectorOptions::default()
        }
    };
    #[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
    let semantic = semantic_inspector(&shared, &engine_options);
    // Without the `ai` feature there is no ONNX pool — explicit Tier 1 degrade.
    #[cfg(all(not(feature = "ai"), feature = "adaptive-selectors"))]
    let semantic: Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>> = None;
    // No adaptive engine to feed — keep the shared assets from becoming an
    // unused binding on the ai-only combo.
    #[cfg(all(feature = "ai", not(feature = "adaptive-selectors")))]
    let _ = shared;
    #[cfg(feature = "adaptive-selectors")]
    let adaptive_engine = build_adaptive_engine(&opts, semantic, engine_options);

    #[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
    {
        orchestrator::run(opts, ai_cleaner, adaptive_engine, vault_ports).await
    }
    #[cfg(all(feature = "ai", not(feature = "adaptive-selectors")))]
    {
        orchestrator::run(opts, ai_cleaner, vault_ports).await
    }
    #[cfg(all(not(feature = "ai"), feature = "adaptive-selectors"))]
    {
        orchestrator::run(opts, adaptive_engine, vault_ports).await
    }
    #[cfg(all(not(feature = "ai"), not(feature = "adaptive-selectors")))]
    {
        orchestrator::run(opts, vault_ports).await
    }
}

/// Assemble the vault-search AI ports (#433) from the cleaner's shared model.
///
/// Builds the ONNX embedding adapter + Markdown chunker (always under `ai`) from
/// the semantic cleaner's shared inference pool + tokenizer — so the `--ai` path
/// loads the ONNX model exactly once — plus the SQLite note repository (under
/// `persistence`). Embedding + chunker assembly is infallible (the components are
/// already valid); only the note repository can fail, and it degrades gracefully
/// — the port stays `None` and the capability answers with an honest error rather
/// than aborting the run.
#[cfg(feature = "ai")]
async fn build_vault_ports(
    pool: Arc<webfang_ai::InferencePool>,
    tokenizer: Arc<webfang_ai::MiniLmTokenizer>,
) -> webfang_core::application::container::VaultAiPorts {
    use webfang_core::application::container::VaultAiPorts;

    let mut ports = VaultAiPorts::default();

    // Assemble the embedding adapter from the cleaner's shared pool + tokenizer.
    // Infallible — no model resolution happens here (that already happened once
    // inside `SemanticCleanerImpl::new`), so there is nothing to degrade around.
    let adapter = webfang_ai::EmbeddingAdapter::new(pool, tokenizer);
    ports.embedding_port = Some(Arc::new(adapter));
    ports.text_chunker = Some(Arc::new(webfang_ai::MarkdownChunker::new()));

    #[cfg(feature = "persistence")]
    {
        use webfang_core::infrastructure::autotuning::{env_db_path, resolve_db_path};
        use webfang_core::infrastructure::persistence::{
            create_pool, setup_schema, SqliteVectorRepository,
        };

        let db_path = resolve_db_path(None, env_db_path());
        match create_pool(&db_path, 4) {
            Ok(db_pool) => match setup_schema(&db_pool).await {
                Ok(()) => {
                    ports.note_repository = Some(Arc::new(SqliteVectorRepository::new(db_pool)));
                },
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "note repository schema init failed, persistence disabled"
                    );
                },
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "note repository pool creation failed, persistence disabled"
                );
            },
        }
    }

    ports
}
