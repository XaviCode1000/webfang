#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
//! WebFang - Production-ready web scraper with Clean Architecture
//!
//! Extracts clean, structured content from web pages using readability algorithm.
//!
//! # Architecture
//!
//! Following Clean Architecture:
//!
//! ```text
//! main.rs (thin entry point) -> orchestrator::run()
//!     |
//!     ├─→ Args::try_parse()           ← CLI parsing
//!     ├─→ handle_completions()        ← Subcommand handling
//!     ├─→ load_config_defaults()       ← TOML config
//!     ├─→ preflight::normalize()      ← Config merge
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     └─→ orchestrator::run()         ← Full pipeline
//! ```
//!
//! **Golden Rule:** The application layer stays UI-free — the CLI, MCP
//! server and AI pipeline are thin adapters over it, never the reverse.

use webfang_core::cli::orchestrator;

use std::panic;
use std::sync::Arc;
#[cfg(feature = "ai")]
use webfang_ai::{ModelConfig, SemanticCleanerImpl, SemanticError};
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::{AdaptiveSelectorEngine, AdaptiveSelectorOptions};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::config::{load_config_defaults, resolve_config_path, ConfigDefaults};
use webfang_core::cli::error::{CliError, CliExit};
use webfang_core::cli::preflight;
use webfang_core::cli::preflight::ArgSources;
#[cfg(feature = "ai")]
use webfang_core::domain::semantic_cleaner::SemanticCleaner;
use webfang_core::infrastructure::platform::init_console_output_encoding;
#[cfg(feature = "adaptive-selectors")]
use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;
use webfang_core::{init_logging_dual, is_no_color, Args, Commands};
#[tokio::main]
pub async fn main() -> CliExit {
    // XP-K-03 (#1808): UTF-8 console code page. FIRST statement of the
    // process, before the tracing subscriber and before any user-facing
    // write, because a code page set after output has started leaves the
    // earlier lines mojibake. Documented no-op off Windows.
    init_console_output_encoding();

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

    // 4. URL handling with interactive wizard
    if let Err(exit) = resolve_url(&mut args).await {
        return exit;
    }

    // 5. Load config file. The platform default may be absent (silent
    //    defaults); an explicit WEBFANG_CONFIG must be usable or the run stops
    //    here — a typo must not read as "ran with your config" (#1659).
    let config_defaults = match load_config_defaults() {
        Ok(config) => config,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    // 5b. Nothing to validate: `--url`/positional are parsed into hardened
    // `ValidUrl` at the argv boundary (#1239), so an invalid or
    // credential-bearing URL is a clap usage error before this point.

    // 6. Trace file (cloned before normalize borrows args)
    let trace_file = args.crawler.trace_file.clone();

    // 6b. Single normalization pipeline (design D3) — replaces legacy merges
    let normalized = match preflight::normalize(&args, &arg_sources, &config_defaults) {
        Ok(n) => n,
        Err(e) => return e,
    };
    // Project contested fields; copy non-contested from Args via From
    let base = CrawlOptions::from(args);
    // CLI-explicit budget knobs live ONLY in `base` (--concurrency /
    // --batch-concurrency / --download-concurrency / --rate-limit-burst via
    // From<Args>); capture them before the move below (#897 item 1).
    let cli_budget = base.budget_overrides;
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
    // No sitemap coercion here (#1190): `into_crawl_options` already
    // applies the cross-field rule, and `SitemapConfig::resolve` is the
    // single home of the `Some(url) implies intent` rule at projection.
    // Budget overrides (#897 item 1): merge FIELD-WISE, never by slot, with
    // per-tier provenance rules (see `merge_budget_overrides`): the ranked
    // pipeline value wins the crawl tier; explicit CLI knobs win the rest.
    // Before #897 this assignment did not exist at all, so TOML/TUI values
    // were silently dropped from enforcement.
    opts.budget_overrides =
        preflight::merge_budget_overrides(cli_budget, projected.budget_overrides);

    // 6b2. Initialize logging (hoisted above the preflight gates on purpose):
    // the 6c-6f2 gates emit `warn!` diagnostics, and a subscriber is only
    // installed by `init_logging_dual`. A gate that fails fast must leave its
    // reason on stderr — otherwise the WARN would be silently dropped (#796).
    // Still stderr-only, respects quiet + NO_COLOR.
    let no_color = is_no_color();
    let log_level =
        resolve_log_level_with_config(opts.verbosity, config_defaults.log_level.as_deref());
    // #1814: the trace file is an explicitly requested artifact — a failure to
    // create it aborts the run (exit 78) instead of continuing untraced.
    let file_trace_layer = match build_file_trace_layer(trace_file) {
        Ok(layer) => layer,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    // Initialize logging (stderr + optional JSONL file trace layer).
    // #1814: a subscriber that cannot be installed fails the run — silently
    // continuing without the requested log output hid the misconfiguration.
    if let Err(e) = init_logging_dual(log_level, opts.export.quiet, no_color, file_trace_layer) {
        return CliExit::ConfigError(e.to_string());
    }

    // #1431: replay parse-time diagnostics recorded before the subscriber
    // existed (e.g. the --rate-limit-burst substitution notice). The note
    // text already names the offending value; it travels once more as a
    // structured field so trace queries can select it without parsing.
    for note in webfang_core::cli::preflight_notes::take() {
        tracing::warn!(preflight_note = %note, "{note}");
    }

    // The `--max-tokens` deprecation warning lives HERE, not where the value is
    // resolved, for the same #796 reason the notes above are replayed: argv
    // resolution runs before `init_logging_dual`, so a `warn!` emitted there
    // would be dropped and the deprecation would be silent (ADR-0004). It is
    // emitted exactly once, at the one point that has both the provenance
    // (`AiConfig::deprecated_max_tokens`) and a live subscriber.
    //
    // Shape follows the existing env-var deprecation
    // (`webfang_ai::infrastructure_ai::compat::read_ai_model_id_with`): English
    // structured field names, user-facing message text in Spanish, with the
    // migration target and the removal schedule stated.
    if let Some(max_tokens) = opts.ai_config.deprecated_max_tokens {
        tracing::warn!(
            flag = "--max-tokens",
            env = "WEBFANG_MAX_TOKENS",
            replacement = "--max-chars",
            max_tokens,
            chars_per_token = webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            max_chars = opts.ai_config.max_chars,
            "--max-tokens está obsoleto y se eliminará en la próxima versión principal. \
             Migrado a --max-chars ({} tokens × {} caracteres/token = {} caracteres). \
             Usa --max-chars directamente.",
            max_tokens,
            webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            opts.ai_config.max_chars,
        );
    }

    // 6c. JS strategy dependency preflight (#685): --js-strategy full needs
    // Chrome installed — fail fast with exit 78 before any crawl starts.
    if let Err(exit) = preflight::check_js_dependencies(&opts) {
        return exit;
    }
    // F-52-c (#1278): pin the launcher to the gate-certified binary. The
    // resolution runs once here (process PATH, post-gate) and travels in
    // CrawlOptions so every chromium launch uses `chrome_executable`
    // instead of chromiumoxide auto-detection (which omits bare
    // `google-chrome` and accepts the `/opt/google/chrome` directory).
    // `None` (non-Full strategies, or a lost post-gate race) keeps
    // historical auto-detection on paths that never run this gate.
    opts.network.chrome_binary = preflight::resolve_chrome_binary();

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

    // 6e2. The `--adaptive-selectors` capability gate (#1813) is NOT here.
    //
    // It used to be this step, mirroring 6e. That was a fail-open waiting to
    // happen: a check wired into ONE caller is skipped by every other caller,
    // and the sibling `--rate-limit-burst` fix (#1813 T1) had already moved its
    // validation into the shared staging pipeline for exactly that reason. The
    // gate now runs inside `preflight::normalize` (step 6b, above), which every
    // `Args` front door traverses.
    //
    // This call site was provably dead once that moved: `normalize` runs at
    // :116 and returns before reaching here, and `From<Args>` copies
    // `args.crawler.adaptive_selectors` into `opts.adaptive_selectors`
    // unchanged, so `opts.adaptive_selectors` cannot be true when `normalize`
    // passed. Keeping it would have left a reader believing this was the
    // enforcement point. `preflight::check_adaptive_selectors_feature` remains
    // public for a caller that builds `CrawlOptions` directly.

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

/// Ensure a URL is present; without `--url` (and outside batch mode) scraping cannot start.
async fn resolve_url(args: &mut Args) -> Result<(), CliExit> {
    // Batch mode reads URLs from stdin/file — --url is not required
    let is_batch = args.export.batch || args.export.batch_file.is_some();

    // If a URL is provided (or batch mode), nothing to resolve.
    if args.crawler.url.is_some() || is_batch {
        return Ok(());
    }

    Err(CliExit::UsageError("--url is required for scraping".into()))
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

/// Resolve the effective log level, honoring `log_level` from the config
/// file when the operator gave no `-v` flags (P4-3 of #1296: the key was
/// loaded by `ConfigDefaults` but never consumed — repro posted in the
/// issue). Explicit flags always win; an invalid config value warns on
/// stderr — the tracing subscriber does not exist yet — and falls back to
/// WARN, matching the `ConfigDefaults` graceful-degradation contract.
fn resolve_log_level_with_config(verbosity: u8, config_level: Option<&str>) -> &'static str {
    if verbosity > 0 {
        return resolve_log_level(verbosity);
    }
    let normalized = config_level
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    match normalized.as_deref() {
        None | Some("warn") => "warn",
        Some("error") => "error",
        Some("info") => "info",
        Some("debug") => "debug",
        Some("trace") => "trace",
        Some(other) => {
            eprintln!(
                "Advertencia: log_level «{other}» inválido en el archivo de configuración; se usa «warn»."
            );
            "warn"
        },
    }
}

#[cfg(test)]
mod log_level_tests {
    use super::resolve_log_level_with_config;

    #[test]
    fn explicit_flags_win_over_config() {
        assert_eq!(resolve_log_level_with_config(1, Some("trace")), "info");
        assert_eq!(resolve_log_level_with_config(2, Some("error")), "debug");
        assert_eq!(resolve_log_level_with_config(3, Some("info")), "trace");
    }

    #[test]
    fn config_level_applies_when_no_flags() {
        assert_eq!(resolve_log_level_with_config(0, Some("info")), "info");
        assert_eq!(resolve_log_level_with_config(0, Some("TRACE")), "trace");
        assert_eq!(resolve_log_level_with_config(0, Some("  debug  ")), "debug");
        assert_eq!(resolve_log_level_with_config(0, Some("warn")), "warn");
    }

    #[test]
    fn missing_or_invalid_config_falls_back_to_warn() {
        assert_eq!(resolve_log_level_with_config(0, None), "warn");
        assert_eq!(resolve_log_level_with_config(0, Some("")), "warn");
        assert_eq!(resolve_log_level_with_config(0, Some("   ")), "warn");
        assert_eq!(resolve_log_level_with_config(0, Some("loud")), "warn");
    }
}

/// Create FileTraceLayer when --trace-file is present (always available, no feature gate).
///
/// The trace file is an explicitly requested artifact: a failure to create it
/// must abort the run (fail closed) instead of silently continuing without the
/// requested trace output (issue #1814).
fn build_file_trace_layer(
    trace_file: Option<std::path::PathBuf>,
) -> Result<Option<webfang_core::infrastructure::observability::FileTraceLayer>, CliError> {
    match trace_file {
        Some(path) => {
            webfang_core::infrastructure::observability::FileTraceLayer::new(path.clone())
                .map(Some)
                .map_err(|e| CliError::ConfigFile {
                    msg: format!(
                        "no se pudo crear el archivo de trazas en '{}': {e}",
                        path.display()
                    ),
                    suggestion:
                        "verifique que la ruta padre de --trace-file exista y sea escribible, o elija otra ruta"
                            .to_string(),
                })
        },
        None => Ok(None),
    }
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

    /// Recording `EmbeddingPort` that returns a fixed vector and counts the
    /// texts it was asked to embed. Used to prove *which* port the inspector
    /// is bound to, not just that it runs.
    ///
    /// Wrapped in a newtype because the orphan rule forbids implementing a
    /// foreign trait for `Arc<T>` — and `Arc<CountingEmbedder>` would be the
    /// otherwise-natural shape.
    ///
    /// `ai`-gated because the only test that uses it is, and this module
    /// compiles with `adaptive-selectors` alone: without the gate the whole
    /// fixture is dead code in that combination.
    #[cfg(feature = "ai")]
    struct CountingEmbedder {
        inner: Arc<CountingInner>,
    }

    #[cfg(feature = "ai")]
    struct CountingInner {
        vec: Vec<f32>,
        embedded: std::sync::atomic::AtomicUsize,
    }

    #[cfg(feature = "ai")]
    impl CountingEmbedder {
        fn new(vec: Vec<f32>) -> Self {
            Self {
                inner: Arc::new(CountingInner {
                    vec,
                    embedded: std::sync::atomic::AtomicUsize::new(0),
                }),
            }
        }

        fn as_port(&self) -> Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort> {
            Arc::new(Self {
                inner: Arc::clone(&self.inner),
            })
        }

        fn embedded(&self) -> usize {
            self.inner
                .embedded
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[cfg(feature = "ai")]
    impl webfang_core::domain::embedding_port::EmbeddingPort for CountingEmbedder {
        fn embed<'a>(
            &'a self,
            _text: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Vec<f32>, webfang_core::error::SemanticError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.inner
                    .embedded
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(self.inner.vec.clone())
            })
        }

        fn embedding_dim(&self) -> usize {
            self.inner.vec.len()
        }
    }

    /// Pins the SHAPE of the Tier 2 wiring: an inspector is built only from a
    /// `Tier2EmbeddingPort`, and it embeds through the port that type carries.
    ///
    /// What this test does NOT claim: it does not prove the inspector is
    /// *prevented* from reaching the vault slot. `vault_slot` below is built
    /// and then never handed to anything — `remote.embedded() == 0` is true by
    /// construction, not by an observed refusal. That invariant is carried by
    /// the type system: `semantic_inspector` takes `Option<Tier2EmbeddingPort>`
    /// while `VaultAiPorts::embedding_port` is `Option<Arc<dyn EmbeddingPort>>`,
    /// so passing the slot is a compile error (issue #1977). Compile-time
    /// evidence is the T1 swap experiment, not this test.
    ///
    /// What it does prove: the port the inspector embeds through is the one
    /// handed in, all the way down to the adapter — the part that would break
    /// first if `semantic_inspector` or `EmbeddingSemanticInspector::new`
    /// started resolving its own port.
    #[cfg(feature = "ai")]
    #[test]
    fn tier2_inspector_stays_bound_to_the_local_port() {
        let local = Arc::new(CountingEmbedder::new(vec![1.0, 0.0]));
        let remote = Arc::new(CountingEmbedder::new(vec![0.0, 1.0]));

        let shared =
            Some(webfang_core::domain::embedding_port::LocalEmbeddingPort::new(local.as_port()));

        // Models the composition order: the vault slot gets overwritten by a
        // remote provider after `build_ai_cleaner` already handed out the local
        // port. The inspector only ever sees `shared`.
        let vault_slot: Option<Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>> =
            Some(remote.as_port());

        let tier2_port =
            shared.map(webfang_core::domain::embedding_port::Tier2EmbeddingPort::Local);
        let inspector = semantic_inspector(tier2_port, &AdaptiveSelectorOptions::default())
            .expect("a shared local port must produce an inspector");

        // Drive one Tier 2 call through the port-backed inspector.
        let ctx = SemanticContext {
            target_text: "contenido real".to_owned(),
            dom_fragments: vec![".contenido".to_owned()],
            domain_hint: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");
        let outcome = runtime.block_on(inspector.find_semantic_match(ctx));

        assert!(
            local.embedded() > 0,
            "the inspector must embed through the port it was handed"
        );
        assert_eq!(
            remote.embedded(),
            0,
            "the inspector reached a port it was never given"
        );
        // `vault_slot` exists purely to model the overwrite; binding the
        // inspector must not consult it.
        assert!(vault_slot.is_some());
        assert!(outcome.is_ok(), "the Tier 2 call must not error");
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

/// Build the Tier 2 semantic inspector with an explicit embedding port (ADR-0004).
///
/// The slot is either:
/// - [`Tier2EmbeddingPort::Local`](webfang_core::domain::embedding_port::Tier2EmbeddingPort::Local)
///   — the shared LOCAL embedding port when `--tier2-embedding-provider` is NOT
///   set, OR
/// - [`Tier2EmbeddingPort::Remote`](webfang_core::domain::embedding_port::Tier2EmbeddingPort::Remote)
///   — a dedicated remote adapter when it IS set.
///
/// This is SEPARATE from the vault-search embedding slot (`vault_ports.embedding_port`)
/// which is controlled by `--embedding-provider`. Vault search and Tier 2 repair
/// are distinct privacy domains (issue #1977), and the type system enforces it:
/// the parameter is `Option<Tier2EmbeddingPort>`, and the vault slot's
/// `Option<Arc<dyn EmbeddingPort>>` is not assignable to it.
///
/// Gated on `adaptive-selectors` alone, NOT `all(ai, adaptive-selectors)`:
/// both the `ai` and the `not(ai)` call sites reach it — without `ai` there
/// is no local pool, but an explicit `--tier2-embedding-provider` still
/// resolves. Requiring `ai` here left the `not(ai)` arm calling a function
/// that did not exist, which is exactly what the Feature-matrix job exists
/// to catch (`cargo hack check --each-feature`).
#[cfg(feature = "adaptive-selectors")]
fn semantic_inspector(
    tier2_port: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort>,
    options: &AdaptiveSelectorOptions,
) -> Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>> {
    tier2_port.map(|slot| {
        let inspector = webfang_core::infrastructure::scraper::embedding_semantic_inspector::EmbeddingSemanticInspector::new(
            Arc::clone(slot.port()),
            options.semantic_threshold,
        );
        Arc::new(inspector) as Arc<dyn webfang_core::domain::SemanticInspectorPort>
    })
}

/// Map an AI cleaner init failure to its CLI exit: download faults are
/// transient network errors, everything else is a config error.
#[cfg(feature = "ai")]
fn ai_init_error(e: SemanticError) -> CliExit {
    let msg = format!("No se pudo inicializar el limpiador semántico AI: {e}");
    match e {
        SemanticError::Download { .. } => CliExit::NetworkError(msg),
        _ => CliExit::ConfigError(msg),
    }
}

/// Build the AI semantic cleaner and its vault-search ports, surfacing the
/// cleaner's shared LOCAL embedding port so Tier 2 semantic repair reuses it
/// without a second model load (ADR-0004 slice 1).
///
/// The third element is deliberately the local `EmbeddingPort` and not the
/// engine+tokenizer bundle it was before: Tier 2 is now port-backed, and the
/// port is the only thing it needs. Returning it **from here**, rather than
/// having the inspector read a slot later, is what makes the binding safe —
/// see the slot-overwrite note at the call site.
///
/// It is returned as a
/// [`LocalEmbeddingPort`](webfang_core::domain::embedding_port::LocalEmbeddingPort)
/// rather than a bare trait object, so the origin survives the trip: the caller
/// cannot pass it to anything that would lose the fact that it is local
/// (issue #1977).
#[cfg(feature = "ai")]
async fn build_ai_cleaner(
    opts: &CrawlOptions,
) -> Result<
    (
        Option<Arc<dyn SemanticCleaner>>,
        webfang_core::application::container::VaultAiPorts,
        Option<webfang_core::domain::embedding_port::LocalEmbeddingPort>,
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
        // Flag absent: honor AI_MODEL_ID if set, failing loudly when its
        // value is not a valid model ID (#874). Unset → silent Granite-97M
        // default (the user made no choice).
        match webfang_ai::AiModel::from_env() {
            Ok(Some(variant)) => variant,
            Ok(None) => webfang_ai::AiModel::default(),
            Err(e) => {
                return Err(CliExit::UsageError(format!(
                    "Modelo AI inválido para la variable de entorno AI_MODEL_ID: {e}"
                )));
            },
        }
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

    // Engine selection (P0-001 MEASURE, issue #1456): env-first via
    // `WEBFANG_AI_ENGINE` (`single` | `pool:<N>`), unset meaning the
    // MEASURE-calibrated `Pool` default (rollout shipped per
    // `docs/p0-001-n-decision.md`: N=4 knee, 5.44× wall, bit-identical
    // correctness). `WEBFANG_AI_ENGINE=single` is the rollback hatch. Set-but-
    // invalid is a loud startup error, never a silent fallback (#874
    // discipline shared with `AI_MODEL_ID` above). No `--ai-engine` flag by
    // design: a flag could never reach the MCP daemon (no per-run argv); the
    // env var reaches both the CLI and the MCP daemon from the shared
    // `webfang_ai` layer (#1569).
    let engine_config = match webfang_ai::infrastructure_ai::EngineConfig::from_env() {
        Ok(config) => config,
        Err(e) => return Err(CliExit::ConfigError(e)),
    };

    let model_config = ModelConfig::default()
        .with_model_variant(model_variant)
        .with_relevance_threshold(opts.ai_config.threshold)
        .map(|c| {
            c.with_max_chars(opts.ai_config.max_chars)
                .with_offline_mode(opts.ai_config.offline)
        });

    match model_config {
        Ok(config) => {
            // ONE composition root for the local embedding stack (#1569,
            // ADR-0004): `build_onnx_embedding_port` resolves the model ONCE
            // and returns the port plus the SAME engine + tokenizer the vault
            // ports and the Tier 2 inspector need — `Single` routes through
            // `build_engine` (byte-for-byte today's pool construction, drainer
            // contract included) and `Pool { N }` through the N-session
            // engine. The cleaner itself only takes the port.
            match webfang_ai::infrastructure_ai::embedding_adapter::build_onnx_embedding_port(
                &config,
                engine_config,
            )
            .await
            {
                Ok((port, engine, tokenizer)) => {
                    let cleaner = SemanticCleanerImpl::from_parts(Arc::clone(&port), config);
                    // Hand the SAME local port to Tier 2. Wrapped in
                    // `LocalEmbeddingPort` — the only production construction
                    // of that type — and captured here, before
                    // `resolve_remote_embedding` overwrites the vault slot, so
                    // `--embedding-provider remote` can never reach the
                    // inspector's input (ADR-0004 slice 1, issue #1977). The
                    // ordering is the convention; the wrapper is the type.
                    let shared =
                        Some(webfang_core::domain::embedding_port::LocalEmbeddingPort::new(port));
                    let cleaner: Option<Arc<dyn SemanticCleaner>> = Some(Arc::new(cleaner));
                    let mut ports = build_vault_ports(engine, tokenizer).await;
                    ports.cleaner = cleaner.clone();
                    Ok((cleaner, ports, shared))
                },
                Err(e) => Err(ai_init_error(e)),
            }
        },
        Err(e) => Err(CliExit::ConfigError(format!(
            "Configuración de umbral AI inválida: {e}"
        ))),
    }
}

/// Resolve the remote embedding adapter for the embedding slot (#1462).
///
/// An explicit `--embedding-provider` always resolves (fail fast even
/// without `ai`, so vault search works with no local model); with `ai` the
/// default slot may also select a remote provider. Otherwise `None` with no
/// Load the provider registry from the config file.
///
/// Shared by the vault and Tier 2 embedding resolvers, which otherwise repeat
/// this prologue verbatim — the duplication ratchet (`scripts/check_duplication.sh`)
/// counts it against a DESCENDING baseline, so it lives here once.
fn load_providers() -> webfang_core::domain::providers::ProvidersConfig {
    let providers_cfg = ConfigDefaults::load(&resolve_config_path());
    webfang_core::domain::providers::ProvidersConfig {
        providers: providers_cfg.providers,
    }
}

/// config load and no probe — plain runs never dial an embeddings endpoint.
async fn resolve_remote_embedding(
    opts: &CrawlOptions,
) -> Result<
    Option<
        std::sync::Arc<webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter>,
    >,
    CliExit,
> {
    if opts.embedding_provider.is_none() && !opts.ai {
        return Ok(None);
    }
    let providers = load_providers();
    webfang_core::cli::llm_wire::build_embedding_provider(opts, &providers, opts.ai_config.offline)
        .await
}

/// Resolve the remote embedding adapter for the Tier 2 semantic inspector (ADR-0004).
///
/// Separate from `resolve_remote_embedding` on purpose: vault search and
/// Tier 2 repair are distinct privacy domains (issue #1977). An explicit
/// `--tier2-embedding-provider` always resolves (fail fast even without `ai`);
/// with `ai` the default slot may also select a remote provider. Otherwise
/// `None` with no config load and no probe — plain runs never dial an
/// embeddings endpoint for Tier 2.
///
/// Consent check: if the resolved provider is remote (not `local_onnx`),
/// `--tier2-consent` / `WEBFANG_TIER2_CONSENT` MUST be set. A hash of
/// `provider_id|base_url` is logged for auditability.
///
/// The gate is decided inside the builder, at the pure resolution step, so an
/// unauthorized remote slot never reaches the credential or the dial.
#[cfg(feature = "adaptive-selectors")]
async fn resolve_tier2_remote_embedding(
    opts: &CrawlOptions,
) -> Result<
    Option<
        std::sync::Arc<webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter>,
    >,
    CliExit,
> {
    if opts.tier2_embedding_provider.is_none() && !opts.ai {
        return Ok(None);
    }
    let providers = load_providers();
    // By the time this returns an adapter, the remote endpoint was authorized
    // first and probed second, in that order.
    let adapter = webfang_core::cli::llm_wire::build_tier2_embedding_provider(
        opts.tier2_embedding_provider.as_deref(),
        opts.tier2_consent,
        &providers,
        opts.ai_config.offline,
    )
    .await?;

    // If adapter is Some, it's a remote provider (local_onnx returns None)
    if let Some(ref remote) = adapter {
        // Log consent hash for auditability
        let consent_hash = compute_consent_hash(remote.id(), remote.base_url());
        tracing::info!(
            tier2_consent_hash = %consent_hash,
            "Tier 2 remote embeddings consent granted"
        );
    }
    Ok(adapter)
}

/// Compute a short hash for audit logging: sha256(provider_id + "|" + base_url) truncated to 16 chars.
///
/// Gated with its only call site: under default features the caller
/// (`resolve_tier2_remote_embedding`) is compiled out and this would otherwise
/// trip `dead_code` on every non-`adaptive-selectors` build.
#[cfg(feature = "adaptive-selectors")]
fn compute_consent_hash(provider_id: &str, base_url: &url::Url) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update(b"|");
    hasher.update(base_url.as_str().as_bytes());
    let hash = hasher.finalize();
    // Manual hex encoding (avoid hex crate dependency)
    hash.iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string()
}

/// Build optional engines and dispatch to the orchestrator.
///
/// The AI cleaner is built FIRST (#702): the adaptive engine needs the
/// cleaner's shared ONNX assets to wire Tier 2 semantic repair. The argument
/// list passed to the orchestrator depends on which optional features are
/// compiled in, so each combination is spelled out explicitly.
///
/// If `--extract-with-llm` is set, builds the LLM provider from config,
/// injects it into a Container, and validates startup (caducidad del DEBE §8b:
/// el binario DEBE validar llm_port().is_some() en startup cuando el flag está
/// activo).
#[allow(clippy::too_many_lines)]
async fn build_and_run(opts: CrawlOptions) -> CliExit {
    // Build LLM provider if --extract-with-llm is set.
    // This is NOT gated by the `ai` feature — the remote provider port lives
    // in webfang_core unconditionally (docs/src/ai-providers-design.md §8).
    let llm_port = if opts.extract_with_llm {
        let providers_cfg = ConfigDefaults::load(&resolve_config_path());
        let registry = webfang_core::domain::providers::ProviderRegistry::new(
            webfang_core::domain::providers::ProvidersConfig {
                providers: providers_cfg.providers,
            },
        );
        let provider_cfg = match registry.resolve_default_completion() {
            Ok(cfg) => cfg,
            Err(e) => {
                return CliExit::ConfigError(format!(
                    "--extract-with-llm activado pero no hay proveedor completion configurado: {e}"
                ));
            },
        };
        let http = match webfang_core::infrastructure::llm::provider::build_default_http_client() {
            Ok(h) => h,
            Err(e) => return CliExit::ConfigError(format!("cliente HTTP: {e}")),
        };
        let provider =
            match webfang_core::infrastructure::llm::provider::OpenAiCompatibleProvider::with_http(
                provider_cfg.clone(),
                http,
            ) {
                Ok(p) => p,
                Err(e) => return CliExit::ConfigError(format!("provider LLM: {e}")),
            };

        // Build Container and inject provider — validates startup per §8b
        let container = match webfang_core::application::container::Container::new(
            webfang_core::domain::CrawlerConfig::new(opts.url.as_url().clone()),
            webfang_core::domain::config::ScraperConfig::default(),
        )
        .await
        {
            Ok(c) => c,
            Err(e) => return CliExit::ConfigError(format!("contenedor: {e}")),
        }
        .with_llm_port(Arc::new(provider));

        // DEBE §8b: validar en startup que el puerto está disponible
        let llm_port = match container.llm_port() {
            Some(p) => p,
            None => {
                return CliExit::ConfigError(
                    "fallo interno: llm_port() es None tras inyección".to_string(),
                );
            },
        };
        Some(llm_port)
    } else {
        None
    };

    #[cfg(feature = "ai")]
    let (ai_cleaner, mut vault_ports, shared) = match build_ai_cleaner(&opts).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    #[cfg(not(feature = "ai"))]
    let mut vault_ports = webfang_core::application::container::VaultAiPorts::default();

    match resolve_remote_embedding(&opts).await {
        Ok(Some(adapter)) => {
            // Explicit remote wins over the local pool adapter: same
            // `embedding_port` slot, overwritten (not merged).
            vault_ports.embedding_port = Some(adapter);
        },
        Ok(None) => {},
        Err(e) => return e,
    }

    // Resolve Tier 2 remote embedding (separate flag, separate privacy domain)
    // Only when adaptive-selectors is enabled, but defined here so it's
    // accessible after the orchestrator call for metrics collection.
    #[cfg(feature = "adaptive-selectors")]
    let tier2_remote: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort> = {
        match resolve_tier2_remote_embedding(&opts).await {
            Ok(Some(adapter)) => Some(
                webfang_core::domain::embedding_port::Tier2EmbeddingPort::Remote(
                    adapter as Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>,
                ),
            ),
            Ok(None) => None,
            Err(e) => return e,
        }
    };
    #[cfg(not(feature = "adaptive-selectors"))]
    let _tier2_remote: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort> = None;

    #[cfg(feature = "adaptive-selectors")]
    {
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
        let semantic = {
            // Tier 2 slot priority: explicit remote > shared local. BOTH arms
            // produce `Tier2EmbeddingPort`, so `vault_ports.embedding_port` —
            // in scope right here — cannot reach `semantic_inspector`.
            let tier2_port = tier2_remote.clone().or_else(|| {
                shared.as_ref().map(|s| {
                    webfang_core::domain::embedding_port::Tier2EmbeddingPort::Local(s.clone())
                })
            });
            semantic_inspector(tier2_port, &engine_options)
        };
        #[cfg(all(not(feature = "ai"), feature = "adaptive-selectors"))]
        let semantic = {
            // Without `ai` feature, no local ONNX pool exists.
            // Tier 2 ONLY works if explicit --tier2-embedding-provider is set.
            semantic_inspector(tier2_remote.clone(), &engine_options)
        };
        // Build adaptive engine inside the cfg block so semantic and engine_options are in scope
        let adaptive_engine = build_adaptive_engine(&opts, semantic, engine_options);

        #[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
        {
            let threshold = opts.ai_config.threshold;
            let exit =
                orchestrator::run(opts, ai_cleaner, adaptive_engine, vault_ports, llm_port).await;
            // Print embedding summary if Tier 2 remote was used
            if let Some(ref tier2) = tier2_remote {
                print_embedding_summary(tier2.port(), threshold).await;
            }
            exit
        }
        #[cfg(all(not(feature = "ai"), feature = "adaptive-selectors"))]
        {
            let threshold = opts.ai_config.threshold;
            let exit = orchestrator::run(opts, adaptive_engine, vault_ports, llm_port).await;
            if let Some(ref tier2) = tier2_remote {
                print_embedding_summary(tier2.port(), threshold).await;
            }
            exit
        }
    }
    #[cfg(not(feature = "adaptive-selectors"))]
    {
        // AdaptiveSelectorOptions is gated behind adaptive-selectors feature.
        // Define the default inline for when the feature is disabled.
        use std::time::Duration;
        #[allow(dead_code)]
        struct EngineOptions {
            pub lexical_threshold: f64,
            pub escalation_lower_bound: f64,
            pub semantic_threshold: f32,
            pub cache_ttl: Duration,
            pub max_cache_entries: usize,
            pub max_concurrent_inference: usize,
        }
        impl Default for EngineOptions {
            fn default() -> Self {
                Self {
                    lexical_threshold: 0.85,
                    escalation_lower_bound: 0.70,
                    semantic_threshold: 0.75,
                    cache_ttl: Duration::from_secs(30 * 60),
                    max_cache_entries: 10_000,
                    max_concurrent_inference: 4,
                }
            }
        }
        let _engine_options = EngineOptions::default();
        // No adaptive engine to feed — keep the shared assets from becoming an
        // unused binding on the ai-only combo.
        #[cfg(all(feature = "ai", not(feature = "adaptive-selectors")))]
        let _ = shared;

        #[cfg(all(feature = "ai", not(feature = "adaptive-selectors")))]
        {
            orchestrator::run(opts, ai_cleaner, vault_ports, llm_port).await
        }
        #[cfg(all(not(feature = "ai"), not(feature = "adaptive-selectors")))]
        {
            orchestrator::run(opts, vault_ports, llm_port).await
        }
    }
}

/// Assemble the vault-search AI ports (#433) from the cleaner's shared model.
///
/// Builds the ONNX embedding adapter + Markdown chunker (always under `ai`) from
/// the semantic cleaner's shared erased engine + tokenizer — so the `--ai` path
/// loads the ONNX model exactly once, in `Single` and `Pool` modes alike (#1569)
/// — plus the SQLite note repository (under `persistence`). Embedding + chunker
/// assembly is infallible (the components are already valid); only the note
/// repository can fail, and it degrades gracefully — the port stays `None` and
/// the capability answers with an honest error rather than aborting the run.
#[cfg(feature = "ai")]
async fn build_vault_ports(
    pool: Arc<dyn webfang_ai::infrastructure_ai::InferenceEngine + Send + Sync>,
    tokenizer: Arc<webfang_ai::MiniLmTokenizer>,
) -> webfang_core::application::container::VaultAiPorts {
    use webfang_core::application::container::VaultAiPorts;

    let mut ports = VaultAiPorts::default();

    // Assemble the embedding adapter from the cleaner's shared engine + tokenizer.
    // Infallible — no model resolution happens here (that already happened once
    // inside the cleaner constructor), so there is nothing to degrade around.
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

/// Print embedding metrics summary at the end of a successful run.
///
/// Only called when Tier 2 remote embedding adapter was used (explicit
/// `--tier2-embedding-provider`). Reads the accumulated metrics and prints
/// them via `ScrapeSummary` alongside the threshold used.
#[cfg(feature = "adaptive-selectors")]
async fn print_embedding_summary(
    remote: &Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>,
    threshold_used: f32,
) {
    // Downcast to RemoteEmbeddingAdapter to access metrics()
    if let Some(remote_adapter) = remote.as_any().downcast_ref::<webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter>() {
        let metrics = remote_adapter.metrics();
        if metrics.request_count > 0 {
            use webfang_core::cli::summary::ScrapeSummary;
            use std::time::Duration;
            let _summary = ScrapeSummary::new(
                0, 0, 0, 0, 0, 0, Duration::from_secs(0),
                metrics.request_count as usize,
                metrics.prompt_tokens,
                metrics.total_tokens,
                threshold_used,
            );
            // Print only the embedding line (not the full scrape summary)
            let no_color = std::env::var("NO_COLOR").is_ok() || std::env::var("TERM").unwrap_or_default() == "dumb";
            let embedding_line = if no_color {
                format!(
                    "[EMBEDDINGS] {} requests, {} prompt tokens, {} total tokens, threshold={:.2}",
                    metrics.request_count, metrics.prompt_tokens, metrics.total_tokens, threshold_used
                )
            } else {
                format!(
                    "🧠 embeddings: {} requests, {} prompt tokens, {} total tokens, threshold={:.2}",
                    metrics.request_count, metrics.prompt_tokens, metrics.total_tokens, threshold_used
                )
            };
            println!("{embedding_line}");
        }
    }
}
