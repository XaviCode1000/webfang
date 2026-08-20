use crate::domain::config::ConcurrencyConfig;
use crate::domain::JsStrategy;
use clap::Args;
use scraper::Selector;

/// Validate `--download-concurrency`: must be >= 1. A value of 0 would make
/// `buffer_unordered(0)` hang forever (deadlock, D1). Rejecting here satisfies
/// the "Zero Silent Loss" philosophy with a clear CLI error instead of a hang.
pub(crate) fn parse_download_concurrency(s: &str) -> Result<usize, String> {
    let v: usize = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido para --download-concurrency"))?;
    if v == 0 {
        return Err(
            "--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)"
                .to_string(),
        );
    }
    Ok(v)
}

/// Validate `--timeout-secs`: must be >= 1. A value of 0 makes wreq apply
/// `Duration::from_secs(0)` as the request timeout, so every request fails
/// instantly with "operation timed out". Rejecting here gives a clear CLI
/// error instead of a crawl where every page fails.
/// Validate `--selector`: must parse as a CSS selector via `scraper`. Invalid
/// selectors currently surface only at scrape time (sometimes as a 30s network
/// timeout), so reject them up front with a clear Spanish usage error (exit 64
/// once `parse_args` maps the clap error). See issue #797.
pub(crate) fn parse_selector(s: &str) -> Result<String, String> {
    Selector::parse(s).map_err(|e| format!("selector CSS inválido «{s}»: {e}"))?;
    Ok(s.to_string())
}

pub(crate) fn parse_timeout_secs(s: &str) -> Result<u64, String> {
    let v: u64 = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido para --timeout-secs"))?;
    if v == 0 {
        return Err(
            "--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)".to_string(),
        );
    }
    Ok(v)
}

/// Validate `--max-pages`: must be >= 1. A value of 0 would panic
/// `tokio::sync::mpsc::channel(0)` inside `ResultsCollector::new`
/// (SIGABRT, #780 — the MCP path already rejects this; #598/#611 only
/// covered MCP). Rejecting here gives a clear usage error (exit 64)
/// instead of a runtime panic (exit 134).
pub(crate) fn parse_max_pages(s: &str) -> Result<usize, String> {
    let v: usize = s
        .parse()
        .map_err(|_| format!("'{s}' no es un número válido para --max-pages"))?;
    if v == 0 {
        return Err("--max-pages debe ser >= 1 (0 no deja páginas para scrapear)".to_string());
    }
    Ok(v)
}

/// Crawler and discovery configuration arguments.
#[derive(Args, Debug, Default)]
pub struct CrawlerArgs {
    // ========== Target ==========
    /// URL to scrape (required unless using a subcommand)
    #[arg(short, long, env = "WEBFANG_URL")]
    #[clap(next_help_heading = "Target")]
    pub url: Option<String>,

