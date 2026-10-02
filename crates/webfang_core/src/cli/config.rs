//! CLI Configuration Module
//!
//! T-010, T-011, T-012: Configuration defaults loading, NO_COLOR support.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

/// Default configuration values that can be overridden by a TOML file.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ConfigDefaults {
    /// Default output format (markdown, text, json)
    pub format: Option<String>,
    /// Default export format (jsonl, vector, auto)
    pub export_format: Option<String>,
    /// Default concurrency level (number or "auto")
    pub concurrency: Option<String>,
    /// Default CSS selector
    pub selector: Option<String>,
    /// Default maximum pages to scrape
    pub max_pages: Option<usize>,
    /// Default delay between requests (ms)
    pub delay_ms: Option<u64>,
    /// Default log level
    pub log_level: Option<String>,
    /// Whether to use sitemap by default
    pub use_sitemap: Option<bool>,
    /// Default Obsidian wiki-links setting
    pub obsidian_wiki_links: Option<bool>,
    /// Default Obsidian tags (comma-separated string)
    pub obsidian_tags: Option<String>,
    /// Default Obsidian relative assets setting
    pub obsidian_relative_assets: Option<bool>,
    /// Default Obsidian vault path
    pub vault_path: Option<String>,
    /// Bypass WAF/CAPTCHA detection by default (REQ-WAF-07)
    pub ignore_waf: Option<bool>,
    /// Default explicit rate-limiter burst permits (budget model Q1 knob;
    /// 0 is rejected at staging with a Spanish error).
    pub rate_limit_burst: Option<u32>,
    /// LLM provider declarations (`ai-providers-design.md` §4).
    ///
    /// TOML shape: `[[providers]]` entries with `id`, `display_name`, `kind`,
    /// `base_url`, `auth`, `capabilities`, and optional `model` /
    /// `embedding_dim`. Absent section = no providers (the default); a
    /// malformed section falls back to defaults with a loud `error!` — same
    /// contract as the rest of this file.
    #[serde(default)]
    pub providers: Vec<crate::domain::providers::ProviderConfig>,
}

/// Why a configuration file could not be used (#1659).
///
/// Every variant carries the offending path. That is not decoration: the
/// defect this type exists to close is a run that used defaults for a file the
/// operator believed was applied, so a message that omits the path is the same
/// bug in a different costume. Messages are user-facing, therefore Spanish;
/// the `tracing` events that accompany them are English.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    /// An explicit `WEBFANG_CONFIG` override was not an absolute path.
    #[error("WEBFANG_CONFIG debe ser una ruta absoluta: {path}")]
    NotAbsolute {
        /// The rejected value, verbatim.
        path: String,
    },
    /// No file exists at the configured path.
    #[error("no existe el archivo de configuración: {path}")]
    NotFound {
        /// The path that was looked up.
        path: String,
    },
    /// The file exists but could not be read — `PermissionDenied`, a path
    /// that is a directory (`EISDIR`), invalid UTF-8, and friends.
    #[error("no se pudo leer el archivo de configuración {path}: {source}")]
    Io {
        /// The path that was read.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The file was read but is not valid TOML.
    #[error("el archivo de configuración {path} no es TOML válido: {source}")]
    Malformed {
        /// The path that was parsed.
        path: String,
        /// The underlying TOML failure.
        #[source]
        source: toml::de::Error,
    },
}

impl ConfigDefaults {
    /// Load configuration from a TOML file, degrading to defaults.
    ///
    /// `NotFound` is the ONLY silent degradation: it means "no config yet",
    /// which is the normal state of a fresh install. Every other reason the
    /// file is unusable — `PermissionDenied`, a path that is a directory,
    /// invalid UTF-8, malformed TOML — logs a loud `error!` carrying the path
    /// before falling back. Before #1659 a `let Ok(content) = … else { return
    /// Self::default() }` swallowed all of them indistinguishably, so a
    /// `config.toml` that was never read produced a run indistinguishable
    /// from one where it was.
    pub fn load(path: &Path) -> Self {
        match Self::read_and_parse(path) {
            Ok(config) => config,
            // The benign case, and deliberately the only silent one.
            Err(ConfigLoadError::NotFound { .. }) => Self::default(),
            Err(e) => {
                tracing::error!(
                    path = %path.display(),
                    error = %e,
                    "Config file unusable — all settings ignored, using defaults"
                );
                Self::default()
            },
        }
    }

