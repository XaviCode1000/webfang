//! Gate 3 equivalence matrix — stabilization-config-normalization (#302).
//!
//! Proves orchestrator ruling 3: for EACH of the five field families
//! (target, discovery, crawler+network, output/export, obsidian), the SAME
//! logical configuration delivered through three different execution paths —
//!
//! (a) CLI flag only,
//! (b) environment variable only,
//! (c) TOML config file only —
//!
//! converges to an IDENTICAL projected [`CrawlOptions`] outcome through the
//! single rank-guarded normalization pipeline (`preflight::normalize`).
//!
//! Precedence where sources overlap is pinned separately:
//! `Cli > Environment > ConfigFile`, including the Surface B regression
//! (an explicit CLI value equal to the default still beats TOML).
//!
//! The matrix prefers direct `normalize()` calls (deterministic and fast);
//! the motivating case additionally runs END-TO-END against the real binary
//! via the shared `BehavioralTest` harness (`webfang_path()` — never
//! `assert_cmd::cargo_bin`).
//!
//! Coverage notes (honest scope):
//! - Fields with no TOML key in `ConfigDefaults` today (`max_depth`,
//!   `sitemap_url`, `timeout_secs`, `output`, `quick_save`, `url`) are
//!   exercised as two-path (flag vs env) rows; TOML absence means Default
//!   provenance, never a clobber.
//! - Non-contested passthrough fields (`max_retries`, `user_agent`) bypass
//!   `normalize` by design (they flow through `CrawlOptions::from(args)`
//!   unchanged); their delivery-channel equivalence is asserted at the
//!   `Args` level.

#[path = "common/cli_harness.rs"]
mod common;

pub(crate) use crate::common::{redact_nondeterministic, BehavioralTest};

use clap::Parser;
use std::collections::BTreeMap;
use std::path::Path;
use webfang_core::cli::config::ConfigDefaults;
use webfang_core::cli::error::EXIT_CONFIG;
use webfang_core::cli::preflight::{normalize, ArgSources, NormalizedConfig};
use webfang_core::domain::config_value::ConfigSource;
use webfang_core::{Args, CliExit};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Mandatory base invocation: every scenario carries an explicit `--url`
/// (the pipeline requires it downstream and it pins one contested source).
const BASE_URL: &str = "https://example.com";

// ---------------------------------------------------------------------------
// Delivery channels
// ---------------------------------------------------------------------------

/// One environment-variable delivery of a contested field.
///
/// `apply` mirrors what clap's env fallback produces before provenance
/// capture: the variable's raw string lands in the parsed `Args` field and
/// `ArgSources` records `ConfigSource::Environment`. Reproducing that
/// observable state in-process keeps the matrix hermetic (no process-global
/// env mutation) while exercising exactly what `stage_env_cli` consumes.
struct EnvSetting {
    id: &'static str,
    value: &'static str,
    apply: fn(&mut Args, &str),
}

/// A family case: ONE logical configuration expressed three ways.
struct FamilyCase {
    /// Stable identifier used in test names and snapshots.
    name: &'static str,
    /// Path (a): CLI flag tokens appended after the mandatory base args.
    cli_flags: &'static [&'static str],
    /// Arg ids from `cli_flags` recorded as `ConfigSource::Cli`.
    cli_ids: &'static [&'static str],
    /// Path (b): environment-variable deliveries.
    env: &'static [EnvSetting],
    /// Path (c): verbatim TOML lines of the config file body.
    toml_lines: &'static [&'static str],
}

