//! Hardware autotuning for the elastic ingestion pipeline (Issue #51).
//!
//! Derives pipeline sizing defaults from the host hardware:
//! - CPU cores via the canonical budget detector seam (Rayon pool / DB pool fan-out).
//! - RAM budget via `sysinfo::System::total_memory()` (bytes) × 0.7 (byte-weighted
//!   semaphore). ONE source of RAM: `sysinfo` documents the unit it returns
//!   (bytes), which the previous `sys-info` dependency did not.
//!
//! Configuration resolution priority (frozen design decision #12):
//! **CLI flag > `WEBFANG_*` env var > auto-detected default**.
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

/// Fallback RAM budget (2 GiB in bytes) when the RAM reading fails.
///
/// Frozen: the user's task and the hardware-autotuning spec agree on 2 GiB
/// (2_147_483_648 bytes) — NOT `8 GiB × 0.7`.
pub const FALLBACK_RAM_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default per-resource byte ceiling (25 MiB) for the download semaphore.
pub const DEFAULT_MAX_RESOURCE_BYTES: u64 = 25 * 1024 * 1024;

/// Minimum DB connection pool size (frozen design decision #6: floor 4).
pub const MIN_DB_POOL_SIZE: usize = 4;

/// Default SQLite database path: `~/.webfang/crawl.db`.
pub const DEFAULT_DB_DIR: &str = ".webfang";
pub(crate) const DEFAULT_DB_FILE: &str = "crawl.db";

// ============================================================================
// Environment variable names (frozen design decision #12 — snake-upper)
// ============================================================================

pub(crate) const ENV_CPU_CORES: &str = "WEBFANG_CPU_CORES";
pub(crate) const ENV_RAM_BUDGET: &str = "WEBFANG_RAM_BUDGET";
pub(crate) const ENV_MAX_RESOURCE_MB: &str = "WEBFANG_MAX_RESOURCE_MB";
pub(crate) const ENV_DB_PATH: &str = "WEBFANG_DB_PATH";

// ============================================================================
// Hardware detection primitives
// ============================================================================

/// Detect the number of available CPU cores via the canonical detector seam
/// (Q2 UNIFY NOW — identical source to every other subsystem's "auto").
///
/// Falls back to [`FALLBACK_CPU_CORES`] when the platform reports 0.
#[must_use]
pub fn detect_cpu_cores() -> usize {
    // Canonical detector seam (Q2 UNIFY NOW): one hardware source process-
    // wide, identical to every other subsystem's "auto".
    let n = crate::domain::budget::detector::system_parallelism().get();
    if n == 0 {
        FALLBACK_CPU_CORES
    } else {
        n
    }
}

/// Compute the RAM budget (bytes) from an optional total-RAM reading in BYTES.
///
/// - `Some(total_bytes)` → 70 % of the total RAM, truncated.
/// - `None` (detection failed) → [`FALLBACK_RAM_BUDGET_BYTES`] + a `tracing::warn!`.
///
/// This is the ONLY public function that takes a reading of the system, and
/// the unit is in the NAME: the value comes from `sysinfo::System::total_memory()`,
/// which `sysinfo` documents as BYTES. The 70 % rule lives in the private
/// `ram_budget_from_kib`, so the bytes→KiB conversion happens exactly once,
/// exactly here, and no caller can hand a KiB value to a bytes parameter (the
/// defect that `total_kib` used to hide: `sys-info` never documented its unit).
///
/// Exposed as a pure function (taking the detected value, not calling `sysinfo`
/// directly) so the fallback path is unit-testable without mocking the OS.
#[must_use]
pub fn ram_budget_from_bytes(total_bytes: Option<u64>) -> u64 {
    ram_budget_from_kib(total_bytes.map(|bytes| bytes / 1024))
}

/// The 70 % rule, in KiB. PRIVATE on purpose: two public functions that differ
/// only in the unit of their argument are the shape of the defect this closes
/// — the unit would live in the parameter name and nowhere else.
fn ram_budget_from_kib(total_kib: Option<u64>) -> u64 {
    match total_kib {
        Some(kib) => kib.saturating_mul(1024).saturating_mul(7) / 10,
        None => {
            warn!(
                error = "sysinfo::System::total_memory falló",
                fallback_bytes = FALLBACK_RAM_BUDGET_BYTES,
                "Usando 2 GiB como presupuesto de RAM"
            );
            FALLBACK_RAM_BUDGET_BYTES
        },
    }
}

