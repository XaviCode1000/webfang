//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.

use std::path::PathBuf;
use tracing::warn;

use crate::cli::config::ConfigDefaults;
use crate::{Args, ConcurrencyConfig, ExportFormat, OutputFormat};

// ============================================================================
// Config Defaults Merge
// ============================================================================

/// Apply config file defaults where CLI args are still at their hardcoded defaults.
///
/// Precedence: CLI > env (handled by clap) > config file > struct defaults.
pub fn apply_config_defaults(mut args: Args, config: &ConfigDefaults) -> Args {
    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "markdown" => OutputFormat::Markdown,
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if args.format == OutputFormat::Markdown && target != OutputFormat::Markdown {
            args.format = target;
        }
    }

    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "jsonl" => ExportFormat::Jsonl,
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if args.export_format == ExportFormat::Jsonl && target != ExportFormat::Jsonl {
            args.export_format = target;
        }
    }

    if let Some(ref c) = config.concurrency {
        // ConcurrencyConfig doesn't implement PartialEq, so check via is_auto()
        if args.concurrency.is_auto() {
            args.concurrency = ConcurrencyConfig::from(c.as_str());
        }
    }

    if let Some(ref s) = config.selector {
        if args.selector == "body" {
            args.selector = s.clone();
        }
    }

    if let Some(n) = config.max_pages {
        if args.max_pages == 10 {
            args.max_pages = n;
        }
    }

    if let Some(n) = config.delay_ms {
        if args.delay_ms == 1000 {
            args.delay_ms = n;
        }
    }

    if let Some(v) = config.use_sitemap {
        if !args.use_sitemap && v {
            args.use_sitemap = v;
        }
    }

    // Obsidian config — trim whitespace from CLI tags (clap value_delimiter doesn't trim)
    if let Some(ref mut tags) = args.obsidian_tags {
        for tag in tags.iter_mut() {
            *tag = tag.trim().to_string();
        }
        tags.retain(|t| !t.is_empty());
    }
    if let Some(ref tags_str) = config.obsidian_tags {
        if args.obsidian_tags.is_none() {
            args.obsidian_tags = Some(
                tags_str
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
            );
        }
    }
    if let Some(v) = config.obsidian_wiki_links {
        if !args.obsidian_wiki_links && v {
            args.obsidian_wiki_links = v;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if !args.obsidian_relative_assets && v {
            args.obsidian_relative_assets = v;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if args.vault.is_none() {
            args.vault = Some(PathBuf::from(vault));
        }
    }

    args
}

// ============================================================================
// TUI Config Merge
// ============================================================================

/// Apply config values from TUI form to Args.
///
/// This runs after config_tui returns user-submitted values.
/// Precedence: TUI values > CLI args (they override what was passed).
pub fn apply_tui_config(mut args: Args, config_values: &serde_json::Value) -> Args {
    use crate::OutputFormat as O;
    use crate::ExportFormat as E;

    // Output directory
    if let Some(output) = config_values.get("output").and_then(|v| v.as_str()) {
        args.output = PathBuf::from(output);
    }

    // Output format (markdown, json, text)
    if let Some(fmt) = config_values.get("format").and_then(|v| v.as_str()) {
        args.format = match fmt {
            "json" => O::Json,
            "text" => O::Text,
            _ => O::Markdown,
        };
    }

    // Export format (jsonl, vector, auto)
    if let Some(fmt) = config_values.get("export_format").and_then(|v| v.as_str()) {
        args.export_format = match fmt {
            "vector" => E::Vector,
            "auto" => E::Auto,
            _ => E::Jsonl,
        };
    }

    // Discovery: use_sitemap
    if let Some(v) = config_values.get("use_sitemap").and_then(|v| v.as_bool()) {
        args.use_sitemap = v;
    }

    // Discovery: max_pages
    if let Some(v) = config_values.get("max_pages").and_then(|v| v.as_str()) {
        if let Ok(n) = v.parse() {
            args.max_pages = n;
        }
    }

    // Crawler: max_depth
    if let Some(v) = config_values.get("max_depth").and_then(|v| v.as_str()) {
        if let Ok(n) = v.parse() {
            args.max_depth = n;
        }
    }

    // Behavior: download_images
    if let Some(v) = config_values.get("download_images").and_then(|v| v.as_bool()) {
        args.download_images = v;
    }

    // Behavior: download_documents
    if let Some(v) = config_values.get("download_documents").and_then(|v| v.as_bool()) {
        args.download_documents = v;
    }

    // Obsidian: obsidian_wiki_links
    if let Some(v) = config_values.get("obsidian_wiki_links").and_then(|v| v.as_bool()) {
        args.obsidian_wiki_links = v;
    }

    // Obsidian: vault path
    if let Some(vault) = config_values.get("vault").and_then(|v| v.as_str()) {
        if !vault.is_empty() {
            args.vault = Some(PathBuf::from(vault));
        }
    }

    // Obsidian: quick_save
    if let Some(v) = config_values.get("quick_save").and_then(|v| v.as_bool()) {
        args.quick_save = v;
    }

    // AI: clean_ai (only applies when feature is enabled)
    #[cfg(feature = "ai")]
    if let Some(v) = config_values.get("clean_ai").and_then(|v| v.as_bool()) {
        args.clean_ai = v;
    }

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
        Err(e) => return PreflightResult::Failed(format!("failed to create HTTP client: {}", e)),
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
                PreflightResult::Failed(format!("network error: {}", e))
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
        Err(e) => PreflightResult::Failed(format!("HEAD y GET fallaron: {}", e)),
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