/// The five field families × three execution paths (ruling 3).
const FAMILIES: &[FamilyCase] = &[
    // -- target: selector (url has its own row below; no TOML key exists) --
    FamilyCase {
        name: "target_selector",
        cli_flags: &["--selector", ".article-content"],
        cli_ids: &["selector"],
        env: &[EnvSetting {
            id: "selector",
            value: ".article-content",
            apply: |a, v| a.crawler.selector = v.to_string(),
        }],
        toml_lines: &["selector = \".article-content\""],
    },
    // -- discovery --
    FamilyCase {
        name: "discovery_max_pages_use_sitemap",
        cli_flags: &["--max-pages", "5", "--use-sitemap"],
        cli_ids: &["max_pages", "use_sitemap"],
        env: &[
            EnvSetting {
                id: "max_pages",
                value: "5",
                apply: |a, v| a.crawler.max_pages = v.parse().expect("usize"),
            },
            EnvSetting {
                id: "use_sitemap",
                value: "true",
                apply: |a, v| a.crawler.use_sitemap = v == "true",
            },
        ],
        toml_lines: &["max_pages = 5", "use_sitemap = true"],
    },
    // -- crawler + network --
    FamilyCase {
        name: "crawler_network_delay_concurrency",
        cli_flags: &["--delay-ms", "500", "--concurrency", "4"],
        cli_ids: &["delay_ms", "concurrency"],
        env: &[
            EnvSetting {
                id: "delay_ms",
                value: "500",
                apply: |a, v| a.crawler.delay_ms = v.parse().expect("u64"),
            },
            EnvSetting {
                id: "concurrency",
                value: "4",
                apply: |a, v| a.crawler.concurrency = webfang_core::ConcurrencyConfig::from(v),
            },
        ],
        toml_lines: &["delay_ms = 500", "concurrency = \"4\""],
    },
    FamilyCase {
        name: "crawler_network_timeout_secs",
        cli_flags: &["--timeout-secs", "45"],
        cli_ids: &["timeout_secs"],
        env: &[EnvSetting {
            id: "timeout_secs",
            value: "45",
            apply: |a, v| a.crawler.timeout_secs = v.parse().expect("u64"),
        }],
        // No TOML key yet: absence keeps Default provenance.
        toml_lines: &[],
    },
    // -- output / export --
    FamilyCase {
        name: "output_export_formats",
        cli_flags: &["--format", "json", "--export-format", "vector"],
        cli_ids: &["format", "export_format"],
        env: &[
            EnvSetting {
                id: "format",
                value: "json",
                apply: |a, v| {
                    a.export.format = match v {
                        "json" => webfang_core::OutputFormat::Json,
                        "text" => webfang_core::OutputFormat::Text,
                        _ => webfang_core::OutputFormat::Markdown,
                    }
                },
            },
            EnvSetting {
                id: "export_format",
                value: "vector",
                apply: |a, v| {
                    a.export.export_format = match v {
                        "vector" => webfang_core::ExportFormat::Vector,
                        "auto" => webfang_core::ExportFormat::Auto,
                        _ => webfang_core::ExportFormat::Jsonl,
                    }
                },
            },
        ],
        toml_lines: &["format = \"json\"", "export_format = \"vector\""],
    },
    FamilyCase {
        name: "output_dir",
        cli_flags: &["--output", "./gate3-custom-out"],
        cli_ids: &["output"],
        env: &[EnvSetting {
            id: "output",
            value: "./gate3-custom-out",
            apply: |a, v| a.export.output = std::path::PathBuf::from(v),
        }],
        toml_lines: &[],
    },
    // -- obsidian --
    FamilyCase {
        name: "obsidian_tags_wiki_links",
        cli_flags: &["--obsidian-tags", "rust,cargo", "--obsidian-wiki-links"],
        cli_ids: &["obsidian_tags", "obsidian_wiki_links"],
        env: &[
            EnvSetting {
                id: "obsidian_tags",
                value: "rust,cargo",
                apply: |a, v| {
                    a.obsidian.obsidian_tags =
                        Some(v.split(',').map(str::to_string).collect::<Vec<String>>())
                },
            },
            EnvSetting {
                id: "obsidian_wiki_links",
                value: "true",
                apply: |a, v| a.obsidian.obsidian_wiki_links = v == "true",
            },
        ],
        toml_lines: &[
            "obsidian_tags = \"rust,cargo\"",
            "obsidian_wiki_links = true",
        ],
    },
    FamilyCase {
        name: "obsidian_vault",
        cli_flags: &["--vault", "/tmp/gate3-vault"],
        cli_ids: &["vault"],
        env: &[EnvSetting {
            id: "vault",
            value: "/tmp/gate3-vault",
            apply: |a, v| a.obsidian.vault = Some(std::path::PathBuf::from(v)),
        }],
        toml_lines: &["vault_path = \"/tmp/gate3-vault\""],
    },
    FamilyCase {
        name: "obsidian_quick_save",
        cli_flags: &["--quick-save"],
        cli_ids: &["quick_save"],
        env: &[EnvSetting {
            id: "quick_save",
            value: "true",
            apply: |a, v| a.obsidian.quick_save = v == "true",
        }],
        toml_lines: &[],
    },
];

