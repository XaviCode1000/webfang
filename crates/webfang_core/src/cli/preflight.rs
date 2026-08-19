//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.

use std::path::PathBuf;
use std::process::Command;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::domain::JsStrategy;
use crate::{Args, CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};

// ============================================================================
// Config Defaults Merge
// ============================================================================

/// Apply config file defaults where CrawlOptions fields are still at their hardcoded defaults.
///
/// Precedence: CLI > env (handled by clap) > config file > struct defaults.
pub fn apply_config_defaults(mut opts: CrawlOptions, config: &ConfigDefaults) -> CrawlOptions {
    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "markdown" => OutputFormat::Markdown,
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if opts.export.output_format == OutputFormat::Markdown && target != OutputFormat::Markdown {
            opts.export.output_format = target;
        }
    }

    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "jsonl" => ExportFormat::Jsonl,
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if opts.export.export_format == ExportFormat::Jsonl && target != ExportFormat::Jsonl {
            opts.export.export_format = target;
        }
    }

    if let Some(ref c) = config.concurrency {
        // ConcurrencyConfig doesn't implement PartialEq, so check via is_auto()
        if opts.network.concurrency.is_auto() {
            opts.network.concurrency = ConcurrencyConfig::from(c.as_str());
        }
    }

    if let Some(ref s) = config.selector {
        if opts.crawl.selector == "body" {
            opts.crawl.selector = s.clone();
        }
    }

    if let Some(n) = config.max_pages {
        if opts.crawl.max_pages == 10 {
            opts.crawl.max_pages = n;
        }
    }

    if let Some(n) = config.delay_ms {
        if opts.network.delay_ms == 1000 {
            opts.network.delay_ms = n;
        }
    }

    if let Some(v) = config.use_sitemap {
        if !opts.crawl.use_sitemap && v {
            opts.crawl.use_sitemap = v;
        }
    }

    // --sitemap-url implies --use-sitemap (#491): an explicit sitemap URL
    // logically enables sitemap discovery, so the user shouldn't need both flags.
    if opts.crawl.sitemap_url.is_some() {
        opts.crawl.use_sitemap = true;
    }

    if let Some(v) = config.ignore_waf {
        if !opts.crawl.ignore_waf && v {
            opts.crawl.ignore_waf = v;
        }
    }

    // Obsidian config — trim whitespace from tags
    for tag in opts.export.obsidian_tags.iter_mut() {
        *tag = tag.trim().to_string();
    }
    opts.export.obsidian_tags.retain(|t| !t.is_empty());

    if let Some(ref tags_str) = config.obsidian_tags {
        if opts.export.obsidian_tags.is_empty() {
            opts.export.obsidian_tags = tags_str
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
        }
    }
    if let Some(v) = config.obsidian_wiki_links {
        if !opts.export.obsidian_wiki_links && v {
            opts.export.obsidian_wiki_links = v;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if !opts.export.obsidian_relative_assets && v {
            opts.export.obsidian_relative_assets = v;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if opts.export.obsidian_vault.is_none() {
            opts.export.obsidian_vault = Some(PathBuf::from(vault));
        }
    }

    opts
}

// ============================================================================
// JS strategy dependency preflight (#685)
// ============================================================================

/// Chrome binary candidates probed for `--js-strategy full` (#685).
///
/// The audit spec named `google-chrome` only, but Linux distributions ship
/// the engine under several names, so one missing distro binary must not
/// mask an installed Chrome. Order matters: the first binary that reports a
/// version wins.
const DEFAULT_CHROME_CANDIDATES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium-browser",
    "chromium",
];

/// Preflight: verify the local environment can satisfy the configured JS
/// strategy before any crawl starts (#685, #758, #787).
///
/// Strategy-specific checks:
///
/// - [`Static`](crate::domain::JsStrategy::Static): no external binary is
///   needed — returns immediately without touching disk or spawning
///   processes.
/// - [`Hybrid`](crate::domain::JsStrategy::Hybrid) (#787, #793): Layer 2 shells
///   out to Obscura (`ObscuraDownloader`), so the configured
///   `--obscura-binary` (default `obscura`) must exist. A value with a path
///   separator must exist as a file; a bare name must resolve via `PATH`.
///   This turns a missing binary into a clean config error (exit 78)
///   instead of a silent Layer-2 fallback mid-crawl. Once resolved, the
///   binary must report a version of at least [`MINIMUM_OBSCURA_VERSION`]
///   (#793): an older dump format would silently feed the wrong content
///   shape to Layer 2. A failing or unreadable `--version` probe degrades
///   to a warning (best-effort for unknown builds).
/// - [`Full`](crate::domain::JsStrategy::Full) renders pages through
///   Chromiumoxide, which spawns a real Chrome/Chromium binary. Two
///   independent preconditions are checked, in order:
///
///   1. **Compile-time capability** (#758): the binary must have been built
///      with the `chromium` feature. Without it, `ChromiumoxideDownloader`
///      is a stub that fails mid-crawl with an instruction the CLI binary
///      cannot follow. This is a build-configuration error, checked first
///      because probing the PATH is pointless when the binary cannot render
///      JS at all.
///   2. **Runtime environment**: probing the installed candidates turns a
///      missing browser into a clean config error (exit 78) instead of a
///      confusing mid-crawl browser launch failure.
///
/// Runs once, in a synchronous context, before crawl start — a brief
/// blocking `--version` probe (Full and Hybrid) is acceptable there.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the strategy is
/// [`Hybrid`](crate::domain::JsStrategy::Hybrid) and the configured Obscura
/// binary does not exist (neither as a path nor on `PATH`) or reports a
/// version older than [`MINIMUM_OBSCURA_VERSION`] (#793), or when it is
/// [`Full`](crate::domain::JsStrategy::Full) and either the binary was
/// built without the `chromium` feature, or no Chrome/Chromium candidate
/// on `PATH` reports a version.
pub fn check_js_dependencies(opts: &CrawlOptions) -> Result<(), CliExit> {
    // The PATH value is injected so the core check stays pure and testable —
    // tests pass a controlled PATH instead of mutating process-global env.
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    check_js_dependencies_with(
        &DEFAULT_CHROME_CANDIDATES,
        cfg!(feature = "chromium"),
        &path_value,
        opts,
    )
}

