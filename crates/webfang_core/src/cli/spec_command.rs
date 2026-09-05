//! Spec-driven clap assembly for migrated flag groups (ADR-002 slice 3, #924).
//!
//! The export and crawler `Command` args are built FROM the `OptionsSpec`
//! data ([`OptionSpec`]) instead of from clap's derive attributes. The spec
//! owns identity (long/short/aliases), env, default, help text, heading,
//! value domain, and feature gating; this module supplies only the concrete
//! Rust type bindings that a declarative spec cannot carry (the typed value
//! parsers and the two special arities) — the "non-spec concerns" allowed by
//! the issue's acceptance criteria.
//!
//! # Heading policy
//!
//! The spec owns each arg's `help_heading`; the runtime build path
//! ([`Headings::Applied`]) wires it onto every arg so `--help` renders
//! grouped sections (`Target:`, `Discovery:`, `Crawler Settings:`,
//! `Competitive Features:`, …) instead of one flat `Options:` block (#932,
//! post-ADR-002). The parity suite builds the same specs through the same
//! `Headings::Applied` and pins `get_help_heading() == spec.heading` on
//! every entry as the construction-time correctness oracle.
//!
//! # Deferred crawler fields
//!
//! Six fields stay outside the spec (structurally unsuitable; see
//! `options_spec::crawler`): `concurrency`, `rate_limit_burst`,
//! `include_patterns`, `exclude_patterns`, `headers`, `cookies`. They are
//! built here by hand at their exact declaration-order positions so the
//! help listing order never changes.

use crate::domain::options_spec::{self, OptionSpec, ValueKind};
use clap::builder::{ArgAction, PossibleValuesParser, ValueParser};

/// Whether assembled args carry their spec heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Headings {
    /// Runtime shape: headings omitted (legacy; only the parity suite still
    /// pins it to keep the `Headings::Applied` path honest by contrast).
    Omitted,
    /// Parity-suite shape: `help_heading` applied from the spec so
    /// `get_help_heading()` can be asserted against `spec.heading`.
    Applied,
}

// Non-test reference so `Headings::Omitted` is not flagged as dead when
// compiling without `cfg(test)` — the variant is exercised by the parity
// suite, but `dead_code` only sees non-test builds.
#[allow(dead_code)]
const _HEADINGS_OMITTED_USED: Headings = Headings::Omitted;