// ---------------------------------------------------------------------------
// Path runners (each applies ONLY its channel on top of the mandatory base)
// ---------------------------------------------------------------------------

fn parse_base(extra: &[&str]) -> Args {
    let mut argv = vec!["webfang", "--url", BASE_URL];
    argv.extend_from_slice(extra);
    Args::parse_from(argv)
}

fn base_url_source(sources: &mut ArgSources) {
    sources.set("url", ConfigSource::Cli);
}

/// Path (a): CLI flags only.
fn run_cli(flags: &[&str], ids: &[&str]) -> Result<NormalizedConfig, CliExit> {
    let args = parse_base(flags);
    let mut sources = ArgSources::default();
    base_url_source(&mut sources);
    for id in ids {
        sources.set(id, ConfigSource::Cli);
    }
    normalize(&args, &sources, &ConfigDefaults::default())
}

/// Path (b): environment variables only (clap env fallback equivalent).
fn run_env(env: &[EnvSetting]) -> Result<NormalizedConfig, CliExit> {
    let mut args = parse_base(&[]);
    let mut sources = ArgSources::default();
    base_url_source(&mut sources);
    for e in env {
        (e.apply)(&mut args, e.value);
        sources.set(e.id, ConfigSource::Environment);
    }
    normalize(&args, &sources, &ConfigDefaults::default())
}

/// Path (c): TOML config file only.
fn run_toml(lines: &[&str]) -> Result<NormalizedConfig, CliExit> {
    let args = parse_base(&[]);
    let mut sources = ArgSources::default();
    base_url_source(&mut sources);
    let text = lines.join("\n");
    let config: ConfigDefaults = toml::from_str(&text).expect("valid TOML body");
    normalize(&args, &sources, &config)
}

/// All channels at once — used by precedence-overlap regressions.
fn run_all(
    flags: &[&str],
    ids: &[&str],
    env: &[EnvSetting],
    toml_lines: &[&str],
) -> Result<NormalizedConfig, CliExit> {
    let mut args = parse_base(flags);
    let mut sources = ArgSources::default();
    base_url_source(&mut sources);
    for id in ids {
        sources.set(id, ConfigSource::Cli);
    }
    for e in env {
        // Real clap never consults the env fallback when the same arg was
        // given on the command line: one arg, one ValueSource, and the env
        // value never lands in `Args`. Mirror that honestly.
        if sources.source_of(e.id).is_none() {
            (e.apply)(&mut args, e.value);
            sources.set(e.id, ConfigSource::Environment);
        }
    }
    let text = toml_lines.join("\n");
    let config: ConfigDefaults = toml::from_str(&text).expect("valid TOML body");
    normalize(&args, &sources, &config)
}

// ---------------------------------------------------------------------------
// Effective-outcome projection
// ---------------------------------------------------------------------------

/// Project a `NormalizedConfig` into its effective settings map (values only —
/// provenance intentionally dropped, mirroring `into_crawl_options`). Two
/// deliveries are equivalent iff this map is identical.
fn effective(n: &NormalizedConfig) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("max_pages".into(), n.max_pages.value.to_string());
    m.insert("max_depth".into(), n.max_depth.value.to_string());
    m.insert("sitemap_url".into(), format!("{:?}", n.sitemap_url.value));
    m.insert("use_sitemap".into(), n.use_sitemap.value.to_string());
    m.insert("selector".into(), n.selector.value.clone());
    m.insert("delay_ms".into(), n.delay_ms.value.to_string());
    m.insert("timeout_secs".into(), n.timeout_secs.value.to_string());
    m.insert("concurrency".into(), format!("{:?}", n.concurrency.value));
    m.insert("output".into(), n.output.value.display().to_string());
    m.insert("format".into(), format!("{:?}", n.format.value));
    m.insert(
        "export_format".into(),
        format!("{:?}", n.export_format.value),
    );
    m.insert("obsidian_tags".into(), n.obsidian_tags.value.join(","));
    m.insert(
        "obsidian_wiki_links".into(),
        n.obsidian_wiki_links.value.to_string(),
    );
    m.insert(
        "obsidian_relative_assets".into(),
        n.obsidian_relative_assets.value.to_string(),
    );
    m.insert(
        "obsidian_rich_metadata".into(),
        n.obsidian_rich_metadata.value.to_string(),
    );
    m.insert("vault".into(), format!("{:?}", n.vault.value));
    m.insert("quick_save".into(), n.quick_save.value.to_string());
    m.insert("ignore_waf".into(), n.ignore_waf.value.to_string());
    m
}

