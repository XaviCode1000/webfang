//! Crawler flag group (ADR-002 slice 2): mirrors `cli::args::CrawlerArgs`
//! field-by-field for every option whose surface is spec-compatible. The
//! parity tests in that module enforce lockstep.
//!
//! Slice 3 (#924) unlocked the previously deferred feature-gated pair
//! `clean_ai` / `adaptive_selectors` via [`OptionSpec::feature_gate`] +
//! [`OptionSpec::active`], mirroring the derive's `cfg` duplication in the
//! SSOT.
//!
//! Still deferred (structurally unsuitable, recorded in the PR):
//! `concurrency` (custom `ConcurrencyConfig` FromStr with auto detection),
//! `rate_limit_burst` (raw-string staging validated in preflight,
//! warn-and-default semantics — not a clap bound), and `include_patterns`,
//! `exclude_patterns`, `headers`, `cookies` (`Vec` args with value
//! delimiters).
use super::{NumericPolicy, OptionSpec, ValueKind};

/// `--url <URL>` (short `-u`)
pub const URL: OptionSpec = OptionSpec {
    id: "url",
    long: "url",
    short: Some('u'),
    aliases: &[],
    env: Some("WEBFANG_URL"),
    default: None,
    help: "URL to scrape (required unless using a subcommand)",
    heading: Some("Target"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `-s, --selector <SELECTOR>`
pub const SELECTOR: OptionSpec = OptionSpec {
    id: "selector",
    long: "selector",
    short: Some('s'),
    aliases: &[],
    env: Some("WEBFANG_SELECTOR"),
    default: Some("body"),
    help: "CSS selector for content extraction",
    heading: Some("Target"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--delay-ms <DELAY_MS>` — metadata-only entry: parsing stays with
/// clap's built-in `u64` parser (its exact error strings are English and
/// must not change); `policy: None` records that no bound exists today.
pub const DELAY_MS: OptionSpec = OptionSpec {
    id: "delay_ms",
    long: "delay-ms",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DELAY_MS"),
    default: Some("1000"),
    help: "Delay between requests in milliseconds",
    heading: Some("Discovery"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--max-pages <MAX_PAGES>` — FULLY migrated: bound enforced through
/// [`OptionSpec::parse_uint`] with verbatim legacy messages (#780).
pub const MAX_PAGES: OptionSpec = OptionSpec {
    id: "max_pages",
    long: "max-pages",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_PAGES"),
    default: Some("10"),
    help: "Maximum pages to scrape",
    heading: Some("Discovery"),
    kind: ValueKind::uint(NumericPolicy::legacy_verbatim(
        1,
        "--max-pages debe ser >= 1 (0 no deja páginas para scrapear)",
        "'{value}' no es un número válido para --max-pages",
    )),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--use-sitemap`
pub const USE_SITEMAP: OptionSpec = OptionSpec {
        id: "use_sitemap",
        long: "use-sitemap",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_USE_SITEMAP"),
        // No explicit `default_value` attr: clap's implicit SetTrue default
        // is not introspectable via `get_default_values()`.
        default: None,
        help: "Use sitemap for URL discovery NOTE: HTTP redirects (301/302) are resolved at scrape-time, not parse-time. This avoids redundant HEAD requests during sitemap parsing for better performance",
        heading: Some("Discovery"),
        kind: ValueKind::Bool,
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--sitemap-url <SITEMAP_URL>`
pub const SITEMAP_URL: OptionSpec = OptionSpec {
    id: "sitemap_url",
    long: "sitemap-url",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_SITEMAP_URL"),
    default: None,
    help: "Explicit sitemap URL",
    heading: Some("Discovery"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--single-page`
pub const SINGLE_PAGE: OptionSpec = OptionSpec {
    id: "single_page",
    long: "single-page",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_SINGLE_PAGE"),
    default: Some("false"),
    help: "Scrape only the seed URL without discovery or crawling",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--resume`
pub const RESUME: OptionSpec = OptionSpec {
    id: "resume",
    long: "resume",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_RESUME"),
    // No explicit `default_value` attr: clap's implicit SetTrue default
    // is not introspectable via `get_default_values()`.
    default: None,
    help: "Resume mode - skip URLs already processed",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--state-dir <STATE_DIR>`
pub const STATE_DIR: OptionSpec = OptionSpec {
    id: "state_dir",
    long: "state-dir",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_STATE_DIR"),
    default: None,
    help: "Custom state directory for resume mode",
    heading: Some("Behavior"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--download-images`
pub const DOWNLOAD_IMAGES: OptionSpec = OptionSpec {
    id: "download_images",
    long: "download-images",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DOWNLOAD_IMAGES"),
    default: Some("false"),
    help: "Download images from the page",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--download-documents`
pub const DOWNLOAD_DOCUMENTS: OptionSpec = OptionSpec {
    id: "download_documents",
    long: "download-documents",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DOWNLOAD_DOCUMENTS"),
    default: Some("false"),
    help: "Download documents from the page",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--download-assets`
pub const DOWNLOAD_ASSETS: OptionSpec = OptionSpec {
    id: "download_assets",
    long: "download-assets",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DOWNLOAD_ASSETS"),
    default: Some("false"),
    help: "Download all assets (images + documents) from the page",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--extraction-fingerprint`
pub const EXTRACTION_FINGERPRINT: OptionSpec = OptionSpec {
        id: "extraction_fingerprint",
        long: "extraction-fingerprint",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_EXTRACTION_FINGERPRINT"),
        default: Some("false"),
        // Byte-exact transcription of clap's rendering of the multi-line doc
        // comment (lines joined with spaces; only the FINAL period stripped,
        // interior sentence periods retained).
        help: "Record extraction failure fingerprints in SQLite and attach them to low-quality extraction hints (#792). Repeated low-score extractions on the same site/selector pair accumulate a failure count surfaced in the hint, instead of degrading silently",
        heading: Some("Behavior"),
        kind: ValueKind::Bool,
        visible_aliases: &[],
        feature_gate: None,
    };

/// `-v, --verbose` — count action; metadata-only (`u8` count has no
/// bound).
pub const VERBOSE: OptionSpec = OptionSpec {
    id: "verbose",
    long: "verbose",
    short: Some('v'),
    aliases: &[],
    env: Some("WEBFANG_VERBOSE"),
    default: None,
    help: "Verbosity level: -v (INFO), -vv (DEBUG), -vvv (TRACE)",
    heading: Some("Display"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `-q, --quiet`
pub const QUIET: OptionSpec = OptionSpec {
    id: "quiet",
    long: "quiet",
    short: Some('q'),
    aliases: &[],
    env: Some("WEBFANG_QUIET"),
    default: Some("false"),
    help: "Quiet mode — suppress info/debug output",
    heading: Some("Display"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `-n, --dry-run`
pub const DRY_RUN: OptionSpec = OptionSpec {
    id: "dry_run",
    long: "dry-run",
    short: Some('n'),
    aliases: &[],
    env: Some("WEBFANG_DRY_RUN"),
    default: Some("false"),
    help: "Dry-run mode — discover URLs and print without scraping",
    heading: Some("Display"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--trace-file <TRACE_FILE>`
pub const TRACE_FILE: OptionSpec = OptionSpec {
    id: "trace_file",
    long: "trace-file",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_TRACE_FILE"),
    default: None,
    help: "Path to write OTel spans as JSONL for offline debugging",
    heading: Some("Display"),
    kind: ValueKind::Path,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--max-depth <MAX_DEPTH>` — metadata-only: 0 is meaningful ("only seed
/// URL"), so no bound exists today.
pub const MAX_DEPTH: OptionSpec = OptionSpec {
    id: "max_depth",
    long: "max-depth",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_DEPTH"),
    default: Some("2"),
    help: "Maximum depth to crawl (0 = only seed URL)",
    heading: Some("Crawler Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--timeout-secs <TIMEOUT_SECS>` — FULLY migrated: bound enforced
/// through [`OptionSpec::parse_uint`] with verbatim legacy messages.
pub const TIMEOUT_SECS: OptionSpec = OptionSpec {
    id: "timeout_secs",
    long: "timeout-secs",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_TIMEOUT_SECS"),
    default: Some("30"),
    help: "Request timeout in seconds",
    heading: Some("Crawler Settings"),
    kind: ValueKind::uint(NumericPolicy::legacy_verbatim(
        1,
        "--timeout-secs debe ser >= 1 (0 hace que cada request falle al instante)",
        "'{value}' no es un número válido para --timeout-secs",
    )),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--asset-naming <ASSET_NAMING>`
pub const ASSET_NAMING: OptionSpec = OptionSpec {
        id: "asset_naming",
        long: "asset-naming",
        short: None,
        aliases: &[],
        env: None,
        default: Some("hash"),
        help: "Estrategia de nombre de archivo para assets descargados: hash (default), slug, content-disposition",
        heading: None,
        kind: ValueKind::Enum {
            variants: &["hash", "slug", "content-disposition"],
        },
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--download-concurrency <DOWNLOAD_CONCURRENCY>` — FULLY migrated:
/// bound enforced through [`OptionSpec::parse_uint`] with verbatim legacy
/// messages (D1 deadlock guard).
pub const DOWNLOAD_CONCURRENCY: OptionSpec = OptionSpec {
    id: "download_concurrency",
    long: "download-concurrency",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DOWNLOAD_CONCURRENCY"),
    default: None,
    // Explicit `help = ...` attribute overrides the doc comment.
    help: "Máximo de descargas de assets concurrentes por página (mínimo 1)",
    heading: None,
    kind: ValueKind::uint(NumericPolicy::legacy_verbatim(
        1,
        "--download-concurrency debe ser >= 1 (0 causa un deadlock / hang infinito)",
        "'{value}' no es un número válido para --download-concurrency",
    )),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--max-retries <MAX_RETRIES>` — metadata-only (no bound today).
pub const MAX_RETRIES: OptionSpec = OptionSpec {
    id: "max_retries",
    long: "max-retries",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_RETRIES"),
    default: Some("3"),
    help: "Maximum number of retry attempts",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--backoff-base-ms <BACKOFF_BASE_MS>` — metadata-only (no bound
/// today).
pub const BACKOFF_BASE_MS: OptionSpec = OptionSpec {
    id: "backoff_base_ms",
    long: "backoff-base-ms",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BACKOFF_BASE_MS"),
    default: Some("1000"),
    help: "Base delay for exponential backoff (ms)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--backoff-max-ms <BACKOFF_MAX_MS>` — metadata-only (no bound today).
pub const BACKOFF_MAX_MS: OptionSpec = OptionSpec {
    id: "backoff_max_ms",
    long: "backoff-max-ms",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_BACKOFF_MAX_MS"),
    default: Some("10000"),
    help: "Maximum delay for exponential backoff (ms)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--accept-language <ACCEPT_LANGUAGE>`
pub const ACCEPT_LANGUAGE: OptionSpec = OptionSpec {
    id: "accept_language",
    long: "accept-language",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_ACCEPT_LANGUAGE"),
    default: Some("en-US,en;q=0.9"),
    help: "Accept-Language header value",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--user-agent <USER_AGENT>`
pub const USER_AGENT: OptionSpec = OptionSpec {
    id: "user_agent",
    long: "user-agent",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_USER_AGENT"),
    default: None,
    help: "Custom User-Agent header value (overrides Chrome 145 default)",
    heading: Some("HTTP Client Settings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--max-file-size <MAX_FILE_SIZE>` — metadata-only (no bound today).
pub const MAX_FILE_SIZE: OptionSpec = OptionSpec {
    id: "max_file_size",
    long: "max-file-size",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_MAX_FILE_SIZE"),
    default: Some("52428800"),
    help: "Maximum file size to download in bytes (default: 50MB)",
    heading: Some("Download Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--download-timeout <DOWNLOAD_TIMEOUT>` — metadata-only (no bound
/// today).
pub const DOWNLOAD_TIMEOUT: OptionSpec = OptionSpec {
    id: "download_timeout",
    long: "download-timeout",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_DOWNLOAD_TIMEOUT"),
    default: Some("30"),
    help: "Timeout for individual asset downloads in seconds",
    heading: Some("Download Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--sitemap-depth <SITEMAP_DEPTH>` — metadata-only (no bound today).
pub const SITEMAP_DEPTH: OptionSpec = OptionSpec {
    id: "sitemap_depth",
    long: "sitemap-depth",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_SITEMAP_DEPTH"),
    default: Some("3"),
    help: "Maximum recursion depth for sitemap indexes",
    heading: Some("Sitemap Settings"),
    kind: ValueKind::uint_unbounded(),
    visible_aliases: &[],
    feature_gate: None,
};

/// `--checkpoint-interval <CHECKPOINT_INTERVAL>` — metadata-only (0 =
/// disabled by design, no bound today).
pub const CHECKPOINT_INTERVAL: OptionSpec = OptionSpec {
        id: "checkpoint_interval",
        long: "checkpoint-interval",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_CHECKPOINT_INTERVAL"),
        default: Some("100"),
        help: "Pages between automatic checkpoint saves (0 = disabled) NOTE: Checkpoint is for programmatic use (Engine API) only. CLI --resume uses StateStore instead of checkpoints",
        heading: Some("Competitive Features"),
        kind: ValueKind::uint_unbounded(),
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--no-checkpoint`
pub const NO_CHECKPOINT: OptionSpec = OptionSpec {
        id: "no_checkpoint",
        long: "no-checkpoint",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_NO_CHECKPOINT"),
        default: Some("false"),
        help: "Disable checkpoint persistence entirely NOTE: Checkpoint is for programmatic use (Engine API) only. CLI --resume uses StateStore instead of checkpoints",
        heading: Some("Competitive Features"),
        kind: ValueKind::Bool,
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--ignore-robots`
pub const IGNORE_ROBOTS: OptionSpec = OptionSpec {
    id: "ignore_robots",
    long: "ignore-robots",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_IGNORE_ROBOTS"),
    default: Some("false"),
    help: "Skip robots.txt enforcement",
    heading: Some("Competitive Features"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--ignore-waf`
pub const IGNORE_WAF: OptionSpec = OptionSpec {
    id: "ignore_waf",
    long: "ignore-waf",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_IGNORE_WAF"),
    default: Some("false"),
    help: "Bypass WAF/CAPTCHA detection entirely (never block on challenge markers)",
    heading: Some("Competitive Features"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--autoscale`
pub const AUTOSCALE: OptionSpec = OptionSpec {
    id: "autoscale",
    long: "autoscale",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_AUTOSCALE"),
    default: Some("false"),
    help: "Enable autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage",
    heading: Some("Competitive Features"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--no-session-health`
pub const NO_SESSION_HEALTH: OptionSpec = OptionSpec {
    id: "no_session_health",
    long: "no-session-health",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_NO_SESSION_HEALTH"),
    default: Some("false"),
    help: "Disable session pool health checks",
    heading: Some("Competitive Features"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--h2-profile <H2_PROFILE>`
pub const H2_PROFILE: OptionSpec = OptionSpec {
    id: "h2_profile",
    long: "h2-profile",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_H2_PROFILE"),
    default: Some("Chrome145"),
    help: "TLS/HTTP2 profile name (default: Chrome145)",
    heading: Some("Competitive Features"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--js-strategy <JS_STRATEGY>`
pub const JS_STRATEGY: OptionSpec = OptionSpec {
        id: "js_strategy",
        long: "js-strategy",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_JS_STRATEGY"),
        default: Some("static"),
        help: "JavaScript rendering strategy: static (wreq only), hybrid (3-layer), full (Chromiumoxide only)",
        heading: Some("JS Rendering"),
        kind: ValueKind::Enum {
            variants: &["static", "hybrid", "full"],
        },
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--obscura-binary <OBSCURA_BINARY>`
pub const OBSCURA_BINARY: OptionSpec = OptionSpec {
    id: "obscura_binary",
    long: "obscura-binary",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_OBSCURA_BINARY"),
    default: Some("obscura"),
    help: "Path to the obscura binary (default: \"obscura\")",
    heading: Some("JS Rendering"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    feature_gate: None,
};

/// `--dom-preprune` — optional-value bool (`num_args(0..=1)`); the arity
/// nuance stays in the derive, the spec carries identity/metadata.
pub const DOM_PREPRUNE: OptionSpec = OptionSpec {
        id: "dom_preprune",
        long: "dom-preprune",
        short: None,
        aliases: &[],
        env: Some("WEBFANG_DOM_PREPRUNE"),
        default: Some("true"),
        help: "Enable DOM pre-pruning before Readability (removes invisible/empty wrappers). Default: enabled (true). Set to false via --dom-preprune=false or WEBFANG_DOM_PREPRUNE=false",
        heading: Some("Cleanup"),
        kind: ValueKind::Bool,
        visible_aliases: &[],
        feature_gate: None,
    };

/// `--clean-ai` (alias `--ai`) — feature-gated: materializes only under
/// the `ai` cargo feature; without it the runtime command keeps the hidden
/// compatibility placeholder (builder concern, see slice 3).
pub const CLEAN_AI: OptionSpec = OptionSpec {
    id: "clean_ai",
    long: "clean-ai",
    short: None,
    aliases: &[],
    visible_aliases: &["ai"],
    env: Some("WEBFANG_CLEAN_AI"),
    default: Some("false"),
    help: "Use AI-powered semantic cleaning for better RAG output",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    feature_gate: Some("ai"),
};

/// `--adaptive-selectors` — feature-gated under `adaptive-selectors`.
pub const ADAPTIVE_SELECTORS: OptionSpec = OptionSpec {
    id: "adaptive_selectors",
    long: "adaptive-selectors",
    short: None,
    aliases: &[],
    visible_aliases: &[],
    env: Some("WEBFANG_ADAPTIVE_SELECTORS"),
    default: Some("false"),
    help: "Enable adaptive CSS selector repair (2-tier cascade)",
    heading: Some("Behavior"),
    kind: ValueKind::Bool,
    feature_gate: Some("adaptive-selectors"),
};

/// All crawler-group options, in `CrawlerArgs` field-declaration order
/// (structurally-deferred fields omitted; see the module documentation).
/// Feature-gated entries stay listed: consumers filter with
/// [`OptionSpec::active`].
pub const GROUP: &[OptionSpec] = &[
    URL,
    SELECTOR,
    DELAY_MS,
    MAX_PAGES,
    USE_SITEMAP,
    SITEMAP_URL,
    SINGLE_PAGE,
    RESUME,
    STATE_DIR,
    DOWNLOAD_IMAGES,
    DOWNLOAD_DOCUMENTS,
    DOWNLOAD_ASSETS,
    EXTRACTION_FINGERPRINT,
    CLEAN_AI,
    ADAPTIVE_SELECTORS,
    VERBOSE,
    QUIET,
    DRY_RUN,
    TRACE_FILE,
    MAX_DEPTH,
    TIMEOUT_SECS,
    ASSET_NAMING,
    DOWNLOAD_CONCURRENCY,
    MAX_RETRIES,
    BACKOFF_BASE_MS,
    BACKOFF_MAX_MS,
    ACCEPT_LANGUAGE,
    USER_AGENT,
    MAX_FILE_SIZE,
    DOWNLOAD_TIMEOUT,
    SITEMAP_DEPTH,
    CHECKPOINT_INTERVAL,
    NO_CHECKPOINT,
    IGNORE_ROBOTS,
    IGNORE_WAF,
    AUTOSCALE,
    NO_SESSION_HEALTH,
    H2_PROFILE,
    JS_STRATEGY,
    OBSCURA_BINARY,
    DOM_PREPRUNE,
];