/// Candidate-, feature-, and PATH-injectable core of
/// [`check_js_dependencies`] — tests probe binaries that exist
/// deterministically in CI (e.g. `true`) instead of the real Chrome names,
/// inject the feature flag because `cfg!` cannot be toggled per test, and
/// inject the `PATH` value so the Hybrid Obscura lookup never races with
/// concurrent tests over the process-global environment (#787).
fn check_js_dependencies_with(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    match opts.network.js_strategy {
        // Static crawls with wreq only — no external binary to check.
        JsStrategy::Static => Ok(()),
        // Hybrid escalates through Obscura: fail fast when the binary is
        // missing instead of letting Layer 2 fail in the middle of the crawl
        // (#787).
        JsStrategy::Hybrid => check_obscura_binary(&opts.network.obscura_binary, path_value),
        JsStrategy::Full => check_chrome_binary(candidates, chromium_enabled),
    }
}

/// Full-strategy check (#685, #758): `chromium` feature + a Chrome/Chromium
/// candidate that reports a version.
fn check_chrome_binary(candidates: &[&str], chromium_enabled: bool) -> Result<(), CliExit> {
    if !chromium_enabled {
        return Err(CliExit::ConfigError(
            "--js-strategy full requiere un binario compilado con la feature `chromium`; \
                     recompilá con --features chromium o usá --js-strategy hybrid"
                .into(),
        ));
    }

    let chrome_present = candidates
        .iter()
        .any(|binary| matches!(binary_reports_version(binary), Ok(s) if s.success()));
    if chrome_present {
        tracing::info!(strategy = "full", "chrome_dependency_checked");
        return Ok(());
    }

    Err(CliExit::ConfigError(
        "--js-strategy full requiere Google Chrome instalado".into(),
    ))
}

/// Whether `binary` names an explicit path (absolute or relative) rather
/// than a bare executable name resolved from `PATH`.
fn has_path_separator(binary: &str) -> bool {
    binary.contains('/') || binary.contains('\\')
}

/// Scan `PATH` entries (in order) for a file named `name` — the same lookup
/// the OS performs for a bare executable name (#787).
///
/// Pure: takes the `PATH` value as input so tests control the search space
/// without touching the process-global environment.
fn resolve_executable_in_path(name: &str, path_value: &str) -> Option<PathBuf> {
    if name.is_empty() || has_path_separator(name) {
        return None;
    }
    std::env::split_paths(path_value)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Resolve the configured Obscura binary to an existing file.
///
/// A value with a path separator must exist as a file exactly as given; a
/// bare name must resolve through `PATH` (#787).
fn resolve_obscura_binary(binary: &str, path_value: &str) -> Option<PathBuf> {
    if has_path_separator(binary) {
        PathBuf::from(binary)
            .is_file()
            .then(|| PathBuf::from(binary))
    } else {
        resolve_executable_in_path(binary, path_value)
    }
}

/// Hybrid-strategy check (#787): the configured Obscura binary exists.
fn check_obscura_binary(binary: &str, path_value: &str) -> Result<(), CliExit> {
    match resolve_obscura_binary(binary, path_value) {
        Some(resolved) => check_obscura_version(binary, &resolved),
        None if has_path_separator(binary) => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario obscura: la ruta \"{binary}\" no existe \
             o no es un archivo; verificá --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
        None => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere el binario \"{binary}\": no se encontró en PATH; \
             instalalo o configurá una ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}

/// Minimum Obscura version for the Layer 2 dump-format contract (#793):
/// `obscura fetch --dump html` was verified in 0.2.0; an older binary may
/// change dump semantics and silently feed the wrong content shape to Layer 2.
const MINIMUM_OBSCURA_VERSION: (u64, u64, u64) = (0, 2, 0);

/// User-facing spelling of [`MINIMUM_OBSCURA_VERSION`] for Spanish errors.
const MINIMUM_OBSCURA_VERSION_STR: &str = "0.2.0";

/// Verdict of the Obscura version assessment (#793).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionVerdict {
    /// Parsed version is >= the minimum.
    Meets,
    /// Parsed version is below the minimum.
    TooOld,
    /// Probe failed or output was unparseable — degrade, do not block.
    Unknown,
}

/// Parse a pure MAJOR.MINOR.PATCH token into a semantic triple.
///
/// Tolerates a leading `v`/`V`, a `-`/`+` suffix on the patch segment
/// (`0.2.0-rc.1`, `0.2.0+build`), and extra dotted segments (ignored).
fn parse_version_token(token: &str) -> Option<(u64, u64, u64)> {
    let cleaned = token.strip_prefix(['v', 'V']).unwrap_or(token);
    let mut parts = cleaned.splitn(4, '.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?.split(['-', '+']).next()?;
    Some((major.parse().ok()?, minor.parse().ok()?, patch.parse().ok()?))
}

/// Extract the first semver-like version from raw `--version` output (#793).
///
/// Pure: no process or environment access — unit-tested without mutation.
fn parse_obscura_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(parse_version_token)
}

/// Classify an optional parsed version against the minimum (#793). Pure.
fn assess_obscura_version(parsed: Option<(u64, u64, u64)>) -> VersionVerdict {
    match parsed {
        Some(version) if version >= MINIMUM_OBSCURA_VERSION => VersionVerdict::Meets,
        Some(_) => VersionVerdict::TooOld,
        None => VersionVerdict::Unknown,
    }
}

/// Run `<resolved> --version` once and parse its output (#793).
///
/// Returns `None` when the probe cannot run, exits non-zero, or prints no
/// semver-like token on stdout or stderr. Blocking spawn: acceptable once,
/// pre-crawl (same pattern as [`binary_reports_version`] for Full).
fn probe_obscura_version(resolved: &std::path::Path) -> Option<(u64, u64, u64)> {
    let output = Command::new(resolved).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_obscura_version(&stdout)
        .or_else(|| parse_obscura_version(&String::from_utf8_lossy(&output.stderr)))
}

/// Version half of the Hybrid check (#793): enforce the minimum contract on
/// a resolved binary. An unreadable build degrades to a warning (unknown
/// custom builds must not be hard-blocked); a parseable older version fails
/// fast with exit 78.
fn check_obscura_version(binary: &str, resolved: &std::path::Path) -> Result<(), CliExit> {
    let parsed = probe_obscura_version(resolved);
    let version = parsed.map_or_else(
        || "unknown".to_string(),
        |(major, minor, patch)| format!("{major}.{minor}.{patch}"),
    );

    match assess_obscura_version(parsed) {
        VersionVerdict::Meets => {
            tracing::info!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                version = %version,
                "obscura_dependency_checked"
            );
            Ok(())
        },
        VersionVerdict::Unknown => {
            warn!(
                strategy = "hybrid",
                binary = %binary,
                resolved = %resolved.display(),
                "obscura_version_unreadable: --version probe failed or unparseable — \
                 continuing best-effort"
            );
            Ok(())
        },
        VersionVerdict::TooOld => Err(CliExit::ConfigError(format!(
            "--js-strategy hybrid requiere obscura {MINIMUM_OBSCURA_VERSION_STR} o superior: \
             el binario \"{binary}\" reporta la versión {version}; actualizalo o cambiá la \
             ruta con --obscura-binary o WEBFANG_OBSCURA_BINARY"
        ))),
    }
}

/// Preflight: `--elastic` must have at least one wirable vector sink (#695).
///
/// `--elastic` and `--output-vectors` are orthogonal vector destinations
/// (#636): the SQLite sink only exists under the `persistence` feature,
/// while the JSONL stream sink is available in every build. Without
/// `persistence` AND without `--output-vectors`, `--elastic` would wire no
/// sink at all and the run would silently report success with no artifact.
///
/// Fail fast instead: an explicit request the binary cannot honor is a
/// configuration error (exit 78), never a silent no-op.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--elastic` is
/// enabled, no `--output-vectors` path was given, and the binary was built
/// without the `persistence` feature.
pub fn check_elastic_sink(opts: &CrawlOptions) -> Result<(), CliExit> {
    if !opts.elastic.enabled {
        return Ok(());
    }
    if opts.elastic.output_vectors.is_some() {
        return Ok(());
    }
    if cfg!(feature = "persistence") {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        "--elastic requiere un destino de vectores: este binario fue compilado sin la \
             feature `persistence` (sink SQLite); use --output-vectors <ruta> o un binario \
             compilado con persistencia"
            .into(),
    ))
}