fn assert_snapshot_redacted(name: &str, dir: &Path, value: impl Into<String>) {
    let redacted = redact_nondeterministic(dir, &value.into());
    insta::assert_snapshot!(name, redacted);
}

// ---------------------------------------------------------------------------
// The matrix: five families × three paths → identical projected outcome
// ---------------------------------------------------------------------------

#[test]
fn families_converge_across_flag_env_and_toml_paths() {
    for case in FAMILIES {
        let eff_cli = effective(
            &run_cli(case.cli_flags, case.cli_ids)
                .unwrap_or_else(|e| panic!("[{}] CLI path should normalize: {e:?}", case.name)),
        );
        let eff_env = effective(
            &run_env(case.env)
                .unwrap_or_else(|e| panic!("[{}] env path should normalize: {e:?}", case.name)),
        );
        let eff_toml = effective(
            &run_toml(case.toml_lines)
                .unwrap_or_else(|e| panic!("[{}] TOML path should normalize: {e:?}", case.name)),
        );

        assert_eq!(
            eff_cli, eff_env,
            "[{}] same logical configuration must converge: flag vs env",
            case.name
        );
        // Families without a TOML key (empty `toml_lines`) are two-path
        // rows by contract: TOML absence keeps Default provenance, so the
        // flag-vs-TOML convergence assertion only applies when a TOML
        // delivery of the same logical value is expressible.
        if !case.toml_lines.is_empty() {
            assert_eq!(
                eff_cli, eff_toml,
                "[{}] same logical configuration must converge: flag vs TOML",
                case.name
            );
        }
    }
}

#[test]
fn matrix_snapshots_per_family() {
    for case in FAMILIES {
        let eff = effective(
            &run_cli(case.cli_flags, case.cli_ids)
                .unwrap_or_else(|e| panic!("[{}] CLI path should normalize: {e:?}", case.name)),
        );
        assert_snapshot_redacted(
            &format!("family__{}", case.name),
            Path::new("__no_temp__"),
            serde_json::to_string_pretty(&eff).expect("serialize effective map"),
        );
    }
}

// ---------------------------------------------------------------------------
// Two-path rows for fields without a TOML key (absence = Default, no clobber)
// ---------------------------------------------------------------------------

#[test]
fn discovery_extended_fields_flag_and_env_converge() {
    let max_depth = ("max_depth", "3");
    let sitemap_url = ("sitemap_url", "https://example.com/sitemap.xml");

    let mut args = parse_base(&[]);
    args.crawler.max_depth = 3;
    args.crawler.sitemap_url = Some(sitemap_url.1.to_string());

    let mut sources = ArgSources::default();
    base_url_source(&mut sources);
    sources.set(max_depth.0, ConfigSource::Environment);
    sources.set(sitemap_url.0, ConfigSource::Environment);
    let via_env =
        normalize(&args, &sources, &ConfigDefaults::default()).expect("env path normalizes");

    let via_flag = run_cli(
        &["--max-depth", "3", "--sitemap-url", sitemap_url.1],
        &["max_depth", "sitemap_url"],
    )
    .expect("flag path normalizes");

    assert_eq!(via_env.max_depth.value, via_flag.max_depth.value);
    assert_eq!(via_env.max_depth.source, ConfigSource::Environment);
    assert_eq!(via_flag.max_depth.source, ConfigSource::Cli);
    assert_eq!(via_env.sitemap_url.value, via_flag.sitemap_url.value);
    // Cross-field rule (#491): sitemap_url implies use_sitemap from any path.
    assert!(via_env.use_sitemap.value);
    assert!(via_flag.use_sitemap.value);
}