/// Detect the RAM budget via `sysinfo::System::total_memory()`, falling back
/// to 2 GiB.
///
/// The RAM probe is only available under the `persistence` feature (the SQLite
/// path). When `persistence` is OFF the lightweight core binary has no RAM
/// probe, so detection simply returns the 2 GiB fallback —
/// `ram_budget_from_bytes(None)` is already unit-tested.
///
/// `sysinfo` calls `sysconf` in the constructor of `System`, which Miri cannot
/// execute, so the reading is compiled out under Miri — the same gate as
/// `domain::budget::detector::SystemDetector::total_ram_bytes`.
#[must_use]
pub fn detect_ram_budget() -> u64 {
    #[cfg(feature = "persistence")]
    {
        #[cfg(not(miri))]
        let total_bytes = {
            let mut system = sysinfo::System::new();
            system.refresh_memory();
            Some(system.total_memory())
        };
        #[cfg(miri)]
        let total_bytes: Option<u64> = None;

        ram_budget_from_bytes(total_bytes)
    }
    #[cfg(not(feature = "persistence"))]
    {
        FALLBACK_RAM_BUDGET_BYTES
    }
}

/// Default SQLite database path: `~/.webfang/crawl.db`.
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

/// Read `WEBFANG_CPU_CORES` override (parsed as `usize`).
#[must_use]
pub fn env_cpu_cores() -> Option<usize> {
    std::env::var(ENV_CPU_CORES)
        .ok()
        .and_then(|v| v.trim().parse().ok())
}

/// Read `WEBFANG_RAM_BUDGET` override (bytes, or suffixed e.g. `8GB`).
#[must_use]
pub fn env_ram_budget() -> Option<u64> {
    std::env::var(ENV_RAM_BUDGET)
        .ok()
        .and_then(|v| parse_ram_bytes(&v))
}

/// Read `WEBFANG_MAX_RESOURCE_MB` override (interpreted as MiB → bytes).
#[must_use]
pub fn env_max_resource_bytes() -> Option<u64> {
    std::env::var(ENV_MAX_RESOURCE_MB)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1024).saturating_mul(1024))
}

/// Read `WEBFANG_DB_PATH` override.
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

/// Overrides supplied by CLI flags (PR5) — domain-owned DTO.
///
/// Re-exported from `crate::domain::config` so `application::crawl_options`
/// can depend on it without `application→infrastructure` (ADR-0010).
pub use crate::domain::config::{AutotuningConfig, ElasticOverrides};

/// Resolved elastic ingestion configuration.
///
/// Combines auto-detected hardware sizing with the full pipeline parameters.
/// `db_pool_size` is derived as `cpu_cores` with a floor of 4 (frozen decision #6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ElasticConfig {
    /// Detected/overridden CPU core count (Rayon pool size).
    pub cpu_cores: usize,
    /// Detected/overridden RAM budget in bytes.
    ///
    /// This value IS the elastic-ingestion `Semaphore` permit budget, expressed
    /// directly in bytes (one permit == one byte). `ResourceDownloader` consumes
    /// it via `acquire_many(chunk_len)`, so it MUST be sized in bytes — NOT in a
    /// count of resources. It must be `>= max_resource_bytes`; otherwise a
    /// single max-size resource could never acquire enough permits and the
    /// pipeline would deadlock (issue #544). Capped at
    /// `tokio::sync::Semaphore::MAX_PERMITS` by the wiring code.
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

// ============================================================================
// AutotuningConfig resolve helpers (moved from the retired infrastructure
// config shim — issue #1099). Infra-owned logic next to `ElasticConfig::resolve`
// and the env_*/resolve_* helpers so `domain` stays free of
// `infrastructure::autotuning` imports (inward-only).
// ============================================================================

impl AutotuningConfig {
    /// Resolve the autotuning snapshot.
    ///
    /// Priority: `cpu_override`/`ram_override` > `WEBFANG_*` env > auto-detected.
    #[must_use]
    pub fn resolve(cpu_override: Option<usize>, ram_override: Option<u64>) -> Self {
        Self {
            cpu_cores: resolve_cpu_cores(cpu_override, env_cpu_cores()),
            ram_budget_bytes: resolve_ram_budget(ram_override, env_ram_budget()),
        }
    }