/// Preflight: `--clean-ai` on a binary built WITHOUT the `ai` feature must
/// fail before any network request (#761).
///
/// Previously the check lived only in the export flow, so a non-AI build
/// downloaded and extracted the whole page before erroring out. This mirrors
/// the #685 pattern: a build-configuration problem is a config error (exit 78)
/// detected before the crawl starts.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] when `clean_ai` is requested and
/// the `ai` feature is not compiled in.
pub fn check_clean_ai_feature(opts: &CrawlOptions) -> Result<(), CliExit> {
    check_clean_ai_feature_with(cfg!(feature = "ai"), opts)
}

/// Feature-injectable core of [`check_clean_ai_feature`] — `cfg!` cannot be
/// toggled per test.
fn check_clean_ai_feature_with(ai_enabled: bool, opts: &CrawlOptions) -> Result<(), CliExit> {
    if !opts.ai || ai_enabled {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        "--clean-ai requiere un binario compilado con la feature `ai`; \
             recompilá con --features ai"
            .into(),
    ))
}

/// Spawn `<binary> --version` once and report its exit status.
///
/// A missing or non-executable binary surfaces as an `io::Error`, which the
/// caller treats as "not installed". Output is discarded — only the exit
/// status matters, and the probe must stay silent on the terminal.
fn binary_reports_version(binary: &str) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new(binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
}

// ============================================================================
// TUI Config Merge
// ============================================================================

/// Apply config values from TUI form to CrawlOptions.
///
/// This runs after config_tui returns user-submitted values.
/// Precedence: TUI values > CLI args (they override what was passed).
pub fn apply_tui_config(mut opts: CrawlOptions, config_values: &serde_json::Value) -> CrawlOptions {
    use crate::ExportFormat as E;
    use crate::OutputFormat as O;

    // Output directory
    if let Some(output) = config_values.get("output").and_then(|v| v.as_str()) {
        opts.export.output_dir = PathBuf::from(output);
    }

    // Output format (markdown, json, text)
    if let Some(fmt) = config_values.get("format").and_then(|v| v.as_str()) {
        opts.export.output_format = match fmt {
            "json" => O::Json,
            "text" => O::Text,
            _ => O::Markdown,
        };
    }

    // Export format (jsonl, vector, auto)
    if let Some(fmt) = config_values.get("export_format").and_then(|v| v.as_str()) {
        opts.export.export_format = match fmt {
            "vector" => E::Vector,
            "auto" => E::Auto,
            _ => E::Jsonl,
        };
    }

    // Discovery: use_sitemap
    if let Some(v) = config_values.get("use_sitemap").and_then(|v| v.as_bool()) {
        opts.crawl.use_sitemap = v;
    }

    // Discovery: max_pages
    if let Some(v) = config_values.get("max_pages").and_then(|v| v.as_str()) {
        if let Ok(n) = v.parse() {
            opts.crawl.max_pages = n;
        }
    }

    // Crawler: max_depth
    if let Some(v) = config_values.get("max_depth").and_then(|v| v.as_str()) {
        if let Ok(n) = v.parse() {
            opts.crawl.max_depth = n;
        }
    }

    // Behavior: download_images
    if let Some(v) = config_values
        .get("download_images")
        .and_then(|v| v.as_bool())
    {
        opts.network.download_images = v;
    }

    // Behavior: download_documents
    if let Some(v) = config_values
        .get("download_documents")
        .and_then(|v| v.as_bool())
    {
        opts.network.download_documents = v;
    }

    // Obsidian: obsidian_wiki_links
    if let Some(v) = config_values
        .get("obsidian_wiki_links")
        .and_then(|v| v.as_bool())
    {
        opts.export.obsidian_wiki_links = v;
    }

    // Obsidian: vault path
    if let Some(vault) = config_values.get("vault").and_then(|v| v.as_str()) {
        if !vault.is_empty() {
            opts.export.obsidian_vault = Some(PathBuf::from(vault));
        }
    }

    // Obsidian: quick_save
    if let Some(v) = config_values.get("quick_save").and_then(|v| v.as_bool()) {
        opts.export.quick_save = v;
    }

    // AI: clean_ai from config file → CrawlOptions.ai (wired to ExportConfig.clean_ai)
    if let Some(v) = config_values.get("clean_ai").and_then(|v| v.as_bool()) {
        opts.ai = v;
    }

    opts
}

