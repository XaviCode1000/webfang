//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.

use std::path::PathBuf;
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
/// strategy before any crawl starts (#685).
///
/// `JsStrategy::Full` renders pages through Chromiumoxide, which spawns a
/// real Chrome/Chromium binary. Probing the installed candidates here turns
/// a missing browser into a clean config error (exit 78) instead of a
/// confusing mid-crawl browser launch failure. Other strategies need no
/// external binary and return immediately without spawning processes.
///
/// Runs once, in a synchronous context, before crawl start — a brief
/// blocking `--version` probe is acceptable there.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the strategy is
/// [`Full`](crate::domain::JsStrategy::Full) and no Chrome/Chromium
/// candidate on `PATH` reports a version.
pub fn check_js_dependencies(opts: &CrawlOptions) -> Result<(), CliExit> {
    check_js_dependencies_with(&DEFAULT_CHROME_CANDIDATES, opts)
}

/// Candidate-injectable core of [`check_js_dependencies`] — tests probe
/// binaries that exist deterministically in CI (e.g. `true`) instead of the
/// real Chrome names.
fn check_js_dependencies_with(candidates: &[&str], opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.network.js_strategy != JsStrategy::Full {
        return Ok(());
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
            check_js_dependencies_with(&["definitely-not-installed-9x4k"], &opts).is_ok(),
            "Static strategy must not require Chrome or spawn processes"
        );
    }

    /// `Full` strategy with a present, exit-0 binary (`true` exists in CI)
    /// passes the preflight check.
    #[test]
    fn full_strategy_ok_when_binary_reports_version() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        assert!(
            check_js_dependencies_with(&["true"], &opts).is_ok(),
            "a candidate that exits 0 must satisfy the Full-strategy check"
        );
    }

    /// `Full` strategy with no working candidate yields a config error whose
    /// message names Chrome (user-facing, Spanish).
    #[test]
    fn full_strategy_errors_when_no_binary_found() {
        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        let err = check_js_dependencies_with(&["definitely-not-installed-9x4k"], &opts)
            .expect_err("no candidate binary present means the check must fail");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("Chrome"),
                "config error must name Chrome, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }
}