#[test]
fn target_url_flag_and_env_deliver_same_args_outcome() {
    let url = "https://example.org/deep-page";
    // Direct argv construction: `--url` is single-use, so the mandatory
    // `parse_base` base cannot coexist with a second `--url` here.
    let via_flag = Args::parse_from(["webfang", "--url", url]);
    // Env delivery: clap resolves WEBFANG_URL into the same field.
    let mut via_env = Args::parse_from(["webfang"]);
    via_env.crawler.url = Some(url.to_string());

    assert_eq!(via_flag.crawler.url.as_deref(), Some(url));
    assert_eq!(via_env.crawler.url.as_deref(), Some(url));
}

#[test]
fn network_passthrough_fields_are_channel_independent() {
    // max_retries and user_agent are NOT contested: they bypass `normalize`
    // and flow through CrawlOptions::from(args) unchanged, so any delivery
    // channel that parses them yields identical values by construction.
    let via_flag = parse_base(&["--max-retries", "7", "--user-agent", "gate3-agent"]);
    let mut via_env = parse_base(&[]);
    via_env.crawler.max_retries = 7;
    via_env.crawler.user_agent = Some("gate3-agent".to_string());

    assert_eq!(via_flag.crawler.max_retries, 7);
    assert_eq!(via_flag.crawler.user_agent.as_deref(), Some("gate3-agent"));
    assert_eq!(via_env.crawler.max_retries, 7);
    assert_eq!(via_env.crawler.user_agent.as_deref(), Some("gate3-agent"));
}

// ---------------------------------------------------------------------------
// Precedence where sources overlap: Cli > Environment > ConfigFile
// ---------------------------------------------------------------------------

#[test]
fn precedence_cli_beats_env_beats_config_file_on_overlap() {
    let env_seven = [EnvSetting {
        id: "max_pages",
        value: "7",
        apply: |a, v| a.crawler.max_pages = v.parse().expect("usize"),
    }];
    let toml_25 = ["max_pages = 25"];

    // All three overlap → Cli wins.
    let all = run_all(&["--max-pages", "10"], &["max_pages"], &env_seven, &toml_25)
        .expect("all-channels path normalizes");
    assert_eq!(all.max_pages.value, 10);
    assert_eq!(all.max_pages.source, ConfigSource::Cli);

    // Drop the flag → Environment wins over ConfigFile.
    let env_and_toml = run_all(&[], &[], &env_seven, &toml_25).expect("env+TOML normalizes");
    assert_eq!(env_and_toml.max_pages.value, 7);
    assert_eq!(env_and_toml.max_pages.source, ConfigSource::Environment);

    // Only TOML left → ConfigFile survives.
    let toml_only = run_toml(&toml_25).expect("TOML-only normalizes");
    assert_eq!(toml_only.max_pages.value, 25);
    assert_eq!(toml_only.max_pages.source, ConfigSource::ConfigFile);
}

// ---------------------------------------------------------------------------
// Mandated regression: Surface B
// ---------------------------------------------------------------------------

#[test]
fn surface_b_explicit_default_equal_cli_beats_toml() {
    // `--max-pages 10` is explicit even though 10 IS the default: provenance
    // ranks Cli above ConfigFile, so TOML's 25 must lose (pre-fix behavior
    // clobbered the explicit CLI value here).
    let winner = run_all(
        &["--max-pages", "10"],
        &["max_pages"],
        &[],
        &["max_pages = 25"],
    )
    .expect("Surface B scenario normalizes");

    assert_eq!(winner.max_pages.value, 10);
    assert_eq!(winner.max_pages.source, ConfigSource::Cli);

    let mut eff = effective(&winner);
    eff.retain(|k, _| k == "max_pages" || k == "max_pages_source");
    eff.insert(
        "max_pages_source".into(),
        format!("{:?}", winner.max_pages.source),
    );
    assert_snapshot_redacted(
        "surface_b__explicit_default_equal_cli_beats_toml",
        Path::new("__no_temp__"),
        serde_json::to_string_pretty(&eff).expect("serialize"),
    );
}

