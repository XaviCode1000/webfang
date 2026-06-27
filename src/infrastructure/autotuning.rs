//! Hardware autotuning for the elastic ingestion pipeline (Issue #51).
//!
//! Derives pipeline sizing defaults from the host hardware:
//! - CPU cores via [`num_cpus::get()`] (Rayon pool / DB pool fan-out).
//! - RAM budget via [`sys_info::mem_info()`] × 0.7 (byte-weighted semaphore).
//!
//! Configuration resolution priority (frozen design decision #12):
//! **CLI flag > `RUST_SCRAPER_*` env var > auto-detected default**.
//!
//! All resolution logic is exposed as pure functions taking `(cli_override, env_value)`
//! so the priority order is unit-testable without mutating the process environment
//! (which would be racy under parallel test execution).

use std::path::{Path, PathBuf};

use tracing::warn;

// ============================================================================
// Constants (frozen design decisions #6, #11; spec hardware-autotuning §Fallback)
// ============================================================================

/// Fallback CPU core count when detection fails or returns 0.
pub const FALLBACK_CPU_CORES: usize = 4;

/// Fallback RAM budget (2 GiB in bytes) when `sys_info` fails.
///
/// Frozen: the user's task and the hardware-autotuning spec agree on 2 GiB
/// (2_147_483_648 bytes) — NOT `8 GiB × 0.7`.
pub const FALLBACK_RAM_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default per-resource byte ceiling (25 MiB) for the download semaphore.
pub const DEFAULT_MAX_RESOURCE_BYTES: u64 = 25 * 1024 * 1024;

/// Minimum DB connection pool size (frozen design decision #6: floor 4).
pub const MIN_DB_POOL_SIZE: usize = 4;

/// Default SQLite database path: `~/.rust_scraper/crawl.db`.
pub const DEFAULT_DB_DIR: &str = ".rust_scraper";
pub const DEFAULT_DB_FILE: &str = "crawl.db";

// ============================================================================
// Environment variable names (frozen design decision #12 — snake-upper)
// ============================================================================

pub const ENV_CPU_CORES: &str = "RUST_SCRAPER_CPU_CORES";
pub const ENV_RAM_BUDGET: &str = "RUST_SCRAPER_RAM_BUDGET";
pub const ENV_MAX_RESOURCE_MB: &str = "RUST_SCRAPER_MAX_RESOURCE_MB";
pub const ENV_DB_PATH: &str = "RUST_SCRAPER_DB_PATH";

// ============================================================================
// Hardware detection primitives
// ============================================================================

/// Detect the number of available CPU cores via [`num_cpus::get()`].
///
/// Falls back to [`FALLBACK_CPU_CORES`] when the platform reports 0.
#[must_use]
pub fn detect_cpu_cores() -> usize {
    let n = num_cpus::get();
    if n == 0 {
        FALLBACK_CPU_CORES
    } else {
        n
    }
}

/// Compute the RAM budget (bytes) from an optional total-RAM value in KiB.
///
/// - `Some(total_kib)` → `total_kib * 1024 * 0.7` (70% of total RAM, truncated).
/// - `None` (detection failed) → [`FALLBACK_RAM_BUDGET_BYTES`] + a `tracing::warn!`.
///
/// Exposed as a pure function (taking the detected value, not calling `sys_info`
/// directly) so the fallback path is unit-testable without mocking the OS.
#[must_use]
pub fn ram_budget_from(total_kib: Option<u64>) -> u64 {
    match total_kib {
        Some(kib) => kib.saturating_mul(1024).saturating_mul(7) / 10,
        None => {
            warn!(
                error = "sys_info::mem_info falló",
                fallback_bytes = FALLBACK_RAM_BUDGET_BYTES,
                "Usando 2 GiB como presupuesto de RAM"
            );
            FALLBACK_RAM_BUDGET_BYTES
        },
    }
}

/// Detect the RAM budget via [`sys_info::mem_info()`], falling back to 2 GiB.
#[must_use]
pub fn detect_ram_budget() -> u64 {
    ram_budget_from(sys_info::mem_info().ok().map(|m| m.total))
}