// ============================================================================
// TUI Config Merge — Args variant (for pre-conversion use in main.rs)
// ============================================================================

/// Apply config values from TUI form to Args.
///
/// This runs on Args before conversion to CrawlOptions, because the TUI
/// Apply TUI config values to Args.
///
/// Handles all 39 fields from CollapsibleConfig.
/// Only applies values that are present in the JSON (non-null, non-empty).
/// Apply config values from the TUI form to `Args`.
///
/// Pure field mapping. Each logical group is applied by a dedicated helper so
/// the orchestrator stays linear and auditably simple (issue #516).
pub fn apply_tui_config_args(mut args: Args, config_values: &serde_json::Value) -> Args {
    apply_tui_target(&mut args, config_values);
    apply_tui_output(&mut args, config_values);
    apply_tui_discovery(&mut args, config_values);
    apply_tui_crawler(&mut args, config_values);
    apply_tui_network(&mut args, config_values);
    apply_tui_download(&mut args, config_values);
    apply_tui_obsidian(&mut args, config_values);
    #[cfg(feature = "ai")]
    apply_tui_ai(&mut args, config_values);
    apply_tui_advanced(&mut args, config_values);
    args
}

// ============================================================================
// TUI config merge — typed field helpers.
//
// The macros are module-level and parameterized by the config `Value` so each
// group lives in a small, independently auditable function instead of one
// 76-complexity blob (issue #516).
// ============================================================================

macro_rules! apply_str {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                $field = v.to_string();
            }
        }
    };
}

macro_rules! apply_str_opt {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                $field = Some(v.to_string());
            }
        }
    };
}

macro_rules! apply_path_opt {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                $field = Some(PathBuf::from(v));
            }
        }
    };
}

macro_rules! apply_path {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                $field = PathBuf::from(v);
            }
        }
    };
}

macro_rules! apply_bool {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_bool()) {
            $field = v;
        }
    };
}

macro_rules! apply_u64 {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if let Ok(n) = v.parse() {
                $field = n;
            }
        }
    };
}

macro_rules! apply_usize {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if let Ok(n) = v.parse() {
                $field = n;
            }
        }
    };
}

macro_rules! apply_u8 {
    ($value:expr, $key:expr, $field:expr) => {
        if let Some(v) = $value.get($key).and_then(|v| v.as_str()) {
            if let Ok(n) = v.parse() {
                $field = n;
            }
        }
    };
}

fn apply_tui_target(args: &mut Args, value: &serde_json::Value) {
    apply_str_opt!(value, "url", args.crawler.url);
    apply_str!(value, "selector", args.crawler.selector);
}

fn apply_tui_output(args: &mut Args, value: &serde_json::Value) {
    apply_path!(value, "output", args.export.output);
    if let Some(fmt) = value.get("format").and_then(|v| v.as_str()) {
        args.export.format = match fmt {
            "json" => crate::domain::config::OutputFormat::Json,
            "text" => crate::domain::config::OutputFormat::Text,
            _ => crate::domain::config::OutputFormat::Markdown,
        };
    }
    if let Some(fmt) = value.get("export_format").and_then(|v| v.as_str()) {
        args.export.export_format = match fmt {
            "vector" => crate::domain::config::ExportFormat::Vector,
            "auto" => crate::domain::config::ExportFormat::Auto,
            _ => crate::domain::config::ExportFormat::Jsonl,
        };
    }
}

fn apply_tui_discovery(args: &mut Args, value: &serde_json::Value) {
    apply_bool!(value, "use_sitemap", args.crawler.use_sitemap);
    apply_str_opt!(value, "sitemap_url", args.crawler.sitemap_url);
    apply_usize!(value, "max_pages", args.crawler.max_pages);
    apply_u8!(value, "max_depth", args.crawler.max_depth);
    apply_u8!(value, "sitemap_depth", args.crawler.sitemap_depth);
}

fn apply_tui_crawler(args: &mut Args, value: &serde_json::Value) {
    apply_u64!(value, "timeout_secs", args.crawler.timeout_secs);
    apply_u64!(value, "max_retries", args.crawler.max_retries);
    apply_u64!(value, "delay_ms", args.crawler.delay_ms);
    if let Some(v) = value.get("concurrency").and_then(|v| v.as_str()) {
        if v == "auto" {
            args.crawler.concurrency = crate::ConcurrencyConfig::default();
        } else if let Ok(n) = v.parse::<usize>() {
            args.crawler.concurrency = crate::ConcurrencyConfig::new(n);
        }
    }
    if let Some(v) = value.get("include_pattern").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            args.crawler.include_patterns = v.split(',').map(String::from).collect();
        }
    }
    if let Some(v) = value.get("exclude_pattern").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            args.crawler.exclude_patterns = v.split(',').map(String::from).collect();
        }
    }
}

fn apply_tui_network(args: &mut Args, value: &serde_json::Value) {
    apply_str_opt!(value, "user_agent", args.crawler.user_agent);
    apply_str!(value, "accept_language", args.crawler.accept_language);
    apply_str!(value, "h2_profile", args.crawler.h2_profile);
    if let Some(v) = value.get("js_strategy").and_then(|v| v.as_str()) {
        args.crawler.js_strategy = match v {
            "hybrid" => crate::domain::JsStrategy::Hybrid,
            "full" => crate::domain::JsStrategy::Full,
            _ => crate::domain::JsStrategy::Static,
        };
    }
}