    /// Load configuration from an EXPLICIT `WEBFANG_CONFIG` override,
    /// failing instead of degrading (#1659).
    ///
    /// An override is a statement of intent, so nothing about it is
    /// best-effort:
    ///
    /// - It must be **absolute**. A relative path would resolve against
    ///   whatever working directory the process inherited, which under a
    ///   daemon or service manager is whatever the supervisor chose — so the
    ///   same config means a different file depending on who started it.
    /// - It must **exist**. A typo must fail, not read as "use defaults".
    /// - It must **parse**. The operator named this file; ignoring its
    ///   contents is the silent degradation this variant exists to forbid.
    ///
    /// # Errors
    /// [`ConfigLoadError::NotAbsolute`] for a relative path;
    /// [`ConfigLoadError::NotFound`] when nothing is there;
    /// [`ConfigLoadError::Io`] when it exists but cannot be read;
    /// [`ConfigLoadError::Malformed`] when it is not valid TOML.
    pub fn load_explicit(path: &Path) -> Result<Self, ConfigLoadError> {
        if !path.is_absolute() {
            return Err(ConfigLoadError::NotAbsolute {
                path: path.display().to_string(),
            });
        }
        Self::read_and_parse(path)
    }

    /// Read and parse, classifying every failure.
    ///
    /// The single core both policies delegate to: [`Self::load`] decides
    /// which failures may degrade, [`Self::load_explicit`] decides that none
    /// may. Keeping the classification here is what stops the two from
    /// disagreeing about what counts as "the file is not there".
    fn read_and_parse(path: &Path) -> Result<Self, ConfigLoadError> {
        let content = std::fs::read_to_string(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ConfigLoadError::NotFound {
                    path: path.display().to_string(),
                }
            } else {
                // Includes invalid UTF-8 (`InvalidData`): `read_to_string`
                // rejects it before any TOML is seen, and the operator still
                // needs to hear about it.
                ConfigLoadError::Io {
                    path: path.display().to_string(),
                    source,
                }
            }
        })?;
        toml::from_str(&content).map_err(|source| ConfigLoadError::Malformed {
            path: path.display().to_string(),
            source,
        })
    }
}

/// Resolve the webfang config file path.
///
/// This resolves a PATH only — whether that path must exist is the caller's
/// policy, and the two callers deliberately differ since #1659. The CLI goes
/// through [`load_config_defaults`], which requires an explicit override to be
/// absolute and present; the MCP daemon loads with [`ConfigDefaults::load`],
/// which degrades with a warning so the server keeps serving. Reach for this
/// function directly only when neither policy applies.
///
/// Shared by the CLI and MCP composition roots so both read the same
/// `[[providers]]` declarations (#1462): the daemon owns no argv, and only
/// this file can carry its embedding-slot selection. The base comes from
/// the single platform-paths helper (XP-F-05, #1608): `dirs::config_dir()`
/// — XDG on Linux (unchanged behavior), %APPDATA% on Windows,
/// ~/Library/Application Support on macOS. The `.` fallback keeps this
/// site's previous fail-soft behavior when no user home exists.
///
/// One explicit override is honored ahead of that base. It is not redundant
/// with the `XDG_CONFIG_HOME` support above, which has two gaps it does not
/// close: it only applies when the variable is ABSOLUTE, and it names a
/// DIRECTORY while this names the file itself — so a test or user can point at
/// a `config.toml` that is not in the `webfang/` subdirectory, which is what
/// the budget tests need (#1631).
#[must_use]
pub fn resolve_config_path() -> PathBuf {
    explicit_config_path().unwrap_or_else(default_config_path)
}

