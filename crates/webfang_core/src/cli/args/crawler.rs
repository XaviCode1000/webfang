use crate::domain::config::ConcurrencyConfig;
use crate::domain::options_spec::crawler as crawler_specs;
use crate::domain::JsStrategy;
use clap::Args;
use scraper::Selector;

/// Validate `--download-concurrency`: must be >= 1. A value of 0 would make
/// `buffer_unordered(0)` hang forever (deadlock, D1). Rejecting here satisfies
/// the "Zero Silent Loss" philosophy with a clear CLI error instead of a hang.
/// Bounds and messages come from the OptionsSpec (ADR-002) — the single
/// validation source.
pub(crate) fn parse_download_concurrency(s: &str) -> Result<usize, String> {
    let value = crawler_specs::DOWNLOAD_CONCURRENCY
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| {
        crawler_specs::DOWNLOAD_CONCURRENCY
            .parse_error(s)
            .to_string()
    })
}

/// Validate `--rate-limit-burst`: explicit rate-limiter burst override
/// (`WEBFANG_RATE_LIMIT_BURST`, budget model decision Q1/D1).
///
/// - Numeric values >= 1 are accepted (the derived default never produces 0,
///   so neither may the operator).
/// - `0` is rejected with a Spanish usage error (same "Zero Silent Loss"
///   philosophy as `--download-concurrency`).
/// - Non-numeric input warns and falls back to the hardware-derived default
///   (consistent with the `ConcurrencyConfig` "auto" parser behavior).
#[allow(clippy::unnecessary_wraps)] // non-numeric deliberately warns instead of erroring
pub(crate) fn parse_rate_limit_burst(s: &str) -> Result<Option<u32>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u32>() {
        Ok(0) => Err(
            "--rate-limit-burst debe ser >= 1 (0 no permite ningún request en ráfaga)".to_string(),
        ),
        Ok(v) => Ok(Some(v)),
        Err(_) => {
            tracing::warn!(value = %s, "invalid rate-limit burst, using derived default");
            Ok(None)
        },
    }
}

/// Validate `--timeout-secs`: must be >= 1. A value of 0 makes wreq apply
/// `Duration::from_secs(0)` as the request timeout, so every request fails
/// instantly with "operation timed out". Rejecting here gives a clear CLI
/// error instead of a crawl where every page fails. Bounds and messages come
/// from the OptionsSpec (ADR-002).
/// Validate `--selector`: must parse as a CSS selector via `scraper`. Invalid
/// selectors currently surface only at scrape time (sometimes as a 30s network
/// timeout), so reject them up front with a clear Spanish usage error (exit 64
/// once `parse_args` maps the clap error). See issue #797.
pub(crate) fn parse_selector(s: &str) -> Result<String, String> {
    Selector::parse(s).map_err(|e| format!("selector CSS inválido «{s}»: {e}"))?;
    Ok(s.to_string())
}

pub(crate) fn parse_timeout_secs(s: &str) -> Result<u64, String> {
    crawler_specs::TIMEOUT_SECS
        .parse_uint(s)
        .map_err(|e| e.to_string())
}