/// Default SQLite database path: `~/.rust_scraper/crawl.db`.
///
/// Falls back to `./crawl.db` when the home directory cannot be determined.
#[must_use]
pub fn default_db_path() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(DEFAULT_DB_DIR).join(DEFAULT_DB_FILE),
        None => PathBuf::from(DEFAULT_DB_FILE),
    }
}

// ============================================================================
// Pure resolution (priority: CLI > env > autodetect) — fully unit-testable
// ============================================================================

/// Resolve CPU cores: `cli` wins, then `env`, then auto-detect.
#[must_use]
pub fn resolve_cpu_cores(cli: Option<usize>, env: Option<usize>) -> usize {
    cli.or(env).unwrap_or_else(detect_cpu_cores)
}

/// Resolve RAM budget (bytes): `cli` wins, then `env`, then auto-detect.
#[must_use]
pub fn resolve_ram_budget(cli: Option<u64>, env: Option<u64>) -> u64 {
    cli.or(env).unwrap_or_else(detect_ram_budget)
}

/// Resolve the per-resource byte ceiling: `cli` wins, then `env`, then default.
#[must_use]
pub fn resolve_max_resource_bytes(cli: Option<u64>, env: Option<u64>) -> u64 {
    cli.or(env).unwrap_or(DEFAULT_MAX_RESOURCE_BYTES)
}

/// Resolve the DB path: `cli` wins, then `env`, then [`default_db_path`].
#[must_use]
pub fn resolve_db_path(cli: Option<&Path>, env: Option<PathBuf>) -> PathBuf {
    cli.map(PathBuf::from)
        .or(env)
        .unwrap_or_else(default_db_path)
}

// ============================================================================
// Env readers (used by AutotuningConfig/ElasticConfig wiring)
// ============================================================================

/// Read `RUST_SCRAPER_CPU_CORES` override (parsed as `usize`).
#[must_use]
pub fn env_cpu_cores() -> Option<usize> {
    std::env::var(ENV_CPU_CORES)
        .ok()
        .and_then(|v| v.trim().parse().ok())
}

/// Read `RUST_SCRAPER_RAM_BUDGET` override (bytes, or suffixed e.g. `8GB`).
#[must_use]
pub fn env_ram_budget() -> Option<u64> {
    std::env::var(ENV_RAM_BUDGET)
        .ok()
        .and_then(|v| parse_ram_bytes(&v))
}

/// Read `RUST_SCRAPER_MAX_RESOURCE_MB` override (interpreted as MiB → bytes).
#[must_use]
pub fn env_max_resource_bytes() -> Option<u64> {
    std::env::var(ENV_MAX_RESOURCE_MB)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1024).saturating_mul(1024))
}

/// Read `RUST_SCRAPER_DB_PATH` override.
#[must_use]
pub fn env_db_path() -> Option<PathBuf> {
    std::env::var(ENV_DB_PATH).ok().map(PathBuf::from)
}

/// Parse a RAM budget string: plain bytes, or a number with a binary suffix
/// (`B`, `KB`/`KiB`, `MB`/`MiB`, `GB`/`GiB`, `TB`/`TiB`).
///
/// Per the hardware-autotuning spec scenario (`--ram-budget 8GB` → `8 * 1024^3`),
/// all suffixes are treated as binary (powers of 1024).
#[must_use]
pub fn parse_ram_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut split = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' {
            split = i + c.len_utf8();
        } else {
            break;
        }
    }
    let (num_str, unit) = s.split_at(split);
    let num: f64 = num_str.parse().ok()?;
    let power: u32 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 0,
        "KB" | "KIB" => 1,
        "MB" | "MIB" => 2,
        "GB" | "GIB" => 3,
        "TB" | "TIB" => 4,
        _ => return None,
    };
    let mult: u64 = 1024u64.pow(power);
    if num < 0.0 {
        return None;
    }
    Some((num * mult as f64) as u64)
}

// ============================================================================
// ElasticConfig — full elastic ingestion config (frozen design §Interfaces)
// ============================================================================

/// Overrides supplied by CLI flags (PR5). `None` means "not set → fall back".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElasticOverrides {
    /// `--cpu-cores` override.
    pub cpu_cores: Option<usize>,
    /// `--ram-budget` override (bytes).
    pub ram_budget_bytes: Option<u64>,
    /// `--max-resource-mb` override (bytes).
    pub max_resource_bytes: Option<u64>,
    /// `--db-path` override.
    pub db_path: Option<PathBuf>,
}

