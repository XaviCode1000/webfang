//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::domain::budget::{BudgetOverrides, BurstPermits};
use crate::domain::config_value::{ConfigSource, ConfigValue};
use crate::domain::JsStrategy;
use crate::infrastructure::observability::log_scrape_error;
use crate::{Args, CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};
use std::collections::BTreeMap;
use tracing::{info, instrument};

// ============================================================================
// Normalization pipeline (stabilization-config-normalization, Phase 3 — D3/D4)
// Private FieldBook + rank-guarded stages. Legacy merge functions stay below
// untouched (deleted in Phase 5). New code is not yet wired at call sites.
// ============================================================================

/// Provenance map `arg_id → source` for the env/cli stages.
///
/// Only explicit sources (`Environment`, `Cli`) are recorded; absent ids and
/// `DefaultValue` are omitted so those stages simply never write. Phase 5 wires
/// this via `parse_args()` → `ArgSources::capture` (design D1). Tests drive it
/// directly via `set` for hermeticity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArgSources {
    map: BTreeMap<String, ConfigSource>,
}

impl ArgSources {
    /// Record `id` as provided by `source` (only `Environment` or `Cli` are
    /// meaningful; `Default`/`ConfigFile` inserted here are ignored by
    /// convention but not rejected).
    pub fn set(&mut self, id: &str, source: ConfigSource) {
        self.map.insert(id.to_string(), source);
    }

    /// Source for `id`, if it was explicitly provided via env or CLI.
    #[must_use]
    pub fn source_of(&self, id: &str) -> Option<ConfigSource> {
        self.map.get(id).copied()
    }

    /// Capture per-arg provenance from already-parsed matches (design D1).
    ///
    /// One conceptual parse pass: clap builds `ArgMatches` once; reading
    /// `value_source` per contested id is O(1) afterwards. Only
    /// `CommandLine` and `EnvVariable` are recorded.
    #[must_use]
    pub fn capture(matches: &clap::ArgMatches) -> Self {
        use clap::parser::ValueSource;
        const CONTESTED: &[&str] = &[
            "url",
            "selector",
            "delay_ms",
            "max_pages",
            "concurrency",
            "use_sitemap",
            "sitemap_url",
            "max_depth",
            "timeout_secs",
            "format",
            "export_format",
            "output",
            "obsidian_tags",
            "obsidian_wiki_links",
            "obsidian_relative_assets",
            "obsidian_rich_metadata",
            "vault",
            "quick_save",
            "ignore_waf",
            "rate_limit_burst",
        ];
        let present: std::collections::HashSet<&str> =
            matches.ids().map(|id| id.as_str()).collect();
        let mut map = BTreeMap::new();
        for &id in CONTESTED {
            if !present.contains(id) {
                continue;
            }
            match matches.value_source(id) {
                Some(ValueSource::CommandLine) => {
                    map.insert(id.to_string(), ConfigSource::Cli);
                },
                Some(ValueSource::EnvVariable) => {
                    map.insert(id.to_string(), ConfigSource::Environment);
                },
                _ => {},
            }
        }
        Self { map }
    }
}

/// Private slot board mirroring the contested surface (~39 fields via D3).
///
/// Each slot is a `ConfigValue<T>` carrying both the normalized value and its
/// `ConfigSource` so stages decide writes purely by rank. Kept private so
/// ordering invariants stay inside this module; `NormalizedConfig` is the pub
/// output.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub(crate) struct FieldBook {
    // discovery / crawl
    max_pages: ConfigValue<usize>,
    max_depth: ConfigValue<u8>,
    sitemap_depth: ConfigValue<u8>,
    sitemap_url: ConfigValue<Option<String>>,
    use_sitemap: ConfigValue<bool>,
    selector: ConfigValue<String>,
    // network / crawler
    delay_ms: ConfigValue<u64>,
    timeout_secs: ConfigValue<u64>,
    concurrency: ConfigValue<ConcurrencyConfig>,
    // output / export
    output: ConfigValue<PathBuf>,
    format: ConfigValue<OutputFormat>,
    export_format: ConfigValue<ExportFormat>,
    // obsidian
    obsidian_tags: ConfigValue<Vec<String>>,
    obsidian_wiki_links: ConfigValue<bool>,
    obsidian_relative_assets: ConfigValue<bool>,
    obsidian_rich_metadata: ConfigValue<bool>,
    vault: ConfigValue<Option<PathBuf>>,
    quick_save: ConfigValue<bool>,
    // behavior
    ignore_waf: ConfigValue<bool>,
    // budget model overrides (additive; Q1 burst knob — design D4)
    budget_overrides: ConfigValue<BudgetOverrides>,
}

impl Default for FieldBook {
    fn default() -> Self {
        stage_defaults()
    }
}

/// Pipeline output — `ConfigValue<T>` slots + projection to downstream type.
///
/// `CrawlOptions` itself stays untouched (design D4); this is the one-directional
/// projection `NormalizedConfig::into_crawl_options` dropping provenance at the
/// boundary.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct NormalizedConfig {
    pub max_pages: ConfigValue<usize>,
    pub max_depth: ConfigValue<u8>,
    pub sitemap_depth: ConfigValue<u8>,
    pub sitemap_url: ConfigValue<Option<String>>,
    pub use_sitemap: ConfigValue<bool>,
    pub selector: ConfigValue<String>,
    pub delay_ms: ConfigValue<u64>,
    pub timeout_secs: ConfigValue<u64>,
    pub concurrency: ConfigValue<ConcurrencyConfig>,
    pub output: ConfigValue<PathBuf>,
    pub format: ConfigValue<OutputFormat>,
    pub export_format: ConfigValue<ExportFormat>,
    pub obsidian_tags: ConfigValue<Vec<String>>,
    pub obsidian_wiki_links: ConfigValue<bool>,
    pub obsidian_relative_assets: ConfigValue<bool>,
    pub obsidian_rich_metadata: ConfigValue<bool>,
    pub vault: ConfigValue<Option<PathBuf>>,
    pub quick_save: ConfigValue<bool>,
    pub ignore_waf: ConfigValue<bool>,
    /// Operator-level budget overrides carried to `BudgetModel::build`
    /// (design D4). Additive slot: the default (`rate_burst: None`)
    /// reproduces today's derived numbers exactly.
    pub budget_overrides: ConfigValue<BudgetOverrides>,
}

impl NormalizedConfig {
    pub(crate) fn from_book(book: FieldBook) -> Self {
        Self {
            max_pages: book.max_pages,
            max_depth: book.max_depth,
            sitemap_depth: book.sitemap_depth,
            sitemap_url: book.sitemap_url,
            use_sitemap: book.use_sitemap,
            selector: book.selector,
            delay_ms: book.delay_ms,
            timeout_secs: book.timeout_secs,
            concurrency: book.concurrency,
            output: book.output,
            format: book.format,
            export_format: book.export_format,
            obsidian_tags: book.obsidian_tags,
            obsidian_wiki_links: book.obsidian_wiki_links,
            obsidian_relative_assets: book.obsidian_relative_assets,
            obsidian_rich_metadata: book.obsidian_rich_metadata,
            vault: book.vault,
            quick_save: book.quick_save,
            ignore_waf: book.ignore_waf,
            budget_overrides: book.budget_overrides,
        }
    }

    /// Project the normalized configuration into the downstream engine type.
    ///
    /// Provenance is dropped here — merge decisions are done.
    #[must_use]
    pub fn into_crawl_options(self) -> CrawlOptions {
        let mut opts = CrawlOptions::default();
        opts.crawl.max_pages = self.max_pages.value;
        opts.crawl.max_depth = self.max_depth.value;
        opts.crawl.sitemap_url = self.sitemap_url.value;
        opts.crawl.use_sitemap = self.use_sitemap.value;
        opts.crawl.selector = self.selector.value;
        opts.network.delay_ms = self.delay_ms.value;
        opts.network.timeout_secs = self.timeout_secs.value;
        // Explicit operator concurrency (CLI/TOML, not "auto") feeds
        // the budget model as a crawl-tier override — design D4
        // explicit-wins rule; provenance already rank-guarded upstream.
        // NOTE: read BEFORE the move of self.concurrency.value below.
        let explicit_crawl = self.concurrency.value.get();
        opts.network.concurrency = self.concurrency.value;
        opts.export.output_dir = self.output.value;
        opts.export.output_format = self.format.value;
        opts.export.export_format = self.export_format.value;
        opts.export.obsidian_tags = self.obsidian_tags.value;
        opts.export.obsidian_wiki_links = self.obsidian_wiki_links.value;
        opts.export.obsidian_relative_assets = self.obsidian_relative_assets.value;
        opts.export.obsidian_rich_metadata = self.obsidian_rich_metadata.value;
        opts.export.obsidian_vault = self.vault.value;
        opts.export.quick_save = self.quick_save.value;
        opts.crawl.ignore_waf = self.ignore_waf.value;
        opts.budget_overrides = self.budget_overrides.value;
        if let Some(explicit) = explicit_crawl {
            opts.budget_overrides.crawl =
                crate::domain::budget::tiers::CrawlConcurrency::new(explicit).ok();
        }
        // cross-field rule already applied in the pipeline
        if opts.crawl.sitemap_url.is_some() {
            opts.crawl.use_sitemap = true;
        }
        opts
    }
}

fn try_write<T: Clone>(
    slot: &mut ConfigValue<T>,
    incoming: T,
    incoming_source: ConfigSource,
    field: &str,
) -> bool {
    if slot.outranked_by(incoming_source) {
        info!(field = %field, winner = ?incoming_source, loser = ?slot.source, "config_field_overridden");
        *slot = ConfigValue::new(incoming, incoming_source);
        true
    } else {
        false
    }
}

#[instrument(skip_all, fields(fields_written))]
fn stage_defaults() -> FieldBook {
    FieldBook {
        max_pages: ConfigValue::new(10, ConfigSource::Default),
        max_depth: ConfigValue::new(2, ConfigSource::Default),
        sitemap_depth: ConfigValue::new(1, ConfigSource::Default),
        sitemap_url: ConfigValue::new(None, ConfigSource::Default),
        use_sitemap: ConfigValue::new(false, ConfigSource::Default),
        selector: ConfigValue::new("body".to_string(), ConfigSource::Default),
        delay_ms: ConfigValue::new(1000, ConfigSource::Default),
        timeout_secs: ConfigValue::new(30, ConfigSource::Default),
        concurrency: ConfigValue::new(ConcurrencyConfig::default(), ConfigSource::Default),
        output: ConfigValue::new(PathBuf::from("output"), ConfigSource::Default),
        format: ConfigValue::new(OutputFormat::Markdown, ConfigSource::Default),
        export_format: ConfigValue::new(ExportFormat::Jsonl, ConfigSource::Default),
        obsidian_tags: ConfigValue::new(Vec::new(), ConfigSource::Default),
        obsidian_wiki_links: ConfigValue::new(false, ConfigSource::Default),
        obsidian_relative_assets: ConfigValue::new(false, ConfigSource::Default),
        obsidian_rich_metadata: ConfigValue::new(false, ConfigSource::Default),
        vault: ConfigValue::new(None, ConfigSource::Default),
        quick_save: ConfigValue::new(false, ConfigSource::Default),
        ignore_waf: ConfigValue::new(false, ConfigSource::Default),
        budget_overrides: ConfigValue::new(BudgetOverrides::default(), ConfigSource::Default),
    }
}