/// Build ONE clap arg from an OptionSpec plus its typed bindings. Unknown
/// ids fail loudly at assembly time (a new spec entry without bindings must
/// be wired here first — enforced by the parity suite before it can ship).
fn build_arg(spec: &'static OptionSpec, headings: Headings) -> clap::Arg {
    let mut arg = clap::Arg::new(spec.id)
        .long(spec.long)
        .value_name(spec.value_name)
        .help(spec.help);

    if let Some(short) = spec.short {
        arg = arg.short(short);
    }
    for alias in spec.aliases {
        arg = arg.alias(alias);
    }
    for alias in spec.visible_aliases {
        arg = arg.visible_alias(alias);
    }
    if let Some(env) = spec.env {
        arg = arg.env(env);
    }
    if let Some(delimiter) = spec.value_delimiter {
        arg = arg.value_delimiter(delimiter);
    }
    if let Some(default) = spec.default {
        // `DefaultValue` already implements `Display` (see `options_spec::mod`)
        // — that is the canonical string form for every variant, including any
        // future `Uint` value, so no per-literal arm table is needed here.
        // clap 4 is built without the `string` feature, so `default_value`
        // only accepts `&'static str` via `Into<OsStr>`; `Box::leak` promotes
        // the rendered `String` to `&'static str`. The leak is bounded by the
        // number of spec entries (~54 short strings) and lives for the process
        // lifetime — negligible, and the standard idiom for clap defaults.
        let leaked: &'static str = Box::leak(default.to_string().into_boxed_str());
        arg = arg.default_value(leaked);
    }
    if headings == Headings::Applied {
        if let Some(heading) = spec.heading {
            arg = arg.help_heading(heading);
        }
    }

    arg = match spec.kind {
        ValueKind::Bool => {
            let mut a = arg
                .action(ArgAction::SetTrue)
                .value_parser(clap::value_parser!(bool));
            if spec.id == "dom_preprune" {
                // Optional-value bool: `--dom-preprune=false` must be accepted.
                a = a.num_args(0..=1);
            }
            a
        },
        ValueKind::Enum { .. } => {
            let parser = enum_binding(spec.id).unwrap_or_else(|| {
                panic!(
                    "spec option `{}` needs a typed enum parser binding",
                    spec.id
                )
            });
            arg.value_parser(parser)
        },
        ValueKind::Text => arg.value_parser(clap::value_parser!(String)),
        ValueKind::TextList => {
            // Comma-delimited list: parser stays `String` and the
            // `value_delimiter` (applied above from `spec.value_delimiter`)
            // makes `get_many::<String>` return the split values. Binding
            // the parser to `Vec<String>` is NOT supported by clap 4's
            // `value_parser!` macro; the splitter is the integration
            // point. The `FromArgMatches` consumer reads via
            // [`extract::opt_many`] / [`extract::many`].
            //
            // Action MUST be `Append`: the source `Option<Vec<String>>` /
            // `Vec<String>` derive resolves to `ArgAction::Append`
            // (clap_derive `Ty::OptionVec`), so repeated `--flag a --flag b`
            // appends rather than overwriting. `Set` (the arg default)
            // would keep only the last occurrence and silently drop values.
            arg.action(ArgAction::Append)
                .value_parser(clap::value_parser!(String))
        },
        ValueKind::Path => arg.value_parser(clap::value_parser!(std::path::PathBuf)),
        ValueKind::Uint { .. } | ValueKind::MemorySize { .. } => {
            let parser = numeric_binding(spec.id).unwrap_or_else(|| {
                panic!(
                    "numeric spec option `{}` needs a typed parser binding",
                    spec.id
                )
            });
            let mut a = arg.value_parser(parser);
            if spec.id == "verbose" {
                // `-v` counts occurrences rather than taking a value.
                a = a.action(ArgAction::Count);
            }
            a
        },
    };

    // Feature-gated entries that are compiled OUT keep a hidden compatibility
    // placeholder, mirroring the pre-slice-3 `cfg(not(...))` duplication
    // byte-for-byte (identity/env/default/parse behavior stay identical; only
    // visibility and help text differ). Aliases already applied above match
    // both configurations because hidden args render nothing.
    if !spec.active() {
        arg = arg.help(hidden_placeholder_help(spec)).hide(true);
    }

    arg
}

/// Help text of the hidden compatibility placeholder for a feature-gated
/// option whose gate is off (legacy cfg-duplication surface).
fn hidden_placeholder_help(spec: &OptionSpec) -> &'static str {
    match spec.id {
        "clean_ai" => "Feature flag placeholder when AI is not enabled",
        "adaptive_selectors" => "Feature flag placeholder when adaptive-selectors is not enabled",
        other => panic!("feature-gated spec option `{other}` has no placeholder help binding"),
    }
}

/// Typed enum parser per spec id: the concrete domain enums behind the
/// `ValueKind::Enum` entries (`asset_naming` stays a plain `String` field).
fn enum_binding(id: &str) -> Option<ValueParser> {
    match id {
        "format" => Some(ValueParser::from(clap::value_parser!(
            crate::domain::config::OutputFormat
        ))),
        "export_format" => Some(ValueParser::from(clap::value_parser!(
            crate::domain::config::ExportFormat
        ))),
        "pipeline_output" => Some(ValueParser::from(clap::value_parser!(
            crate::domain::config::PipelineOutputFormat
        ))),
        "js_strategy" => Some(ValueParser::from(clap::value_parser!(
            crate::domain::JsStrategy
        ))),
        "asset_naming" => Some(ValueParser::from(PossibleValuesParser::new([
            "hash",
            "slug",
            "content-disposition",
        ]))),
        _ => None,
    }
}

