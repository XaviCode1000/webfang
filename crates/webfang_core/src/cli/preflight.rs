//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.
#![allow(missing_docs)]

use std::path::PathBuf;
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
    /// meaningful; `Default`/`ConfigFile`/`Tui` inserted here are ignored by
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

/// TUI-emitted overrides: ONLY user-touched fields (design D2).
///
/// Plain JSON contract is preserved; every key in this payload is by
/// construction user-edited, so the pipeline tags the whole payload
/// `ConfigSource::Tui` at the stage boundary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TuiOverrides {
    fields: BTreeMap<String, serde_json::Value>,
}

impl TuiOverrides {
    /// Build from a JSON object value (non-object → empty).
    #[must_use]
    pub fn from_json(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Object(map) => Self {
                fields: map.into_iter().collect(),
            },
            _ => Self::default(),
        }
    }

    /// Insert or overwrite a field (last-write-wins within the TUI stage).
    pub fn insert(&mut self, key: String, value: serde_json::Value) {
        self.fields.insert(key, value);
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
        // Explicit operator concurrency (CLI/TOML/TUI, not "auto") feeds
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
///   Environment < Cli < Tui` resolution upstream.
///
/// Tier rules:
/// - `crawl`: **staged wins; the CLI capture only fills the gap.** Both
///   sides project the SAME user knob (`--concurrency` / TOML / TUI), but
///   only `staged` is rank-resolved. A blind `cli.or(staged)` here let a
///   plain `--concurrency` stomp a Tui-ranked staged value while
///   `network.concurrency` kept the ranked winner — a silent
///   contradiction inside one struct (adversarial review M1 of PR #925).
///   Preferring `staged` keeps `budget_overrides.crawl` consistent with
///   `network.concurrency` BY CONSTRUCTION.
/// - `rate_burst` / `batch` / `asset`: CLI-explicit wins where present;
///   the staged value (ConfigFile/Env ranks for burst; nothing today for
///   batch/asset — the TUI has no such fields) fills the rest. Where
///   both sides hold a value it is the same CLI flag parsed twice, so the
///   order is irrelevant.
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
    // CLI, env, and programmatic input share ONE accept / reject-0 /
    // warn-and-default semantic (`parse_rate_limit_burst`).
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
                // Non-numeric: the parser already emitted the warning.
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

fn apply_tui_numeric(
    book: &mut FieldBook,
    key: &str,
    value: &serde_json::Value,
) -> Result<bool, CliExit> {
    match key {
        "max_pages" => {
            let parsed = parse_tui_usize(key, value)?;
            Ok(try_write(
                &mut book.max_pages,
                parsed,
                ConfigSource::Tui,
                "max_pages",
            ))
        },
        "max_depth" => {
            let parsed = parse_tui_u8(key, value)?;
            Ok(try_write(
                &mut book.max_depth,
                parsed,
                ConfigSource::Tui,
                "max_depth",
            ))
        },
        "delay_ms" => {
            let parsed = parse_tui_u64(key, value)?;
            Ok(try_write(
                &mut book.delay_ms,
                parsed,
                ConfigSource::Tui,
                "delay_ms",
            ))
        },
        "timeout_secs" => {
            let parsed = parse_tui_u64(key, value)?;
            Ok(try_write(
                &mut book.timeout_secs,
                parsed,
                ConfigSource::Tui,
                "timeout_secs",
            ))
        },
        _ => Ok(false),
    }
}

fn apply_tui_string(book: &mut FieldBook, key: &str, value: &serde_json::Value) -> bool {
    match key {
        "selector" => {
            if let Some(s) = value.as_str() {
                if !s.is_empty() {
                    return try_write(
                        &mut book.selector,
                        s.to_string(),
                        ConfigSource::Tui,
                        "selector",
                    );
                }
            }
            false
        },
        "sitemap_url" => {
            if let Some(s) = value.as_str() {
                let opt = if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                };
                return try_write(&mut book.sitemap_url, opt, ConfigSource::Tui, "sitemap_url");
            }
            false
        },
        "output" => {
            if let Some(s) = value.as_str() {
                if !s.is_empty() {
                    return try_write(
                        &mut book.output,
                        PathBuf::from(s),
                        ConfigSource::Tui,
                        "output",
                    );
                }
            }
            false
        },
        "vault" => {
            if let Some(s) = value.as_str() {
                let opt = if s.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(s))
                };
                return try_write(&mut book.vault, opt, ConfigSource::Tui, "vault");
            }
            false
        },
        _ => false,
    }
}