/// The explicit `WEBFANG_CONFIG` override, when one is set.
///
/// An **empty** value counts as unset, so a script that exports the variable
/// before it has a value falls through to the platform lookup instead of
/// failing (#1651).
fn explicit_config_path() -> Option<PathBuf> {
    std::env::var_os("WEBFANG_CONFIG")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// The platform-default config path: `<config base>/webfang/config.toml`.
fn default_config_path() -> PathBuf {
    crate::domain::paths::config_base_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webfang")
        .join("config.toml")
}

/// Load the configuration the CLI runs with, enforcing the override contract.
///
/// This is the CLI's single entry point for configuration, and it owns the
/// asymmetry between the two sources that [`ConfigDefaults::load`] and
/// [`ConfigDefaults::load_explicit`] each assume:
///
/// - **Platform default** → [`ConfigDefaults::load`]. A missing file is the
///   normal state of a fresh install, so it degrades silently.
/// - **Explicit `WEBFANG_CONFIG`** → [`ConfigDefaults::load_explicit`]. The
///   operator named that file, so a typo, a relative path, or an unreadable
///   file stops the run instead of quietly running with defaults.
///
/// The MCP daemon deliberately does NOT use this: its documented convention is
/// to keep serving and degrade with a warning, the opposite of the CLI's
/// fail-closed startup, so it stays on [`ConfigDefaults::load`].
pub fn load_config_defaults() -> Result<ConfigDefaults, ConfigLoadError> {
    match explicit_config_path() {
        Some(path) => ConfigDefaults::load_explicit(&path),
        None => Ok(ConfigDefaults::load(&default_config_path())),
    }
}

/// Check if NO_COLOR env var is set (any non-empty value).
pub fn is_no_color() -> bool {
    std::env::var("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Whether ANSI styling should be emitted on the console stream.
///
/// Pure decision ([`crate::domain::console::ansi_enabled`]): ANSI only on an
/// interactive terminal, unless NO_COLOR / `--no-color` opted out (XP-K-04).
#[must_use]
pub fn ansi_enabled(is_terminal: bool, no_color: bool) -> bool {
    crate::domain::console::ansi_enabled(is_terminal, no_color)
}

/// Whether emoji should be emitted in output.
pub fn should_emit_emoji() -> bool {
    !is_no_color()
}

/// Initialize logging with configurable level, routing ALL output to stderr.
pub fn init_logging(level: &str) {
    init_logging_dual(level, false, is_no_color(), None);
}

/// Dual-mode logging: forces stderr, supports quiet mode and NO_COLOR.
///
/// # Arguments
///
/// * `level` - Log level: "error", "warn", "info", "debug", "trace"
/// * `quiet` - If true, only warn+level output is shown
/// * `no_color` - If true, ANSI colors are disabled
/// * `file_trace_layer` - Optional file trace layer for JSONL output
pub fn init_logging_dual(
    level: &str,
    quiet: bool,
    no_color: bool,
    file_trace_layer: Option<crate::infrastructure::observability::FileTraceLayer>,
) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // Per-layer filters (#489): the console filter respects the user's
    // verbosity choice, while the file trace layer ALWAYS runs at TRACE
    // level so `--trace-file` captures everything independently of `-v`/`-vv`.
    let console_filter = if quiet {
        EnvFilter::new("webfang=warn,tokio=warn,reqwest=warn")
    } else {
        EnvFilter::new(format!("webfang={level},tokio=warn,reqwest=warn"))
    };
    let trace_filter = EnvFilter::new("webfang=trace,tokio=warn,reqwest=warn");

    // XP-K-04 (#1608): ANSI only on an interactive terminal — redirected
    // stderr (files, pipes, CI logs, legacy conhost) gets clean text.
    let ansi = crate::domain::console::ansi_enabled(std::io::stderr().is_terminal(), no_color);

    let fmt_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(ansi)
        .with_target(true)
        .pretty()
        .with_filter(console_filter);

    tracing_subscriber::registry()
        .with(file_trace_layer.with_filter(trace_filter))
        .with(fmt_layer)
        .try_init()
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing path that is absolute on every platform. A literal like
    /// `/nonexistent/...` is only *rooted* on Windows — `Path::is_absolute`
    /// there demands a drive prefix — so `load_explicit` would reject it as
    /// `NotAbsolute` before ever discovering the file does not exist.
    /// `temp_dir()` is absolute on all supported platforms.
    fn missing_absolute_path() -> std::path::PathBuf {
        std::env::temp_dir().join("webfang-nonexistent-config/config.toml")
    }

    #[test]
    fn test_load_defaults_when_no_file() {
        let config = ConfigDefaults::load(Path::new("/nonexistent/path/config.toml"));
        assert!(config.format.is_none());
        assert!(config.concurrency.is_none());
        assert!(config.log_level.is_none());
    }

    #[test]
    fn test_load_from_valid_toml() {
        let tmp = std::env::temp_dir().join("webfang_test_config.toml");
        let content = r#"
format = "json"
concurrency = "auto"
log_level = "debug"
max_pages = 20
"#;
        std::fs::write(&tmp, content).unwrap();
        let config = ConfigDefaults::load(&tmp);
        assert_eq!(config.format, Some("json".to_string()));
        assert_eq!(config.concurrency, Some("auto".to_string()));
        assert_eq!(config.log_level, Some("debug".to_string()));
        assert_eq!(config.max_pages, Some(20));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_ignore_waf_from_toml() {
        // REQ-WAF-07: persistent ignore_waf config field round-trips via TOML.
        let tmp = std::env::temp_dir().join("webfang_test_config_ignore_waf.toml");
        std::fs::write(&tmp, "ignore_waf = true\n").unwrap();
        let config = ConfigDefaults::load(&tmp);
        assert_eq!(config.ignore_waf, Some(true));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_ignore_waf_defaults_to_none() {
        let config = ConfigDefaults::load(Path::new("/nonexistent/path/config.toml"));
        assert!(config.ignore_waf.is_none());
    }

    #[test]
    fn test_is_no_color_default() {
        // Default should be false (no env var set in test)
        let val = is_no_color();
        assert!(!val);
    }

    #[test]
    fn test_should_emit_emoji_default() {
        assert!(should_emit_emoji());
    }

    /// A malformed TOML file must not crash the CLI: `ConfigDefaults::load`
    /// logs an `error!` and falls back to all-default settings (#393). Because
    /// the user's entire configuration is silently lost on a parse failure, the
    /// fallback to defaults — not the parse error itself — is the behavioral
    /// invariant this test pins.
    #[test]
    fn test_load_malformed_toml_falls_back_to_defaults() {
        // Arrange: an existing file whose contents are not valid TOML.
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let config_path = tmp.path().join("config.toml");
        std::fs::write(&config_path, "this is [[[ not valid toml")
            .expect("config file should be written");

        // Act
        let config = ConfigDefaults::load(&config_path);

        // Assert: every field falls back to its default (None); nothing crashes.
        assert!(config.format.is_none(), "format must fall back to default");
        assert!(
            config.export_format.is_none(),
            "export_format must fall back to default"
        );
        assert!(
            config.concurrency.is_none(),
            "concurrency must fall back to default"
        );
        assert!(
            config.selector.is_none(),
            "selector must fall back to default"
        );
        assert!(
            config.max_pages.is_none(),
            "max_pages must fall back to default"
        );
        assert!(
            config.delay_ms.is_none(),
            "delay_ms must fall back to default"
        );
        assert!(
            config.log_level.is_none(),
            "log_level must fall back to default"
        );
        assert!(
            config.use_sitemap.is_none(),
            "use_sitemap must fall back to default"
        );
        assert!(
            config.ignore_waf.is_none(),
            "ignore_waf must fall back to default"
        );
        assert!(
            config.vault_path.is_none(),
            "vault_path must fall back to default"
        );
    }

    // ---------------------------------------------------------------------
    // #1659 — the loader must not degrade silently on anything but NotFound
    // ---------------------------------------------------------------------

    /// The sharpest statement of the fix: for one and the same missing file,
    /// the platform default degrades while the explicit override fails. Before
    /// #1659 both paths returned defaults and the run could not tell an
    /// operator that their config was never read.
    #[test]
    fn explicit_override_fails_where_the_platform_default_degrades() {
        let missing = missing_absolute_path();

        let config = ConfigDefaults::load(&missing);
        assert!(
            config.format.is_none(),
            "a missing platform default must still degrade to defaults"
        );

        assert!(
            matches!(
                ConfigDefaults::load_explicit(&missing),
                Err(ConfigLoadError::NotFound { .. })
            ),
            "a missing explicit override must be an error, not a silent default"
        );
    }

    /// The classification `load` branches on. `NotFound` is the only variant
    /// that degrades silently; every other one is loud. This is the invariant
    /// that the pre-#1659 `let Ok(content) = … else { return default() }` threw
    /// away.
    #[test]
    fn only_a_missing_file_classifies_as_not_found() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");

        // A path that does not exist: the one benign case.
        assert!(matches!(
            ConfigDefaults::read_and_parse(&tmp.path().join("absent.toml")),
            Err(ConfigLoadError::NotFound { .. })
        ));

        // A path that exists but is a directory: `EISDIR`, NOT NotFound.
        let dir_path = tmp.path().join("config.toml");
        std::fs::create_dir(&dir_path).expect("create directory");
        assert!(
            matches!(
                ConfigDefaults::read_and_parse(&dir_path),
                Err(ConfigLoadError::Io { .. })
            ),
            "a directory must classify as Io, never as NotFound"
        );

        // An existing file that is not valid UTF-8: `InvalidData`, NOT
        // NotFound — and never a parse error either, since `read_to_string`
        // rejects the bytes before TOML is involved.
        let utf8_path = tmp.path().join("invalid-utf8.toml");
        std::fs::write(&utf8_path, [0xff_u8, 0xfe, 0x00]).expect("write invalid UTF-8");
        assert!(
            matches!(
                ConfigDefaults::read_and_parse(&utf8_path),
                Err(ConfigLoadError::Io { .. })
            ),
            "invalid UTF-8 must classify as Io"
        );
    }

    /// `load` keeps degrading — the CLI must not crash on an unusable file —
    /// but the degradation is now a classified one rather than a swallowed
    /// error.
    #[test]
    fn load_degrades_to_defaults_for_every_unusable_file() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");

        let dir_path = tmp.path().join("config.toml");
        std::fs::create_dir(&dir_path).expect("create directory");
        assert!(
            ConfigDefaults::load(&dir_path).format.is_none(),
            "a directory must degrade to defaults, not panic"
        );

        let utf8_path = tmp.path().join("invalid-utf8.toml");
        std::fs::write(&utf8_path, [0xff_u8, 0xfe, 0x00]).expect("write invalid UTF-8");
        assert!(
            ConfigDefaults::load(&utf8_path).format.is_none(),
            "invalid UTF-8 must degrade to defaults, not panic"
        );
    }

    /// A relative `WEBFANG_CONFIG` is rejected outright: under a daemon or
    /// service manager the working directory is whatever the supervisor chose,
    /// so the same variable would name a different file depending on who
    /// started the process.
    #[test]
    fn load_explicit_rejects_a_relative_path() {
        let relative = Path::new("config.toml");
        match ConfigDefaults::load_explicit(relative) {
            Err(ConfigLoadError::NotAbsolute { path }) => {
                assert_eq!(
                    path, "config.toml",
                    "the message must name the rejected value"
                );
            },
            other => panic!("a relative override must be NotAbsolute, got {other:?}"),
        }
    }

    /// The typo case from the issue report: a path that does not exist is an
    /// error, never a silent fallback.
    #[test]
    fn load_explicit_rejects_a_missing_file() {
        let missing = missing_absolute_path();
        assert!(matches!(
            ConfigDefaults::load_explicit(&missing),
            Err(ConfigLoadError::NotFound { .. })
        ));
    }

    #[test]
    fn load_explicit_rejects_a_directory() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let dir_path = tmp.path().join("config.toml");
        std::fs::create_dir(&dir_path).expect("create directory");
        assert!(matches!(
            ConfigDefaults::load_explicit(&dir_path),
            Err(ConfigLoadError::Io { .. })
        ));
    }

    #[test]
    fn load_explicit_rejects_invalid_utf8() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, [0xff_u8, 0xfe, 0x00]).expect("write invalid UTF-8");
        assert!(matches!(
            ConfigDefaults::load_explicit(&path),
            Err(ConfigLoadError::Io { .. })
        ));
    }

    #[test]
    fn load_explicit_rejects_malformed_toml() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "this is [[[ not valid toml").expect("write malformed TOML");
        assert!(matches!(
            ConfigDefaults::load_explicit(&path),
            Err(ConfigLoadError::Malformed { .. })
        ));
    }

    #[test]
    fn load_explicit_accepts_a_valid_file() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "format = \"json\"\nmax_pages = 7\n").expect("write valid TOML");

        let config = ConfigDefaults::load_explicit(&path).expect("a valid override must load");
        assert_eq!(config.format, Some("json".to_string()));
        assert_eq!(config.max_pages, Some(7));
    }

    /// An EMPTY file yields exactly the all-`None` defaults the old absent
    /// file produced. This is the equivalence the shared behavioral harness
    /// now depends on: it points every spawned binary at an empty config
    /// because an absent one is no longer accepted as an override.
    #[test]
    fn load_explicit_accepts_an_empty_file_as_all_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "").expect("write empty config");

        let config = ConfigDefaults::load_explicit(&path).expect("an empty override must load");
        assert!(config.format.is_none());
        assert!(config.concurrency.is_none());
        assert!(config.log_level.is_none());
        assert!(config.max_pages.is_none());
        assert!(config.providers.is_empty());
    }

    /// User-facing messages are Spanish AND carry the path. The path is the
    /// load-bearing half: an operator who cannot see WHICH file failed cannot
    /// act on the message, which is the whole defect #1659 closes.
    #[test]
    fn load_explicit_messages_are_spanish_and_name_the_path() {
        let missing = missing_absolute_path();
        let msg = ConfigDefaults::load_explicit(&missing)
            .expect_err("a missing override must fail")
            .to_string();
        assert!(
            msg.starts_with("no existe el archivo de configuración"),
            "user-facing config errors are Spanish, got: {msg}"
        );
        assert!(
            msg.contains(&missing.display().to_string()),
            "the message must name the offending path, got: {msg}"
        );

        let relative = Path::new("config.toml");
        let msg = ConfigDefaults::load_explicit(relative)
            .expect_err("a relative override must fail")
            .to_string();
        assert!(
            msg.starts_with("WEBFANG_CONFIG debe ser una ruta absoluta"),
            "user-facing config errors are Spanish, got: {msg}"
        );
    }

    /// `PermissionDenied` is the case the issue names first, and the one a
    /// developer hits most often in practice. Unix-only because the mode bits
    /// are the mechanism; root ignores them, so the test returns early there
    /// rather than asserting something the OS will not enforce.
    #[cfg(unix)]
    #[test]
    fn load_explicit_reports_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir should be created");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "format = \"json\"\n").expect("write config");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");

        if std::fs::read_to_string(&path).is_ok() {
            // Running with privileges that bypass the mode bits: the case is
            // untestable here, not broken.
            return;
        }

        match ConfigDefaults::load_explicit(&path) {
            Err(ConfigLoadError::Io { source, .. }) => {
                assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
            },
            other => panic!("an unreadable override must be an Io error, got {other:?}"),
        }
    }
}