/// Validate `--max-pages`: must be >= 1. A value of 0 would panic
/// `tokio::sync::mpsc::channel(0)` inside `ResultsCollector::new`
/// (SIGABRT, #780 — the MCP path already rejects this; #598/#611 only
/// covered MCP). Rejecting here gives a clear usage error (exit 64)
/// instead of a runtime panic (exit 134). Bounds and messages come from the
/// OptionsSpec (ADR-002).
pub(crate) fn parse_max_pages(s: &str) -> Result<usize, String> {
    let value = crawler_specs::MAX_PAGES
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| crawler_specs::MAX_PAGES.parse_error(s).to_string())
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

    /// Explicit rate-limiter burst permits (token-bucket capacity).
    ///
    /// Overrides the hardware-derived budget-model default (Q1: burst is
    /// decoupled from crawl concurrency). Raw string here ON PURPOSE:
    /// validation/conversion happens once in preflight staging via
    /// `parse_rate_limit_burst` so CLI, env, and programmatic input all
    /// share one accept / reject-0 / warn-and-default semantic.
    #[arg(long, env = "WEBFANG_RATE_LIMIT_BURST")]
    #[clap(next_help_heading = "Discovery")]
    pub rate_limit_burst: Option<String>,

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

    /// Record extraction failure fingerprints in SQLite and attach them to
    /// low-quality extraction hints (#792). Repeated low-score extractions on
    /// the same site/selector pair accumulate a failure count surfaced in the
    /// hint, instead of degrading silently.
    #[arg(long, default_value = "false", env = "WEBFANG_EXTRACTION_FINGERPRINT")]
    #[clap(next_help_heading = "Behavior")]
    pub extraction_fingerprint: bool,

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

    /// Maximum concurrent asset downloads per page (omit = auto from budget model)
    #[arg(
        long,
        env = "WEBFANG_DOWNLOAD_CONCURRENCY",
        value_parser = parse_download_concurrency,
        help = "Máximo de descargas de assets concurrentes por página (mínimo 1)"
    )]
    pub download_concurrency: Option<usize>,

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
    fn parse_rate_limit_burst_accepts_positive_values() {
        assert_eq!(parse_rate_limit_burst("1"), Ok(Some(1)));
        assert_eq!(parse_rate_limit_burst("5"), Ok(Some(5)));
        assert_eq!(parse_rate_limit_burst("64"), Ok(Some(64)));
    }

    #[test]
    fn parse_rate_limit_burst_rejects_zero_with_spanish_error() {
        // Same "Zero Silent Loss" boundary as --download-concurrency (D1).
        let err = parse_rate_limit_burst("0").unwrap_err();
        assert_eq!(
            err,
            "--rate-limit-burst debe ser >= 1 (0 no permite ningún request en ráfaga)"
        );
    }

    #[test]
    fn parse_rate_limit_burst_non_numeric_warns_and_defaults() {
        // Consistent with the ConcurrencyConfig "auto" parser: invalid input
        // degrades to the derived default instead of aborting the run.
        assert_eq!(parse_rate_limit_burst("abc"), Ok(None));
        assert_eq!(parse_rate_limit_burst("auto"), Ok(None));
        assert_eq!(parse_rate_limit_burst(""), Ok(None));
    }

    #[test]
    fn rate_limit_burst_flag_parses_through_clap() {
        use clap::Parser as _;
        let args = crate::Args::try_parse_from(["webfang", "--rate-limit-burst", "9"])
            .expect("valid flag combination");
        assert_eq!(args.crawler.rate_limit_burst.as_deref(), Some("9"));
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
    #[cfg_attr(
        miri,
        ignore = "servo_arc 0.4.3 Tree Borrows false positive in Selector::parse (scraper/selectors) — #810"
    )]
    fn parse_selector_accepts_valid_css() {
        assert_eq!(parse_selector("article p"), Ok("article p".to_string()));
        assert_eq!(parse_selector("body"), Ok("body".to_string()));
        assert_eq!(
            parse_selector(".content > h1"),
            Ok(".content > h1".to_string())
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "servo_arc 0.4.3 Tree Borrows false positive in Selector::parse (scraper/selectors) — #810"
    )]
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

#[cfg(test)]
mod spec_parity_tests {
    //! ADR-002 equivalence proof (slice 2): the hand-derived clap surface of
    //! [`CrawlerArgs`] must stay in lockstep with the OptionsSpec crawler
    //! group. Written FIRST against the UNMIGRATED parsers (pin); the
    //! migration then routes validation through the spec and must keep them
    //! green.
    //!
    //! Deferred from the spec this slice (asserted here so the list stays
    //! honest): `concurrency`, `rate_limit_burst`, `include_patterns`,
    //! `exclude_patterns`, `headers`, `cookies`, and the feature-gated
    //! `clean_ai` / `adaptive_selectors`.