/// Resolved elastic ingestion configuration.
///
/// Combines auto-detected hardware sizing with the full pipeline parameters.
/// `db_pool_size` is derived as `cpu_cores` with a floor of 4 (frozen decision #6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ElasticConfig {
    /// Detected/overridden CPU core count (Rayon pool size).
    pub cpu_cores: usize,
    /// Detected/overridden RAM budget in bytes (semaphore permits).
    pub ram_budget_bytes: u64,
    /// Per-resource byte ceiling (default 25 MiB).
    pub max_resource_bytes: u64,
    /// DB connection pool size (`cpu_cores`, floor 4).
    pub db_pool_size: usize,
    /// SQLite database file path.
    pub db_path: PathBuf,
}

impl ElasticConfig {
    /// Resolve the full config from CLI overrides, falling back to env vars and
    /// then auto-detected defaults (priority: CLI > env > autodetect).
    #[must_use]
    pub fn resolve(overrides: &ElasticOverrides) -> Self {
        let cpu_cores = resolve_cpu_cores(overrides.cpu_cores, env_cpu_cores());
        let ram_budget_bytes = resolve_ram_budget(overrides.ram_budget_bytes, env_ram_budget());
        let max_resource_bytes =
            resolve_max_resource_bytes(overrides.max_resource_bytes, env_max_resource_bytes());
        let db_path = resolve_db_path(overrides.db_path.as_deref(), env_db_path());
        let db_pool_size = cpu_cores.max(MIN_DB_POOL_SIZE);
        Self {
            cpu_cores,
            ram_budget_bytes,
            max_resource_bytes,
            db_pool_size,
            db_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    // ---- detection primitives ----

    #[test]
    fn test_detect_cpu_cores_matches_num_cpus() {
        let detected = detect_cpu_cores();
        let n = num_cpus::get();
        let expected = if n == 0 { FALLBACK_CPU_CORES } else { n };
        assert_eq!(detected, expected);
    }

    #[test]
    fn test_ram_budget_from_none_falls_back_to_2gib() {
        assert_eq!(ram_budget_from(None), FALLBACK_RAM_BUDGET_BYTES);
        assert_eq!(FALLBACK_RAM_BUDGET_BYTES, 2 * GIB);
    }

    #[test]
    fn test_ram_budget_from_32gib_machine() {
        // 32 GiB total RAM = 33_554_432 KiB → 70% in bytes (truncated).
        let total_kib: u64 = 32 * 1024 * 1024;
        let expected = total_kib * 1024 * 7 / 10;
        assert_eq!(ram_budget_from(Some(total_kib)), expected);
        // Sanity: 70% of 32 GiB is ~22.4 GiB (> 15 GiB, < 24 GiB).
        let result = ram_budget_from(Some(total_kib));
        assert!(result > 15 * GIB && result < 24 * GIB);
    }

    #[test]
    fn test_detect_ram_budget_is_nonzero_and_sane() {
        let budget = detect_ram_budget();
        assert!(budget > 0, "RAM budget must be positive");
        // Either autodetected (> a few MiB) or the 2 GiB fallback.
        assert!(budget >= FALLBACK_RAM_BUDGET_BYTES || budget > 100 * MIB);
    }

    #[test]
    fn test_default_db_path_under_home() {
        let p = default_db_path();
        assert!(p.ends_with(DEFAULT_DB_FILE));
        assert!(p.to_string_lossy().contains(DEFAULT_DB_DIR));
    }

    // ---- pure resolution priority (CLI > env > autodetect) ----

    #[test]
    fn test_resolve_cpu_cores_cli_wins() {
        assert_eq!(resolve_cpu_cores(Some(4), Some(8)), 4);
    }

    #[test]
    fn test_resolve_cpu_cores_env_beats_autodetect() {
        assert_eq!(resolve_cpu_cores(None, Some(8)), 8);
    }

    #[test]
    fn test_resolve_cpu_cores_autodetect_when_no_overrides() {
        assert_eq!(resolve_cpu_cores(None, None), detect_cpu_cores());
    }

    #[test]
    fn test_resolve_ram_budget_priority() {
        assert_eq!(resolve_ram_budget(Some(8 * GIB), Some(4 * GIB)), 8 * GIB);
        assert_eq!(resolve_ram_budget(None, Some(4 * GIB)), 4 * GIB);
        assert_eq!(resolve_ram_budget(None, None), detect_ram_budget());
    }

    #[test]
    fn test_resolve_max_resource_bytes_default_and_overrides() {
        assert_eq!(
            resolve_max_resource_bytes(Some(50 * MIB), Some(10 * MIB)),
            50 * MIB
        );
        assert_eq!(resolve_max_resource_bytes(None, Some(10 * MIB)), 10 * MIB);
        assert_eq!(
            resolve_max_resource_bytes(None, None),
            DEFAULT_MAX_RESOURCE_BYTES
        );
        assert_eq!(DEFAULT_MAX_RESOURCE_BYTES, 25 * MIB);
    }

    #[test]
    fn test_resolve_db_path_priority() {
        let cli = Path::new("/tmp/x.db");
        assert_eq!(resolve_db_path(Some(cli), None), PathBuf::from("/tmp/x.db"));
        assert_eq!(
            resolve_db_path(None, Some(PathBuf::from("/c/d.db"))),
            PathBuf::from("/c/d.db")
        );
        assert_eq!(resolve_db_path(None, None), default_db_path());
    }

    // ---- ElasticConfig.resolve ----

    #[test]
    fn test_elastic_config_resolve_with_full_overrides() {
        let overrides = ElasticOverrides {
            cpu_cores: Some(4),
            ram_budget_bytes: Some(8 * GIB),
            max_resource_bytes: Some(50 * MIB),
            db_path: Some(PathBuf::from("/tmp/elastic.db")),
        };
        let cfg = ElasticConfig::resolve(&overrides);
        assert_eq!(cfg.cpu_cores, 4);
        assert_eq!(cfg.ram_budget_bytes, 8 * GIB);
        assert_eq!(cfg.max_resource_bytes, 50 * MIB);
        assert_eq!(cfg.db_pool_size, 4); // max(4, 4)
        assert_eq!(cfg.db_path, PathBuf::from("/tmp/elastic.db"));
    }

    #[test]
    fn test_elastic_config_db_pool_floor_when_cpu_below_4() {
        // cpu override 2 → db_pool_size must floor at 4 (frozen decision #6).
        let overrides = ElasticOverrides {
            cpu_cores: Some(2),
            ..Default::default()
        };
        let cfg = ElasticConfig::resolve(&overrides);
        assert_eq!(cfg.cpu_cores, 2);
        assert_eq!(cfg.db_pool_size, MIN_DB_POOL_SIZE);
        assert_eq!(MIN_DB_POOL_SIZE, 4);
    }

    #[test]
    fn test_elastic_config_db_pool_matches_cpu_when_above_floor() {
        let overrides = ElasticOverrides {
            cpu_cores: Some(8),
            ..Default::default()
        };
        let cfg = ElasticConfig::resolve(&overrides);
        assert_eq!(cfg.db_pool_size, 8);
    }

    // ---- parse_ram_bytes (env/CLI suffix parsing) ----

    #[test]
    fn test_parse_ram_bytes_plain_and_suffixed() {
        assert_eq!(parse_ram_bytes("8589934592"), Some(8 * GIB));
        assert_eq!(parse_ram_bytes("8GB"), Some(8 * GIB));
        assert_eq!(parse_ram_bytes("8GiB"), Some(8 * GIB));
        assert_eq!(parse_ram_bytes("8MB"), Some(8 * MIB));
        assert_eq!(parse_ram_bytes("8"), Some(8));
        assert_eq!(parse_ram_bytes("  4 gb "), Some(4 * GIB)); // trimmed + lowercase
    }

    #[test]
    fn test_parse_ram_bytes_rejects_garbage() {
        assert_eq!(parse_ram_bytes(""), None);
        assert_eq!(parse_ram_bytes("garbage"), None);
        assert_eq!(parse_ram_bytes("8XB"), None); // unknown unit
        assert_eq!(parse_ram_bytes("-8GB"), None); // negative
    }
}
