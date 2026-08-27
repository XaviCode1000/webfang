use crate::domain::config::{ExportFormat, OutputFormat, PipelineOutputFormat};
use crate::domain::options_spec::export as export_specs;

/// Export format and output configuration arguments.
///
/// Parsing stays derive-driven (`FromArgMatches`); command assembly is
/// spec-built (ADR-002 slice 3); see `cli::spec_command`.
#[derive(Debug, Default)]
pub struct ExportArgs {
    // ========== Output ==========
    /// Output directory for scraped content
    pub output: std::path::PathBuf,

    /// Output format for individual files (markdown, text, json)
    /// NOTE: For RAG pipeline export, use --export-format instead
    pub format: OutputFormat,

    /// Export format for RAG pipeline (jsonl, vector, auto)
    /// NOTE: Use --format for output file format (markdown, text, json)
    pub export_format: ExportFormat,

    // ========== Elastic Ingestion (Issue #51, PR5) ==========
    /// CPU core override for the elastic ingestion Rayon pool (else auto-detect)
    pub cpu_cores: Option<usize>,

    /// RAM budget override for the byte-weighted semaphore (`8GB`, `2048MB`, or bytes)
    pub ram_budget: Option<u64>,

    /// SQLite database path override for persisted resources/chunks
    pub db_path: Option<std::path::PathBuf>,

    /// Enable elastic ingestion pipeline (streaming, SQLite dedup, Rayon CPU bridge)
    pub elastic: bool,

    /// Write extracted vectors to a JSONL file for RAG pipelines. Use `-` for
    /// stdout. No SQLite dependency — available in every build (core binary too).
    pub output_vectors: Option<String>,

    // ========== Batch Processing ==========
    /// Enable batch mode — read URLs from stdin (one per line)
    pub batch: bool,

    /// Path to a file containing URLs to crawl (one per line)
    pub batch_file: Option<std::path::PathBuf>,

    /// Maximum concurrent URLs in batch mode (omit = auto from budget model)
    pub batch_concurrency: Option<usize>,

    // ========== Item Pipeline ==========
    /// Enable item pipeline processing (validate → clean → output)
    pub pipeline: bool,

    /// Pipeline output format: jsonl (default), none
    pub pipeline_output: PipelineOutputFormat,
}

impl clap::FromArgMatches for ExportArgs {
    fn from_arg_matches(m: &clap::ArgMatches) -> Result<Self, clap::Error> {
        use crate::cli::spec_command::extract;
        Ok(Self {
            output: extract::value(m, "output")?,
            format: extract::value(m, "format")?,
            export_format: extract::value(m, "export_format")?,
            cpu_cores: extract::opt(m, "cpu_cores"),
            ram_budget: extract::opt(m, "ram_budget"),
            db_path: extract::opt(m, "db_path"),
            elastic: m.get_flag("elastic"),
            output_vectors: extract::opt(m, "output_vectors"),
            batch: m.get_flag("batch"),
            batch_file: extract::opt(m, "batch_file"),
            batch_concurrency: extract::opt(m, "batch_concurrency"),
            pipeline: m.get_flag("pipeline"),
            pipeline_output: extract::value(m, "pipeline_output")?,
        })
    }

    fn update_from_arg_matches(&mut self, m: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(m)?;
        Ok(())
    }
}

impl clap::Args for ExportArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.args(crate::cli::spec_command::export_args(
            crate::cli::spec_command::Headings::Applied,
        ))
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_args(cmd)
    }
}

/// Validate `--cpu-cores` is a positive integer.
///
/// A zero core count would size the Rayon pool to nothing; rejecting it at the
/// system boundary keeps the invalid value out of the autotuning resolver
/// (#653). Bounds and messages come from the OptionsSpec (ADR-002) — the
/// single validation source.
pub(crate) fn parse_cpu_cores(s: &str) -> Result<usize, String> {
    let value = export_specs::CPU_CORES
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| export_specs::CPU_CORES.parse_error(s).to_string())
}