    use super::*;
    use crate::domain::options_spec as spec;

    /// CrawlerArgs fields intentionally OUTSIDE the spec this slice, with the
    /// reason each was deferred.
    const DEFERRED_FROM_SPEC: &[(&str, &str)] = &[
        (
            "concurrency",
            "custom ConcurrencyConfig FromStr with auto detection",
        ),
        (
            "rate_limit_burst",
            "raw-string preflight staging with warn-and-default semantics",
        ),
        ("include_patterns", "Vec arg with ',' delimiter"),
        ("exclude_patterns", "Vec arg with ',' delimiter"),
        ("headers", "repeatable Vec arg with ';' delimiter"),
        ("cookies", "repeatable Vec arg with ';' delimiter"),
    ];

    /// All clap args generated for `CrawlerArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        CrawlerArgs::augment_args(clap::Command::new("webfang-crawler"))
            .get_arguments()
            .cloned()
            .collect()
    }

    fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from CrawlerArgs command"))
    }

    fn parse_args(extra: &[&str]) -> Result<crate::Args, String> {
        let mut argv = vec!["webfang"];
        argv.extend_from_slice(extra);
        clap::Parser::try_parse_from(argv).map_err(|e| e.to_string())
    }

    #[test]
    fn deferred_list_is_honest_about_its_reasons() {
        assert_eq!(spec::crawler::GROUP.len() + DEFERRED_FROM_SPEC.len(), 47);
        let args = command_args();
        for (id, _) in DEFERRED_FROM_SPEC {
            assert!(
                args.iter().any(|a| a.get_id() == *id),
                "deferred id `{id}` no longer exists in CrawlerArgs — update the list"
            );
        }
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        let args = command_args();
        let deferred_ids: Vec<&str> = DEFERRED_FROM_SPEC.iter().map(|(id, _)| *id).collect();
        for arg in &args {
            let id = arg.get_id().as_str();
            if matches!(id, "help" | "version") || deferred_ids.contains(&id) {
                continue;
            }
            assert!(
                spec::crawler::GROUP.iter().any(|s| s.id == id),
                "clap arg `{id}` has no OptionsSpec entry — spec is out of sync"
            );
        }
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        let args = command_args();
        for s in spec::crawler::GROUP {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(&args, s.id);
            assert_eq!(arg.get_long(), Some(s.long), "long mismatch for `{}`", s.id);
            assert_eq!(arg.get_short(), s.short, "short mismatch for `{}`", s.id);
            let aliases = arg.get_aliases().unwrap_or_default();
            assert_eq!(aliases, s.aliases, "alias mismatch for `{}`", s.id);
            let env = arg.get_env().map(|e| e.to_string_lossy().into_owned());
            assert_eq!(env.as_deref(), s.env, "env var mismatch for `{}`", s.id);
            // NOTE: clap's `next_help_heading` is stateful during command
            // construction and is NOT stored per-Arg, so it cannot be
            // introspected here. `spec.heading` is generator-facing data.
        }
    }

    #[test]
    fn defaults_match_the_spec() {
        let args = command_args();
        for s in spec::crawler::GROUP {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(&args, s.id);
            let defaults: Vec<String> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let expected: Vec<String> = s.default.map(|d| vec![d.to_owned()]).unwrap_or_default();
            assert_eq!(defaults, expected, "default mismatch for `{}`", s.id);
        }
    }

    #[test]
    fn help_text_matches_the_spec() {
        let args = command_args();
        for s in spec::crawler::GROUP {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(&args, s.id);
            let help = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .unwrap_or_else(|| panic!("arg `{}` has no help text", s.id))
                .to_string();
            assert_eq!(
                help.trim(),
                s.help.trim(),
                "help text mismatch for `{}`",
                s.id
            );
        }
    }

    #[test]
    fn representative_values_parse_identically_through_clap() {
        // Defaults.
        let defaults = parse_args(&[]).expect("bare invocation must parse");
        assert_eq!(defaults.crawler.selector, "body");
        assert_eq!(defaults.crawler.delay_ms, 1000);
        assert_eq!(defaults.crawler.max_pages, 10);
        assert_eq!(defaults.crawler.max_depth, 2);
        assert!(!defaults.crawler.use_sitemap);
        assert!(!defaults.crawler.resume);

        // Short forms in isolation (`--url` may only appear once per parse).
        let shorts =
            parse_args(&["-u", "https://example.org", "-s", "main"]).expect("shorts must parse");
        assert_eq!(shorts.crawler.url.as_deref(), Some("https://example.org"));
        assert_eq!(shorts.crawler.selector, "main");
    }

    #[test]
    fn target_discovery_behavior_and_display_flags_parse_identically() {
        let parsed = parse_args(&[
            "--url",
            "https://example.com",
            "--selector",
            "article p",
            "--delay-ms",
            "250",
            "--max-pages",
            "5",
            "--use-sitemap",
            "--sitemap-url",
            "https://example.com/sitemap.xml",
            "--single-page",
            "--resume",
            "--state-dir",
            "/tmp/wf-state",
            "--download-assets",
            "--extraction-fingerprint",
            "-vvv",
            "-q",
            "-n",
            "--trace-file",
            "trace.jsonl",
        ])
        .expect("representative crawler flags must parse");

        let c = &parsed.crawler;
        assert_eq!(c.url.as_deref(), Some("https://example.com"));
        assert_eq!(c.selector, "article p");
        assert_eq!(c.delay_ms, 250);
        assert_eq!(c.max_pages, 5);
        assert!(c.use_sitemap);
        assert_eq!(
            c.sitemap_url.as_deref(),
            Some("https://example.com/sitemap.xml")
        );
        assert!(c.single_page);
        assert!(c.resume);
        assert_eq!(c.state_dir, Some(std::path::PathBuf::from("/tmp/wf-state")));
        assert!(c.download_assets);
        assert!(!c.download_images);
        assert!(!c.download_documents);
        assert!(c.extraction_fingerprint);
        assert_eq!(c.verbose, 3);
        assert!(c.quiet);
        assert!(c.dry_run);
        assert_eq!(c.trace_file, Some(std::path::PathBuf::from("trace.jsonl")));
    }

    #[test]
    fn crawler_http_download_and_feature_flags_parse_identically() {
        let parsed = parse_args(&[
            "--max-depth",
            "4",
            "--timeout-secs",
            "60",
            "--asset-naming",
            "slug",
            "--download-concurrency",
            "8",
            "--max-retries",
            "2",
            "--backoff-base-ms",
            "500",
            "--backoff-max-ms",
            "8000",
            "--accept-language",
            "es-AR,es;q=0.9",
            "--user-agent",
            "webfang-parity/1.0",
            "--max-file-size",
            "1048576",
            "--download-timeout",
            "45",
            "--sitemap-depth",
            "1",
            "--checkpoint-interval",
            "50",
            "--no-checkpoint",
            "--ignore-robots",
            "--ignore-waf",
            "--autoscale",
            "--no-session-health",
            "--h2-profile",
            "Chrome131",
            "--js-strategy",
            "hybrid",
            "--obscura-binary",
            "/usr/local/bin/obscura",
            "--dom-preprune=false",
        ])
        .expect("representative crawler flags must parse");

        let c = &parsed.crawler;
        assert_eq!(c.max_depth, 4);
        assert_eq!(c.timeout_secs, 60);
        assert_eq!(c.asset_naming, "slug");
        assert_eq!(c.download_concurrency, Some(8));
        assert_eq!(c.max_retries, 2);
        assert_eq!(c.backoff_base_ms, 500);
        assert_eq!(c.backoff_max_ms, 8000);
        assert_eq!(c.accept_language, "es-AR,es;q=0.9");
        assert_eq!(c.user_agent.as_deref(), Some("webfang-parity/1.0"));
        assert_eq!(c.max_file_size, 1048576);
        assert_eq!(c.download_timeout, 45);
        assert_eq!(c.sitemap_depth, 1);
        assert_eq!(c.checkpoint_interval, 50);
        assert!(c.no_checkpoint);
        assert!(c.ignore_robots);
        assert!(c.ignore_waf);
        assert!(c.autoscale);
        assert!(c.no_session_health);
        assert_eq!(c.h2_profile, "Chrome131");
        assert_eq!(c.js_strategy, crate::domain::JsStrategy::Hybrid);
        assert_eq!(c.obscura_binary, "/usr/local/bin/obscura");
        assert!(!c.dom_preprune);
    }

    /// Slice 3 (ADR-002) pin: structural clap surface of the spec-covered
    /// crawler options that the spec-driven builder must reproduce
    /// byte-for-byte. Written against the derive BEFORE the migration.
    /// `-v` is exempted: it counts occurrences (`Count`), pinned below.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        let args = command_args();
        for s in spec::crawler::GROUP {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(&args, s.id);
            match s.kind {
                spec::ValueKind::Bool => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::SetTrue),
                        "bool `{}` must use SetTrue",
                        s.id
                    );
                },
                _ if s.id == "verbose" => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::Count),
                        "verbose must use Count",
                    );
                },
                _ => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::Set),
                        "value option `{}` must use Set",
                        s.id
                    );
                },
            }
            let names: Vec<String> = arg
                .get_value_names()
                .unwrap_or_default()
                .iter()
                .map(|id| id.to_string())
                .collect();
            assert_eq!(
                names,
                vec![s.id.to_ascii_uppercase()],
                "value name mismatch for `{}`",
                s.id
            );
            let possible: Vec<String> = arg
                .get_possible_values()
                .into_iter()
                .map(|v| v.get_name().to_string())
                .collect();
            if let spec::ValueKind::Enum { variants } = s.kind {
                assert_eq!(possible, variants, "possible values for `{}`", s.id);
            } else if !matches!(s.kind, spec::ValueKind::Bool) {
                assert!(
                    possible.is_empty(),
                    "`{}` must have no possible values",
                    s.id
                );
            }
            assert!(
                arg.get_long_help().is_none(),
                "`{}` must not carry long help",
                s.id
            );
        }
    }

    /// Slice 3 pin: the two special arities inside the group — `-v`
    /// counts occurrences and `--dom-preprune` accepts an optional value.
    #[test]
    fn count_and_optional_value_bool_arity_is_pinned() {
        let args = command_args();
        let verbose = arg_by_id(&args, "verbose");
        assert!(matches!(verbose.get_action(), clap::ArgAction::Count));

        let dom = arg_by_id(&args, "dom_preprune");
        assert!(matches!(dom.get_action(), clap::ArgAction::SetTrue));
        let range = dom
            .get_num_args()
            .expect("dom-preprune must declare num_args");
        assert_eq!((range.min_values(), range.max_values()), (0, 1));
        let defaults: Vec<String> = dom
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        assert_eq!(defaults, vec!["true"]);
    }

    /// Slice 3 pin: exact surface of every field DEFERRED from the spec,
    /// which the migrated command must keep building by hand. Help texts
    /// are byte-exact transcriptions (final period stripped by clap's doc
    /// rendering on SHORT help; multi-paragraph docs split into help +
    /// long_help). Split per arg to stay under the #516 ratchets.
    #[test]
    fn deferred_concurrency_surface_is_pinned_for_spec_build() {
        let args = command_args();
        let c = arg_by_id(&args, "concurrency");
        assert_eq!(c.get_long(), Some("concurrency"));
        assert_eq!(c.get_short(), None);
        assert_eq!(
            c.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_CONCURRENCY")
        );
        assert!(matches!(c.get_action(), clap::ArgAction::Set));
        assert_eq!(
            c.get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["auto"]
        );
        assert_eq!(help_of(c), "Concurrency level (auto or number)");
        assert!(c.get_long_help().is_none());
    }

    #[test]
    fn deferred_rate_limit_burst_surface_is_pinned_for_spec_build() {
        let args = command_args();
        let r = arg_by_id(&args, "rate_limit_burst");
        assert_eq!(r.get_long(), Some("rate-limit-burst"));
        assert_eq!(
            r.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_RATE_LIMIT_BURST")
        );
        assert!(matches!(r.get_action(), clap::ArgAction::Set));
        assert!(r.get_default_values().is_empty());
        assert_eq!(
            r.get_help()
                .expect("rate-limit-burst must carry short help")
                .to_string()
                .trim(),
            "Explicit rate-limiter burst permits (token-bucket capacity)"
        );
        let long = r
            .get_long_help()
            .expect("rate-limit-burst must carry long help")
            .to_string();
        assert!(long.contains("Overrides the hardware-derived budget-model default"));
        // Empirical byte truth: the LONG form keeps the final period even
        // though the SHORT form strips it.
        assert!(long.trim_end().ends_with("warn-and-default semantic."));
    }

    #[test]
    fn deferred_pattern_surfaces_are_pinned_for_spec_build() {
        let args = command_args();

        // --include-pattern <INCLUDE_PATTERNS>
        let inc = arg_by_id(&args, "include_patterns");
        assert_eq!(inc.get_long(), Some("include-pattern"));
        assert_eq!(
            inc.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_INCLUDE")
        );
        assert!(matches!(inc.get_action(), clap::ArgAction::Append));
        assert_eq!(inc.get_value_delimiter(), Some(','));
        assert_eq!(
            inc.get_help()
                .expect("include-pattern must carry short help")
                .to_string()
                .trim(),
            "URL patterns to include (glob-style). Three modes:"
        );
        assert!(inc.get_long_help().is_some());

        // --exclude-pattern <EXCLUDE_PATTERNS>
        let exc = arg_by_id(&args, "exclude_patterns");
        assert_eq!(exc.get_long(), Some("exclude-pattern"));
        assert_eq!(
            exc.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_EXCLUDE")
        );
        assert!(matches!(exc.get_action(), clap::ArgAction::Append));
        assert_eq!(exc.get_value_delimiter(), Some(','));
        assert_eq!(
            help_of(exc),
            "URL patterns to exclude (glob-style, same three modes as --include-pattern). Deny takes precedence over allow"
        );
        assert!(exc.get_long_help().is_none());
    }

    #[test]
    fn deferred_header_and_cookie_surfaces_are_pinned_for_spec_build() {
        let args = command_args();

        // -H, --header <NAME: VALUE>
        let h = arg_by_id(&args, "headers");
        assert_eq!(h.get_long(), Some("header"));
        assert_eq!(h.get_short(), Some('H'));
        assert_eq!(
            h.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_HEADER")
        );
        assert!(matches!(h.get_action(), clap::ArgAction::Append));
        assert_eq!(h.get_value_delimiter(), Some(';'));
        let names: Vec<String> = h
            .get_value_names()
            .unwrap_or_default()
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(names, vec!["NAME: VALUE"]);
        assert!(h.get_long_help().is_some());

        // --cookie <NAME=VALUE>
        let ck = arg_by_id(&args, "cookies");
        assert_eq!(ck.get_long(), Some("cookie"));
        assert_eq!(ck.get_short(), None);
        assert_eq!(
            ck.get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_COOKIE")
        );
        assert!(matches!(ck.get_action(), clap::ArgAction::Append));
        assert_eq!(ck.get_value_delimiter(), Some(';'));
        let names: Vec<String> = ck
            .get_value_names()
            .unwrap_or_default()
            .iter()
            .map(|i| i.to_string())
            .collect();
        assert_eq!(names, vec!["NAME=VALUE"]);
        assert!(ck.get_long_help().is_some());
    }

    /// Slice 3 pin: the feature-gated pair keeps its identity in BOTH
    /// compile configurations — visible flag under the feature, hidden
    /// compatibility placeholder without it (`visible_alias` only under
    /// `ai`, exactly like today's derive output).
    #[test]
    fn feature_gated_flags_keep_identity_across_cfg_combinations() {
        let args = command_args();

        let clean = arg_by_id(&args, "clean_ai");
        assert_eq!(clean.get_long(), Some("clean-ai"));
        assert_eq!(
            clean
                .get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_CLEAN_AI")
        );
        assert!(matches!(clean.get_action(), clap::ArgAction::SetTrue));
        assert_eq!(
            clean
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["false"]
        );
        if cfg!(feature = "ai") {
            assert_eq!(
                help_of(clean),
                "Use AI-powered semantic cleaning for better RAG output"
            );
            assert!(!clean.is_hide_set(), "clean_ai is visible under `ai`");
            let vis: Vec<&str> = clean.get_visible_aliases().into_iter().flatten().collect();
            assert_eq!(
                vis,
                vec!["ai"],
                "visible alias `ai` only under the ai feature"
            );
        } else {
            assert_eq!(
                help_of(clean),
                "Feature flag placeholder when AI is not enabled"
            );
            assert!(
                clean.is_hide_set(),
                "clean_ai placeholder must stay hidden without `ai`"
            );
        }

        let adaptive = arg_by_id(&args, "adaptive_selectors");
        assert_eq!(adaptive.get_long(), Some("adaptive-selectors"));
        assert_eq!(
            adaptive
                .get_env()
                .map(|e| e.to_string_lossy().into_owned())
                .as_deref(),
            Some("WEBFANG_ADAPTIVE_SELECTORS")
        );
        assert!(matches!(adaptive.get_action(), clap::ArgAction::SetTrue));
        assert_eq!(
            adaptive
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["false"]
        );
        if cfg!(feature = "adaptive-selectors") {
            assert_eq!(
                help_of(adaptive),
                "Enable adaptive CSS selector repair (2-tier cascade)"
            );
        } else {
            assert_eq!(
                help_of(adaptive),
                "Feature flag placeholder when adaptive-selectors is not enabled"
            );
        }
        assert_eq!(
            cfg!(feature = "adaptive-selectors"),
            !adaptive.is_hide_set()
        );
    }

    fn help_of(arg: &clap::Arg) -> String {
        arg.get_long_help()
            .or_else(|| arg.get_help())
            .unwrap_or_else(|| panic!("arg `{}` has no help text", arg.get_id()))
            .to_string()
            .trim()
            .to_owned()
    }

    #[test]
    fn out_of_bounds_and_malformed_inputs_error_exactly_as_before() {
        let err = parse_args(&["--max-pages", "0"]).expect_err("zero pages rejected");
        assert!(
            err.contains("--max-pages debe ser >= 1 (0 no deja páginas para scrapear)"),
            "got: {err}"
        );

        let err = parse_args(&["--max-pages", "abc"]).expect_err("text rejected");
        assert!(
            err.contains("'abc' no es un número válido para --max-pages"),
            "got: {err}"
        );

        let err = parse_args(&["--timeout-secs", "0"]).expect_err("zero timeout rejected");
        assert!(
            err.contains(
                "--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)"
            ),
            "got: {err}"
        );

        let err = parse_args(&["--timeout-secs", "9x"]).expect_err("malformed timeout rejected");
        assert!(
            err.contains("'9x' no es un número válido para --timeout-secs"),
            "got: {err}"
        );

        let err =
            parse_args(&["--download-concurrency", "0"]).expect_err("zero concurrency rejected");
        assert!(
            err.contains(
                "--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)"
            ),
            "got: {err}"
        );

        let err = parse_args(&["--download-concurrency", "muchas"])
            .expect_err("malformed concurrency rejected");
        assert!(
            err.contains("'muchas' no es un número válido para --download-concurrency"),
            "got: {err}"
        );
    }
}