/// Typed numeric parser per spec id. Custom validators route their bounds
/// and messages through the spec (single enforcement point); metadata-only
/// entries bind clap's built-in integer parsers of the EXACT field width.
fn numeric_binding(id: &str) -> Option<ValueParser> {
    fn str_fn<T, F>(f: F) -> ValueParser
    where
        T: Send + Sync + Clone + 'static,
        F: Fn(&str) -> Result<T, String> + Send + Sync + Clone + 'static,
    {
        ValueParser::from(f)
    }

    match id {
        "cpu_cores" => Some(str_fn(super::args::export::parse_cpu_cores)),
        "batch_concurrency" => Some(str_fn(super::args::export::parse_batch_concurrency)),
        "ram_budget" => Some(str_fn(super::args::export::parse_ram_budget)),
        "max_pages" => Some(str_fn(super::args::crawler::parse_max_pages)),
        "timeout_secs" => Some(str_fn(super::args::crawler::parse_timeout_secs)),
        "download_concurrency" => Some(str_fn(super::args::crawler::parse_download_concurrency)),
        "selector" => Some(str_fn(super::args::crawler::parse_selector)),
        "max_depth" => Some(str_fn(super::args::crawler::parse_max_depth)),
        "max_tokens" => Some(ValueParser::from(clap::value_parser!(usize))),
        "delay_ms"
        | "backoff_base_ms"
        | "backoff_max_ms"
        | "max_file_size"
        | "download_timeout"
        | "checkpoint_interval" => Some(ValueParser::from(clap::value_parser!(u64))),
        "verbose" | "sitemap_depth" => Some(ValueParser::from(clap::value_parser!(u8))),
        "max_retries" => Some(ValueParser::from(clap::value_parser!(u32))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Export group
// ---------------------------------------------------------------------------

/// All export-group args in declaration order — pure spec build.
pub(crate) fn export_args(headings: Headings) -> Vec<clap::Arg> {
    options_spec::export::GROUP
        .iter()
        .map(|s| build_arg(s, headings))
        .collect()
}

// ---------------------------------------------------------------------------
// AI group
// ---------------------------------------------------------------------------

/// One slot of the AI layout: either a spec entry or a hand-built deferred
/// arg, positioned exactly where the field is declared. Mirrors the
/// [`CrawlerSlot`] pattern (slice 2). The manual `threshold` slot carries
/// the `f32` parser + range check + verbatim Spanish error message that
/// the spec SSOT does not yet model — see the defer note in
/// [`options_spec::ai::THRESHOLD`].
#[cfg(feature = "ai")]
enum AiSlot {
    Spec(&'static OptionSpec),
    Manual(fn() -> clap::Arg),
}

/// AI declaration order: the hand-built `threshold` first, then the three
/// spec entries in `AiArgs` field-declaration order.
#[cfg(feature = "ai")]
const AI_LAYOUT: &[AiSlot] = &[
    AiSlot::Manual(manual_threshold),
    AiSlot::Spec(&options_spec::ai::MAX_TOKENS),
    AiSlot::Spec(&options_spec::ai::OFFLINE),
    AiSlot::Spec(&options_spec::ai::AI_MODEL),
];

/// All AI-group args in declaration order. With `cfg(not(feature = "ai"))`
/// the AI group is empty (the pre-migration derive produced zero AI args
/// under that configuration; the spec's `feature_gate = Some("ai")` would
/// otherwise render hidden placeholders — wrong for this group).
#[cfg(feature = "ai")]
pub(crate) fn ai_args(headings: Headings) -> Vec<clap::Arg> {
    AI_LAYOUT
        .iter()
        .map(|slot| match slot {
            AiSlot::Spec(spec) => build_arg(spec, headings),
            AiSlot::Manual(build) => build(),
        })
        .collect()
}

/// `cfg(not(feature = "ai"))` counterpart: zero args, matching the
/// pre-migration derive's behavior of producing no AI flags without the
/// cargo feature.
#[cfg(not(feature = "ai"))]
pub(crate) fn ai_args(_headings: Headings) -> Vec<clap::Arg> {
    Vec::new()
}

/// Hand-built `--threshold` (deferred from the spec — see
/// [`options_spec::ai::THRESHOLD`]). Parser + range + error messages come
/// from [`super::args::ai::parse_threshold`]. The spec records the
/// identity (id, long, env, default, help, heading) verbatim.
#[cfg(feature = "ai")]
fn manual_threshold() -> clap::Arg {
    use super::args::ai::parse_threshold;
    clap::Arg::new("threshold")
        .long("threshold")
        .value_name("THRESHOLD")
        .env("WEBFANG_THRESHOLD")
        .default_value("0.3")
        .value_parser(parse_threshold)
        .allow_negative_numbers(true)
        .help("Relevance threshold for AI semantic filtering (0.0-1.0)")
        .help_heading("AI Settings")
}

// ---------------------------------------------------------------------------
// Obsidian group
// ---------------------------------------------------------------------------

/// All Obsidian-group args in declaration order — pure spec build.
pub(crate) fn obsidian_args(headings: Headings) -> Vec<clap::Arg> {
    options_spec::obsidian::GROUP
        .iter()
        .map(|s| build_arg(s, headings))
        .collect()
}

// ---------------------------------------------------------------------------
// Crawler group
// ---------------------------------------------------------------------------

/// One slot of the crawler layout: either a spec entry or a hand-built
/// deferred arg, positioned exactly where the field is declared.
enum CrawlerSlot {
    Spec(&'static OptionSpec),
    Manual(fn() -> clap::Arg),
}

/// Crawler declaration order: spec slots plus the six manual fields.
const CRAWLER_LAYOUT: &[CrawlerSlot] = &[
    CrawlerSlot::Spec(&options_spec::crawler::URL),
    CrawlerSlot::Spec(&options_spec::crawler::SELECTOR),
    CrawlerSlot::Spec(&options_spec::crawler::DELAY_MS),
    CrawlerSlot::Spec(&options_spec::crawler::MAX_PAGES),
    CrawlerSlot::Manual(manual_concurrency),
    CrawlerSlot::Manual(manual_rate_limit_burst),
    CrawlerSlot::Spec(&options_spec::crawler::USE_SITEMAP),
    CrawlerSlot::Spec(&options_spec::crawler::SITEMAP_URL),
    CrawlerSlot::Spec(&options_spec::crawler::SINGLE_PAGE),
    CrawlerSlot::Spec(&options_spec::crawler::RESUME),
    CrawlerSlot::Spec(&options_spec::crawler::STATE_DIR),
    CrawlerSlot::Spec(&options_spec::crawler::DOWNLOAD_IMAGES),
    CrawlerSlot::Spec(&options_spec::crawler::DOWNLOAD_DOCUMENTS),
    CrawlerSlot::Spec(&options_spec::crawler::DOWNLOAD_ASSETS),
    CrawlerSlot::Spec(&options_spec::crawler::EXTRACTION_FINGERPRINT),
    CrawlerSlot::Spec(&options_spec::crawler::CLEAN_AI),
    CrawlerSlot::Spec(&options_spec::crawler::ADAPTIVE_SELECTORS),
    CrawlerSlot::Spec(&options_spec::crawler::VERBOSE),
    CrawlerSlot::Spec(&options_spec::crawler::QUIET),
    CrawlerSlot::Spec(&options_spec::crawler::DRY_RUN),
    CrawlerSlot::Spec(&options_spec::crawler::TRACE_FILE),
    CrawlerSlot::Spec(&options_spec::crawler::MAX_DEPTH),
    CrawlerSlot::Spec(&options_spec::crawler::TIMEOUT_SECS),
    CrawlerSlot::Manual(manual_include_patterns),
    CrawlerSlot::Manual(manual_exclude_patterns),
    CrawlerSlot::Spec(&options_spec::crawler::ASSET_NAMING),
    CrawlerSlot::Spec(&options_spec::crawler::DOWNLOAD_CONCURRENCY),
    CrawlerSlot::Spec(&options_spec::crawler::MAX_RETRIES),
    CrawlerSlot::Spec(&options_spec::crawler::BACKOFF_BASE_MS),
    CrawlerSlot::Spec(&options_spec::crawler::BACKOFF_MAX_MS),
    CrawlerSlot::Spec(&options_spec::crawler::ACCEPT_LANGUAGE),
    CrawlerSlot::Spec(&options_spec::crawler::USER_AGENT),
    CrawlerSlot::Manual(manual_headers),
    CrawlerSlot::Manual(manual_cookies),
    CrawlerSlot::Spec(&options_spec::crawler::MAX_FILE_SIZE),
    CrawlerSlot::Spec(&options_spec::crawler::DOWNLOAD_TIMEOUT),
    CrawlerSlot::Spec(&options_spec::crawler::SITEMAP_DEPTH),
    CrawlerSlot::Spec(&options_spec::crawler::CHECKPOINT_INTERVAL),
    CrawlerSlot::Spec(&options_spec::crawler::NO_CHECKPOINT),
    CrawlerSlot::Spec(&options_spec::crawler::IGNORE_ROBOTS),
    CrawlerSlot::Spec(&options_spec::crawler::IGNORE_WAF),
    CrawlerSlot::Spec(&options_spec::crawler::AUTOSCALE),
    CrawlerSlot::Spec(&options_spec::crawler::NO_SESSION_HEALTH),
    CrawlerSlot::Spec(&options_spec::crawler::H2_PROFILE),
    CrawlerSlot::Spec(&options_spec::crawler::JS_STRATEGY),
    CrawlerSlot::Spec(&options_spec::crawler::OBSCURA_BINARY),
    CrawlerSlot::Spec(&options_spec::crawler::DOM_PREPRUNE),
];

/// All crawler-group args in declaration order.
pub(crate) fn crawler_args(headings: Headings) -> Vec<clap::Arg> {
    CRAWLER_LAYOUT
        .iter()
        .map(|slot| match slot {
            CrawlerSlot::Spec(spec) => build_arg(spec, headings),
            CrawlerSlot::Manual(build) => build(),
        })
        .collect()
}

// -- Hand-built deferred fields (surface pinned by the parity suite) --------

fn manual_concurrency() -> clap::Arg {
    clap::Arg::new("concurrency")
        .long("concurrency")
        .value_name("CONCURRENCY")
        .env("WEBFANG_CONCURRENCY")
        .default_value("auto")
        .value_parser(clap::value_parser!(
            crate::domain::config::ConcurrencyConfig
        ))
        .help("Concurrency level (auto or number)")
        .help_heading("Discovery")
}

fn manual_rate_limit_burst() -> clap::Arg {
    clap::Arg::new("rate_limit_burst")
        .long("rate-limit-burst")
        .value_name("RATE_LIMIT_BURST")
        .env("WEBFANG_RATE_LIMIT_BURST")
        .action(ArgAction::Set)
        .value_parser(clap::value_parser!(String))
        .help("Explicit rate-limiter burst permits (token-bucket capacity)")
        .long_help(
            "Explicit rate-limiter burst permits (token-bucket capacity).\n\n\
             Overrides the hardware-derived budget-model default (Q1: burst is \
             decoupled from crawl concurrency). Raw string here ON PURPOSE: \
             validation/conversion happens once in preflight staging via \
             `parse_rate_limit_burst` so CLI, env, and programmatic input all \
             share one accept / reject-0 / warn-and-default semantic.",
        )
        .help_heading("Discovery")
}

fn manual_include_patterns() -> clap::Arg {
    clap::Arg::new("include_patterns")
        .long("include-pattern")
        .value_name("INCLUDE_PATTERNS")
        .env("WEBFANG_INCLUDE")
        .action(ArgAction::Append)
        .value_delimiter(',')
        .value_parser(clap::value_parser!(String))
        .help("URL patterns to include (glob-style). Three modes:")
        .long_help(
            "URL patterns to include (glob-style). Three modes:\n\n\
             * Path: starts with `/` \u{2192} matched against URL path, e.g. \
             `/pricing`, `/admin/*` * Path glob: starts with `*/` \u{2192} matched \
             against URL path, e.g. `*/api/*` * Host (default): matched against \
             hostname, e.g. `example.com`, `*.example.com`\n\n\
             Example: to exclude a path, use `--exclude-pattern \"/admin/*\"`, \
             not `*admin*`",
        )
        .help_heading("Crawler Settings")
}

fn manual_exclude_patterns() -> clap::Arg {
    clap::Arg::new("exclude_patterns")
        .long("exclude-pattern")
        .value_name("EXCLUDE_PATTERNS")
        .env("WEBFANG_EXCLUDE")
        .action(ArgAction::Append)
        .value_delimiter(',')
        .value_parser(clap::value_parser!(String))
        .help(
            "URL patterns to exclude (glob-style, same three modes as \
             --include-pattern). Deny takes precedence over allow",
        )
        .help_heading("Crawler Settings")
}

fn manual_headers() -> clap::Arg {
    clap::Arg::new("headers")
        .short('H')
        .long("header")
        .value_name("NAME: VALUE")
        .env("WEBFANG_HEADER")
        .action(ArgAction::Append)
        .value_delimiter(';')
        .value_parser(clap::value_parser!(String))
        .help("Inject a custom HTTP header as `Name: Value` (repeatable)")
        .long_help(
            "Inject a custom HTTP header as `Name: Value` (repeatable).\n\n\
             Overrides any default header with the same (case-insensitive) name. \
             Example: `-H \"Authorization: Bearer TOKEN\"`.",
        )
        .help_heading("HTTP Client Settings")
}

fn manual_cookies() -> clap::Arg {
    clap::Arg::new("cookies")
        .long("cookie")
        .value_name("NAME=VALUE")
        .env("WEBFANG_COOKIE")
        .action(ArgAction::Append)
        .value_delimiter(';')
        .value_parser(clap::value_parser!(String))
        .help("Inject a custom cookie as `name=value` (repeatable)")
        .long_help(
            "Inject a custom cookie as `name=value` (repeatable).\n\n\
             Seeded into the cookie jar before the first request so authenticated \
             crawls work without a prior login round-trip. Example: \
             `--cookie \"session=abc123\"`.",
        )
        .help_heading("HTTP Client Settings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::options_spec::DefaultValue;

    /// Every active spec option must assemble without panicking (legacy heading shape).
    #[test]
    fn all_groups_assemble_omitted() {
        assert_eq!(
            export_args(Headings::Omitted).len(),
            options_spec::export::GROUP.len()
        );
        assert_eq!(crawler_args(Headings::Omitted).len(), CRAWLER_LAYOUT.len());
        assert_eq!(
            obsidian_args(Headings::Omitted).len(),
            options_spec::obsidian::GROUP.len()
        );
        #[cfg(feature = "ai")]
        assert_eq!(ai_args(Headings::Omitted).len(), AI_LAYOUT.len());
    }

    /// Every active spec option must assemble without panicking (runtime heading shape).
    #[test]
    fn all_groups_assemble_applied() {
        assert_eq!(
            export_args(Headings::Applied).len(),
            options_spec::export::GROUP.len()
        );
        assert_eq!(crawler_args(Headings::Applied).len(), CRAWLER_LAYOUT.len());
        assert_eq!(
            obsidian_args(Headings::Applied).len(),
            options_spec::obsidian::GROUP.len()
        );
        #[cfg(feature = "ai")]
        assert_eq!(ai_args(Headings::Applied).len(), AI_LAYOUT.len());
    }

    #[test]
    #[cfg(not(feature = "ai"))]
    fn ai_args_is_empty_without_feature() {
        assert!(
            ai_args(Headings::Omitted).is_empty(),
            "ai_args must be empty without feature (Omitted)"
        );
        assert!(
            ai_args(Headings::Applied).is_empty(),
            "ai_args must be empty without feature (Applied)"
        );
    }

    #[test]
    #[cfg(feature = "ai")]
    fn ai_args_has_four_entries_with_feature() {
        assert_eq!(
            ai_args(Headings::Omitted).len(),
            4,
            "ai_args must have 4 entries with feature (Omitted)"
        );
        assert_eq!(
            ai_args(Headings::Applied).len(),
            4,
            "ai_args must have 4 entries with feature (Applied)"
        );
        assert_eq!(AI_LAYOUT.len(), 4, "AI_LAYOUT must have 4 slots");
    }

    /// A `Uint` default outside any previously-known literal set must
    /// assemble without panicking — the clap `default_value` path derives
    /// the string from `DefaultValue::Display`, not a hard-coded arm table.
    ///
    /// The synthetic spec reuses the `delay_ms` id so the (separate,
    /// intentional) `numeric_binding` parser lookup succeeds; that isolates
    /// the default-value rendering path that used to carry the hard-coded
    /// `Uint` arm table with an `unreachable!()` wildcard. `999_999` is not
    /// in that old table, so the previous implementation panicked here.
    #[test]
    fn unknown_uint_default_does_not_panic() {
        const SYNTH: OptionSpec = OptionSpec {
            id: "delay_ms",
            long: "synth-new-uint",
            short: None,
            aliases: &[],
            visible_aliases: &[],
            value_name: "SYNTH_NEW_UINT",
            env: None,
            default: Some(DefaultValue::Uint(999_999)),
            nullable: false,
            description_override: None,
            help: "synthetic option for the unknown-Uint default test",
            heading: None,
            kind: ValueKind::uint_unbounded(),
            feature_gate: None,
            value_delimiter: None,
        };
        let arg = build_arg(&SYNTH, Headings::Omitted);
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        assert_eq!(defaults, vec!["999999"]);
    }

    /// Heading parity BY CONSTRUCTION: the same specs built through
    /// [`Headings::Applied`] expose exactly the spec heading on every arg.
    #[test]
    fn heading_parity_holds_by_construction() {
        for spec in options_spec::export::GROUP {
            let arg = export_args(Headings::Applied)
                .into_iter()
                .find(|a| a.get_id() == spec.id)
                .unwrap_or_else(|| panic!("export arg `{}` missing", spec.id));
            assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
        }
        for slot in CRAWLER_LAYOUT {
            if let CrawlerSlot::Spec(spec) = slot {
                let arg = crawler_args(Headings::Applied)
                    .into_iter()
                    .find(|a| a.get_id() == spec.id)
                    .unwrap_or_else(|| panic!("crawler arg `{}` missing", spec.id));
                assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
            }
        }
        for spec in options_spec::obsidian::GROUP {
            let arg = obsidian_args(Headings::Applied)
                .into_iter()
                .find(|a| a.get_id() == spec.id)
                .unwrap_or_else(|| panic!("obsidian arg `{}` missing", spec.id));
            assert_eq!(arg.get_help_heading(), spec.heading, "`{}`", spec.id);
        }
    }

    /// The runtime shape carries headings (#932, post-ADR-002): every active
    /// spec entry built through the runtime path exposes the same heading as
    /// the parity-suite path. This is the post-flip dual of the previous
    /// "inert heading" guard; the parity suite already pins the per-arg
    /// mapping, this test pins the runtime wiring.
    #[test]
    fn runtime_shape_applies_spec_headings() {
        let mut specs: Vec<&'static OptionSpec> = options_spec::export::GROUP.iter().collect();
        for slot in CRAWLER_LAYOUT.iter() {
            if let CrawlerSlot::Spec(s) = slot {
                specs.push(*s);
            }
        }
        specs.extend(options_spec::obsidian::GROUP.iter());
        #[cfg(feature = "ai")]
        for slot in AI_LAYOUT.iter() {
            if let AiSlot::Spec(s) = slot {
                specs.push(*s);
            }
        }

        let args: Vec<clap::Arg> = export_args(Headings::Applied)
            .into_iter()
            .chain(crawler_args(Headings::Applied))
            .chain(obsidian_args(Headings::Applied))
            .chain({
                #[cfg(feature = "ai")]
                {
                    ai_args(Headings::Applied)
                }
                #[cfg(not(feature = "ai"))]
                {
                    Vec::new()
                }
            })
            .collect();

        for spec in specs {
            let arg = args
                .iter()
                .find(|a| a.get_id() == spec.id)
                .unwrap_or_else(|| panic!("runtime arg `{}` missing", spec.id));
            assert_eq!(
                arg.get_help_heading(),
                spec.heading,
                "runtime arg `{}` heading mismatch",
                spec.id
            );
        }
    }

    /// Manual (structurally-deferred) crawler args carry their layout-adjacent
    /// headings unconditionally so no flag renders under the orphan `Options:` bucket (#976).
    #[test]
    fn manual_args_carry_headings() {
        let expectations = [
            ("concurrency", Some("Discovery")),
            ("rate_limit_burst", Some("Discovery")),
            ("include_patterns", Some("Crawler Settings")),
            ("exclude_patterns", Some("Crawler Settings")),
            ("headers", Some("HTTP Client Settings")),
            ("cookies", Some("HTTP Client Settings")),
        ];
        for (id, expected) in expectations {
            for headings in [Headings::Omitted, Headings::Applied] {
                let arg = crawler_args(headings)
                    .into_iter()
                    .find(|a| a.get_id() == id)
                    .unwrap_or_else(|| panic!("manual arg `{id}` missing for {headings:?}"));
                assert_eq!(
                    arg.get_help_heading(),
                    expected,
                    "manual `{id}` {headings:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-written `FromArgMatches` support
// ---------------------------------------------------------------------------

/// Extraction helpers shared by the hand-written [`clap::FromArgMatches`]
/// impls of [`ExportArgs`](super::args::ExportArgs) and
/// [`CrawlerArgs`](super::args::CrawlerArgs). Field ids MUST equal the spec
/// ids — the parity suite enforces lockstep.
///
/// NOTE: both `update_from_arg_matches` impls replace the struct wholesale
/// (`*self = from_arg_matches(..)`), unlike clap derive's field-by-field
/// updates. Equivalent for this CLI's single-parse flow; a future partial-
/// update consumer must switch to per-field extraction first.
pub(crate) mod extract {
    use clap::{error::ErrorKind, ArgMatches, Error};

    fn missing(id: &str) -> Error {
        Error::raw(
            ErrorKind::InvalidValue,
            format!("missing parsed value for `{id}`"),
        )
    }

    /// Required (or defaulted) single value.
    pub(crate) fn value<T: Clone + Send + Sync + 'static>(
        matches: &ArgMatches,
        id: &str,
    ) -> Result<T, Error> {
        matches.get_one::<T>(id).cloned().ok_or_else(|| missing(id))
    }

    /// Optional single value (`Option<T>` field).
    pub(crate) fn opt<T: Clone + Send + Sync + 'static>(
        matches: &ArgMatches,
        id: &str,
    ) -> Option<T> {
        matches.get_one::<T>(id).cloned()
    }

    /// Repeated values (`Vec<T>` field with `Append` action).
    pub(crate) fn many<T: Clone + Send + Sync + 'static>(matches: &ArgMatches, id: &str) -> Vec<T> {
        matches
            .get_many::<T>(id)
            .map(|values| values.cloned().collect())
            .unwrap_or_default()
    }

    /// Optional repeated values (`Option<Vec<T>>` field with `value_delimiter`).
    /// `None` when the flag was never provided; `Some(vec)` (possibly empty
    /// after clap splits) when the user typed at least one value. This
    /// matches the legacy clap-derive semantics for `Option<Vec<String>>`:
    /// missing flag = `None`, present flag = `Some(Vec)`.
    pub(crate) fn opt_many<T: Clone + Send + Sync + 'static>(
        matches: &ArgMatches,
        id: &str,
    ) -> Option<Vec<T>> {
        if matches.value_source(id).is_some() {
            Some(many::<T>(matches, id))
        } else {
            None
        }
    }
}