/// Parse and validate `--ram-budget` into bytes.
///
/// Accepts plain bytes or a binary suffix (`8GB`, `2048MB`). An unparseable or
/// zero budget is rejected here instead of being silently dropped by
/// `Option::and_then` further down the pipeline (#653). Suffix parsing stays
/// in `infrastructure::autotuning` (layering); the bound and messages come
/// from the OptionsSpec (ADR-002).
pub(crate) fn parse_ram_budget(s: &str) -> Result<u64, String> {
    let value = crate::infrastructure::autotuning::parse_ram_bytes(s)
        .ok_or_else(|| export_specs::RAM_BUDGET.parse_error(s).to_string())?;
    // Issue #948 F7: `check_bound` now returns a typed `BoundError`. The
    // CLI parity path needs the Spanish verbatim message, so we look up
    // the spec policy here (one access, locally scoped).
    let policy = match export_specs::RAM_BUDGET.kind {
        crate::domain::options_spec::ValueKind::MemorySize { policy: Some(p) } => p,
        _ => unreachable!("RAM_BUDGET is MemorySize with policy"),
    };
    export_specs::RAM_BUDGET
        .check_bound(value)
        .map_err(|bound| match bound {
            crate::domain::options_spec::BoundError::MinViolated { .. } => {
                policy.below_min_message.to_owned()
            },
            crate::domain::options_spec::BoundError::MaxViolated { .. } => {
                policy.above_max_message.to_owned()
            },
        })
}

/// Validate `--batch-concurrency` is greater than zero.
///
/// Clap's `value_parser!(usize)` does not expose `.range()` in the derive API,
/// so a spec-driven validator enforces the invariant at the system boundary
/// (#640, ADR-002).
pub(crate) fn parse_batch_concurrency(s: &str) -> Result<usize, String> {
    let value = export_specs::BATCH_CONCURRENCY
        .parse_uint(s)
        .map_err(|e| e.to_string())?;
    usize::try_from(value).map_err(|_| export_specs::BATCH_CONCURRENCY.parse_error(s).to_string())
}

#[cfg(test)]
mod spec_parity_tests {
    //! ADR-002 equivalence proof (slice 1): the hand-derived clap surface of
    //! [`ExportArgs`] must stay in lockstep with the OptionsSpec export
    //! group. These tests pin the CURRENT behavior first; the migration then
    //! routes validation through the spec and must keep them green.

    use super::*;
    use crate::domain::options_spec as spec;
    use clap::Args as _;

    /// All clap args generated for `ExportArgs`, keyed by arg id.
    fn command_args() -> Vec<clap::Arg> {
        // `#[derive(Args)]` augments a parent command; wrap it to inspect the
        // exact arg set the flatten produces inside `crate::Args`.
        ExportArgs::augment_args(clap::Command::new("webfang-export"))
            .get_arguments()
            .cloned()
            .collect()
    }

