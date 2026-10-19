#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
// Explicit per docs-truthfulness audit (S5): these rustdoc lints are
// allow-by-default, so CI's `-D warnings` never reaches them. Declare intent.
#![allow(rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_doc_tests)]
#![allow(rustdoc::unescaped_backticks)]
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
#[cfg(feature = "adaptive-selectors")]
use webfang_core::application::adaptive_engine::{AdaptiveSelectorEngine, AdaptiveSelectorOptions};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::config::{load_config_defaults, resolve_config_path, ConfigDefaults};
use webfang_core::cli::error::{CliError, CliExit};
use webfang_core::cli::parse::subcommand_args;
use webfang_core::cli::preflight;
use webfang_core::cli::preflight::ArgSources;
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

/// Dispatch a parsed subcommand. Split out of `__main` so the top-level
/// function stays under the cognitive-complexity / length ratchet.
async fn dispatch_subcommand(args: &Args) -> CliExit {
    match args.subcommand.as_ref() {
        Some(Commands::Completions { shell }) => orchestrator::handle_completions(*shell),
        Some(Commands::Connect { .. }) => handle_connect(args).await,
        Some(Commands::Doctor) => handle_doctor(args).await,
        Some(Commands::Show) => handle_show(args),
        // Callers only reach here when a subcommand is present.
        None => CliExit::Success,
    }
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

    // 2. Handle subcommands. `connect` / `doctor` / `show` (issue #1933,
    //    ADR-0004 step 4) run BEFORE URL resolution: they do not touch the
    //    scrape path, and `connect`/`doctor` legitimately run with no URL.
    if args.subcommand.is_some() {
        return dispatch_subcommand(&args).await;
    }

    // 3. URL handling with interactive wizard
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
    // ADR-0004 step 5: capture the deprecation verdict BEFORE `args` is
    // consumed by the projection below. The trigger is a parse-time fact (two
    // env vars clap never sees, plus a `value_source` distinction that does
    // not survive into `AiConfig`), while the emission needs a live subscriber
    // that does not exist yet — so the answer is carried, not the text.
    let pending_notices = args.ai.pending_notices;
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
    //
    // Suited to SHORT substitution notices only: the compact formatter prints
    // the note as the message and again as the field value, which is invisible
    // at ~40 characters and unreadable at the ~600 a deprecation needs. The two
    // ADR-0004 notices below are therefore NOT routed through this buffer.
    for note in webfang_core::cli::preflight_notes::take() {
        tracing::warn!(preflight_note = %note, "{note}");
    }

    // ADR-0004 step 5, deprecation announcements (spec T4 + T5). Split out of
    // `__main` for the same reason `dispatch_subcommand` is: the complexity and
    // length ratchets (#516) apply per function, and two `if` blocks plus their
    // comment belong in a function named after what they do.
    emit_adr0004_deprecation_notices(pending_notices);

    // The `--max-tokens` deprecation warning lives HERE, not where the value is
    // resolved, for the same #796 reason the notes above are replayed: argv
    // resolution runs before `init_logging_dual`, so a `warn!` emitted there
    // would be dropped and the deprecation would be silent (ADR-0004). It is
    // emitted exactly once, at the one point that has both the provenance
    // (`AiConfig::deprecated_max_tokens`) and a live subscriber.
    //
    // Shape follows the existing env-var deprecation: English structured
    // field names, user-facing message text in Spanish, with the migration
    // target and the removal schedule stated.
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

    // The `--ai-model` shim (ADR-0004 paso 6). Same rule as `--max-tokens`
    // above, for the same reason (#796): the flag still PARSES, but the local
    // ONNX model it selected is gone, so the value is IGNORED — and a silently
    // ignored flag is precisely what this notice exists to prevent.
    emit_ai_model_shim_notice(&opts);

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

/// Render ADR-0004 step 5's in-execution deprecation announcements (spec T4 +
/// T5) as `WARN` events, if this run owes any.
///
/// MUST be called after `init_logging_dual`: argv resolution runs before the
/// subscriber exists, so a `warn!` emitted at parse time would be silently
/// dropped (#796). Which notices are owed was decided at parse time —
/// `webfang_core::cli::args::ai::PendingNotices` — because the trigger needs a
/// `value_source` distinction and two env vars that do not survive into
/// `CrawlOptions`.
///
/// Shape follows the `--max-tokens` deprecation: English structured fields
/// carrying the facts, the Spanish prose as the event message. The prose
/// appears EXACTLY once. The earlier cut of this feature routed the notices
/// through `preflight_notes`, whose replay is `warn!(preflight_note = %note,
/// "{note}")` — the compact formatter then printed the same ~600 characters
/// twice per line (message plus field value), which is invisible for the short
/// substitution notices that buffer was built for and unreadable here. The
/// fields below carry only what is meant to be QUERIED: which notice, what
/// disappears, where to migrate, where to object, when. Never a second copy
/// of the sentence.
fn emit_adr0004_deprecation_notices(pending: webfang_core::cli::args::ai::PendingNotices) {
    if pending.onnx_deprecation {
        tracing::warn!(
            notice = "onnx_deprecation",
            removed_feature = webfang_core::cli::args::ai::REMOVED_AI_FEATURE,
            removed_capability = "onnx_local",
            replacement = "webfang connect|webfang doctor|webfang show",
            removed_in_version = "3.0.0",
            shims_retire_in_version = "3.2.0",
            "{}",
            webfang_core::cli::args::ai::ONNX_DEPRECATION_NOTICE,
        );
    }
    if let Some(threshold) = pending.orphan_threshold {
        tracing::warn!(
            notice = "orphan_relevance_threshold",
            // `display`, not the raw f32: tracing's `Value` impl for `f32`
            // prints the IEEE-754 round-trip form (`0.30000001192092896`), which
            // is a second thing to explain to an operator. The field stays a
            // string in the JSONL — still queryable, and it reads `0.3`.
            threshold_used = tracing::field::display(format_args!("{threshold}")),
            calibrated_for = "granite",
            replacement = "--threshold|WEBFANG_THRESHOLD",
            "{}",
            webfang_core::cli::args::ai::orphan_threshold_notice(threshold),
        );
    }
}

/// Emit the ONE `--ai-model` migration notice (ADR-0004 paso 6).
///
/// `--ai-model` — and its env vars `WEBFANG_AI_MODEL_ID` / `AI_MODEL_ID` —
/// still PARSE: they are shims, not errors. But the local ONNX model they
/// used to select is GONE, so the value is IGNORED, and a silently ignored
/// flag is exactly what this notice exists to prevent. The operator is told,
/// in Spanish, which knob replaced it: the remote provider.
///
/// Fires ONLY when the operator actually used the shim, and exactly once per
/// run:
/// - the flag or `WEBFANG_AI_MODEL_ID` — both reach `AiConfig::model`
///   (clap binds the env var to the arg), and
/// - the legacy `AI_MODEL_ID`, which is NOT clap-bound and therefore never
///   reaches `CrawlOptions`; it is read here, from the same constant the
///   core module uses.
///
/// Shape follows the `--max-tokens` deprecation: English structured fields
/// carrying the facts, the Spanish prose as the event message.
fn emit_ai_model_shim_notice(opts: &CrawlOptions) {
    let requested = opts.ai_config.model.trim();
    let legacy = std::env::var(webfang_core::cli::args::ai::LEGACY_AI_MODEL_ENV).ok();
    if requested.is_empty() && legacy.is_none() {
        return;
    }
    let requested_model = if requested.is_empty() {
        legacy.as_deref().unwrap_or_default()
    } else {
        requested
    };
    tracing::warn!(
        flag = "--ai-model",
        env = "WEBFANG_AI_MODEL_ID",
        legacy_env = webfang_core::cli::args::ai::LEGACY_AI_MODEL_ENV,
        replacement = "--embedding-provider",
        requested_model,
        "el modelo local ya no existe: `--ai-model` (y `WEBFANG_AI_MODEL_ID` / \
         `AI_MODEL_ID`) se ignoran. La limpieza semántica ahora la resuelve un \
         provider remoto: pasá `--embedding-provider <id>` (o exportá \
         `WEBFANG_EMBEDDING_PROVIDER`).",
    );
}

/// Handle the `connect` subcommand (issue #1933, ADR-0004 step 4).
///
/// The variant carries the endpoint, the required model, and an optional
/// provider id. With no id, a `connect-<hex>` name is generated from the URL
/// so repeated runs against the same endpoint are idempotent-ish and readable.
async fn handle_connect(args: &Args) -> CliExit {
    let Some(Commands::Connect { url, model, id }) = args.subcommand.as_ref() else {
        return CliExit::UsageError("connect invocado sin sus argumentos".into());
    };
    let id = id
        .clone()
        .unwrap_or_else(|| generated_provider_id(url.as_str()));
    let config_path = resolve_config_path();
    webfang_core::cli::commands::connect::run(url.as_url(), &id, model, &config_path).await
}

/// Handle the `doctor` subcommand: diagnose the embedding path from config.
async fn handle_doctor(args: &Args) -> CliExit {
    let config_path = resolve_config_path();
    let sub = subcommand_args(args);
    webfang_core::cli::commands::doctor::run(&config_path, &sub).await
}

/// Handle the `show` subcommand: print the effective profile + budget.
fn handle_show(args: &Args) -> CliExit {
    let config_path = resolve_config_path();
    let sub = subcommand_args(args);
    webfang_core::cli::commands::show::run(&sub, &config_path)
}

/// A stable, readable provider id derived from the endpoint URL.
///
/// FNV-1a over the URL, formatted as 64 bits: deterministic across runs and
/// toolchain versions (unlike `DefaultHasher`, which std documents as not
/// stable across Rust releases), and wide enough that two endpoints do not
/// collide in any realistic config. Not a cryptographic choice — a naming
/// convenience the operator can override with `--id`. Because the id is
/// derived from the URL, a repeat `connect` of the same URL names the same
/// record; `write_provider` then MERGES into it field by field (refreshing the
/// probe-owned keys, preserving the operator-owned ones) instead of replacing
/// it or appending a duplicate.
fn generated_provider_id(url: &str) -> String {
    // FNV-1a, 64-bit.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("connect-{hash:016x}")
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

/// `generated_provider_id` is the fallback identity `webfang connect` writes
/// when the operator gave no `--id`: the SAME endpoint must always resolve to
/// the SAME provider id, or a reconnect would append a second record for one
/// endpoint instead of updating the existing one.
#[cfg(test)]
mod generated_provider_id_tests {
    use super::generated_provider_id;

    #[test]
    fn the_same_url_always_yields_the_same_id() {
        let url = "https://api.example.test/v1";
        assert_eq!(
            generated_provider_id(url),
            generated_provider_id(url),
            "the FNV-1a hash must be deterministic across calls"
        );
    }

    #[test]
    fn the_id_is_connect_prefixed_with_16_lowercase_hex_digits() {
        let id = generated_provider_id("https://api.example.test/v1");
        let hash = id
            .strip_prefix("connect-")
            .unwrap_or_else(|| panic!("the id must carry the connect- prefix, got: {id}"));
        assert_eq!(hash.len(), 16, "a 64-bit hash is 16 hex digits, got: {id}");
        assert!(
            hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "the hash must be lowercase hex, got: {id}"
        );
    }

    #[test]
    fn different_urls_yield_different_ids() {
        assert_ne!(
            generated_provider_id("https://api.example.test/v1"),
            generated_provider_id("https://other.example.test/v1"),
            "two distinct endpoints must not collide on one provider id"
        );
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
    /// `None`, so this marker is the cascade's own record of which tier
    /// answered.
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
    /// Ungated since ADR-0004 paso 6: the fixture needs only the core
    /// `EmbeddingPort` trait, and the test it serves (the Tier 2 slot stays
    /// bound to the port it was handed) is a TYPE-level invariant that
    /// outlived the local ONNX pool.
    struct CountingEmbedder {
        inner: Arc<CountingInner>,
    }

    struct CountingInner {
        vec: Vec<f32>,
        embedded: std::sync::atomic::AtomicUsize,
    }

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
    ///
    /// The LOCAL pool it models is gone (ADR-0004 paso 6); what the test still
    /// pins is the TYPE separation — `Tier2EmbeddingPort::Local` and the vault
    /// slot are different types, so the vault slot can never reach the
    /// inspector by accident (issue #1977).
    #[test]
    fn tier2_inspector_stays_bound_to_the_local_port() {
        let local = Arc::new(CountingEmbedder::new(vec![1.0, 0.0]));
        let remote = Arc::new(CountingEmbedder::new(vec![0.0, 1.0]));

        let shared =
            Some(webfang_core::domain::embedding_port::LocalEmbeddingPort::new(local.as_port()));

        // Models the composition order: the vault slot holds a DIFFERENT
        // provider than the one the inspector was handed. The inspector only
        // ever sees `shared`.
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
/// The `semantic` parameter carries the Tier 2 provider when the run resolved
/// one (an explicit `--tier2-embedding-provider`); `None` degrades the engine
/// to Tier 1 (lexical) repair. `options` is the shared engine configuration —
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
/// Gated on `adaptive-selectors` alone: it is the feature that owns the
/// `AdaptiveSelectorOptions` type, and the only consumer of this function.
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

/// Build the AI semantic cleaner and the vault-search ports (ADR-0004 paso 6).
///
/// `--clean-ai` carries no local ONNX model: the cleaner is built over
/// whatever `EmbeddingPort` the run resolved — remote only, since ADR-0004
/// removed the local stack. The port is the SAME one the vault slot gets
/// (ONE resolution, ONE probe dial per run), resolved with the pattern that
/// already fills that slot: `--embedding-provider` (flag) >
/// `WEBFANG_EMBEDDING_PROVIDER` (env) > the configured default embedding
/// provider. A `--clean-ai` run with no resolvable remote provider is a
/// `ConfigError` in Spanish naming `--embedding-provider` (exit 78) — never
/// a silent no-op.
///
/// WITHOUT `--clean-ai` this returns no cleaner and the ports the run would
/// have had anyway: today's default build never carried an AI cleaner, and
/// the vault slot is wired only when a provider resolves.
fn build_ai_cleaner(
    opts: &CrawlOptions,
    embedding_port: Option<Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>>,
) -> Result<
    (
        Option<Arc<dyn SemanticCleaner>>,
        webfang_core::application::container::VaultAiPorts,
    ),
    CliExit,
> {
    let mut ports = webfang_core::application::container::VaultAiPorts::default();
    if let Some(port) = &embedding_port {
        ports.embedding_port = Some(Arc::clone(port));
    }

    if !opts.ai || opts.export.dry_run {
        return Ok((None, ports));
    }

    let Some(port) = embedding_port else {
        return Err(CliExit::ConfigError(
            "--clean-ai requiere un provider de embeddings remoto: pasá \
             '--embedding-provider <id>' (o exportá WEBFANG_EMBEDDING_PROVIDER) \
             con un provider configurado en el archivo de config."
                .to_string(),
        ));
    };

    let cleaner = webfang_core::infrastructure::semantic_cleaner::SemanticCleanerImpl::new(
        port,
        opts.ai_config.max_chars,
        opts.ai_config.threshold,
    );
    let cleaner: Option<Arc<dyn SemanticCleaner>> = Some(Arc::new(cleaner));
    ports.cleaner = cleaner.clone();
    Ok((cleaner, ports))
}

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

/// Resolve the embedding port for the whole run (ADR-0004 paso 6).
///
/// An explicit `--embedding-provider` always resolves (fail fast even
/// without `--clean-ai`, so vault search works with no local model); with
/// `--clean-ai` the default slot may also select a remote provider.
/// Otherwise `None` with no config load and no probe — plain runs never dial
/// an embeddings endpoint.
///
/// The port is returned ERASED (`Arc<dyn EmbeddingPort>`), not as the
/// concrete adapter: the `--clean-ai` cleaner is built over the trait object
/// and the vault slot stores the same one, so the endpoint is probed exactly
/// once per run.
async fn resolve_remote_embedding(
    opts: &CrawlOptions,
) -> Result<Option<std::sync::Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>>, CliExit>
{
    if opts.embedding_provider.is_none() && !opts.ai {
        return Ok(None);
    }
    let providers = load_providers();
    webfang_core::cli::llm_wire::build_embedding_provider(opts, &providers, opts.ai_config.offline)
        .await
        .map(|adapter| {
            adapter.map(|a| {
                a as std::sync::Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>
            })
        })
}

/// Resolve the remote embedding adapter for the Tier 2 semantic inspector (ADR-0004).
///
/// Separate from `resolve_remote_embedding` on purpose: vault search and
/// Tier 2 repair are distinct privacy domains (issue #1977). An explicit
/// `--tier2-embedding-provider` always resolves (fail fast even without
/// `--clean-ai`). Without it, Tier 2 has NO provider at all: the local ONNX
/// pool that used to serve this slot is gone (ADR-0004 paso 6), and the
/// vault slot's default embedding provider is never adopted, so a remote
/// provider configured for vault search cannot open a Tier 2 egress (#1981).
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
    // Absent id → the local pool (#1981). Return before loading providers, so
    // the vault-search default can never be adopted here and no config is read.
    if opts.tier2_embedding_provider.is_none() {
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
/// The AI cleaner is built FIRST (#702): the adaptive engine needs a
/// semantic provider to wire Tier 2 repair. The argument list passed to the
/// orchestrator depends on which optional features are compiled in, so each
/// combination is spelled out explicitly.
///
/// If `--extract-with-llm` is set, builds the LLM provider from config,
/// injects it into a Container, and validates startup (caducidad del DEBE §8b:
/// el binario DEBE validar llm_port().is_some() en startup cuando el flag está
/// activo).
#[allow(clippy::too_many_lines)]
async fn build_and_run(opts: CrawlOptions) -> CliExit {
    // Build LLM provider if --extract-with-llm is set.
    // This is NOT gated by the `ai` feature — the remote provider port lives
    // in webfang_core unconditionally (docs/architecture/ai-providers-design.md §8).
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

    // Resolve the embedding slot ONCE (ADR-0004 paso 6): the vault-search
    // slot and the `--clean-ai` cleaner are the SAME port, so the endpoint
    // is probed once per run instead of twice.
    let embedding_port = match resolve_remote_embedding(&opts).await {
        Ok(port) => port,
        Err(e) => return e,
    };

    let (ai_cleaner, vault_ports) = match build_ai_cleaner(&opts, embedding_port) {
        Ok(v) => v,
        Err(e) => return e,
    };

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
        // Tier 2 has its OWN privacy domain (issue #1977): the inspector
        // binds ONLY to an explicit `--tier2-embedding-provider`. The vault
        // slot's provider is never adopted here — it is a DIFFERENT type
        // (`Tier2EmbeddingPort` vs `Option<Arc<dyn EmbeddingPort>>`), so
        // `vault_ports.embedding_port` cannot reach `semantic_inspector` even
        // by accident. The local ONNX pool that used to serve this slot is
        // gone (ADR-0004 paso 6), so the explicit remote is the only arm left.
        let semantic = semantic_inspector(tier2_remote.clone(), &engine_options);
        // Build adaptive engine inside the cfg block so semantic and engine_options are in scope
        let adaptive_engine = build_adaptive_engine(&opts, semantic, engine_options);

        let threshold = opts.ai_config.threshold;
        let exit =
            orchestrator::run(opts, ai_cleaner, adaptive_engine, vault_ports, llm_port).await;
        // Print embedding summary if Tier 2 remote was used
        if let Some(ref tier2) = tier2_remote {
            print_embedding_summary(tier2.port(), threshold).await;
        }
        exit
    }
    #[cfg(not(feature = "adaptive-selectors"))]
    {
        // No adaptive engine to feed — call orchestrator without it.
        // The `adaptive-selectors` feature gates the `AdaptiveSelectorOptions` type,
        // so we don't define a local copy here anymore.
        orchestrator::run(opts, ai_cleaner, vault_ports, llm_port).await
    }
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