fn apply_tui_download(args: &mut Args, value: &serde_json::Value) {
    apply_bool!(value, "download_images", args.crawler.download_images);
    apply_bool!(value, "download_documents", args.crawler.download_documents);
    apply_u64!(value, "max_file_size", args.crawler.max_file_size);
    apply_u64!(value, "download_timeout", args.crawler.download_timeout);
}

fn apply_tui_obsidian(args: &mut Args, value: &serde_json::Value) {
    apply_bool!(
        value,
        "obsidian_wiki_links",
        args.obsidian.obsidian_wiki_links
    );
    if let Some(v) = value.get("obsidian_tags").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            args.obsidian.obsidian_tags = Some(v.split(',').map(String::from).collect());
        }
    }
    apply_bool!(
        value,
        "obsidian_relative_assets",
        args.obsidian.obsidian_relative_assets
    );
    apply_bool!(
        value,
        "obsidian_rich_metadata",
        args.obsidian.obsidian_rich_metadata
    );
    apply_path_opt!(value, "vault", args.obsidian.vault);
    apply_bool!(value, "quick_save", args.obsidian.quick_save);
}

#[cfg(feature = "ai")]
fn apply_tui_ai(args: &mut Args, value: &serde_json::Value) {
    apply_bool!(value, "clean_ai", args.crawler.clean_ai);
    apply_usize!(value, "max_tokens", args.ai.max_tokens);
    if let Some(v) = value.get("threshold").and_then(|v| v.as_str()) {
        if let Ok(n) = v.parse::<f32>() {
            args.ai.threshold = n;
        }
    }
    apply_bool!(value, "offline", args.ai.offline);
}

fn apply_tui_advanced(args: &mut Args, value: &serde_json::Value) {
    apply_bool!(value, "elastic", args.export.elastic);
    apply_bool!(value, "pipeline", args.export.pipeline);
    if let Some(v) = value.get("pipeline_output").and_then(|v| v.as_str()) {
        args.export.pipeline_output = match v {
            "none" => crate::domain::config::PipelineOutputFormat::None,
            _ => crate::domain::config::PipelineOutputFormat::Jsonl,
        };
    }
    apply_bool!(value, "batch", args.export.batch);
    apply_path_opt!(value, "batch_file", args.export.batch_file);
    apply_usize!(value, "batch_concurrency", args.export.batch_concurrency);
    apply_u64!(
        value,
        "checkpoint_interval",
        args.crawler.checkpoint_interval
    );
    apply_bool!(value, "no_checkpoint", args.crawler.no_checkpoint);
    apply_bool!(value, "ignore_robots", args.crawler.ignore_robots);
    apply_bool!(value, "autoscale", args.crawler.autoscale);
    apply_bool!(value, "no_session_health", args.crawler.no_session_health);
    apply_u8!(value, "verbose", args.crawler.verbose);
    apply_bool!(value, "quiet", args.crawler.quiet);
    apply_bool!(value, "dry_run", args.crawler.dry_run);
    apply_path_opt!(value, "trace_file", args.crawler.trace_file);
}

// ============================================================================
// Pre-flight HTTP Connectivity Check (T-070)
// ============================================================================

/// Result of a pre-flight connectivity check.
pub enum PreflightResult {
    /// 2xx or 3xx response — all good
    Ok,
    /// 4xx or 5xx response — connectivity OK but server issue
    Warning(u16),
    /// DNS failure, connection refused, timeout — cannot reach host
    Failed(String),
}

/// Send a HEAD request to verify connectivity before starting discovery.
/// Falls back to GET with Range: bytes=0-0 if HEAD is blocked (405) or times out.
pub async fn preflight_check(url: &url::Url) -> PreflightResult {
    let client = match crate::create_http_client() {
        Ok(c) => c,
        Err(e) => return PreflightResult::Failed(format!("failed to create HTTP client: {e}")),
    };

    match client
        .head(url.as_str())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if status < 400 {
                PreflightResult::Ok
            } else if status == 405 {
                warn!("HEAD request blocked (405), trying GET fallback...");
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Warning(status)
            }
        },
        Err(e) => {
            if e.is_timeout() || e.is_connect() {
                warn!("HEAD request failed ({}), trying GET fallback...", e);
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Failed(format!("network error: {e}"))
            }
        },
    }
}

/// Fallback to GET with Range: bytes=0-0 when HEAD is blocked.
async fn preflight_get_fallback(client: &wreq::Client, url: &url::Url) -> PreflightResult {
    match client
        .get(url.as_str())
        .header("Range", "bytes=0-0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => PreflightResult::Ok,
        Ok(resp) => PreflightResult::Warning(resp.status().as_u16()),
        Err(e) => PreflightResult::Failed(format!("HEAD y GET fallaron: {e}")),
    }
}

// ============================================================================
// Display Helpers
// ============================================================================