    fn arg_by_id<'a>(args: &'a [clap::Arg], id: &str) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from ExportArgs command"))
    }

    fn parse_args(extra: &[&str]) -> Result<crate::Args, String> {
        let mut argv = vec!["webfang"];
        argv.extend_from_slice(extra);
        clap::Parser::try_parse_from(argv).map_err(|e| e.to_string())
    }

    /// Bare parse with clap `env` fallbacks neutralized — see
    /// [`crate::cli::args::test_support::with_clap_env_cleared`] (issue #926).
    fn parse_args_hermetic(extra: &[&str]) -> Result<crate::Args, String> {
        crate::cli::args::test_support::with_clap_env_cleared(|| parse_args(extra))
    }

    #[test]
    fn clap_surface_is_fully_covered_by_the_spec() {
        let args = command_args();
        for arg in &args {
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }
            assert!(
                spec::export::GROUP.iter().any(|s| s.id == arg.get_id()),
                "clap arg `{}` has no OptionsSpec entry — spec is out of sync",
                arg.get_id()
            );
        }
    }

    #[test]
    fn long_short_aliases_env_and_heading_match_the_spec() {
        let args = command_args();
        for s in spec::export::GROUP {
            let arg = arg_by_id(&args, s.id);
            assert_eq!(arg.get_long(), Some(s.long), "long mismatch for `{}`", s.id);
            assert_eq!(arg.get_short(), s.short, "short mismatch for `{}`", s.id);
            let aliases = arg.get_aliases().unwrap_or_default();
            assert_eq!(aliases, s.aliases, "alias mismatch for `{}`", s.id);
            let env = arg.get_env().map(|e| e.to_string_lossy().into_owned());
            assert_eq!(env.as_deref(), s.env, "env var mismatch for `{}`", s.id);
            // NOTE: clap's `next_help_heading` is stateful during command
            // construction and is NOT stored per-Arg, so it cannot be
            // introspected here. `spec.heading` is generator-facing data —
            // exercised when a later slice BUILDS commands from specs.
        }
    }

    #[test]
    fn defaults_match_the_spec() {
        let args = command_args();
        for s in spec::export::GROUP {
            let arg = arg_by_id(&args, s.id);
            let defaults: Vec<String> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let expected: Vec<String> = s.default.map(|d| vec![d.to_string()]).unwrap_or_default();
            assert_eq!(defaults, expected, "default mismatch for `{}`", s.id);
        }
    }

    #[test]
    fn help_text_matches_the_spec() {
        let args = command_args();
        for s in spec::export::GROUP {
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
        let defaults = parse_args_hermetic(&[]).expect("bare invocation must parse");
        assert_eq!(defaults.export.output, std::path::PathBuf::from("output"));
        assert_eq!(
            defaults.export.format,
            crate::domain::config::OutputFormat::Markdown
        );
        assert_eq!(
            defaults.export.export_format,
            crate::domain::config::ExportFormat::Jsonl
        );
        assert!(!defaults.export.elastic && !defaults.export.batch && !defaults.export.pipeline);

        // Explicit values, short forms, and the `--export` alias. `--output`
        // and its `-o` short form are exercised in separate invocations (an
        // option may only be used once per parse).
        let parsed = parse_args(&[
            "--output",
            "custom-dir",
            "-f",
            "text",
            "--export-format",
            "vector",
            "--cpu-cores",
            "4",
            "--ram-budget",
            "8GB",
            "--db-path",
            "/tmp/wf.db",
            "--elastic",
            "--output-vectors",
            "-",
            "--batch-file",
            "urls.txt",
            "--pipeline-output",
            "none",
        ])
        .expect("representative export flags must parse");
        assert_eq!(parsed.export.output, std::path::PathBuf::from("custom-dir"));
        assert_eq!(
            parsed.export.format,
            crate::domain::config::OutputFormat::Text
        );
        assert_eq!(
            parsed.export.export_format,
            crate::domain::config::ExportFormat::Vector
        );
        assert_eq!(parsed.export.cpu_cores, Some(4));
        assert_eq!(parsed.export.ram_budget, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(
            parsed.export.db_path,
            Some(std::path::PathBuf::from("/tmp/wf.db"))
        );
        assert!(parsed.export.elastic);
        assert_eq!(parsed.export.output_vectors.as_deref(), Some("-"));
        assert_eq!(
            parsed.export.batch_file,
            Some(std::path::PathBuf::from("urls.txt"))
        );
        assert_eq!(
            parsed.export.pipeline_output,
            crate::domain::config::PipelineOutputFormat::None
        );

        let shorts = parse_args(&["-o", "short-dir"]).expect("short forms must parse");
        assert_eq!(shorts.export.output, std::path::PathBuf::from("short-dir"));

        let alias = parse_args(&["--export", "auto"]).expect("`--export` alias must parse");
        assert_eq!(
            alias.export.export_format,
            crate::domain::config::ExportFormat::Auto
        );
    }

    /// Slice 3 (ADR-002) pin: the structural clap surface that the
    /// spec-driven builder must reproduce byte-for-byte. Written against
    /// the derive BEFORE the migration so any builder divergence fails
    /// here first.
    #[test]
    fn structural_actions_value_names_and_possible_values_match_the_spec() {
        let args = command_args();
        for s in spec::export::GROUP {
            let arg = arg_by_id(&args, s.id);
            match s.kind {
                spec::ValueKind::Bool => {
                    assert!(
                        matches!(arg.get_action(), clap::ArgAction::SetTrue),
                        "bool `{}` must use SetTrue",
                        s.id
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
                // Bools carry clap's implicit boolish parser (possible
                // values `true,false`) — pinned in the crawler suite.
                assert!(
                    possible.is_empty(),
                    "`{}` must have no possible values",
                    s.id
                );
            }
            // Single-form help only: no separate long help exists today,
            // so `--help` and `-h` render the same text per option.
            assert!(
                arg.get_long_help().is_none(),
                "`{}` must not carry long help",
                s.id
            );
        }
    }

    #[test]
    fn out_of_bounds_and_malformed_inputs_error_exactly_as_before() {
        let err = parse_args(&["--cpu-cores", "0"]).expect_err("zero cores rejected");
        assert!(err.contains("cpu-cores debe ser > 0"), "got: {err}");

        let err = parse_args(&["--cpu-cores", "abc"]).expect_err("text rejected");
        assert!(
            err.contains("`abc` no es un número entero válido"),
            "got: {err}"
        );

        let err = parse_args(&["--ram-budget", "nope"]).expect_err("bad size rejected");
        assert!(
            err.contains("`nope` no es un tamaño de memoria válido"),
            "got: {err}"
        );

        let err = parse_args(&["--ram-budget", "0"]).expect_err("zero budget rejected");
        assert!(err.contains("ram-budget debe ser > 0"), "got: {err}");

        let err = parse_args(&["--batch-concurrency", "0"]).expect_err("zero concurrency rejected");
        assert!(err.contains("batch-concurrency debe ser > 0"), "got: {err}");
    }
}