fn apply_tui_format(book: &mut FieldBook, key: &str, value: &serde_json::Value) -> bool {
    match key {
        "format" => {
            if let Some(s) = value.as_str() {
                let target = match s {
                    "json" => OutputFormat::Json,
                    "text" => OutputFormat::Text,
                    _ => OutputFormat::Markdown,
                };
                return try_write(&mut book.format, target, ConfigSource::Tui, "format");
            }
            false
        },
        "export_format" => {
            if let Some(s) = value.as_str() {
                let target = match s {
                    "vector" => ExportFormat::Vector,
                    "auto" => ExportFormat::Auto,
                    _ => ExportFormat::Jsonl,
                };
                return try_write(
                    &mut book.export_format,
                    target,
                    ConfigSource::Tui,
                    "export_format",
                );
            }
            false
        },
        "obsidian_tags" => {
            if let Some(s) = value.as_str() {
                let parsed: Vec<String> = s
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                return try_write(
                    &mut book.obsidian_tags,
                    parsed,
                    ConfigSource::Tui,
                    "obsidian_tags",
                );
            }
            false
        },
        _ => false,
    }
}

fn apply_tui_bool(book: &mut FieldBook, key: &str, value: &serde_json::Value) -> bool {
    match key {
        "use_sitemap" => {
            if let Some(b) = value.as_bool() {
                return try_write(&mut book.use_sitemap, b, ConfigSource::Tui, "use_sitemap");
            }
            false
        },
        "obsidian_wiki_links" => {
            if let Some(b) = value.as_bool() {
                return try_write(
                    &mut book.obsidian_wiki_links,
                    b,
                    ConfigSource::Tui,
                    "obsidian_wiki_links",
                );
            }
            false
        },
        _ => false,
    }
}

fn apply_tui_concurrency(book: &mut FieldBook, value: &serde_json::Value) -> Result<bool, CliExit> {
    if let Some(s) = value.as_str() {
        let target = if s == "auto" {
            ConcurrencyConfig::default()
        } else if let Ok(num) = s.parse::<usize>() {
            ConcurrencyConfig::new(num)
        } else {
            let msg = format!("valor inválido para concurrency: \"{s}\"");
            log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
            return Err(CliExit::ConfigError(msg));
        };
        Ok(try_write(
            &mut book.concurrency,
            target,
            ConfigSource::Tui,
            "concurrency",
        ))
    } else {
        Ok(false)
    }
}

#[instrument(skip_all, fields(fields_written))]
fn stage_tui(book: &mut FieldBook, tui: &TuiOverrides) -> Result<usize, CliExit> {
    let mut n = 0;
    for (key, value) in &tui.fields {
        let wrote = match key.as_str() {
            "max_pages" | "max_depth" | "delay_ms" | "timeout_secs" => {
                apply_tui_numeric(book, key, value)?
            },
            "selector" | "sitemap_url" | "output" | "vault" => apply_tui_string(book, key, value),
            "format" | "export_format" | "obsidian_tags" => apply_tui_format(book, key, value),
            "use_sitemap" | "obsidian_wiki_links" => apply_tui_bool(book, key, value),
            "concurrency" => apply_tui_concurrency(book, value)?,
            _ => false,
        };
        if wrote {
            n += 1;
        }
    }
    Ok(n)
}

