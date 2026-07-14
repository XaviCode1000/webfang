//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.

use std::path::PathBuf;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::{Args, ConcurrencyConfig, ExportFormat, OutputFormat};

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
pub fn apply_tui_config_args(mut args: Args, config_values: &serde_json::Value) -> Args {
    use crate::domain::config::{ExportFormat as E, OutputFormat as O, PipelineOutputFormat as P};
    use crate::domain::JsStrategy;

    // ========================================================================
    // Helper macros for type conversion
    // ========================================================================
    macro_rules! apply_str {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    $field = v.to_string();
                }
            }
        };
    }

    macro_rules! apply_str_opt {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    $field = Some(v.to_string());
                }
            }
        };
    }

    macro_rules! apply_path_opt {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    $field = Some(PathBuf::from(v));
                }
            }
        };
    }

    macro_rules! apply_path {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if !v.is_empty() {
                    $field = PathBuf::from(v);
                }
            }
        };
    }

    macro_rules! apply_bool {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_bool()) {
                $field = v;
            }
        };
    }

    macro_rules! apply_u64 {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if let Ok(n) = v.parse() {
                    $field = n;
                }
            }
        };
    }

    macro_rules! apply_usize {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if let Ok(n) = v.parse() {
                    $field = n;
                }
            }
        };
    }

    macro_rules! apply_u8 {
        ($key:expr, $field:expr) => {
            if let Some(v) = config_values.get($key).and_then(|v| v.as_str()) {
                if let Ok(n) = v.parse() {
                    $field = n;
                }
            }
        };
    }

    // ========================================================================
    // Target
    // ========================================================================
    apply_str_opt!("url", args.url);
    apply_str!("selector", args.selector);

    // ========================================================================
    // Output
    // ========================================================================
    apply_path!("output", args.output);
    if let Some(fmt) = config_values.get("format").and_then(|v| v.as_str()) {
        args.format = match fmt {
            "json" => O::Json,
            "text" => O::Text,
            _ => O::Markdown,
        };
    }
    if let Some(fmt) = config_values.get("export_format").and_then(|v| v.as_str()) {
        args.export_format = match fmt {
            "vector" => E::Vector,
            "auto" => E::Auto,
            _ => E::Jsonl,
        };
    }

    // ========================================================================
    // Discovery
    // ========================================================================
    apply_bool!("use_sitemap", args.use_sitemap);
    apply_str_opt!("sitemap_url", args.sitemap_url);
    apply_usize!("max_pages", args.max_pages);
    apply_u8!("max_depth", args.max_depth);
    apply_u8!("sitemap_depth", args.sitemap_depth);

    // ========================================================================
    // Crawler
    // ========================================================================
    apply_u64!("timeout_secs", args.timeout_secs);
    apply_u64!("max_retries", args.max_retries);
    apply_u64!("delay_ms", args.delay_ms);
    // Concurrency: special handling (auto or number)
    if let Some(v) = config_values.get("concurrency").and_then(|v| v.as_str()) {
        if v == "auto" {
            args.concurrency = crate::ConcurrencyConfig::default();
        } else if let Ok(n) = v.parse::<usize>() {
            args.concurrency = crate::ConcurrencyConfig::new(n);
        }
    }
    // Include/exclude patterns
    if let Some(v) = config_values
        .get("include_pattern")
        .and_then(|v| v.as_str())
    {
        if !v.is_empty() {
            args.include_patterns = v.split(',').map(String::from).collect();
        }
    }
    if let Some(v) = config_values
        .get("exclude_pattern")
        .and_then(|v| v.as_str())
    {
        if !v.is_empty() {
            args.exclude_patterns = v.split(',').map(String::from).collect();
        }
    }

    // ========================================================================
    // Network
    // ========================================================================
    apply_str_opt!("user_agent", args.user_agent);
    apply_str!("accept_language", args.accept_language);
    apply_str!("h2_profile", args.h2_profile);
    if let Some(v) = config_values.get("js_strategy").and_then(|v| v.as_str()) {
        args.js_strategy = match v {
            "hybrid" => JsStrategy::Hybrid,
            "full" => JsStrategy::Full,
            _ => JsStrategy::Static,
        };
    }
    apply_bool!("force_js_render", args.force_js_render);

    // ========================================================================
    // Download
    // ========================================================================
    apply_bool!("download_images", args.download_images);
    apply_bool!("download_documents", args.download_documents);
    apply_u64!("max_file_size", args.max_file_size);
    apply_u64!("download_timeout", args.download_timeout);

    // ========================================================================
    // Obsidian
    // ========================================================================
    apply_bool!("obsidian_wiki_links", args.obsidian_wiki_links);
    // Tags: comma-separated string → Vec<String>
    if let Some(v) = config_values.get("obsidian_tags").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            args.obsidian_tags = Some(v.split(',').map(String::from).collect());
        }
    }
    apply_bool!("obsidian_relative_assets", args.obsidian_relative_assets);
    apply_bool!("obsidian_rich_metadata", args.obsidian_rich_metadata);
    apply_path_opt!("vault", args.vault);
    apply_bool!("quick_save", args.quick_save);

    // ========================================================================
    // AI (feature-gated)
    // ========================================================================
    #[cfg(feature = "ai")]
    {
        apply_bool!("clean_ai", args.clean_ai);
        apply_usize!("max_tokens", args.max_tokens);
        if let Some(v) = config_values.get("threshold").and_then(|v| v.as_str()) {
            if let Ok(n) = v.parse::<f32>() {
                args.threshold = n;
            }
        }
        apply_bool!("offline", args.offline);
    }

    // ========================================================================
    // Advanced
    // ========================================================================
    apply_bool!("elastic", args.elastic);
    // Note: cpu_cores, ram_budget, db_path are handled in orchestrator via ElasticOverrides
    apply_bool!("pipeline", args.pipeline);
    if let Some(v) = config_values
        .get("pipeline_output")
        .and_then(|v| v.as_str())
    {
        args.pipeline_output = match v {
            "none" => P::None,
            _ => P::Jsonl,
        };
    }
    apply_bool!("batch", args.batch);
    apply_path_opt!("batch_file", args.batch_file);
    apply_usize!("batch_concurrency", args.batch_concurrency);
    apply_u64!("checkpoint_interval", args.checkpoint_interval);
    apply_bool!("no_checkpoint", args.no_checkpoint);
    apply_bool!("ignore_robots", args.ignore_robots);
    apply_bool!("autoscale", args.autoscale);
    apply_bool!("no_session_health", args.no_session_health);
    apply_u8!("verbose", args.verbose);
    apply_bool!("quiet", args.quiet);
    apply_bool!("dry_run", args.dry_run);
    apply_path_opt!("trace_file", args.trace_file);

    args
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