/// Return emoji or ASCII equivalent based on NO_COLOR setting.
#[inline]
pub fn icon(emoji: &str, ascii: &str) -> String {
    if crate::should_emit_emoji() {
        emoji.to_string()
    } else {
        ascii.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // TASK-13 — persistent ignore_waf merge (REQ-WAF-07)
    // ========================================================================

    #[test]
    fn apply_config_defaults_merges_ignore_waf() {
        let opts = CrawlOptions::default();
        assert!(!opts.crawl.ignore_waf);
        let config = ConfigDefaults {
            ignore_waf: Some(true),
            ..Default::default()
        };
        let merged = apply_config_defaults(opts, &config);
        assert!(merged.crawl.ignore_waf, "config file ignore_waf must apply");
    }

    #[test]
    fn apply_config_defaults_ignore_waf_absent_is_noop() {
        let opts = CrawlOptions::default();
        let config = ConfigDefaults::default(); // ignore_waf: None
        let merged = apply_config_defaults(opts, &config);
        assert!(!merged.crawl.ignore_waf);
    }

    // ========================================================================
    // #491 — --sitemap-url implies --use-sitemap
    // ========================================================================

    #[test]
    fn sitemap_url_implies_use_sitemap() {
        let mut opts = CrawlOptions::default();
        assert!(!opts.crawl.use_sitemap);
        opts.crawl.sitemap_url = Some("https://example.com/sitemap.xml".into());
        let config = ConfigDefaults::default();
        let merged = apply_config_defaults(opts, &config);
        assert!(
            merged.crawl.use_sitemap,
            "sitemap_url must imply use_sitemap"
        );
    }

    #[test]
    fn no_sitemap_url_leaves_use_sitemap_false() {
        let opts = CrawlOptions::default();
        let config = ConfigDefaults::default();
        let merged = apply_config_defaults(opts, &config);
        assert!(!merged.crawl.use_sitemap);
    }

    // ========================================================================
    // #516 — apply_tui_config_args field mapping (decomposed groups)
    // ========================================================================

    #[test]
    fn apply_tui_config_args_maps_all_groups() {
        let json = serde_json::json!({
            "url": "https://example.com",
            "selector": ".content",
            "output": "/out",
            "format": "json",
            "export_format": "vector",
            "use_sitemap": true,
            "max_pages": "50",
            "max_depth": "3",
            "concurrency": "auto",
            "user_agent": "CustomUA",
            "js_strategy": "hybrid",
            "download_images": true,
            "obsidian_wiki_links": true,
            "obsidian_tags": "a,b",
            "vault": "/vault",
            "elastic": true,
            "pipeline": true,
            "pipeline_output": "none",
            "batch": true,
            "verbose": "2",
            "quiet": false
        });
        let args = apply_tui_config_args(Args::default(), &json);

        assert_eq!(args.crawler.url, Some("https://example.com".to_string()));
        assert_eq!(args.crawler.selector, ".content".to_string());
        assert_eq!(args.export.output, std::path::PathBuf::from("/out"));
        assert_eq!(
            args.export.format,
            crate::domain::config::OutputFormat::Json
        );
        assert_eq!(
            args.export.export_format,
            crate::domain::config::ExportFormat::Vector
        );
        assert!(args.crawler.use_sitemap);
        assert_eq!(args.crawler.max_pages, 50);
        assert_eq!(args.crawler.max_depth, 3);
        assert!(args.crawler.concurrency.is_auto());
        assert_eq!(args.crawler.user_agent, Some("CustomUA".to_string()));
        assert_eq!(args.crawler.js_strategy, crate::domain::JsStrategy::Hybrid);
        assert!(args.crawler.download_images);
        assert!(args.obsidian.obsidian_wiki_links);
        assert_eq!(
            args.obsidian.obsidian_tags,
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            args.obsidian.vault,
            Some(std::path::PathBuf::from("/vault"))
        );
        assert!(args.export.elastic);
        assert!(args.export.pipeline);
        assert_eq!(
            args.export.pipeline_output,
            crate::domain::config::PipelineOutputFormat::None
        );
        assert!(args.export.batch);
        assert_eq!(args.crawler.verbose, 2);
        assert!(!args.crawler.quiet);
    }

    // ========================================================================
    // #685 — --js-strategy full preflight Chrome dependency check
    // ========================================================================

    /// `Static` strategy never needs Chrome: the check must return `Ok`
    /// without probing any binary, so the injected candidate list is
    /// irrelevant.
    #[test]
    fn static_strategy_ok_without_spawn() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Static;
        // A nonexistent candidate proves the check short-circuits before it
        // could attempt to spawn anything for a non-Full strategy.
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts).is_ok(),
            "Static strategy must not require Chrome or spawn processes"
        );
    }

    /// `Static` strategy must not validate the Obscura binary at all (#787):
    /// an unresolvable `--obscura-binary` is a no-op for static crawls.
    #[test]
    fn static_strategy_ignores_missing_obscura_binary() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Static;
        opts.network.obscura_binary = "/definitely/not/here/obscura".to_string();
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts).is_ok(),
            "Static strategy must not check --obscura-binary"
        );
    }

    /// `Hybrid` strategy escalates through Obscura (Layer 2), which works
    /// without the `chromium` feature — the preflight must not gate it
    /// (#758). The Obscura binary itself is supplied through a controlled
    /// PATH pointing at a tempdir with a fake `obscura` file, so this test
    /// stays free of process-global env mutation.
    #[test]
    fn hybrid_strategy_ok_without_chromium_feature() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("obscura");
        std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        let path_value = tmp.path().to_string_lossy();
        assert!(
            check_js_dependencies_with(
                &["definitely-not-installed-9x4k"],
                false,
                &path_value,
                &opts
            )
            .is_ok(),
            "Hybrid strategy must not require the chromium feature or a Chrome binary"
        );
    }

    // ========================================================================
    // #787 — --js-strategy hybrid obscura binary preflight
    // ========================================================================

    /// `has_path_separator` distinguishes explicit paths from bare names.
    #[test]
    fn has_path_separator_detects_explicit_paths() {
        assert!(has_path_separator("/usr/local/bin/obscura"));
        assert!(has_path_separator("./obscura"));
        assert!(has_path_separator("bin/obscura"));
        assert!(has_path_separator("C:\\tools\\obscura.exe"));
        assert!(!has_path_separator("obscura"));
        assert!(!has_path_separator(""));
    }

    /// `resolve_executable_in_path` scans PATH entries in order and returns
    /// the first existing file with the given name — a pure function over an
    /// injected PATH value, no process-global state.
    #[test]
    fn resolve_executable_in_path_finds_file_in_path_entries() {
        let empty = tempfile::TempDir::new().expect("tempdir");
        let with_bin = tempfile::TempDir::new().expect("tempdir");
        let bin_path = with_bin.path().join("obscura");
        std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

        // Found in the second entry.
        let joined = std::env::join_paths([empty.path(), with_bin.path()]).expect("join_paths");
        let path_value = joined.to_string_lossy();
        let resolved = resolve_executable_in_path("obscura", &path_value)
            .expect("obscura must resolve through the injected PATH");
        assert_eq!(resolved, bin_path);

        // Empty PATH resolves nothing.
        assert!(resolve_executable_in_path("obscura", "").is_none());
        // Explicit names with separators are rejected.
        assert!(resolve_executable_in_path("./obscura", &path_value).is_none());
        // A directory named `obscura` is not an executable file.
        let dir_only = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(dir_only.path().join("obscura")).expect("mkdir");
        let dir_path = dir_only.path().to_string_lossy();
        assert!(resolve_executable_in_path("obscura", &dir_path).is_none());
    }

    /// Hybrid + nonexistent absolute path: config error naming the flag and
    /// the env var (#787). No spawn — pure filesystem lookup.
    #[test]
    fn hybrid_nonexistent_absolute_path_errors() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = "/definitely/not/here/obscura".to_string();
        let err = check_js_dependencies_with(&["true"], true, "", &opts)
            .expect_err("a missing obscura path must fail hybrid preflight");
        match err {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("--obscura-binary") && msg.contains("WEBFANG_OBSCURA_BINARY"),
                    "config error must name the flag and the env var, got: {msg}"
                );
                assert!(
                    msg.contains("/definitely/not/here/obscura"),
                    "config error must name the offending path, got: {msg}"
                );
            },
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// Hybrid + nonexistent relative path: same config-error shape as the
    /// absolute case.
    #[test]
    fn hybrid_nonexistent_relative_path_errors() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = "definitely/not/here/obscura".to_string();
        let err = check_js_dependencies_with(&["true"], true, "", &opts)
            .expect_err("a missing relative obscura path must fail hybrid preflight");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--obscura-binary"),
                "config error must name the flag, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// Hybrid + bare `obscura` with PATH pointing at an empty directory:
    /// the binary cannot be on PATH, so the check must fail fast.
    #[test]
    fn hybrid_bare_name_missing_from_path_errors() {
        let empty = tempfile::TempDir::new().expect("tempdir");
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = "obscura".to_string();
        let path_value = empty.path().to_string_lossy();
        let err = check_js_dependencies_with(&["true"], true, &path_value, &opts)
            .expect_err("obscura missing from PATH must fail hybrid preflight");
        match err {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("PATH")
                        && msg.contains("--obscura-binary")
                        && msg.contains("WEBFANG_OBSCURA_BINARY"),
                    "config error must name PATH, the flag and the env var, got: {msg}"
                );
                assert!(
                    msg.contains("\"obscura\""),
                    "config error must name the missing binary, got: {msg}"
                );
            },
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// Hybrid + a fake `obscura` file on the injected PATH: the check
    /// passes with no `chromium` feature required.
    #[test]
    fn hybrid_binary_found_on_path_ok() {
        let bin_dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(bin_dir.path().join("obscura"), "#!/bin/sh\n").expect("write fake binary");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = "obscura".to_string();
        let path_value = bin_dir.path().to_string_lossy();
        assert!(
            check_js_dependencies_with(
                &["definitely-not-installed-9x4k"],
                false,
                &path_value,
                &opts
            )
            .is_ok(),
            "an obscura binary on PATH must satisfy hybrid preflight"
        );
    }

    /// Hybrid + explicit path to an existing file: the check passes even
    /// with an empty PATH (paths never fall back to PATH lookup).
    #[test]
    fn hybrid_explicit_existing_path_ok_with_empty_path() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("obscura");
        std::fs::write(&bin_path, "#!/bin/sh\n").expect("write fake obscura binary");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        // Absolute path to the fake binary created above.
        opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
                .is_ok(),
            "an existing explicit obscura path must satisfy hybrid preflight"
        );
    }

    // ========================================================================
    // #793 — Obscura minimum-version contract (parse / assess / gate)
    // ========================================================================

    /// `parse_obscura_version` extracts the first semver-like token from raw
    /// `--version` output — pure, no process or env access.
    #[test]
    fn parse_obscura_version_extracts_semver_token() {
        assert_eq!(parse_obscura_version("obscura 0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_obscura_version("obscura 0.1.9"), Some((0, 1, 9)));
        assert_eq!(parse_obscura_version("0.10.3 (build 42)"), Some((0, 10, 3)));
        assert_eq!(parse_obscura_version("v1.2.3"), Some((1, 2, 3)));
    }

    /// Pre-release/build suffixes on the patch segment still parse; missing
    /// versions, non-numeric segments, and empty output do not.
    #[test]
    fn parse_obscura_version_rejects_garbage() {
        assert_eq!(parse_obscura_version("obscura 0.2.0-rc.1"), Some((0, 2, 0)));
        assert_eq!(parse_obscura_version("obscura 0.2.0+build"), Some((0, 2, 0)));
        assert_eq!(parse_obscura_version("no version here"), None);
        assert_eq!(parse_obscura_version(""), None);
        assert_eq!(parse_obscura_version("version x.y.z"), None);
        assert_eq!(parse_obscura_version("obscura 0.2"), None);
    }

    /// `assess_obscura_version` classifies against the 0.2.0 minimum — the
    /// exact boundary, above it, below it, and the missing case.
    #[test]
    fn assess_obscura_version_classifies_meets_too_old_unknown() {
        assert_eq!(assess_obscura_version(Some((0, 2, 0))), VersionVerdict::Meets);
        assert_eq!(assess_obscura_version(Some((0, 3, 0))), VersionVerdict::Meets);
        assert_eq!(assess_obscura_version(Some((1, 0, 0))), VersionVerdict::Meets);
        assert_eq!(assess_obscura_version(Some((0, 1, 9))), VersionVerdict::TooOld);
        assert_eq!(assess_obscura_version(None), VersionVerdict::Unknown);
    }

    /// Write an executable fake `obscura` whose `--version` prints
    /// `obscura <version>` (#793). Deterministic: no network, no real binary.
    #[cfg(unix)]
    fn write_obscura_with_version(dir: &std::path::Path, version: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin_path = dir.join("obscura");
        std::fs::write(
            &bin_path,
            format!("#!/bin/sh\necho \"obscura {version}\"\n"),
        )
        .expect("write fake obscura");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x fake obscura");
        bin_path
    }

    /// Hybrid + obscura 0.2.0: the version contract is met, preflight passes.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn hybrid_version_020_passes() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_obscura_with_version(tmp.path(), "0.2.0");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
                .is_ok(),
            "obscura 0.2.0 must satisfy the version contract"
        );
    }

    /// Hybrid + obscura 0.1.9: config error (exit 78) naming the found
    /// version, the required version, and both override surfaces.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn hybrid_version_below_minimum_errors() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = write_obscura_with_version(tmp.path(), "0.1.9");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
        let err = check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
            .expect_err("obscura below 0.2.0 must fail hybrid preflight");
        match err {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("0.1.9") && msg.contains(MINIMUM_OBSCURA_VERSION_STR),
                    "config error must name found vs required version, got: {msg}"
                );
                assert!(
                    msg.contains("--obscura-binary") && msg.contains("WEBFANG_OBSCURA_BINARY"),
                    "config error must name the override surfaces, got: {msg}"
                );
            },
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// Hybrid + obscura printing garbage on `--version`: warn-and-continue —
    /// unknown builds are not hard-blocked.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn hybrid_version_garbage_degrades_to_warning() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("obscura");
        std::fs::write(&bin_path, "#!/bin/sh\necho \"custom build\"\n").expect("write fake obscura");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x fake obscura");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
                .is_ok(),
            "an unparseable --version must degrade to a warning, not block"
        );
    }

    /// Hybrid + obscura exiting non-zero on `--version`: same warn-degrade.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn hybrid_version_probe_failure_degrades_to_warning() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("obscura");
        std::fs::write(&bin_path, "#!/bin/sh\nexit 3\n").expect("write fake obscura");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x fake obscura");

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Hybrid;
        opts.network.obscura_binary = bin_path.to_string_lossy().into_owned();
        assert!(
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts)
                .is_ok(),
            "a failing --version probe must degrade to a warning, not block"
        );
    }


    /// `Full` strategy on a binary built WITHOUT the `chromium` feature
    /// fails fast with a config error naming the feature — before any PATH
    /// probe (#758).
    #[test]
    fn full_strategy_errors_without_chromium_feature() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        // `true` exists in CI, so a passing candidate list proves the
        // feature gate fires BEFORE the binary probe.
        let err = check_js_dependencies_with(&["true"], false, "", &opts)
            .expect_err("feature gate must fail before probing binaries");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("chromium"),
                "config error must name the chromium feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// `Full` strategy with a present, exit-0 binary (`true` exists in CI)
    /// passes the preflight check.
    // Miri cannot emulate posix_spawn (Command::status), so both tests that
    // actually probe a candidate binary are skipped there (#775). The
    // short-circuit tests (static/hybrid/feature-gate) above stay active.
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
    #[test]
    fn full_strategy_ok_when_binary_reports_version() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        assert!(
            check_js_dependencies_with(&["true"], true, "", &opts).is_ok(),
            "a candidate that exits 0 must satisfy the Full-strategy check"
        );
    }

    /// `Full` strategy with no working candidate yields a config error whose
    /// message names Chrome (user-facing, Spanish).
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
    #[test]
    fn full_strategy_errors_when_no_binary_found() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        let err = check_js_dependencies_with(&["definitely-not-installed-9x4k"], true, "", &opts)
            .expect_err("no candidate binary present means the check must fail");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("Chrome"),
                "config error must name Chrome, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    // ========================================================================
    // #695 — --elastic sink availability preflight
    // ========================================================================

    /// `--elastic` disabled: no sink is required, check passes.
    #[test]
    fn elastic_disabled_ok() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = false;
        assert!(check_elastic_sink(&opts).is_ok());
    }

    /// `--elastic` + `--output-vectors`: the JSONL stream sink exists in
    /// every build, so the check passes regardless of `persistence`.
    #[test]
    fn elastic_with_output_vectors_ok() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        opts.elastic.output_vectors = Some("vectors.jsonl".into());
        assert!(check_elastic_sink(&opts).is_ok());
    }

    /// `--elastic` alone without the `persistence` feature: no wirable
    /// sink — must fail fast with a config error naming the flag.
    #[cfg(not(feature = "persistence"))]
    #[test]
    fn elastic_without_persistence_errors() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        opts.elastic.output_vectors = None;
        let err =
            check_elastic_sink(&opts).expect_err("no sink available without persistence must fail");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--elastic"),
                "config error must name the flag, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// `--elastic` alone with the `persistence` feature: the SQLite sink
    /// is wirable, check passes.
    #[cfg(feature = "persistence")]
    #[test]
    fn elastic_with_persistence_ok() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        opts.elastic.output_vectors = None;
        assert!(check_elastic_sink(&opts).is_ok());
    }

    // ========================================================================
    // #761 — --clean-ai preflight feature check
    // ========================================================================

    /// `--clean-ai` without the `ai` feature: config error naming the
    /// feature, before any network request (#761).
    #[test]
    fn clean_ai_without_feature_errors() {
        let opts = CrawlOptions {
            ai: true,
            ..CrawlOptions::default()
        };
        let err = check_clean_ai_feature_with(false, &opts)
            .expect_err("non-AI build must reject --clean-ai in preflight");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--clean-ai") && msg.contains("ai"),
                "config error must name the flag and the feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// `--clean-ai` with the `ai` feature compiled in: passes.
    #[test]
    fn clean_ai_with_feature_ok() {
        let opts = CrawlOptions {
            ai: true,
            ..CrawlOptions::default()
        };
        assert!(check_clean_ai_feature_with(true, &opts).is_ok());
    }

    /// No `--clean-ai`: passes regardless of the feature state.
    #[test]
    fn no_clean_ai_ok_without_feature() {
        let opts = CrawlOptions::default();
        assert!(check_clean_ai_feature_with(false, &opts).is_ok());
    }
}