    /// CSS selector for content extraction
    #[arg(
            short,
            long,
            default_value = "body",
            env = "WEBFANG_SELECTOR",
            value_parser = parse_selector
        )]
    #[clap(next_help_heading = "Target")]
    pub selector: String,

    // ========== Discovery ==========
    /// Delay between requests in milliseconds
    #[arg(long, default_value = "1000", env = "WEBFANG_DELAY_MS")]
    #[clap(next_help_heading = "Discovery")]
    pub delay_ms: u64,

    /// Maximum pages to scrape
    #[arg(
        long,
        default_value = "10",
        env = "WEBFANG_MAX_PAGES",
        value_parser = parse_max_pages
    )]
    #[clap(next_help_heading = "Discovery")]
    pub max_pages: usize,

    /// Concurrency level (auto or number)
    #[arg(long, default_value_t = ConcurrencyConfig::default(), env = "WEBFANG_CONCURRENCY")]
    #[clap(next_help_heading = "Discovery")]
    pub concurrency: ConcurrencyConfig,

    /// Use sitemap for URL discovery
    /// NOTE: HTTP redirects (301/302) are resolved at scrape-time, not parse-time.
    /// This avoids redundant HEAD requests during sitemap parsing for better performance.
    #[arg(long, env = "WEBFANG_USE_SITEMAP")]
    #[clap(next_help_heading = "Discovery")]
    pub use_sitemap: bool,

    /// Explicit sitemap URL
    #[arg(long, env = "WEBFANG_SITEMAP_URL")]
    #[clap(next_help_heading = "Discovery")]
    pub sitemap_url: Option<String>,

    // ========== Behavior ==========
    /// Scrape only the seed URL without discovery or crawling
    #[arg(long, default_value = "false", env = "WEBFANG_SINGLE_PAGE")]
    #[clap(next_help_heading = "Behavior")]
    pub single_page: bool,

    /// Resume mode - skip URLs already processed
    #[arg(long, env = "WEBFANG_RESUME")]
    #[clap(next_help_heading = "Behavior")]
    pub resume: bool,

    /// Custom state directory for resume mode
    #[arg(long, env = "WEBFANG_STATE_DIR")]
    #[clap(next_help_heading = "Behavior")]
    pub state_dir: Option<std::path::PathBuf>,

    /// Download images from the page
    #[arg(long, default_value = "false", env = "WEBFANG_DOWNLOAD_IMAGES")]
    #[clap(next_help_heading = "Behavior")]
    pub download_images: bool,

    /// Download documents from the page
    #[arg(long, default_value = "false", env = "WEBFANG_DOWNLOAD_DOCUMENTS")]
    #[clap(next_help_heading = "Behavior")]
    pub download_documents: bool,

    /// Download all assets (images + documents) from the page
    #[arg(long, default_value = "false", env = "WEBFANG_DOWNLOAD_ASSETS")]
    #[clap(next_help_heading = "Behavior")]
    pub download_assets: bool,

    /// Use AI-powered semantic cleaning for better RAG output
    #[cfg(feature = "ai")]
    #[arg(
        long,
        default_value = "false",
        visible_alias = "ai",
        env = "WEBFANG_CLEAN_AI"
    )]
    #[clap(next_help_heading = "Behavior")]
    pub clean_ai: bool,

    /// Feature flag placeholder when AI is not enabled
    #[cfg(not(feature = "ai"))]
    #[arg(
        long,
        default_value = "false",
        hide = true,
        visible_alias = "ai",
        env = "WEBFANG_CLEAN_AI"
    )]
    pub clean_ai: bool,

    /// Enable adaptive CSS selector repair (2-tier cascade)
    #[cfg(feature = "adaptive-selectors")]
    #[arg(long, default_value = "false", env = "WEBFANG_ADAPTIVE_SELECTORS")]
    #[clap(next_help_heading = "Behavior")]
    pub adaptive_selectors: bool,

    /// Feature flag placeholder when adaptive-selectors is not enabled
    #[cfg(not(feature = "adaptive-selectors"))]
    #[arg(
        long,
        default_value = "false",
        hide = true,
        env = "WEBFANG_ADAPTIVE_SELECTORS"
    )]
    pub adaptive_selectors: bool,

    // ========== Display ==========
    /// Verbosity level: -v (INFO), -vv (DEBUG), -vvv (TRACE)
    #[arg(short, long, action = clap::ArgAction::Count, env = "WEBFANG_VERBOSE")]
    #[clap(next_help_heading = "Display")]
    pub verbose: u8,

    /// Quiet mode — suppress info/debug output
    #[arg(short = 'q', long, default_value = "false", env = "WEBFANG_QUIET")]
    #[clap(next_help_heading = "Display")]
    pub quiet: bool,

    /// Dry-run mode — discover URLs and print without scraping
    #[arg(short = 'n', long, default_value = "false", env = "WEBFANG_DRY_RUN")]
    #[clap(next_help_heading = "Display")]
    pub dry_run: bool,

    /// Path to write OTel spans as JSONL for offline debugging
    #[arg(long, env = "WEBFANG_TRACE_FILE")]
    #[clap(next_help_heading = "Display")]
    pub trace_file: Option<std::path::PathBuf>,

    // ========== Crawler Settings ==========
    /// Maximum depth to crawl (0 = only seed URL)
    #[arg(long, default_value = "2", env = "WEBFANG_MAX_DEPTH")]
    #[clap(next_help_heading = "Crawler Settings")]
    pub max_depth: u8,

    /// Request timeout in seconds
    #[arg(
        long,
        default_value = "30",
        env = "WEBFANG_TIMEOUT_SECS",
        value_parser = parse_timeout_secs
    )]
    #[clap(next_help_heading = "Crawler Settings")]
    pub timeout_secs: u64,

    /// URL patterns to include (glob-style). Three modes:
    ///
    /// * Path: starts with `/` → matched against URL path, e.g. `/pricing`, `/admin/*`
    /// * Path glob: starts with `*/` → matched against URL path, e.g. `*/api/*`
    /// * Host (default): matched against hostname, e.g. `example.com`, `*.example.com`
    ///
    /// Example: to exclude a path, use `--exclude-pattern "/admin/*"`, not `*admin*`
    #[arg(
        long = "include-pattern",
        env = "WEBFANG_INCLUDE",
        value_delimiter = ','
    )]
    #[clap(next_help_heading = "Crawler Settings")]
    pub include_patterns: Vec<String>,

    /// URL patterns to exclude (glob-style, same three modes as --include-pattern).
    /// Deny takes precedence over allow.
    #[arg(
        long = "exclude-pattern",
        env = "WEBFANG_EXCLUDE",
        value_delimiter = ','
    )]
    #[clap(next_help_heading = "Crawler Settings")]
    pub exclude_patterns: Vec<String>,

    /// Estrategia de nombre de archivo para assets descargados: hash (default), slug, content-disposition
    #[arg(long, default_value = "hash", value_parser = ["hash", "slug", "content-disposition"])]
    pub asset_naming: String,

    /// Maximum concurrent asset downloads per page (default: 3)
    #[arg(
        long,
        default_value = "3",
        env = "WEBFANG_DOWNLOAD_CONCURRENCY",
        value_parser = parse_download_concurrency,
        help = "Máximo de descargas de assets concurrentes por página (mínimo 1)"
    )]
    pub download_concurrency: usize,

    // ========== HTTP Client Settings ==========
    /// Maximum number of retry attempts
    #[arg(long, default_value = "3", env = "WEBFANG_MAX_RETRIES")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub max_retries: u32,

    /// Base delay for exponential backoff (ms)
    #[arg(long, default_value = "1000", env = "WEBFANG_BACKOFF_BASE_MS")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub backoff_base_ms: u64,

    /// Maximum delay for exponential backoff (ms)
    #[arg(long, default_value = "10000", env = "WEBFANG_BACKOFF_MAX_MS")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub backoff_max_ms: u64,

    /// Accept-Language header value
    #[arg(
        long,
        default_value = "en-US,en;q=0.9",
        env = "WEBFANG_ACCEPT_LANGUAGE"
    )]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub accept_language: String,

    /// Custom User-Agent header value (overrides Chrome 145 default)
    #[arg(long, env = "WEBFANG_USER_AGENT")]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub user_agent: Option<String>,

    /// Inject a custom HTTP header as `Name: Value` (repeatable).
    ///
    /// Overrides any default header with the same (case-insensitive) name.
    /// Example: `-H "Authorization: Bearer TOKEN"`.
    #[arg(
        short = 'H',
        long = "header",
        value_name = "NAME: VALUE",
        env = "WEBFANG_HEADER",
        value_delimiter = ';'
    )]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub headers: Vec<String>,

    /// Inject a custom cookie as `name=value` (repeatable).
    ///
    /// Seeded into the cookie jar before the first request so authenticated
    /// crawls work without a prior login round-trip.
    /// Example: `--cookie "session=abc123"`.
    #[arg(
        long = "cookie",
        value_name = "NAME=VALUE",
        env = "WEBFANG_COOKIE",
        value_delimiter = ';'
    )]
    #[clap(next_help_heading = "HTTP Client Settings")]
    pub cookies: Vec<String>,

    // ========== Download Settings ==========
    /// Maximum file size to download in bytes (default: 50MB)
    #[arg(long, default_value = "52428800", env = "WEBFANG_MAX_FILE_SIZE")]
    #[clap(next_help_heading = "Download Settings")]
    pub max_file_size: u64,

    /// Timeout for individual asset downloads in seconds
    #[arg(long, default_value = "30", env = "WEBFANG_DOWNLOAD_TIMEOUT")]
    #[clap(next_help_heading = "Download Settings")]
    pub download_timeout: u64,

    // ========== Sitemap Settings ==========
    /// Maximum recursion depth for sitemap indexes
    #[arg(long, default_value = "3", env = "WEBFANG_SITEMAP_DEPTH")]
    #[clap(next_help_heading = "Sitemap Settings")]
    pub sitemap_depth: u8,

    // ========== Competitive Features Phase 1 ==========
    /// Pages between automatic checkpoint saves (0 = disabled)
    /// NOTE: Checkpoint is for programmatic use (Engine API) only.
    /// CLI --resume uses StateStore instead of checkpoints.
    #[arg(long, default_value = "100", env = "WEBFANG_CHECKPOINT_INTERVAL")]
    #[clap(next_help_heading = "Competitive Features")]
    pub checkpoint_interval: u64,

    /// Disable checkpoint persistence entirely
    /// NOTE: Checkpoint is for programmatic use (Engine API) only.
    /// CLI --resume uses StateStore instead of checkpoints.
    #[arg(long, default_value = "false", env = "WEBFANG_NO_CHECKPOINT")]
    #[clap(next_help_heading = "Competitive Features")]
    pub no_checkpoint: bool,

    /// Skip robots.txt enforcement
    #[arg(long, default_value = "false", env = "WEBFANG_IGNORE_ROBOTS")]
    #[clap(next_help_heading = "Competitive Features")]
    pub ignore_robots: bool,

    /// Bypass WAF/CAPTCHA detection entirely (never block on challenge markers)
    #[arg(long, default_value = "false", env = "WEBFANG_IGNORE_WAF")]
    #[clap(next_help_heading = "Competitive Features")]
    pub ignore_waf: bool,

    /// Enable autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage
    #[arg(long, default_value = "false", env = "WEBFANG_AUTOSCALE")]
    #[clap(next_help_heading = "Competitive Features")]
    pub autoscale: bool,

    /// Disable session pool health checks
    #[arg(long, default_value = "false", env = "WEBFANG_NO_SESSION_HEALTH")]
    #[clap(next_help_heading = "Competitive Features")]
    pub no_session_health: bool,

    /// TLS/HTTP2 profile name (default: Chrome145)
    #[arg(long, default_value = "Chrome145", env = "WEBFANG_H2_PROFILE")]
    #[clap(next_help_heading = "Competitive Features")]
    pub h2_profile: String,

    /// JavaScript rendering strategy: static (wreq only), hybrid (3-layer), full (Chromiumoxide only)
    #[arg(
        long,
        default_value = "static",
        value_enum,
        env = "WEBFANG_JS_STRATEGY"
    )]
    #[clap(next_help_heading = "JS Rendering")]
    pub js_strategy: JsStrategy,

    /// Path to the obscura binary (default: "obscura")
    #[arg(long, default_value = "obscura", env = "WEBFANG_OBSCURA_BINARY")]
    #[clap(next_help_heading = "JS Rendering")]
    pub obscura_binary: String,

    /// Enable DOM pre-pruning before Readability (removes invisible/empty wrappers).
    /// Default: enabled (true). Set to false via --dom-preprune=false or WEBFANG_DOM_PREPRUNE=false.
    #[arg(
        long,
        env = "WEBFANG_DOM_PREPRUNE",
        default_value = "true",
        num_args(0..=1)
    )]
    #[clap(next_help_heading = "Cleanup")]
    pub dom_preprune: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timeout_secs_accepts_valid_value() {
        assert_eq!(parse_timeout_secs("30"), Ok(30));
    }

    #[test]
    fn parse_timeout_secs_rejects_zero() {
        let err = parse_timeout_secs("0").unwrap_err();
        assert_eq!(
            err,
            "--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)"
        );
    }

    #[test]
    fn parse_timeout_secs_rejects_non_numeric() {
        let err = parse_timeout_secs("abc").unwrap_err();
        assert_eq!(err, "'abc' no es un número válido para --timeout-secs");
    }

    #[test]
    fn parse_max_pages_accepts_valid_value() {
        assert_eq!(parse_max_pages("5"), Ok(5));
    }

    #[test]
    fn parse_max_pages_rejects_zero() {
        // Bug #780: max_pages 0 panics mpsc::channel(0) → SIGABRT.
        let err = parse_max_pages("0").unwrap_err();
        assert_eq!(
            err,
            "--max-pages debe ser >= 1 (0 no deja páginas para scrapear)"
        );
    }

    #[test]
    fn parse_max_pages_rejects_non_numeric() {
        let err = parse_max_pages("abc").unwrap_err();
        assert_eq!(err, "'abc' no es un número válido para --max-pages");
    }

    #[test]
    fn parse_selector_accepts_valid_css() {
        assert_eq!(parse_selector("article p"), Ok("article p".to_string()));
        assert_eq!(parse_selector("body"), Ok("body".to_string()));
        assert_eq!(
            parse_selector(".content > h1"),
            Ok(".content > h1".to_string())
        );
    }

    #[test]
    fn parse_selector_rejects_invalid_css() {
        // Regression for #797: an invalid CSS selector must be rejected at
        // parse time (clap error -> exit 64), not at scrape time (30s timeout).
        let err = parse_selector(":::invalid").unwrap_err();
        assert!(
            err.contains("selector CSS inválido"),
            "expected Spanish invalid-selector message, got: {err}"
        );
    }
}