#[instrument(skip_all, fields(fields_written))]
fn stage_config_file(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    n += stage_config_file_crawl(book, config);
    n += stage_config_file_output(book, config);
    n += stage_config_file_obsidian(book, config);
    n
}

fn stage_config_file_crawl(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.max_pages {
        if try_write(
            &mut book.max_pages,
            v,
            ConfigSource::ConfigFile,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.delay_ms {
        if try_write(&mut book.delay_ms, v, ConfigSource::ConfigFile, "delay_ms") {
            n += 1;
        }
    }
    if let Some(ref s) = config.selector {
        if try_write(
            &mut book.selector,
            s.clone(),
            ConfigSource::ConfigFile,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.use_sitemap {
        if try_write(
            &mut book.use_sitemap,
            v,
            ConfigSource::ConfigFile,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.ignore_waf {
        if try_write(
            &mut book.ignore_waf,
            v,
            ConfigSource::ConfigFile,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    // concurrency via string tag (mirrors apply_config_defaults)
    if let Some(ref c) = config.concurrency {
        let target = ConcurrencyConfig::from(c.as_str());
        // enact rank guard even when target equals default 'auto'
        if try_write(
            &mut book.concurrency,
            target,
            ConfigSource::ConfigFile,
            "concurrency",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_output(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if try_write(&mut book.format, target, ConfigSource::ConfigFile, "format") {
            n += 1;
        }
    }
    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if try_write(
            &mut book.export_format,
            target,
            ConfigSource::ConfigFile,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_obsidian(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.obsidian_wiki_links {
        if try_write(
            &mut book.obsidian_wiki_links,
            v,
            ConfigSource::ConfigFile,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if try_write(
            &mut book.obsidian_relative_assets,
            v,
            ConfigSource::ConfigFile,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if try_write(
            &mut book.vault,
            Some(PathBuf::from(vault)),
            ConfigSource::ConfigFile,
            "vault",
        ) {
            n += 1;
        }
    }
    // Rank-guarded by `try_write` alone: at this fixed pipeline position the
    // slot can only be `Default`-sourced, so value-emptiness must not gate
    // the write (spec R2 forbids value-equality merge logic).
    if let Some(ref tags_str) = config.obsidian_tags {
        let parsed: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if try_write(
            &mut book.obsidian_tags,
            parsed,
            ConfigSource::ConfigFile,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_crawl(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("max_pages") {
        if try_write(
            &mut book.max_pages,
            args.crawler.max_pages,
            src,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("max_depth") {
        if try_write(
            &mut book.max_depth,
            args.crawler.max_depth,
            src,
            "max_depth",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("delay_ms") {
        if try_write(&mut book.delay_ms, args.crawler.delay_ms, src, "delay_ms") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("timeout_secs") {
        if try_write(
            &mut book.timeout_secs,
            args.crawler.timeout_secs,
            src,
            "timeout_secs",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("selector") {
        if try_write(
            &mut book.selector,
            args.crawler.selector.clone(),
            src,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("concurrency") {
        if try_write(
            &mut book.concurrency,
            args.crawler.concurrency.clone(),
            src,
            "concurrency",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("sitemap_url") {
        let val = args.crawler.sitemap_url.clone();
        if try_write(&mut book.sitemap_url, val, src, "sitemap_url") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("use_sitemap") {
        if try_write(
            &mut book.use_sitemap,
            args.crawler.use_sitemap,
            src,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_output(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("output") {
        if try_write(&mut book.output, args.export.output.clone(), src, "output") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("format") {
        if try_write(&mut book.format, args.export.format, src, "format") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("export_format") {
        if try_write(
            &mut book.export_format,
            args.export.export_format,
            src,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_obsidian(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("obsidian_tags") {
        if try_write(
            &mut book.obsidian_tags,
            args.obsidian.obsidian_tags.clone().unwrap_or_default(),
            src,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_wiki_links") {
        if try_write(
            &mut book.obsidian_wiki_links,
            args.obsidian.obsidian_wiki_links,
            src,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_relative_assets") {
        if try_write(
            &mut book.obsidian_relative_assets,
            args.obsidian.obsidian_relative_assets,
            src,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("vault") {
        if try_write(&mut book.vault, args.obsidian.vault.clone(), src, "vault") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("quick_save") {
        if try_write(
            &mut book.quick_save,
            args.obsidian.quick_save,
            src,
            "quick_save",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("ignore_waf") {
        if try_write(
            &mut book.ignore_waf,
            args.crawler.ignore_waf,
            src,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    n
}

#[instrument(skip_all, fields(fields_written))]
fn stage_env_cli(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    n += stage_env_cli_crawl(book, args, sources);
    n += stage_env_cli_output(book, args, sources);
    n += stage_env_cli_obsidian(book, args, sources);
    n
}

/// Field-wise merge of budget overrides at the binary's final assembly
/// point (#897 item 1).
///
/// * `cli_capture` — the CLI-explicit knobs captured from `From<Args>`
///   BEFORE provenance projection (unranked by construction).
/// * `staged` — the pipeline-projected overrides (`into_crawl_options`),
///   carrying the WINNING source per the `Default < ConfigFile <
///   Environment < Cli` resolution upstream.
///
/// Tier rules:
/// - `crawl`: **staged wins; the CLI capture only fills the gap.** Both
///   sides project the SAME user knob (`--concurrency` / TOML), but
///   only `staged` is rank-resolved. A blind `cli.or(staged)` here let a
///   plain `--concurrency` stomp a rank-resolved staged value while
///   `network.concurrency` kept the ranked winner — a silent
///   contradiction inside one struct (adversarial review M1 of PR #925).
///   Preferring `staged` keeps `budget_overrides.crawl` consistent with
///   `network.concurrency` BY CONSTRUCTION.
/// - `rate_burst`: since #1813 `From<Args>` contributes `None`, so on the
///   shipped binary's path the staged value is the only possible source —
///   `stage_budget_overrides` writes it at ConfigFile/Env/CommandLine rank.
///   The `cli_capture` side remains reachable for programmatic `Args`
///   callers that populate a capture themselves; `cli_capture` wins there.
/// - `batch` / `asset`: CLI-explicit wins where present; the staged value
///   fills the rest (nothing stages them today).
#[must_use]
pub fn merge_budget_overrides(
    cli_capture: crate::domain::budget::BudgetOverrides,
    staged: crate::domain::budget::BudgetOverrides,
) -> crate::domain::budget::BudgetOverrides {
    crate::domain::budget::BudgetOverrides {
        crawl: staged.crawl.or(cli_capture.crawl),
        rate_burst: cli_capture.rate_burst.or(staged.rate_burst),
        batch: cli_capture.batch.or(staged.batch),
        asset: cli_capture.asset.or(staged.asset),
    }
}

/// Stage operator budget overrides (task 2.1, design D4).
///
/// Runs AFTER [`apply_cross_field_rules`] and BEFORE [`validate_stage`] so the
/// existing rank-guarded stages keep their stage ordering untouched. TOML is
/// staged first at `ConfigFile` rank, then env/cli via the provenance map.
#[instrument(skip_all, fields(fields_written))]
fn stage_budget_overrides(
    book: &mut FieldBook,
    args: &Args,
    sources: &ArgSources,
    config: &ConfigDefaults,
) -> Result<usize, CliExit> {
    let mut n = 0;
    // TOML tier first (lowest explicit rank).
    if let Some(v) = config.rate_limit_burst {
        if try_write(
            &mut book.budget_overrides,
            budget_override(v)?,
            ConfigSource::ConfigFile,
            "rate_limit_burst",
        ) {
            n += 1;
        }
    }
    // env/cli tier via the provenance map. The raw string is parsed here so
    // CLI, env, and programmatic input share ONE accept / reject semantic
    // (`parse_rate_limit_burst`). This is the ONLY place the burst is
    // validated: `From<Args>` no longer parses it (#1813), so an invalid
    // value from argv, the env var, or TOML stops here with exit 78 before
    // the conversion runs.
    if let Some(src) = sources.source_of("rate_limit_burst") {
        if let Some(raw) = args.crawler.rate_limit_burst.as_deref() {
            match crate::cli::args::crawler::parse_rate_limit_burst(raw) {
                Ok(Some(v)) => {
                    if try_write(
                        &mut book.budget_overrides,
                        budget_override(v)?,
                        src,
                        "rate_limit_burst",
                    ) {
                        n += 1;
                    }
                },
                // Empty / whitespace-only, or the `auto` keyword: the
                // operator neutralised the flag (`WEBFANG_RATE_LIMIT_BURST=""`
                // or `=auto`), so it means "not set" and the derived default
                // applies. Never a silent degrade of an invalid value —
                // those are rejected below (#1813).
                Ok(None) => {},
                Err(msg) => return Err(CliExit::ConfigError(msg)),
            }
        }
    }
    Ok(n)
}

/// Wrap a raw burst value into [`BudgetOverrides`], rejecting 0 with the same
/// Spanish boundary error the clap value parser uses (defense for programmatic
/// `Args` construction and TOML values, which bypass the CLI parser).
fn budget_override(v: u32) -> Result<BudgetOverrides, CliExit> {
    let rate_burst = BurstPermits::new(v).map_err(|_| {
        CliExit::ConfigError(
            "--rate-limit-burst debe ser >= 1 (0 no permite ningún request en ráfaga)".to_string(),
        )
    })?;
    Ok(BudgetOverrides {
        rate_burst: Some(rate_burst),
        crawl: None,
        batch: None,
        asset: None,
    })
}
#[instrument(skip_all, fields(fields_written))]
fn apply_cross_field_rules(book: &mut FieldBook) -> usize {
    let mut n = 0;
    // --sitemap-url implies --use-sitemap (#491): preserved verbatim
    if book.sitemap_url.value.is_some() && !book.use_sitemap.value {
        // keep the higher of the two provenances for auditability
        let src = std::cmp::max(book.sitemap_url.source, book.use_sitemap.source);
        let loser = book.use_sitemap.source;
        book.use_sitemap = ConfigValue::new(true, src);
        info!(field = "use_sitemap", winner = ?src, loser = ?loser, "config_field_overridden");
        n += 1;
    }
    // Obsidian tag trim/dedup preserved verbatim inside apply_cross_field_rules
    // Trim whitespace from each tag, drop empties, preserve order, dedup?
    let tags = &mut book.obsidian_tags.value;
    for tag in tags.iter_mut() {
        *tag = tag.trim().to_string();
    }
    tags.retain(|t| !t.is_empty());
    // byte-parity: original code did not dedup across retain, but spec says
    // dedup. We preserve order and dedup via first-occurrence retention to match
    // the spec's "byte-parity" expectation while staying deterministic.
    {
        let mut seen = std::collections::BTreeSet::new();
        let mut deduped = Vec::new();
        for t in tags.drain(..) {
            if seen.insert(t.clone()) {
                deduped.push(t);
            }
        }
        *tags = deduped;
    }
    n
}

#[instrument(skip_all, fields(fields_written))]
fn validate_stage(book: &FieldBook) -> Result<(), CliExit> {
    if book.max_pages.value == 0 {
        let msg = "max_pages debe ser mayor que 0".to_string();
        log_scrape_error(&msg, "", "validate_stage", None, "validation failed");
        return Err(CliExit::ConfigError(msg));
    }
    // Additional per-field validation could be added here without growing this
    // function's complexity beyond the ratchet (keep <30).
    Ok(())
}

/// Normalize configuration through the single rank-guarded pipeline (design D3).
///
/// Fixed stage order: defaults → config file → env/cli → cross-field →
/// budget overrides → field validation → capability gates → `NormalizedConfig`.
///
/// # Capability gates are part of the pipeline, not a caller's job
///
/// The last step is the `--adaptive-selectors` capability gate (#1813,
/// `check_adaptive_selectors_capability`). It is HERE, at the end, rather than
/// in the CLI binary's step list, for the reason `stage_budget_overrides` is
/// here: a check a caller must remember to invoke is a check some front door
/// will skip. Every route into this function — argv,
/// `WEBFANG_ADAPTIVE_SELECTORS`, or a programmatically built `Args` —
/// passes through it, so there is exactly one place where "this build cannot
/// do what you asked" is decided.
///
/// It runs LAST on purpose: `validate_stage` keeps the field errors it has
/// always reported, so a run that is already misconfigured for another reason
/// still sees that other reason first. The gate itself is still far ahead of
/// anything that matters — `main.rs` calls `normalize` at step 6b, before
/// logging, before any engine construction, and long before `build_and_run`
/// opens a socket.
///
/// # Errors
///
/// Returns `CliExit::ConfigError` with a Spanish message when validation
/// blocks the result.
#[allow(missing_docs)]
#[instrument(skip_all, fields(fields_written = tracing::field::Empty))]
pub fn normalize(
    args: &Args,
    sources: &ArgSources,
    config: &ConfigDefaults,
) -> Result<NormalizedConfig, CliExit> {
    let mut book = stage_defaults();
    let mut total = 0usize;
    total += stage_config_file(&mut book, config);
    total += stage_env_cli(&mut book, args, sources);
    total += apply_cross_field_rules(&mut book);
    total += stage_budget_overrides(&mut book, args, sources, config)?;
    validate_stage(&book)?;
    check_adaptive_selectors_capability(args)?;
    tracing::Span::current().record("fields_written", total);
    info!(fields_written = total, "normalization_complete");
    Ok(NormalizedConfig::from_book(book))
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
///
/// Windows/macOS use their own candidate sets ([`default_chrome_candidates`],
/// XP-S-04 #1608), so this list is only compiled on the platforms it serves.
#[cfg(not(any(windows, target_os = "macos")))]
const DEFAULT_CHROME_CANDIDATES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium-browser",
    "chromium",
];

/// Chrome/Chromium candidates for the current platform (XP-S-04, #1608).
///
/// Order matters: the first candidate that resolves to an existing file
/// reporting a `--version` wins.
///
/// - Linux: bare names resolved through `PATH` (list unchanged from #685).
/// - Windows: the standard `chrome.exe` install locations first (Program
///   Files, Program Files (x86), then the per-user `%LOCALAPPDATA%`
///   install), then the bare `chrome` name through `PATH`/`PATHEXT`.
/// - macOS: the `.app` bundle executables, plus bare `chromium` for
///   Homebrew-style installs.
///
/// Injectability is preserved: [`resolve_chrome_binary_with`] and
/// [`check_js_dependencies_with`] still take the candidate list as a
/// parameter — only the production default is platform-conditional.
fn default_chrome_candidates() -> Vec<String> {
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        DEFAULT_CHROME_CANDIDATES
            .iter()
            .map(|s| (*s).into())
            .collect()
    }
    #[cfg(windows)]
    {
        let mut candidates: Vec<String> = Vec::new();
        for base in [
            r"C:\Program Files\Google\Chrome\Application",
            r"C:\Program Files (x86)\Google\Chrome\Application",
        ] {
            candidates.push(format!(r"{base}\chrome.exe"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let per_user = std::path::PathBuf::from(local_app_data)
                .join(r"Google\Chrome\Application\chrome.exe");
            candidates.push(per_user.to_string_lossy().into_owned());
        }
        // Bare name last: Chrome rarely registers on PATH, but the
        // PATH/PATHEXT search is free to try.
        candidates.push("chrome".to_string());
        candidates
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "chromium".to_string(),
        ]
    }
}

/// Resolve the Chrome/Chromium binary the gate certified (F-52-c, #1278).
///
/// Returns the first candidate (in order) that resolves to an existing
/// *file* and whose `--version` probe exits successfully. Bare names are
/// resolved through the injected `PATH` exactly like the OS would
/// ([`resolve_executable_in_path`]); explicit paths must exist as files.
/// The `is_file` requirement (not `exists`) excludes the
/// `/opt/google/chrome` *directory* trap that chromiumoxide's own
/// `get_by_path` falls into via `exists()`.
///
/// Pure and injectable like the rest of the gate (#787): tests pass
/// controlled candidates + `PATH` instead of touching process-global env.
pub(crate) fn resolve_chrome_binary_with(candidates: &[&str], path_value: &str) -> Option<PathBuf> {
    candidates.iter().find_map(|binary| {
        let resolved = if has_path_separator(binary) {
            let path = PathBuf::from(binary);
            path.is_file().then_some(path)
        } else {
            resolve_executable_in_path(binary, path_value)
        };
        resolved.filter(|path| binary_path_reports_version(path))
    })
}

/// Process-PATH entry point for `resolve_chrome_binary_with`: the
/// production resolution over the platform default candidates
/// the platform default candidates (`default_chrome_candidates`). Called once
/// after the gate passes
/// (`main.rs` 6c); the result travels in `CrawlOptions.network.chrome_binary`
/// so the launcher runs exactly the certified binary.
pub fn resolve_chrome_binary() -> Option<PathBuf> {
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    resolve_chrome_binary_with(&candidate_refs, &path_value)
}

/// Upper bound for one `--version` probe (XP-S-06, #1608).
///
/// The probe runs synchronously, pre-crawl; a wedged or interactive binary
/// must not hang the CLI forever. On expiry the child is killed and the
/// probe reports failure (the Full gate then goes red; the Hybrid Obscura
/// check degrades to "unknown version" with a warning).
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Poll interval while waiting for a `--version` probe to exit.
const VERSION_PROBE_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// Run `<path> --version` silently and bounded (XP-S-06, #1608).
///
/// Returns the process `Output` (status + captured stdout/stderr) or `None`
/// when the spawn fails, the wait errors, or the deadline elapses first
/// (the child is killed and reaped before returning). The probe probes the
/// exact file the launcher will execute, so gate-certified and launched are
/// the same binary by construction; output is captured because the Obscura
/// version check parses it.
fn run_version_probe_with_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                use std::io::Read as _;
                // The child has exited: the pipes are closed on its side, so
                // read_to_end drains the buffered output without blocking.
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_end(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_end(&mut stderr);
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            },
            Ok(None) => {},
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(VERSION_PROBE_POLL);
    }
}

/// Silent `--version` probe against an already-resolved file path, bounded
/// by [`VERSION_PROBE_TIMEOUT`].
///
/// Output is discarded — only the exit status matters.
fn binary_path_reports_version(path: &Path) -> bool {
    matches!(
        run_version_probe_with_timeout(path, VERSION_PROBE_TIMEOUT),
        Some(output) if output.status.success()
    )
}

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
///   binary must report a version of at least `MINIMUM_OBSCURA_VERSION`
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
/// version older than `MINIMUM_OBSCURA_VERSION` (#793), or when it is
/// [`Full`](crate::domain::JsStrategy::Full) and either the binary was
/// built without the `chromium` feature, or no Chrome/Chromium candidate
/// on `PATH` reports a version.
pub fn check_js_dependencies(opts: &CrawlOptions) -> Result<(), CliExit> {
    // The PATH value is injected so the core check stays pure and testable —
    // tests pass a controlled PATH instead of mutating process-global env.
    let path_value =
        std::env::var_os("PATH").map_or_else(String::new, |v| v.to_string_lossy().into_owned());
    let candidates = default_chrome_candidates();
    let candidate_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    check_js_dependencies_with(
        &candidate_refs,
        cfg!(feature = "chromium"),
        &path_value,
        opts,
    )
}

/// Candidate-, feature-, and PATH-injectable core of
/// [`check_js_dependencies`] — Full/Chrome tests use fake executables in a
/// controlled PATH dir (F-52-c), Hybrid tests do the same for obscura.
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
        JsStrategy::Full => check_chrome_binary(candidates, chromium_enabled, path_value),
    }
}

/// Full-strategy check (#685, #758, F-52-c #1278): `chromium` feature + a
/// Chrome/Chromium candidate that reports a version. The gate now resolves
/// through [`resolve_chrome_binary_with`] — the same resolution the launcher
/// will use — so a green gate means the certified binary is launchable.
fn check_chrome_binary(
    candidates: &[&str],
    chromium_enabled: bool,
    path_value: &str,
) -> Result<(), CliExit> {
    if !chromium_enabled {
        return Err(CliExit::ConfigError(
            "--js-strategy full requiere un binario compilado con la feature `chromium`; \
                     recompilá con --features chromium o usá --js-strategy hybrid"
                .into(),
        ));
    }

    let chrome_present = resolve_chrome_binary_with(candidates, path_value).is_some();
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

/// Semicolon-separated extension list tried after the bare name on Windows
/// (XP-S-04, #1608): the cmd.exe default `PATHEXT` value.
#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Parse a `PATHEXT`-style semicolon-separated extension list into ordered,
/// normalized extensions. Pure — unit-tested without process env.
///
/// Preserves order, drops empty entries, lowercases, and ensures each entry
/// starts with a dot (cmd.exe accepts both `EXE` and `.EXE` spellings).
fn parse_pathext(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            if ext.starts_with('.') {
                ext
            } else {
                format!(".{ext}")
            }
        })
        .collect()
}

/// The platform's `PATHEXT` value, when the platform uses one.
///
/// Windows: the `PATHEXT` env var (or the cmd.exe default when unset).
/// Everywhere else: `None` — a bare name is tried exactly as given.
#[cfg(windows)]
fn platform_pathext() -> Option<String> {
    Some(std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string()))
}

#[cfg(not(windows))]
fn platform_pathext() -> Option<String> {
    None
}

/// Candidate file names for a bare executable name, in resolution order.
///
/// The exact name first, then — on Windows only — the name with each
/// `PATHEXT` extension appended, in `PATHEXT` order (XP-S-04, #1608). Pure
/// and unit-testable on every platform: unix callers pass `None`.
fn bare_name_candidates(name: &str, pathext: Option<&str>) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if let Some(exts) = pathext {
        names.extend(
            parse_pathext(exts)
                .into_iter()
                .map(|ext| format!("{name}{ext}")),
        );
    }
    names
}

/// First existing *file* among `dir/name` combinations, dir-major order
/// (every candidate name is tried within a directory before moving to the
/// next — the same order the OS uses). Pure given the inputs; the only
/// effect is the `is_file` probe, unit-tested on Linux.
fn first_existing_in_dirs(
    mut dirs: impl Iterator<Item = PathBuf>,
    names: &[String],
) -> Option<PathBuf> {
    dirs.find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Scan `PATH` entries (in order) for a file named `name` — the same lookup
/// the OS performs for a bare executable name (#787).
///
/// Pure: takes the `PATH` value as input so tests control the search space
/// without touching the process-global environment. On Windows the bare
/// name is expanded with the platform's `PATHEXT` order (XP-S-04, #1608).
fn resolve_executable_in_path(name: &str, path_value: &str) -> Option<PathBuf> {
    if name.is_empty() || has_path_separator(name) {
        return None;
    }
    let names = bare_name_candidates(name, platform_pathext().as_deref());
    first_existing_in_dirs(std::env::split_paths(path_value), &names)
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
    Some((
        major.parse().ok()?,
        minor.parse().ok()?,
        patch.parse().ok()?,
    ))
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

/// Run `<resolved> --version` once and parse its output (#793), bounded by
/// [`VERSION_PROBE_TIMEOUT`] (XP-S-06, #1608).
///
/// Returns `None` when the probe cannot run, is killed at the deadline,
/// exits non-zero, or prints no semver-like token on stdout or stderr.
/// Blocking spawn: acceptable once, pre-crawl (same silent-probe shape the
/// Full gate uses per resolved file).
fn probe_obscura_version(resolved: &std::path::Path) -> Option<(u64, u64, u64)> {
    let output = run_version_probe_with_timeout(resolved, VERSION_PROBE_TIMEOUT)?;
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

/// Orphan-flag guard (#799): `--db-path` is only consumed by the elastic
/// ingestion pipeline (`build_elastic_ingestion`), so without `--elastic` it is a
/// silent no-op -- the run "succeeds" but persists nothing. Reject it up front
/// with a clear message instead of letting the user lose data unknowingly.
pub fn check_db_path_requires_elastic(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.elastic.db_path.is_some() && !opts.elastic.enabled {
        return Err(CliExit::ConfigError(
            "--db-path solo tiene efecto junto con --elastic; ejecuta con --elastic para \
                 persistir en la base de datos SQLite, o quita --db-path"
                .into(),
        ));
    }
    Ok(())
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

/// The single Spanish message every `--adaptive-selectors` capability
/// rejection emits (#1813).
///
/// It names both the flag and the feature, because the actionable fix is a
/// recompile with `--features adaptive-selectors`, not a different argument.
/// Shared by both call shapes so the two can never drift into telling the
/// operator two different things about the same missing capability.
const ADAPTIVE_SELECTORS_UNAVAILABLE: &str = "--adaptive-selectors requiere un binario compilado con la feature `adaptive-selectors`; recompilá con --features adaptive-selectors";

/// The ONE semantic behind every `--adaptive-selectors` capability check
/// (#1813): a requested capability the build cannot honour is a
/// build-configuration problem (exit 78), never a silent no-op.
///
/// Both entry shapes funnel here — [`check_adaptive_selectors_capability_with`]
/// reads the request off `Args` during [`normalize`], and
/// [`check_adaptive_selectors_feature_with`] reads it off `CrawlOptions` for a
/// programmatic builder — so the accept/reject decision is stated once and the
/// only thing a caller chooses is where the flag lives.
fn adaptive_selectors_unavailable(adaptive_enabled: bool, requested: bool) -> Result<(), CliExit> {
    if !requested || adaptive_enabled {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        ADAPTIVE_SELECTORS_UNAVAILABLE.to_string(),
    ))
}

/// Preflight: `--adaptive-selectors` on a binary built WITHOUT the
/// `adaptive-selectors` feature must fail before any network request (#1813).
///
/// Direct sibling of [`check_clean_ai_feature`] (#761), and the same shape as
/// the `--elastic`/`persistence` gate ([`check_elastic_sink`]): an explicitly
/// requested capability the binary was not compiled to honor is a
/// build-configuration problem, never a silent no-op. Before this gate the
/// flag survived argv parsing as the hidden compatibility placeholder built in
/// `cli::spec_command`, `From<Args>` carried `true` into
/// [`CrawlOptions::adaptive_selectors`], and `build_adaptive_engine`
/// (`webfang_cli`, `#[cfg(feature = "adaptive-selectors")]`) was compiled out
/// entirely — so the run reached the network and exited 0 having done nothing.
///
/// The placeholder is deliberately KEPT: it is the parse half of this
/// two-part gate. Deleting it would turn a named, actionable build error into
/// clap's "unexpected argument" (exit 64) with no mention of the feature.
///
/// The [`CrawlOptions`]-shaped half is kept for a programmatic caller that
/// builds options directly. The shipped CLI does NOT use it — it goes through
/// `check_adaptive_selectors_capability`, which runs inside [`normalize`].
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when
/// `adaptive_selectors` is requested and the `adaptive-selectors` feature is
/// not compiled in.
pub fn check_adaptive_selectors_feature(opts: &CrawlOptions) -> Result<(), CliExit> {
    check_adaptive_selectors_feature_with(cfg!(feature = "adaptive-selectors"), opts)
}

/// Feature-injectable core of [`check_adaptive_selectors_feature`] — `cfg!`
/// is a compile-time constant and cannot be toggled per test.
fn check_adaptive_selectors_feature_with(
    adaptive_enabled: bool,
    opts: &CrawlOptions,
) -> Result<(), CliExit> {
    adaptive_selectors_unavailable(adaptive_enabled, opts.adaptive_selectors)
}

/// #1813 — the SAME capability check, reached from the SHARED staging
/// pipeline instead of from one call site in the CLI binary.
///
/// ## Why it moved out of `main.rs`
///
/// T1 established the rule this follows: a validation that only one front door
/// runs is not a validation, it is a suggestion. `--rate-limit-burst` went into
/// [`stage_budget_overrides`] precisely so argv, env, TOML and programmatic
/// `Args` all share ONE accept/reject semantic. This gate was wired straight
/// into `webfang_cli/src/main.rs` step 6e2 instead, so it was reachable from
/// exactly one caller — and `main.rs:116` (`normalize`) runs long before
/// `main.rs:220` (the gate), which is the ordering proof that every `Args`
/// front door passes through the staging pipeline on its way to the binary.
///
/// ## Why `Args`, not `CrawlOptions`
///
/// `normalize` stages [`Args`]; `CrawlOptions` does not exist yet at that point
/// (it is built from `Args` afterwards, `main.rs:121`). The request itself is
/// unambiguous — `From<Args>` copies `args.crawler.adaptive_selectors` verbatim
/// into [`CrawlOptions::adaptive_selectors`], so the two can never disagree —
/// and `ConfigDefaults::adaptive_selectors` (`domain/config.rs:98`) is declared
/// but read by NOTHING, so TOML is not a fourth front door for this flag.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when the request is
/// present and the feature is not compiled in — the same value the
/// `CrawlOptions`-shaped check returns, from
/// [`ADAPTIVE_SELECTORS_UNAVAILABLE`].
///
/// # Deliberately independent of [`ArgSources`]
///
/// Every field the `FieldBook` stages is rank-guarded: it lands only when
/// provenance recorded it, so a value present in `Args` without a recorded
/// source never reaches the book and the derived default wins. This check is
/// NOT rank-guarded and deliberately reads `Args` directly. An unranked
/// boolean that arrives from argv or from the env var carries no ambiguity to
/// resolve — there is no lower-ranked value it could lose to — and making the
/// gate depend on a provenance entry would mean a front door that forgot to
/// record one silently regains the fail-open this is meant to close.
fn check_adaptive_selectors_capability(args: &Args) -> Result<(), CliExit> {
    check_adaptive_selectors_capability_with(cfg!(feature = "adaptive-selectors"), args)
}

/// Feature-injectable core of [`check_adaptive_selectors_capability`] — the
/// #761/#796 injection seam, so the negative case is runnable in the
/// `--all-features` lane that CI uses.
fn check_adaptive_selectors_capability_with(
    adaptive_enabled: bool,
    args: &Args,
) -> Result<(), CliExit> {
    adaptive_selectors_unavailable(adaptive_enabled, args.crawler.adaptive_selectors)
}

/// Preflight: `--export-format vector` without `--clean-ai` must fail before
/// any network request (#796).
///
/// Mirrors the #703/#652 `output_vectors_gate` in the orchestrator: without
/// `--clean-ai` there are no embeddings, so the vector exporter would write an
/// invalid `export.json` (`dimensions: null`, `model_name: null`, documents
/// without `embeddings`) while still reporting success (exit 0). An explicit
/// request the binary cannot honor is never a silent no-op: with the `ai`
/// feature it is a data-format error (exit 65); on a non-AI build it is a
/// build-configuration error (exit 78), matching the #761 fail-fast pattern
/// of [`check_clean_ai_feature`]. `ExportFormat::Jsonl` (the default) and
/// `ExportFormat::Auto` are never gated.
///
/// # Errors
///
/// Returns [`crate::CliExit::DataFormatError`] (exit 65) when the export
/// format is `Vector`, `--clean-ai` was not given, and the `ai` feature is
/// compiled in. Returns [`crate::CliExit::ConfigError`] (exit 78) when the
/// binary was built without the `ai` feature.
pub fn check_export_format_vector(opts: &CrawlOptions) -> Result<(), CliExit> {
    check_export_format_vector_with(cfg!(feature = "ai"), opts)
}

/// Feature-injectable core of [`check_export_format_vector`] — `cfg!` cannot
/// be toggled per test.
fn check_export_format_vector_with(ai_enabled: bool, opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.export.export_format != ExportFormat::Vector {
        return Ok(());
    }
    if opts.ai && ai_enabled {
        return Ok(());
    }

    if ai_enabled {
        warn!("--export-format vector rejected without --clean-ai; no embeddings to export");
        return Err(CliExit::DataFormatError(
            "No hay vectores para exportar: '--export-format vector' requiere \
             '--clean-ai' para generar embeddings"
                .to_string(),
        ));
    }

    warn!("--export-format vector rejected on a non-AI build; no embeddings to export");
    Err(CliExit::ConfigError(
        "Se requiere compilar con '--features ai' para usar --export-format vector".to_string(),
    ))
}

// ============================================================================
// LLM extraction provider gate (§8b)
// ============================================================================

/// Preflight: `--extract-with-llm` without a configured `completion` provider
/// must fail at startup, not on the first invocation (`ai-providers-design.md`
/// §8b).
///
/// This is the **expiry of the §8b DEBE**: the Container stays permissive
/// (`llm_port()` is `None` without a provider, by contract), but every service
/// binary that exposes an LLM flag MUST validate the port is present before
/// doing any work. Without this gate a daemon reports OK at boot and then
/// fails when a user or cron invokes the feature, with retries amplifying the
/// failure before anyone notices.
///
/// The check is deliberately **static** (flag + config presence) rather than a
/// probe of the constructed port: a reachable-but-broken provider is a runtime
/// concern with its own error classification, while a missing configuration is
/// a startup configuration error (exit 78).
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--extract-with-llm`
/// is set but the config declares no provider with the `completion`
/// capability, or when `--llm-provider` names an id that is unknown or lacks
/// the capability.
pub fn check_extract_with_llm(
    opts: &CrawlOptions,
    providers: &crate::domain::providers::ProvidersConfig,
) -> Result<(), CliExit> {
    if !opts.extract_with_llm {
        return Ok(());
    }

    let registry = crate::domain::providers::ProviderRegistry::new(providers.clone());
    let resolution = match opts.llm_provider.as_deref() {
        Some(id) => registry.resolve(id, crate::domain::providers::Capability::Completion),
        None => registry.resolve_default_completion(),
    };

    match resolution {
        Ok(_) => Ok(()),
        Err(e) => {
            warn!(
                requested_provider = ?opts.llm_provider,
                error = %e,
                "--extract-with-llm rejected: no usable completion provider"
            );
            Err(CliExit::ConfigError(format!(
                "'--extract-with-llm' requiere un provider LLM configurado \
                 con la capacidad `completion`: {e}. Configurá un provider \
                 en el archivo de configuración o quitá el flag."
            )))
        },
    }
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
///
/// # SSRF layer 2 (#1615, G-4)
///
/// This path has no caller in the tree today — the discovery flow resolves its
/// seed through guarded code — which is exactly why it was unguarded: nothing
/// exercised it, so nothing noticed it built a client and dialled whatever it
/// was handed. That is a trap, not a defence: the next wiring would inherit an
/// unguarded fetch path, and a HEAD to a cloud-metadata address is a real
/// request.
///
/// Rather than delete a dead-but-public function (a compatibility change the
/// issue explicitly left as a maintainer decision), the guard goes in now, so
/// the function is safe the moment it is wired. The refusal rides the existing
/// `Failed` arm: to a caller asking "can I reach this host?", a refused target
/// and an unreachable one are the same answer, and the reason is carried in
/// the message rather than hidden.
///
/// The GET fallback is covered by the same check because it runs only after
/// this one passed.
pub async fn preflight_check(url: &url::Url) -> PreflightResult {
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(url) {
        return PreflightResult::Failed(rejection.to_string());
    }

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
    // The fake obscura binary is a `#!/bin/sh` script resolved through an
    // extension-less PATH lookup — unix semantics with no Windows equivalent
    // to assert (#1825).
    #[cfg(unix)]
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
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
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
        assert_eq!(
            parse_obscura_version("obscura 0.2.0+build"),
            Some((0, 2, 0))
        );
        assert_eq!(parse_obscura_version("no version here"), None);
        assert_eq!(parse_obscura_version(""), None);
        assert_eq!(parse_obscura_version("version x.y.z"), None);
        assert_eq!(parse_obscura_version("obscura 0.2"), None);
    }

    /// `assess_obscura_version` classifies against the 0.2.0 minimum — the
    /// exact boundary, above it, below it, and the missing case.
    #[test]
    fn assess_obscura_version_classifies_meets_too_old_unknown() {
        assert_eq!(
            assess_obscura_version(Some((0, 2, 0))),
            VersionVerdict::Meets
        );
        assert_eq!(
            assess_obscura_version(Some((0, 3, 0))),
            VersionVerdict::Meets
        );
        assert_eq!(
            assess_obscura_version(Some((1, 0, 0))),
            VersionVerdict::Meets
        );
        assert_eq!(
            assess_obscura_version(Some((0, 1, 9))),
            VersionVerdict::TooOld
        );
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
        std::fs::write(&bin_path, "#!/bin/sh\necho \"custom build\"\n")
            .expect("write fake obscura");
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

    /// `Full` strategy with a present, exit-0 binary passes the preflight
    /// check. Hermetic: a fake `google-chrome` executable in a controlled
    /// PATH dir (same pattern as the obscura fakes), so the test never
    /// depends on the real process environment.
    // Miri cannot emulate posix_spawn (Command::status), so both tests that
    // actually probe a candidate binary are skipped there (#775). The
    // short-circuit tests (static/hybrid/feature-gate) above stay active.
    #[cfg_attr(miri, ignore)] // Command::spawn → posix_spawnattr_init unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn full_strategy_ok_when_binary_reports_version() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        write_chrome_like(tmp.path(), "google-chrome", 0);
        let path_value = tmp.path().to_string_lossy().into_owned();

        let mut opts = CrawlOptions::default();
        opts.network.js_strategy = JsStrategy::Full;
        assert!(
            check_js_dependencies_with(&["google-chrome"], true, &path_value, &opts).is_ok(),
            "a candidate that exits 0 must satisfy the Full-strategy check"
        );
    }

    /// F-52-c (#1278): the resolver returns the first candidate (in order)
    /// that exists as a file AND reports a version — gate-certified is
    /// exactly what the launcher will execute.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn resolve_chrome_binary_prefers_first_working_candidate() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        write_chrome_like(tmp.path(), "google-chrome", 0);
        write_chrome_like(tmp.path(), "chromium", 0);
        let path_value = tmp.path().to_string_lossy().into_owned();

        let resolved = resolve_chrome_binary_with(&["google-chrome", "chromium"], &path_value)
            .expect("a working candidate must resolve");
        assert_eq!(resolved, tmp.path().join("google-chrome"));
    }

    /// F-52-c (#1278): a *directory* named like a candidate must be skipped
    /// (`is_file`, not `exists`) — this is the `/opt/google/chrome` trap
    /// where chromiumoxide's `get_by_path` accepted a directory and the
    /// launch died with permission denied.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn resolve_chrome_binary_skips_directories() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(tmp.path().join("google-chrome")).expect("fake chrome directory");
        write_chrome_like(tmp.path(), "chromium", 0);
        let path_value = tmp.path().to_string_lossy().into_owned();

        let resolved = resolve_chrome_binary_with(&["google-chrome", "chromium"], &path_value)
            .expect("the working fallback must resolve");
        assert_eq!(resolved, tmp.path().join("chromium"));
    }

    /// F-52-c (#1278): a candidate whose `--version` probe fails is skipped,
    /// and no resolvable candidate is `None` (gate goes red downstream).
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn resolve_chrome_binary_skips_failing_version_probe() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        write_chrome_like(tmp.path(), "google-chrome", 1);
        let path_value = tmp.path().to_string_lossy().into_owned();

        assert!(
            resolve_chrome_binary_with(&["google-chrome"], &path_value).is_none(),
            "a candidate that fails --version must not resolve"
        );
        assert!(
            resolve_chrome_binary_with(&["definitely-not-installed-9x4k"], &path_value).is_none(),
            "a missing candidate must not resolve"
        );
    }

    /// Write a fake Chrome-like executable: exits `code` on `--version`.
    /// Mirrors `write_obscura_with_version` (#793 pattern).
    #[cfg(unix)]
    fn write_chrome_like(dir: &std::path::Path, name: &str, code: i32) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin_path = dir.join(name);
        std::fs::write(&bin_path, format!("#!/bin/sh\nexit {code}\n"))
            .expect("write fake chrome binary");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x fake chrome binary");
        bin_path
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

    #[test]
    fn db_path_without_elastic_is_rejected() {
        // #799: --db-path without --elastic is a silent no-op; reject it up
        // front so the user does not lose data unknowingly.
        let mut opts = CrawlOptions::default();
        opts.elastic.db_path = Some(std::path::PathBuf::from("/tmp/test.db"));
        assert!(check_db_path_requires_elastic(&opts).is_err());
    }

    #[test]
    fn db_path_with_elastic_is_allowed() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        opts.elastic.db_path = Some(std::path::PathBuf::from("/tmp/test.db"));
        assert!(check_db_path_requires_elastic(&opts).is_ok());
    }

    /// No `--clean-ai`: passes regardless of the feature state.
    #[test]
    fn no_clean_ai_ok_without_feature() {
        let opts = CrawlOptions::default();
        assert!(check_clean_ai_feature_with(false, &opts).is_ok());
    }

    // ========================================================================
    // #1813 — --adaptive-selectors preflight feature check
    // ========================================================================

    /// Build opts with `--adaptive-selectors` requested.
    fn adaptive_selectors_opts() -> CrawlOptions {
        CrawlOptions {
            adaptive_selectors: true,
            ..CrawlOptions::default()
        }
    }

    /// `--adaptive-selectors` without the feature: config error naming the
    /// feature, before any network request (#1813).
    #[test]
    fn adaptive_selectors_without_feature_errors() {
        let err = check_adaptive_selectors_feature_with(false, &adaptive_selectors_opts())
            .expect_err("non-adaptive build must reject --adaptive-selectors in preflight");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
                "config error must name the flag and the feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// `--adaptive-selectors` with the feature compiled in: passes.
    #[test]
    fn adaptive_selectors_with_feature_ok() {
        assert!(check_adaptive_selectors_feature_with(true, &adaptive_selectors_opts()).is_ok());
    }

    /// No `--adaptive-selectors`: passes regardless of the feature state, so
    /// the gate never blocks a run that did not ask for the capability.
    #[test]
    fn no_adaptive_selectors_ok_without_feature() {
        assert!(check_adaptive_selectors_feature_with(false, &CrawlOptions::default()).is_ok());
    }

    /// The gate is a build-capability check, not a value check: `--clean-ai`
    /// on the SAME non-adaptive build still resolves through its own #761 gate
    /// and must not be swallowed by this one (#1813).
    #[test]
    fn adaptive_gate_does_not_shadow_the_clean_ai_gate() {
        let opts = CrawlOptions {
            ai: true,
            ..adaptive_selectors_opts()
        };
        assert!(check_clean_ai_feature_with(true, &opts).is_ok());
        assert!(check_adaptive_selectors_feature_with(false, &opts).is_err());
    }

    // ---- #1813 S1: the gate is reached from the SHARED staging pipeline ----

    /// `Args` that request the capability, i.e. what argv or
    /// `WEBFANG_ADAPTIVE_SELECTORS` produces before `normalize` runs.
    fn adaptive_selectors_args() -> Args {
        let mut args = Args::default();
        args.crawler.adaptive_selectors = true;
        args
    }

    /// The regression S1 is about, in the lane CI actually runs.
    ///
    /// The gate used to be reachable ONLY from `webfang_cli/src/main.rs` step
    /// 6e2, so this `Args`-shaped seam did not exist at all. Now the same
    /// request, read off `Args` exactly the way `normalize` reads it, is
    /// rejected with exit 78 and the feature-naming message.
    ///
    /// This runs in EVERY lane including `--all-features`, which is the whole
    /// point of the injection seam: without it, a gate wired this way would be
    /// invisible to CI for the same reason the original defect was.
    #[test]
    fn adaptive_capability_gate_from_args_rejects_request_without_the_feature() {
        let err = check_adaptive_selectors_capability_with(false, &adaptive_selectors_args())
            .expect_err("a non-adaptive build must reject the request during normalization");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
                "config error must name the flag and the feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// With the feature: the same `Args` passes, so the gate never blocks a
    /// run on a build that CAN honor the request.
    #[test]
    fn adaptive_capability_gate_from_args_accepts_request_with_the_feature() {
        assert!(check_adaptive_selectors_capability_with(true, &adaptive_selectors_args()).is_ok());
    }

    /// No request: passes regardless of the feature state.
    #[test]
    fn adaptive_capability_gate_from_args_is_silent_without_the_request() {
        assert!(check_adaptive_selectors_capability_with(false, &Args::default()).is_ok());
    }

    /// The two input shapes must not drift into two different decisions — they
    /// share one semantic, and a run reaching `normalize` is subject to exactly
    /// the same verdict a programmatic `CrawlOptions` builder would get.
    #[test]
    fn adaptive_capability_gate_agrees_across_both_input_shapes() {
        for adaptive_enabled in [false, true] {
            let from_args = check_adaptive_selectors_capability_with(
                adaptive_enabled,
                &adaptive_selectors_args(),
            );
            let from_opts =
                check_adaptive_selectors_feature_with(adaptive_enabled, &adaptive_selectors_opts());
            assert_eq!(
                from_args.is_ok(),
                from_opts.is_ok(),
                "the Args-shaped and CrawlOptions-shaped gates must reach the same \
                 verdict (adaptive_enabled={adaptive_enabled})"
            );
        }
    }

    // ========================================================================
    // #796 — --export-format vector preflight gate (mirrors #703/#652, #761)
    // ========================================================================

    /// Build opts with `--export-format vector`.
    fn export_format_vector_opts(ai: bool) -> CrawlOptions {
        let mut opts = CrawlOptions {
            ai,
            ..CrawlOptions::default()
        };
        opts.export.export_format = ExportFormat::Vector;
        opts
    }

    /// `--export-format vector` on a non-AI build: config error naming the
    /// feature, before any network request (#796).
    #[test]
    fn export_format_vector_without_ai_feature_errors() {
        let opts = export_format_vector_opts(false);
        let err = check_export_format_vector_with(false, &opts)
            .expect_err("non-AI build must reject --export-format vector");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--export-format vector") && msg.contains("ai"),
                "config error must name the flag and the feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// `--export-format vector` with `--clean-ai` and the `ai` feature: the
    /// embeddings will exist, so the gate passes.
    #[test]
    fn export_format_vector_with_clean_ai_ok() {
        let opts = export_format_vector_opts(true);
        assert!(check_export_format_vector_with(true, &opts).is_ok());
    }

    /// `--export-format vector` WITHOUT `--clean-ai` on an AI build: data
    /// format error (exit 65) with the Spanish message — mirrors the
    /// output-vectors gate (#796).
    #[test]
    fn export_format_vector_without_clean_ai_errors() {
        let opts = export_format_vector_opts(false);
        let err = check_export_format_vector_with(true, &opts)
            .expect_err("AI build must reject --export-format vector without --clean-ai");
        match err {
            CliExit::DataFormatError(msg) => assert!(
                msg.contains("No hay vectores para exportar")
                    && msg.contains("--export-format vector"),
                "data format error must carry the Spanish message, got: {msg}"
            ),
            other => panic!("expected DataFormatError, got: {other:?}"),
        }
    }

    /// `--export-format jsonl` (the default): never gated.
    #[test]
    fn export_format_jsonl_not_gated() {
        let opts = CrawlOptions::default();
        assert_eq!(opts.export.export_format, ExportFormat::Jsonl);
        assert!(check_export_format_vector_with(false, &opts).is_ok());
        assert!(check_export_format_vector_with(true, &opts).is_ok());
    }

    /// `--export-format auto`: never gated — it is not an explicit vector
    /// request.
    #[test]
    fn export_format_auto_not_gated() {
        let mut opts = CrawlOptions::default();
        opts.export.export_format = ExportFormat::Auto;
        assert!(check_export_format_vector_with(false, &opts).is_ok());
        assert!(check_export_format_vector_with(true, &opts).is_ok());
    }

    // ---- XP-S-04 (#1608) — platform-independent PATH resolution ----------

    /// `PATHEXT` parsing: order preserved, empty entries dropped, casing
    /// normalized, dotless entries get the leading dot.
    #[test]
    fn parse_pathext_preserves_order_and_normalizes() {
        assert_eq!(
            parse_pathext(".COM;.EXE;.BAT;.CMD"),
            [".com", ".exe", ".bat", ".cmd"]
        );
        assert_eq!(parse_pathext(".EXE;;  .BAT "), [".exe", ".bat"]);
        assert_eq!(parse_pathext("EXE;.BAT"), [".exe", ".bat"]);
        assert!(parse_pathext("").is_empty());
        assert!(parse_pathext(";;;").is_empty());
    }

    /// A bare name on a non-PATHEXT platform resolves as exactly one
    /// candidate (the name itself).
    #[test]
    fn bare_name_candidates_without_pathext_is_the_name_alone() {
        assert_eq!(bare_name_candidates("chrome", None), ["chrome"]);
    }

    /// A bare name with `PATHEXT` expands to the exact name first, then the
    /// PATHEXT-ordered extensions — the resolution order cmd.exe uses.
    #[test]
    fn bare_name_candidates_expands_pathext_in_order() {
        let candidates = bare_name_candidates("chrome", Some(".COM;.EXE;.BAT;.CMD"));
        assert_eq!(
            candidates,
            [
                "chrome",
                "chrome.com",
                "chrome.exe",
                "chrome.bat",
                "chrome.cmd"
            ]
        );
    }

    /// `first_existing_in_dirs` is dir-major: every candidate name is tried
    /// inside a directory before moving to the next directory.
    ///
    /// The `PATH` value is built with `std::env::join_paths`, never with a
    /// hardcoded separator: a `:`-joined string is one bogus entry for
    /// `split_paths` on Windows (which splits on `;` and keeps `C:` drive
    /// letters intact), which is why this test failed there with "a
    /// candidate must resolve" while passing on Linux/macOS.
    #[test]
    fn first_existing_in_dirs_prefers_earlier_directory() {
        let dir1 = tempfile::TempDir::new().expect("tempdir");
        let dir2 = tempfile::TempDir::new().expect("tempdir");
        // dir1 only has the SECOND candidate name; dir2 has the FIRST.
        std::fs::write(dir1.path().join("chromium"), "#!/bin/sh\n").expect("write");
        std::fs::write(dir2.path().join("google-chrome"), "#!/bin/sh\n").expect("write");

        let names = vec!["google-chrome".to_string(), "chromium".to_string()];
        let path_value =
            std::env::join_paths([dir1.path(), dir2.path()]).expect("joinable temp dirs");
        let resolved = first_existing_in_dirs(std::env::split_paths(&path_value), &names)
            .expect("a candidate must resolve");
        assert_eq!(resolved, dir1.path().join("chromium"));
    }

    /// Windows-shaped resolution through the injectable seam: the
    /// `PATHEXT` expansion is supplied as DATA (`bare_name_candidates`),
    /// never read from process env, so the Windows answer is exercised on
    /// every platform. Dir-major order must hold for the expanded
    /// candidates too: the earlier directory's `chromium.exe` (a PATHEXT
    /// candidate) wins over the later directory's bare `chromium`.
    #[test]
    fn first_existing_in_dirs_resolves_pathext_expansion_in_earlier_directory() {
        let dir1 = tempfile::TempDir::new().expect("tempdir");
        let dir2 = tempfile::TempDir::new().expect("tempdir");
        // dir1 only has an extension-bearing candidate; dir2 has the bare name.
        std::fs::write(dir1.path().join("chromium.exe"), "MZ fake binary\n").expect("write");
        std::fs::write(dir2.path().join("chromium"), "#!/bin/sh\n").expect("write");

        let names = bare_name_candidates("chromium", Some(".COM;.EXE"));
        assert_eq!(names, ["chromium", "chromium.com", "chromium.exe"]);
        let path_value =
            std::env::join_paths([dir1.path(), dir2.path()]).expect("joinable temp dirs");
        let resolved = first_existing_in_dirs(std::env::split_paths(&path_value), &names)
            .expect("a PATHEXT candidate must resolve");
        assert_eq!(
            resolved,
            dir1.path().join("chromium.exe"),
            "the earlier directory's extension candidate must win dir-major"
        );
    }

    /// Directories named like a candidate are skipped (`is_file`, the
    /// F-52-c / #1278 guard), and missing names resolve to `None`.
    #[test]
    fn first_existing_in_dirs_skips_directories_and_reports_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir(dir.path().join("chrome.exe")).expect("mkdir decoy");
        let names = vec!["chrome.exe".to_string()];
        assert!(
            first_existing_in_dirs(std::env::split_paths(dir.path().as_os_str()), &names).is_none()
        );
    }

    // ---- XP-S-06 (#1608) — bounded --version probes ----------------------

    /// A probe whose binary exits normally reports status + captured output.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn version_probe_reports_normal_exit_and_output() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("probe");
        std::fs::write(&bin_path, "#!/bin/sh\necho 'obscura 0.2.0'\n").expect("write probe");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x probe");

        let output = run_version_probe_with_timeout(&bin_path, std::time::Duration::from_secs(5))
            .expect("probe must complete");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("0.2.0"));
    }

    /// A probe that never exits is killed at the deadline and reports `None`
    /// — a wedged `--version` can no longer hang preflight forever.
    #[cfg_attr(miri, ignore)] // Command::spawn unsupported by Miri (#775)
    #[cfg(unix)]
    #[test]
    fn version_probe_kills_a_wedged_binary_at_the_deadline() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let bin_path = tmp.path().join("probe");
        std::fs::write(&bin_path, "#!/bin/sh\nsleep 30\n").expect("write probe");
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x probe");

        let started = std::time::Instant::now();
        let outcome =
            run_version_probe_with_timeout(&bin_path, std::time::Duration::from_millis(150));
        assert!(outcome.is_none(), "a wedged probe must report None");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the probe must return promptly after killing the child"
        );
    }
}

// ============================================================================
// Phase 3 — Normalization pipeline (RED: tests before implementation)
// These tests pin the rank-guard pipeline per design D3/D4. They intentionally
// fail to compile while the production symbols do not exist (strict TDD RED).
// ============================================================================
#[cfg(test)]
mod normalization_pipeline_tests {
    use super::*;
    use crate::domain::config_value::ConfigSource;

    fn dummy_args() -> Args {
        Args::default()
    }

    fn dummy_config() -> ConfigDefaults {
        ConfigDefaults::default()
    }

    #[test]
    fn stage_defaults_seeds_default_sources() {
        let book = stage_defaults();
        assert_eq!(book.max_pages.source, ConfigSource::Default);
        assert_eq!(book.max_pages.value, 10);
        assert_eq!(book.delay_ms.source, ConfigSource::Default);
    }

    #[test]
    fn stage_config_file_writes_config_file_source() {
        let mut book = stage_defaults();
        let config = ConfigDefaults {
            max_pages: Some(25),
            ..dummy_config()
        };
        let written = stage_config_file(&mut book, &config);
        assert!(written >= 1);
        assert_eq!(book.max_pages.value, 25);
        assert_eq!(book.max_pages.source, ConfigSource::ConfigFile);
    }

    #[test]
    fn rank_guard_config_file_rejected_over_cli() {
        let mut book = stage_defaults();
        book.max_pages = crate::domain::config_value::ConfigValue::new(10, ConfigSource::Cli);
        let config = ConfigDefaults {
            max_pages: Some(25),
            ..dummy_config()
        };
        let written = stage_config_file(&mut book, &config);
        assert_eq!(written, 0);
        assert_eq!(book.max_pages.value, 10);
        assert_eq!(book.max_pages.source, ConfigSource::Cli);
    }

    /// #1813 S1 — the `--adaptive-selectors` capability gate is a step of
    /// [`normalize`], not a call in the CLI binary's step list.
    ///
    /// ## Why these two tests are mutually `cfg`-gated
    ///
    /// `normalize` reads the feature state from `cfg!`, which is a
    /// compile-time constant, so no runtime argument can make the real pipeline
    /// fail in an `--all-features` build. The two lanes therefore each prove
    /// something different, and BOTH are non-empty:
    ///
    /// - `#[cfg(not(feature = "adaptive-selectors"))]` (negative lane) — the
    ///   REJECTION, driving the real `normalize`. This is the test that says
    ///   "the gate is inside the shared pipeline", not merely that a helper
    ///   function rejects the same input.
    /// - `#[cfg(feature = "adaptive-selectors")]` (`--all-features` lane) —
    ///   the NON-REGRESSION guard: on a build that HAS the feature, the same
    ///   `normalize` call with the same request must still succeed, so moving
    ///   the gate into the pipeline cannot have broken the working case.
    ///
    /// `cargo nextest list -p webfang_core -E 'test(adaptive)'` on each lane is
    /// the proof that neither compiles to nothing. The end-to-end CLI proof
    /// (real spawned binary, real exit code, zero network requests) lives in
    /// `tests/adaptive_selectors_gate_test.rs`.
    #[cfg(not(feature = "adaptive-selectors"))]
    #[test]
    fn normalize_rejects_adaptive_selectors_on_a_non_adaptive_build() {
        let mut args = dummy_args();
        args.crawler.adaptive_selectors = true;

        let err = normalize(&args, &ArgSources::default(), &dummy_config())
            .expect_err("the shared pipeline must reject the capability it cannot honor");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--adaptive-selectors") && msg.contains("`adaptive-selectors`"),
                "normalize's rejection must name the flag and the feature, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    /// Non-regression guard for the positive lane: with the feature compiled in,
    /// `--adaptive-selectors` through the same pipeline must still succeed.
    ///
    /// The assertion is on the pipeline SUCCEEDING, which is the whole point:
    /// the flag used to work on this lane and must keep working. `adaptive_selectors`
    /// itself is deliberately not asserted on the projected options —
    /// `NormalizedConfig::into_crawl_options` has never carried it; it travels
    /// via `CrawlOptions::from(Args)` — so pinning it here would pin a
    /// projection detail this change did not touch.
    #[cfg(feature = "adaptive-selectors")]
    #[test]
    fn normalize_accepts_adaptive_selectors_on_an_adaptive_build() {
        let mut args = dummy_args();
        args.crawler.adaptive_selectors = true;

        normalize(&args, &ArgSources::default(), &dummy_config())
            .expect("an adaptive build must normalize a --adaptive-selectors request");
    }

    /// A run that did NOT ask for the capability must never be failed closed by
    /// the new pipeline step — in EITHER lane. This one is deliberately not
    /// `cfg`-gated, so it is the single test that holds in both.
    #[test]
    fn normalize_does_not_gate_a_run_that_did_not_request_adaptive_selectors() {
        let args = dummy_args();
        assert!(
            !args.crawler.adaptive_selectors,
            "precondition: the default Args does not request the capability"
        );
        normalize(&args, &ArgSources::default(), &dummy_config())
            .expect("normalize must not fail closed on a default request set");
    }

    /// Field errors keep their existing precedence over the new capability
    /// step. The gate runs LAST inside `normalize` precisely so a run that is
    /// already misconfigured reports the misconfiguration it always did.
    ///
    /// `max_pages` is staged only when the provenance map records it — the
    /// pipeline is rank-guarded, so an `Args` value with no recorded source is
    /// not a staged value and the book keeps its default. Recording the source
    /// is what makes this the conflict case it claims to be.
    #[test]
    fn normalize_reports_field_errors_before_the_capability_gate() {
        let mut args = dummy_args();
        args.crawler.adaptive_selectors = true;
        args.crawler.max_pages = 0;
        let mut sources = ArgSources::default();
        sources.set("max_pages", ConfigSource::Cli);

        let err = normalize(&args, &sources, &dummy_config())
            .expect_err("max_pages 0 must still be rejected");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("max_pages"),
                "the long-standing max_pages error must keep winning, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    #[test]
    fn stage_env_cli_writes_cli_obsidian_tags() {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.obsidian.obsidian_tags = Some(vec!["scraped".into(), "web-dev".into(), "rust".into()]);
        let mut sources = ArgSources::default();
        sources.set("obsidian_tags", ConfigSource::Cli);
        let written = stage_env_cli(&mut book, &args, &sources);
        assert!(written >= 1);
        assert_eq!(book.obsidian_tags.source, ConfigSource::Cli);
        assert_eq!(book.obsidian_tags.value, vec!["scraped", "web-dev", "rust"]);
    }

    #[test]
    fn stage_env_cli_environment_then_cli() {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.max_pages = 7;
        let mut sources = ArgSources::default();
        sources.set("max_pages", ConfigSource::Environment);
        let written_env = stage_env_cli(&mut book, &args, &sources);
        assert!(written_env >= 1);
        assert_eq!(book.max_pages.source, ConfigSource::Environment);
        // Now CLI outranks Environment even when value equals default
        let mut args2 = dummy_args();
        args2.crawler.max_pages = 10;
        let mut sources2 = ArgSources::default();
        sources2.set("max_pages", ConfigSource::Cli);
        let written_cli = stage_env_cli(&mut book, &args2, &sources2);
        assert!(written_cli >= 1);
        assert_eq!(book.max_pages.value, 10);
        assert_eq!(book.max_pages.source, ConfigSource::Cli);
    }

    #[test]
    fn cross_field_sitemap_url_implies_use_sitemap_from_any_source() {
        // Via config file
        let mut book = stage_defaults();
        let _config = ConfigDefaults { ..dummy_config() };
        // Simulate sitemap_url coming from config stage then cross-field rule
        book.sitemap_url = crate::domain::config_value::ConfigValue::new(
            Some("https://example.com/sitemap.xml".to_string()),
            ConfigSource::ConfigFile,
        );
        let _ = apply_cross_field_rules(&mut book);
        assert!(
            book.use_sitemap.value,
            "sitemap_url via ConfigFile must imply use_sitemap"
        );
        // Via CLI
        let mut book2 = stage_defaults();
        book2.sitemap_url = crate::domain::config_value::ConfigValue::new(
            Some("https://example.com/sitemap.xml".to_string()),
            ConfigSource::Cli,
        );
        let _ = apply_cross_field_rules(&mut book2);
        assert!(book2.use_sitemap.value);
        // Via Environment (third covered source)
        let mut book3 = stage_defaults();
        book3.sitemap_url = crate::domain::config_value::ConfigValue::new(
            Some("https://example.com/sitemap.xml".to_string()),
            ConfigSource::Environment,
        );
        let _ = apply_cross_field_rules(&mut book3);
        assert!(book3.use_sitemap.value);
    }

    #[test]
    fn obsidian_tag_trim_dedup_byte_parity() {
        let mut book = stage_defaults();
        book.obsidian_tags = crate::domain::config_value::ConfigValue::new(
            vec![
                " rust ".to_string(),
                "rust".to_string(),
                "cargo".to_string(),
                " ".to_string(),
            ],
            ConfigSource::Default,
        );
        let _ = apply_cross_field_rules(&mut book);
        assert_eq!(
            book.obsidian_tags.value,
            vec!["rust".to_string(), "cargo".to_string()]
        );
    }

    #[test]
    fn validate_stage_blocks_invalid_max_pages() {
        let mut book = stage_defaults();
        book.max_pages = crate::domain::config_value::ConfigValue::new(0, ConfigSource::Cli);
        let err = validate_stage(&book).expect_err("max_pages 0 must fail validation");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("max_pages"),
                "Spanish validation error, got: {msg}"
            ),
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn into_crawl_options_projection_parity() {
        let book = stage_defaults();
        let normalized = NormalizedConfig::from_book(book);
        let opts = normalized.into_crawl_options();
        let expected = CrawlOptions::default();
        assert_eq!(opts.crawl.max_pages, expected.crawl.max_pages);
        assert_eq!(opts.crawl.selector, expected.crawl.selector);
        assert_eq!(opts.export.output_format, expected.export.output_format);
    }

    /// CLI wiring verification (issue #1599, slice 4 — verify first): an
    /// explicit `--concurrency 2` must surface as `BudgetOverrides.crawl`
    /// through `into_crawl_options` (explicit-wins), so the engine derives
    /// its scheduler spawn bound from the override instead of the machine
    /// tier. Expected outcome: verification-only, zero CLI diff.
    #[test]
    fn into_crawl_options_explicit_concurrency_feeds_budget_override() {
        let mut book = stage_defaults();
        book.concurrency = ConfigValue::new(ConcurrencyConfig::new(2), ConfigSource::Cli);
        let opts = NormalizedConfig::from_book(book).into_crawl_options();
        assert_eq!(
            opts.budget_overrides
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(2),
            "explicit --concurrency 2 must reach the engine as a crawl-tier override"
        );
    }

    /// CLI wiring verification (issue #1599, slice 4): auto concurrency
    /// (`None`) stages no crawl-tier override, preserving today's
    /// detector-derived behavior exactly.
    #[test]
    fn into_crawl_options_auto_concurrency_preserves_derived_behavior() {
        let book = stage_defaults();
        let opts = NormalizedConfig::from_book(book).into_crawl_options();
        assert_eq!(
            opts.budget_overrides.crawl, None,
            "auto concurrency must not stage a crawl-tier override"
        );
    }

    #[test]
    fn stage_budget_overrides_writes_cli_burst() {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("7".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);
        let written = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
            .expect("staging ok");
        assert_eq!(written, 1);
        assert_eq!(book.budget_overrides.source, ConfigSource::Cli);
        assert_eq!(
            book.budget_overrides.value.rate_burst,
            BurstPermits::new(7).ok()
        );
    }

    #[test]
    fn stage_budget_overrides_toml_staged_at_config_file_rank_and_cli_outranks() {
        let mut book = stage_defaults();
        let config = ConfigDefaults {
            rate_limit_burst: Some(4),
            ..dummy_config()
        };
        let no_sources = ArgSources::default();
        let written = stage_budget_overrides(&mut book, &dummy_args(), &no_sources, &config)
            .expect("staging ok");
        assert_eq!(written, 1);
        assert_eq!(book.budget_overrides.source, ConfigSource::ConfigFile);
        // CLI outranks the TOML value
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("6".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);
        let written_cli =
            stage_budget_overrides(&mut book, &args, &sources, &config).expect("staging ok");
        assert_eq!(written_cli, 1);
        assert_eq!(book.budget_overrides.source, ConfigSource::Cli);
        assert_eq!(
            book.budget_overrides.value.rate_burst,
            BurstPermits::new(6).ok()
        );
    }

    // ========================================================================
    // merge_budget_overrides (#897 item 1 + review M1 of PR #925)
    // ========================================================================

    #[test]
    fn merge_staged_crawl_outranks_cli_capture() {
        // M1: a plain `--concurrency 2` (unranked CLI capture) must NEVER
        // stomp a ranked staged value — the same contradiction that
        // made `network.concurrency` and the budget model disagree.
        let merged =
            merge_budget_overrides(test_overrides(Some(2), None), test_overrides(Some(5), None));
        assert_eq!(
            merged
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(5),
            "ranked staged crawl must win over the unranked CLI capture"
        );
    }

    #[test]
    fn merge_cli_capture_fills_unstaged_crawl_gap() {
        // Nothing staged (no explicit knob anywhere): the CLI capture is
        // the only source and must survive.
        let merged =
            merge_budget_overrides(test_overrides(Some(3), None), test_overrides(None, None));
        assert_eq!(
            merged
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(3)
        );
    }

    #[test]
    fn merge_cli_explicit_asset_wins_and_staged_toml_crawl_survives() {
        // Sharpest slot-copy guard, at unit level: TOML-staged crawl AND an
        // explicit CLI --download-concurrency must coexist field-wise.
        let merged =
            merge_budget_overrides(test_overrides(None, Some(6)), test_overrides(Some(2), None));
        assert_eq!(
            merged
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(2)
        );
        assert_eq!(
            merged
                .asset
                .map(crate::domain::budget::tiers::DownloadConcurrency::get),
            Some(6)
        );
    }

    /// Test fixture builder: overrides with only the two tiers under test.
    fn test_overrides(crawl: Option<usize>, asset: Option<usize>) -> BudgetOverrides {
        BudgetOverrides {
            rate_burst: None,
            crawl: crawl.and_then(|v| crate::domain::budget::tiers::CrawlConcurrency::new(v).ok()),
            batch: None,
            asset: asset
                .and_then(|v| crate::domain::budget::tiers::DownloadConcurrency::new(v).ok()),
        }
    }

    #[test]
    fn budget_overrides_default_is_none() {
        let book = stage_defaults();
        assert_eq!(book.budget_overrides.value.rate_burst, None);
        assert_eq!(book.budget_overrides.source, ConfigSource::Default);
    }

    #[test]
    fn burst_zero_rejected_with_spanish_error() {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("0".to_string()); // bypasses clap parser (programmatic)
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);
        let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
            .expect_err("zero burst must be rejected");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("--rate-limit-burst debe ser >= 1"),
                "unexpected error: {msg}"
            ),
            other => panic!("expected ConfigError, got {other:?}"),
        }
        // slot untouched
        assert_eq!(book.budget_overrides.value.rate_burst, None);
    }

    // ========================================================================
    // #1813 — the preflight pipeline is the SINGLE validating authority for
    // `--rate-limit-burst`, and `From<Args>` no longer parses it a second time.
    // ========================================================================

    /// `burst_non_numeric_rejected_with_spanish_error` — the last fail-open
    /// arm, at the layer that actually owns the decision.
    ///
    /// `auto` is deliberately NOT in the list: it is a keyword the sibling
    /// `--concurrency` parser understands, not a typo, and hard-`Err`-ing on it
    /// broke a spelling that worked on the base. See
    /// `burst_auto_keyword_stages_as_not_set`.
    #[test]
    fn burst_non_numeric_rejected_with_spanish_error() {
        for raw in ["banana", "1_0", "12x"] {
            let mut book = stage_defaults();
            let mut args = dummy_args();
            args.crawler.rate_limit_burst = Some(raw.to_string());
            let mut sources = ArgSources::default();
            sources.set("rate_limit_burst", ConfigSource::Cli);
            let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
                .expect_err("a non-numeric burst must be rejected, not defaulted");
            match err {
                CliExit::ConfigError(msg) => assert!(
                    msg.contains("no es un número"),
                    "unexpected error for {raw}: {msg}"
                ),
                other => panic!("expected ConfigError for {raw}, got {other:?}"),
            }
            assert_eq!(book.budget_overrides.value.rate_burst, None);
        }
    }

    /// The `auto` counterpart, at the same layer: the keyword must stage as
    /// "not set" — no override written, NO error — so the hardware-derived
    /// budget applies. Asserting the empty slot matters as much as the `Ok`:
    /// writing a value here would silently override the derived burst, which is
    /// the other half of the same regression.
    #[test]
    fn burst_auto_keyword_stages_as_not_set() {
        for raw in ["auto", "AUTO", "  auto  "] {
            let mut book = stage_defaults();
            let mut args = dummy_args();
            args.crawler.rate_limit_burst = Some(raw.to_string());
            let mut sources = ArgSources::default();
            sources.set("rate_limit_burst", ConfigSource::Cli);
            let written = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
                .unwrap_or_else(|e| panic!("`{raw}` must not be rejected: {e:?}"));
            assert_eq!(
                book.budget_overrides.value.rate_burst, None,
                "`{raw}` must leave the burst slot for the derived default"
            );
            assert_eq!(written, 0, "`{raw}` is not set, so it writes no field");
        }
    }

    /// **The regression guard for the #1813 design itself.** `From<Args>` used
    /// to parse the burst independently, so a valid `--rate-limit-burst` was
    /// delivered through TWO paths. This test reproduces `main.rs` step 6b
    /// literally — `normalize` → `into_crawl_options` → `From<Args>` →
    /// `merge_budget_overrides` — because deleting the second parse is only
    /// safe if the first one alone still delivers the override. If
    /// `merge_budget_overrides` ever stops preferring `staged` when the CLI
    /// capture is `None`, the burst would be silently dropped for every
    /// operator and nothing else would notice.
    #[test]
    fn main_flow_merge_preserves_valid_burst_override() {
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("7".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Cli);

        // main.rs:116
        let normalized = normalize(&args, &sources, &dummy_config()).expect("normalize ok");
        // main.rs:121 — the infallible projection; contributes no burst.
        let base = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            base.budget_overrides.rate_burst, None,
            "precondition: `From<Args>` supplies no burst"
        );
        let cli_budget = base.budget_overrides;
        // main.rs:126
        let projected = normalized.into_crawl_options();
        // main.rs:156
        let merged = merge_budget_overrides(cli_budget, projected.budget_overrides);

        assert_eq!(
            merged.rate_burst,
            BurstPermits::new(7).ok(),
            "a valid --rate-limit-burst 7 must still reach the engine as the \
             rate-limiter burst after the duplicate parse was removed"
        );
    }

    /// The env-var front door reaches the same rejection. Same provenance rank
    /// as argv, so this is triangulation rather than a new code path — but it
    /// is the one the #1813 acceptance criterion names explicitly.
    #[test]
    fn burst_non_numeric_from_env_rejected() {
        let mut book = stage_defaults();
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("banana".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Environment);
        let err = stage_budget_overrides(&mut book, &args, &sources, &dummy_config())
            .expect_err("env-sourced non-numeric burst must be rejected");
        assert!(
            matches!(err, CliExit::ConfigError(_)),
            "expected ConfigError, got {err:?}"
        );
    }

    #[test]
    fn normalize_end_to_end_burst_reaches_crawl_options() {
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("9".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Environment);
        let normalized = normalize(&args, &sources, &dummy_config()).expect("normalize ok");
        assert_eq!(
            normalized.budget_overrides.value.rate_burst,
            BurstPermits::new(9).ok()
        );
        let opts = normalized.into_crawl_options();
        assert_eq!(opts.budget_overrides.rate_burst, BurstPermits::new(9).ok());
    }

    #[test]
    fn normalize_precedence_total_order() {
        let args = {
            let mut a = dummy_args();
            a.crawler.max_pages = 10;
            a
        };
        let mut sources = ArgSources::default();
        sources.set("max_pages", ConfigSource::Cli);
        let config = ConfigDefaults {
            max_pages: Some(25),
            ..dummy_config()
        };
        let normalized = normalize(&args, &sources, &config).expect("normalize ok");
        assert_eq!(normalized.max_pages.value, 10);
        assert_eq!(normalized.max_pages.source, ConfigSource::Cli);
    }
}