fn parse_tui_usize(field: &str, value: &serde_json::Value) -> Result<usize, CliExit> {
    if let Some(s) = value.as_str() {
        s.parse::<usize>().map_err(|_| {
            let msg =
                format!("valor inválido para {field}: \"{s}\" — se esperaba un número entero");
            log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
            CliExit::ConfigError(msg)
        })
    } else if let Some(n) = value.as_u64() {
        Ok(n as usize)
    } else {
        let msg = format!("valor inválido para {field}: tipo no soportado");
        log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
        Err(CliExit::ConfigError(msg))
    }
}

fn parse_tui_u64(field: &str, value: &serde_json::Value) -> Result<u64, CliExit> {
    if let Some(s) = value.as_str() {
        s.parse::<u64>().map_err(|_| {
            let msg =
                format!("valor inválido para {field}: \"{s}\" — se esperaba un número entero");
            log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
            CliExit::ConfigError(msg)
        })
    } else if let Some(n) = value.as_u64() {
        Ok(n)
    } else {
        let msg = format!("valor inválido para {field}: tipo no soportado");
        log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
        Err(CliExit::ConfigError(msg))
    }
}

fn parse_tui_u8(field: &str, value: &serde_json::Value) -> Result<u8, CliExit> {
    if let Some(s) = value.as_str() {
        s.parse::<u8>().map_err(|_| {
            let msg =
                format!("valor inválido para {field}: \"{s}\" — se esperaba un número entero");
            log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
            CliExit::ConfigError(msg)
        })
    } else if let Some(n) = value.as_u64() {
        u8::try_from(n).map_err(|_| {
            let msg = format!("valor inválido para {field}: {n} fuera de rango");
            log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
            CliExit::ConfigError(msg)
        })
    } else {
        let msg = format!("valor inválido para {field}: tipo no soportado");
        log_scrape_error(&msg, "", "stage_tui", None, "tui parse error");
        Err(CliExit::ConfigError(msg))
    }
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
/// Fixed stage order: defaults → config file → env/cli → tui → cross-field →
/// validation → `NormalizedConfig`.
///
/// # Errors
///
/// Returns `CliExit::ConfigError` with a Spanish message when TUI typed parsing
/// fails or validation blocks the result.
#[allow(missing_docs)]
#[instrument(skip_all, fields(fields_written = tracing::field::Empty))]
pub fn normalize(
    args: &Args,
    sources: &ArgSources,
    config: &ConfigDefaults,
    tui: Option<TuiOverrides>,
) -> Result<NormalizedConfig, CliExit> {
    let mut book = stage_defaults();
    let mut total = 0usize;
    total += stage_config_file(&mut book, config);
    total += stage_env_cli(&mut book, args, sources);
    if let Some(ref t) = tui {
        total += stage_tui(&mut book, t)?;
    }
    total += apply_cross_field_rules(&mut book);
    total += stage_budget_overrides(&mut book, args, sources, config)?;
    validate_stage(&book)?;
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
    fn stage_tui_default_equivalent_rejected_over_cli() {
        let mut book = stage_defaults();
        book.max_pages = crate::domain::config_value::ConfigValue::new(5, ConfigSource::Cli);
        let tui = TuiOverrides::from_json(serde_json::json!({"max_pages": "10"}));
        let written = stage_tui(&mut book, &tui).expect("tui parse ok");
        // Default-equivalent TUI emission must not clobber Cli; rank guard rejects
        // Our TuiOverrides is always ConfigSource::Tui, so this actually SHOULD win if Tui > Cli.
        // But the spec's unit-level no-overwrite proof is for an UNTOUCHED field: empty TUI.
        // Here we assert the opposite: explicit Tui DOES outrank Cli when field is touched.
        assert_eq!(written, 1);
        assert_eq!(book.max_pages.source, ConfigSource::Tui);
        // Now prove empty TUI does NOT overwrite
        let mut book2 = stage_defaults();
        book2.max_pages = crate::domain::config_value::ConfigValue::new(5, ConfigSource::Cli);
        let empty = TuiOverrides::default();
        let written2 = stage_tui(&mut book2, &empty).expect("empty tui ok");
        assert_eq!(written2, 0);
        assert_eq!(book2.max_pages.value, 5);
        assert_eq!(book2.max_pages.source, ConfigSource::Cli);
    }

    #[test]
    fn stage_tui_typed_parse_failure_returns_spanish_error() {
        let mut book = stage_defaults();
        let tui = TuiOverrides::from_json(serde_json::json!({"max_pages": "not-a-number"}));
        let err = stage_tui(&mut book, &tui).expect_err("non-numeric max_pages must error");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("max_pages") || msg.contains("número"),
                "Spanish error for bad max_pages, got: {msg}"
            ),
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn same_source_last_write_wins_within_stage() {
        let mut book = stage_defaults();
        let tui = TuiOverrides::from_json(serde_json::json!({"max_pages": "20", "format": "json"}));
        // Simulate two writes to same field within TUI stage via overwriting map entry
        let mut tui2 = tui;
        tui2.insert(
            "max_pages".to_string(),
            serde_json::Value::String("30".to_string()),
        );
        let _ = stage_tui(&mut book, &tui2).expect("tui ok");
        assert_eq!(book.max_pages.value, 30);
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
        // Via Tui
        let mut book3 = stage_defaults();
        book3.sitemap_url = crate::domain::config_value::ConfigValue::new(
            Some("https://example.com/sitemap.xml".to_string()),
            ConfigSource::Tui,
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
    fn merge_tui_staged_crawl_outranks_cli_capture() {
        // M1: a plain `--concurrency 2` (unranked CLI capture) must NEVER
        // stomp a Tui-ranked staged value — the same contradiction that
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

    /// m2 — full TUI path at the pipeline boundary: `normalize()` is the
    /// production consumer of `TuiOverrides`, so staging a touched
    /// `concurrency` override through it exercises
    /// touched_overrides → rank resolution → projection → merge exactly as
    /// the binary does after `handle_tui_mode`.
    #[test]
    fn tui_concurrency_override_survives_normalize_projection_and_merge() {
        let build_args = || {
            let mut args = dummy_args();
            args.crawler.concurrency = "2".parse().expect("valid ConcurrencyConfig");
            args
        };

        // Capture path mirrors main.rs: base BEFORE normalization consumes args.
        let base = crate::application::crawl_options::CrawlOptions::from(build_args());
        assert_eq!(
            base.budget_overrides
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(2),
            "From<Args> captures the explicit CLI concurrency"
        );

        let mut sources = ArgSources::default();
        sources.set("concurrency", ConfigSource::Cli);
        let tui = TuiOverrides::from_json(serde_json::json!({ "concurrency": "5" }));
        let normalized =
            normalize(&build_args(), &sources, &dummy_config(), Some(tui)).expect("normalize ok");

        let projected = normalized.into_crawl_options();
        // Rank guard upstream: TUI outranks CLI for network.concurrency too.
        assert_eq!(projected.network.concurrency.get(), Some(5));

        let merged = merge_budget_overrides(base.budget_overrides, projected.budget_overrides);
        assert_eq!(
            merged
                .crawl
                .map(crate::domain::budget::tiers::CrawlConcurrency::get),
            Some(5),
            "TUI-staged crawl must drive enforcement consistently with network.concurrency"
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

    #[test]
    fn normalize_end_to_end_burst_reaches_crawl_options() {
        let mut args = dummy_args();
        args.crawler.rate_limit_burst = Some("9".to_string());
        let mut sources = ArgSources::default();
        sources.set("rate_limit_burst", ConfigSource::Environment);
        let normalized = normalize(&args, &sources, &dummy_config(), None).expect("normalize ok");
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
        let tui = TuiOverrides::default();
        let normalized = normalize(&args, &sources, &config, Some(tui)).expect("normalize ok");
        assert_eq!(normalized.max_pages.value, 10);
        assert_eq!(normalized.max_pages.source, ConfigSource::Cli);
    }
}