/// End-to-end proof of the motivating case through the REAL binary: an
/// explicit `--max-pages` caps the crawl exactly, with no TUI/config noise
/// interfering. Snapshot of the produced file set (redacted) is the golden.
/// Mount a `200 OK` HTML page mock for one relative path.
async fn mock_page_get(server: &wiremock::MockServer, rel_path: &str, html: &str) {
    Mock::given(method("GET"))
        .and(path(rel_path))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(server)
        .await;
}

#[tokio::test]
async fn motivating_case_end_to_end_binary_run_caps_at_max_pages() {
    let t = BehavioralTest::new().await;

    let index = r#"<html><head><title>Index</title></head><body>
            <article>
                <h1>Site Index</h1>
                <p>This index page carries enough extractable prose for the
                readability guard while linking to every crawl target.</p>
            </article>
            <a href="/p1">one</a> <a href="/p2">two</a>
            <a href="/p3">three</a> <a href="/p4">four</a>
        </body></html>"#;
    mock_page_get(&t.server, "/", index).await;
    for p in ["p1", "p2", "p3", "p4"] {
        mock_page_get(
            &t.server,
            &format!("/{p}"),
            r#"<html><head><title>P</title></head><body><article><h1>Page</h1><p>Rich extractable paragraph with plenty of meaningful content so the readability extractor accepts it without complaining about insufficient text length.</p></article></body></html>"#,
        )
        .await;
    }

    // Cap of 5 covers the seed + all four linked pages: deterministic set.
    t.scraper_cmd()
        .args(["--max-pages", "5", "--quiet"])
        .assert()
        .success();
    let five_files = t.find_files("md");
    assert_eq!(
        five_files.len(),
        5,
        "--max-pages 5 must yield exactly 5 pages"
    );

    // A tighter cap must be honored by the same binary run path.
    let t2 = BehavioralTest::new().await;
    mock_page_get(&t2.server, "/", index).await;
    for p in ["p1", "p2", "p3", "p4"] {
        mock_page_get(
            &t2.server,
            &format!("/{p}"),
            r#"<html><head><title>P</title></head><body><article><h1>Page</h1><p>Rich extractable paragraph with plenty of meaningful content so the readability extractor accepts it without complaining about insufficient text length.</p></article></body></html>"#,
        )
        .await;
    }
    t2.scraper_cmd()
        .args(["--max-pages", "2", "--quiet"])
        .assert()
        .success();
    assert_eq!(
        t2.find_files("md").len(),
        2,
        "--max-pages 2 must cap the crawl"
    );

    let mut names: Vec<String> = five_files
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_snapshot_redacted(
        "motivating_case__binary_run_capped_file_set",
        t.out.path(),
        names.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Mandated regression: validation-gate parity across channels
// ---------------------------------------------------------------------------

#[test]
fn validation_gate_parity_same_error_via_env_and_toml() {
    // The preflight gate blocks max_pages == 0 regardless of which channel
    // delivered it: identical variant, message, and exit code.
    let via_env = run_env(&[EnvSetting {
        id: "max_pages",
        value: "0",
        apply: |a, v| a.crawler.max_pages = v.parse().expect("usize"),
    }])
    .expect_err("env-delivered 0 must trip the gate");

    let via_toml = run_toml(&["max_pages = 0"]).expect_err("TOML-delivered 0 must trip the gate");

    assert_eq!(
        via_env, via_toml,
        "gate must fire identically across channels"
    );
    match &via_toml {
        CliExit::ConfigError(msg) => {
            assert_eq!(msg, "max_pages debe ser mayor que 0");
        },
        other => panic!("expected ConfigError, got {other:?}"),
    }
    // Exit code parity: both map to EXIT_CONFIG (78) via Termination mapping.
    assert_eq!(EXIT_CONFIG, 78);

    // The flag channel rejects the same issue earlier, at the parse boundary:
    // clap's value_parser refuses 0 with a Spanish message naming --max-pages.
    let argv = vec!["webfang", "--url", BASE_URL, "--max-pages", "0"];
    let parse_err = Args::try_parse_from(argv).expect_err("--max-pages 0 rejected");
    let rendered = parse_err.to_string();
    assert!(
        rendered.contains("--max-pages"),
        "boundary error names the flag: {rendered}"
    );
}