    /// Build a snapshot from a resolved `ElasticConfig`.
    #[must_use]
    pub fn from_elastic(elastic: &ElasticConfig) -> Self {
        Self {
            cpu_cores: elastic.cpu_cores,
            ram_budget_bytes: elastic.ram_budget_bytes,
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
        assert_eq!(ram_budget_from_bytes(None), FALLBACK_RAM_BUDGET_BYTES);
        assert_eq!(FALLBACK_RAM_BUDGET_BYTES, 2 * GIB);
    }

    #[test]
    fn ram_budget_from_bytes_converts_to_kib() {
        // 32 GiB in BYTES — the unit `sysinfo::System::total_memory()`
        // documents ("the RAM size in bytes").
        let total_bytes: u64 = 32 * GIB;
        let total_kib: u64 = 32 * 1024 * 1024;
        // 70 % of 32 GiB, truncated (the same number the KiB rule produced).
        let expected = total_kib * 1024 * 7 / 10;

        // The two halves of the contract agree: the bytes boundary and the
        // private KiB rule produce the SAME budget.
        assert_eq!(ram_budget_from_bytes(Some(total_bytes)), expected);
        assert_eq!(
            ram_budget_from_bytes(Some(total_bytes)),
            ram_budget_from_kib(Some(total_kib))
        );

        // Truncation, not rounding: a sub-KiB remainder is dropped.
        assert_eq!(ram_budget_from_bytes(Some(total_bytes + 1023)), expected);

        // Saturation, not panic and not wrap: a value that overflows the KiB
        // arithmetic still yields a positive budget.
        let saturated = ram_budget_from_bytes(Some(u64::MAX));
        assert!(saturated > 0, "saturated budget must stay positive");

        // Sanity: 70 % of 32 GiB is ~22.4 GiB (> 15 GiB, < 24 GiB).
        let result = ram_budget_from_bytes(Some(total_bytes));
        assert!(result > 15 * GIB && result < 24 * GIB);
    }

    #[test]
    fn test_ram_budget_from_32gib_machine() {
        // 32 GiB total RAM = 33_554_432 KiB → 70% in bytes (truncated).
        // The input is expressed in BYTES: the reading the system hands over.
        let total_bytes: u64 = 32 * GIB;
        let total_kib: u64 = 32 * 1024 * 1024;
        let expected = total_kib * 1024 * 7 / 10;
        assert_eq!(ram_budget_from_bytes(Some(total_bytes)), expected);
        // Sanity: 70% of 32 GiB is ~22.4 GiB (> 15 GiB, < 24 GiB).
        let result = ram_budget_from_bytes(Some(total_bytes));
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

    // ---- AutotuningConfig resolve/from_elastic (moved from the retired
    // infrastructure config shim — issue #1099) ----

    #[test]
    fn test_autotuning_config_resolve_with_overrides() {
        let cfg = AutotuningConfig::resolve(Some(4), Some(8 * 1024 * 1024 * 1024));
        assert_eq!(cfg.cpu_cores, 4);
        assert_eq!(cfg.ram_budget_bytes, 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_autotuning_config_resolve_without_overrides_is_sane() {
        let cfg = AutotuningConfig::resolve(None, None);
        assert!(cfg.cpu_cores > 0, "cpu_cores must be positive");
        assert!(
            cfg.ram_budget_bytes > 0,
            "ram_budget_bytes must be positive"
        );
    }

    #[test]
    fn test_autotuning_config_serializes_roundtrip() {
        let cfg = AutotuningConfig {
            cpu_cores: 8,
            ram_budget_bytes: 16 * 1024 * 1024 * 1024,
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(json.contains("\"cpu_cores\":8"), "json: {json}");
        assert!(json.contains("\"ram_budget_bytes\":"));
        let back: AutotuningConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, cfg);
    }

    #[test]
    fn test_autotuning_config_from_elastic() {
        let elastic = ElasticConfig {
            cpu_cores: 6,
            ram_budget_bytes: 12 * 1024 * 1024 * 1024,
            max_resource_bytes: 25 * 1024 * 1024,
            db_pool_size: 6,
            db_path: std::path::PathBuf::from("/tmp/elastic.db"),
        };
        let snap = AutotuningConfig::from_elastic(&elastic);
        assert_eq!(snap.cpu_cores, 6);
        assert_eq!(snap.ram_budget_bytes, 12 * 1024 * 1024 * 1024);
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
