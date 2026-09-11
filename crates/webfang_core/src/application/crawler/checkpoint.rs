//! Checkpoint persistence for crawl state — Application layer
//!
//! Saves and loads crawl state (visited URLs, pages crawled, banned domains).
//! The queued frontier is deliberately NOT persisted — see `CrawlCheckpoint`.
//! using JSON serialization with CRC32 integrity checks and atomic writes.
//!
//! # Scope — PersistenceMode unification (persistencemode-5c / #980)
//!
//! `CrawlCheckpoint` / `BincodeCheckpoint` (JSON+CRC32, `version:u32`,
//! `checkpoint_interval=100`) is the engine crash-resume mechanism. Since
//! `persistencemode-5c` the CLI wires it via the domain control-plane
//! `PersistenceMode` (`domain/persistence.rs`) — `Engine::with_persistence`
//! gates `with_checkpoint` when the resolved mode is `Checkpoint` or `Full`.
//! The export resume path (`StateStore` → `RecordStore v2`) and the engine
//! checkpoint path remain separate files/formats (no combined envelope).
//! See `sdd/persistencemode-5c` and `COMPATIBILITY-MATRIX`.
//!
//! # Design Decisions
//!
//! - **Sealed trait pattern** (`api-sealed-trait`): Prevents external implementations
//!   that could violate atomicity or integrity invariants.
//! - **File format**: `[4 bytes CRC32][JSON payload]` — simple, verifiable, human-readable.
//! - **Atomic write**: serialize → write to `.tmp` → `fs::rename` to final path.
//! - **Integrity**: CRC32 of payload stored as header; load verifies before deserializing.
//! - **Generic state**: Accepts any `Serialize + Deserialize` type, not just a fixed struct.

use std::collections::HashSet;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, instrument, warn, Instrument};

// ---------------------------------------------------------------------------
// Sealed trait
// ---------------------------------------------------------------------------

mod private {
    pub trait Sealed {}
}

// ---------------------------------------------------------------------------
// BannedDomain — a domain temporarily banned due to WAF or rate-limiting
// ---------------------------------------------------------------------------

/// A domain temporarily banned due to WAF or rate-limiting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BannedDomain {
    /// The banned domain (e.g. "example.com").
    pub domain: String,
    /// When the ban expires. `None` means banned until restart.
    pub banned_until: Option<DateTime<Utc>>,
    /// Reason for the ban (e.g. "WAF challenge", "rate limit exceeded").
    pub reason: String,
}

// ---------------------------------------------------------------------------
// CheckpointPath — path helper for checkpoint files
// ---------------------------------------------------------------------------

/// Path helper for checkpoint files.
#[derive(Debug, Clone)]
pub struct CheckpointPath {
    base_dir: PathBuf,
}

impl CheckpointPath {
    /// Create a new `CheckpointPath` for the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Get the checkpoint file path.
    ///
    /// Legacy unscoped path (`crawl_checkpoint.json`) — kept for backward
    /// compatibility (pre-F-01 checkpoints, existing tests). New crawls must
    /// use [`file_for_seed`](Self::file_for_seed) so concurrent `--resume`
    /// runs for different sites sharing one base dir cannot collide.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        self.base_dir.join("crawl_checkpoint.json")
    }

    /// Scoped checkpoint file for one seed URL (F-01).
    ///
    /// The file name embeds a stable hash of the seed URL
    /// (`crawl_checkpoint_<16-hex>.json`), so concurrent `--resume` runs for
    /// different sites sharing one `--state-dir` read and write disjoint
    /// files. Deterministic: the same seed always maps to the same name.
    #[instrument(fields(base_dir = %self.base_dir.display()))]
    #[must_use]
    pub fn file_for_seed(&self, seed_url: &str) -> PathBuf {
        self.base_dir
            .join(format!("crawl_checkpoint_{}.json", seed_hash(seed_url)))
    }

    /// Ensure the base directory exists.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the directory cannot be created.
    pub fn ensure_dir(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.base_dir)
            .map_err(|e| format!("failed to create checkpoint dir: {e}"))
    }
}

// ---------------------------------------------------------------------------
// CrawlCheckpoint — the serializable state
// ---------------------------------------------------------------------------

/// The checkpoint schema version this build writes and will resume from.
///
/// Version 2 dropped `queued` (#1234 / F-39): persisting the whole BFS frontier
/// was unbounded, was never re-validated against the resumed run's `max_pages`,
/// and made `--resume` drain a stale frontier from a different run.
pub const CURRENT_CHECKPOINT_VERSION: u32 = 2;

/// Default checkpoint version for forward-compatible schema evolution.
///
/// Applied when a payload omits `version` entirely, i.e. it predates version
/// tracking. That is a superseded schema, so it is discarded like any other
/// stale version rather than assumed current.
fn default_version() -> u32 {
    1
}

/// Serializable crawl state for checkpoint persistence.
///
/// `visited`, the bounded frontier, the page counter, and the banned-domain
/// list. The frontier is capped at the run's own `max_pages` (#1234 / F-39): a
/// SIGINT on a link-dense page used to write ~4955 queued URLs against 49
/// visited, and the next `--resume` drained that stale list first because
/// `max_pages` was never part of the checkpoint.
/// Fields use `#[serde(default)]` for forward-compatible schema evolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrawlCheckpoint {
    /// URLs already visited (fully processed).
    #[serde(default)]
    pub visited: HashSet<String>,
    /// URLs discovered but not yet processed, truncated to the run's own
    /// `max_pages` budget when written.
    ///
    /// An execution artefact, not a map of the site. Before #1234 this held the
    /// entire BFS frontier: one SIGINT on a link-dense page wrote ~4955 queued
    /// URLs against 49 visited, and the next `--resume` drained that stale list
    /// first because `max_pages` was never part of the checkpoint. The bound is
    /// the budget the run already set for itself, so nothing the crawl could
    /// still reach is ever dropped.
    #[serde(default)]
    pub queued: Vec<String>,
    /// Number of pages successfully crawled.
    #[serde(default)]
    pub pages_crawled: u64,
    /// Domains currently banned due to WAF or rate limiting.
    #[serde(default)]
    pub banned_domains: Vec<BannedDomain>,
    /// Checkpoint schema version for forward compatibility.
    #[serde(default = "default_version")]
    pub version: u32,
}

impl CrawlCheckpoint {
    /// Create a new empty checkpoint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            visited: HashSet::new(),
            queued: Vec::new(),
            pages_crawled: 0,
            banned_domains: Vec::new(),
            version: CURRENT_CHECKPOINT_VERSION,
        }
    }
}

impl Default for CrawlCheckpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CrawlCheckpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Checkpoint(pages={}, visited={}, queued={})",
            self.pages_crawled,
            self.visited.len(),
            self.queued.len()
        )
    }
}

// ---------------------------------------------------------------------------
// CheckpointStore trait (sealed)
// ---------------------------------------------------------------------------

/// Trait for checkpoint persistence — save and load crawl state.
///
/// Sealed to prevent external implementations that might skip CRC32
/// verification or break atomic write guarantees.
pub trait CheckpointStore: private::Sealed {
    /// Save checkpoint state to persistent storage.
    ///
    /// # Errors
    ///
    /// Returns `Err` on serialization failure or I/O error during write.
    fn save(&self, state: &CrawlCheckpoint, path: &Path) -> Result<(), String>;

    /// Load checkpoint state from persistent storage.
    ///
    /// Returns `None` if the file doesn't exist, is corrupted,
    /// or fails integrity checks.
    fn load(&self, path: &Path) -> Option<CrawlCheckpoint>;
}

// ---------------------------------------------------------------------------
// BincodeCheckpoint — the default implementation
// ---------------------------------------------------------------------------

/// Old checkpoint schema (pure JSON, no CRC32 header).
///
/// Used for loading checkpoints written by the previous infrastructure-layer
/// implementation. `visited` was a `Vec<String>` in the old format; we convert
/// to `HashSet` on load. Such a file never carries the current `version`, so
/// [`accept_version`] discards it — the shape is parsed only so the discard can
/// name the counts it held.
#[derive(Deserialize)]
struct OldCheckpointSchema {
    visited: Vec<String>,
    #[serde(default)]
    queued: Vec<String>,
    #[serde(default)]
    pages_crawled: u64,
    #[serde(default)]
    version: u32,
    #[serde(default)]
    banned_domains: Vec<BannedDomain>,
}

impl From<OldCheckpointSchema> for CrawlCheckpoint {
    fn from(old: OldCheckpointSchema) -> Self {
        Self {
            visited: old.visited.into_iter().collect(),
            queued: old.queued,
            pages_crawled: old.pages_crawled,
            banned_domains: old.banned_domains,
            version: old.version,
        }
    }
}

/// Checkpoint store using JSON serialization with CRC32 integrity.
///
/// File format: `[4-byte CRC32][JSON payload]`
///
/// Write path: serialize → write `.tmp` → atomic rename.
/// Read path: read full file → verify CRC32 → deserialize.
pub struct BincodeCheckpoint;

impl private::Sealed for BincodeCheckpoint {}

impl CheckpointStore for BincodeCheckpoint {
    #[instrument(skip(self, state), fields(path = %path.display()))]
    fn save(&self, state: &CrawlCheckpoint, path: &Path) -> Result<(), String> {
        // Serialize to JSON
        let payload = serde_json::to_string(state)
            .map_err(|e| format!("checkpoint serialization failed: {e}"))?
            .into_bytes();

        // Compute CRC32 of the payload
        let checksum = crc32fast::hash(&payload);

        // Write to .tmp file first
        let tmp_path = tmp_path_for(path);
        {
            let mut file = std::fs::File::create(&tmp_path)
                .map_err(|e| format!("create checkpoint tmp file: {e}"))?;
            file.write_all(&checksum.to_ne_bytes())
                .map_err(|e| format!("write checkpoint checksum: {e}"))?;
            file.write_all(&payload)
                .map_err(|e| format!("write checkpoint payload: {e}"))?;
            file.sync_all()
                .map_err(|e| format!("sync checkpoint file: {e}"))?;
        }

        // Atomic rename
        if let Err(e) = std::fs::rename(&tmp_path, path) {
            // Clean up tmp file on rename failure
            let _ = std::fs::remove_file(&tmp_path);
            return Err(format!("atomic rename checkpoint: {e}"));
        }

        info!(
            "checkpoint saved: {} ({} bytes)",
            path.display(),
            payload.len()
        );

        Ok(())
    }

    #[instrument(skip(self), fields(path = %path.display()))]
    fn load(&self, path: &Path) -> Option<CrawlCheckpoint> {
        let data = read_checkpoint_bytes(path)?;
        verify_and_parse_checkpoint(&data, path)
    }
}

/// Read the raw checkpoint file, returning `None` when it is missing or too
/// small to contain the 4-byte CRC32 header.
fn read_checkpoint_bytes(path: &Path) -> Option<Vec<u8>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            debug!("checkpoint file not readable: {e}");
            return None;
        },
    };

    if data.len() < 4 {
        warn!("checkpoint file too small ({} bytes)", data.len());
        return None;
    }

    Some(data)
}

/// Verify the CRC32 header, deserialize the payload, and apply the schema version
/// gate. Falls back to the legacy pure-JSON schema when the checksum does not
/// match.
///
/// A payload whose `version` is not [`CURRENT_CHECKPOINT_VERSION`] is DISCARDED
/// with an `info!` log — the Gate 0 discard+log contract `ExportState` already
/// follows. Never resumed half-understood, never a crash. Before #1234 the
/// `version` field was written and never read, so that contract was aspirational.
fn verify_and_parse_checkpoint(data: &[u8], path: &Path) -> Option<CrawlCheckpoint> {
    let stored_checksum = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
    let payload = &data[4..];
    let computed_checksum = crc32fast::hash(payload);

    if stored_checksum != computed_checksum {
        if let Some(state) = migrate_legacy_checkpoint(data, path) {
            return Some(state);
        }
        warn!(
            "checkpoint CRC32 mismatch: stored={:#x}, computed={:#x}",
            stored_checksum, computed_checksum
        );
        return None;
    }

    deserialize_checkpoint(payload, path).and_then(accept_version)
}

/// Try to read a legacy pure-JSON checkpoint (no CRC32 header), returning
/// `None` when `data` is not a valid legacy checkpoint.
fn migrate_legacy_checkpoint(data: &[u8], path: &Path) -> Option<CrawlCheckpoint> {
    let old = serde_json::from_slice::<OldCheckpointSchema>(data).ok()?;
    info!(
        "migrated old-format checkpoint: {} (visited={}, pages={})",
        path.display(),
        old.visited.len(),
        old.pages_crawled
    );
    accept_version(old.into())
}

/// Gate 0 discard+log: accept only the current checkpoint schema version.
///
/// A stale version means the on-disk shape predates a schema change, so its
/// fields cannot be interpreted. The run starts fresh; the file is left in place
/// for inspection and the next save overwrites it.
fn accept_version(state: CrawlCheckpoint) -> Option<CrawlCheckpoint> {
    if state.version == CURRENT_CHECKPOINT_VERSION {
        return Some(state);
    }
    info!(
        found = state.version,
        current = CURRENT_CHECKPOINT_VERSION,
        visited = state.visited.len(),
        "checkpoint discarded: superseded schema version, starting fresh"
    );
    None
}

/// Deserialize a CRC32-verified payload, logging a warning on failure.
fn deserialize_checkpoint(payload: &[u8], path: &Path) -> Option<CrawlCheckpoint> {
    match serde_json::from_slice::<CrawlCheckpoint>(payload) {
        Ok(state) => {
            info!(
                "checkpoint loaded: {} (visited={}, queued={}, pages={})",
                path.display(),
                state.visited.len(),
                state.queued.len(),
                state.pages_crawled
            );
            Some(state)
        },
        Err(e) => {
            warn!("checkpoint deserialization failed: {e}");
            None
        },
    }
}

impl BincodeCheckpoint {
    /// Create a new BincodeCheckpoint store.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// Close-time IO helpers (P6-2 slice 2) — one implementation, two owners
// ---------------------------------------------------------------------------

/// Delete the checkpoint file after a fully-completed crawl (F-01).
///
/// Owned by [`CrawlSession::finish`](super::session::CrawlSession::finish)
/// since P6-2 slice 2 (the engine's own close branch retired in slice 4).
/// A missing file is a no-op; any other failure is logged, never fatal — the
/// crawl result was already produced.
pub(crate) fn delete_checkpoint_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {
            debug!(path = %path.display(), "checkpoint removed after successful crawl");
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "checkpoint cleanup failed"
            );
        },
    }
}

/// Persist a checkpoint on a blocking thread (never blocks the event loop).
pub(crate) async fn persist_checkpoint_state(
    store: BincodeCheckpoint,
    state: CrawlCheckpoint,
    path: PathBuf,
) -> Result<Result<(), String>, tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || store.save(&state, &path))
        .in_current_span()
        .await
}

/// Log the outcome of a checkpoint save attempt.
///
/// The messages are load-bearing: snapshots and the benchmark aggregator key
/// on them — change only with tripwire re-verification.
pub(crate) fn log_checkpoint_save(outcome: Result<Result<(), String>, tokio::task::JoinError>) {
    match outcome {
        Ok(Ok(())) => {
            tracing::debug!("checkpoint saved successfully");
        },
        Ok(Err(e)) => {
            tracing::error!(error = %e, "checkpoint save failed");
        },
        // LCOV_EXCL_START defensive: checkpoint-join-error — a JoinError occurs only when the spawned task panicked, a bug
        Err(join_err) => {
            tracing::error!(error = %join_err, "checkpoint save task panicked");
        },
        // LCOV_EXCL_STOP
    }
}

impl Default for BincodeCheckpoint {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable scope hash for one seed URL (F-01): first 8 bytes of SHA-256 over
/// the seed, hex-encoded (16 chars). No IO, deterministic, collision-safe
/// for checkpoint file scoping.
#[must_use]
pub fn seed_hash(seed_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed_url.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Compute the `.tmp` path for atomic writes.
fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Test-only module: `.unwrap()`/`.expect()` below operate on unreachable
    use super::*;
    use proptest::prelude::*;
    use std::fs;
    use tempfile::TempDir;

    fn arb_datetime_utc() -> impl proptest::strategy::Strategy<Value = DateTime<Utc>> {
        (0i64..=100_000_000_000)
            .prop_map(|secs| DateTime::<Utc>::from_timestamp(secs, 0).expect("bounded"))
    }

    fn arb_banned_domain() -> impl proptest::strategy::Strategy<Value = BannedDomain> {
        (
            any::<String>(),
            proptest::option::of(arb_datetime_utc()),
            any::<String>(),
        )
            .prop_map(|(domain, banned_until, reason)| BannedDomain {
                domain,
                banned_until,
                reason,
            })
    }

    fn arb_checkpoint() -> impl proptest::strategy::Strategy<Value = CrawlCheckpoint> {
        (
            proptest::collection::hash_set(any::<String>(), 0..=20),
            proptest::collection::vec(any::<String>(), 0..=20),
            any::<u64>(),
            proptest::collection::vec(arb_banned_domain(), 0..=20),
            proptest::strategy::Just(CURRENT_CHECKPOINT_VERSION),
        )
            .prop_map(
                |(visited, queued, pages_crawled, banned_domains, version)| CrawlCheckpoint {
                    visited,
                    queued,
                    pages_crawled,
                    banned_domains,
                    version,
                },
            )
    }

    fn sample_checkpoint() -> CrawlCheckpoint {
        let mut visited = HashSet::new();
        visited.insert("https://example.com".to_string());
        visited.insert("https://example.com/about".to_string());

        CrawlCheckpoint {
            visited,
            queued: vec![
                "https://example.com/contact".to_string(),
                "https://example.com/blog".to_string(),
            ],
            pages_crawled: 42,
            banned_domains: Vec::new(),
            version: CURRENT_CHECKPOINT_VERSION,
        }
    }

    #[test]
    fn round_trip_save_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();
        let loaded = store.load(&path).unwrap();

        assert_eq!(original, loaded);
    }

    #[test]
    fn empty_state_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = CrawlCheckpoint::new();

        store.save(&original, &path).unwrap();
        let loaded = store.load(&path).unwrap();

        assert_eq!(original, loaded);
        assert!(loaded.visited.is_empty());
        assert_eq!(loaded.pages_crawled, 0);
    }

    #[test]
    fn corruption_detection_tamper_checksum() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();

        // Tamper with the checksum (first 4 bytes)
        let mut data = fs::read(&path).unwrap();
        data[0] ^= 0xFF; // flip bits in checksum
        fs::write(&path, &data).unwrap();

        // Load should return None (corrupted)
        let loaded = store.load(&path);
        assert!(loaded.is_none(), "corrupted checkpoint should return None");
    }

    #[test]
    fn corruption_detection_tamper_payload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let original = sample_checkpoint();

        store.save(&original, &path).unwrap();

        // Tamper with the payload (after the 4-byte checksum)
        let mut data = fs::read(&path).unwrap();
        if data.len() > 4 {
            data[4] ^= 0xFF;
        }
        fs::write(&path, &data).unwrap();

        // Load should return None (corrupted)
        let loaded = store.load(&path);
        assert!(loaded.is_none(), "corrupted payload should return None");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let store = BincodeCheckpoint::new();
        let loaded = store.load(Path::new("/tmp/nonexistent_checkpoint_12345.bin"));
        assert!(loaded.is_none());
    }

    #[test]
    fn load_truncated_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        // Write only 2 bytes (less than CRC32 header)
        fs::write(&path, [0u8; 2]).unwrap();

        let store = BincodeCheckpoint::new();
        let loaded = store.load(&path);
        assert!(loaded.is_none());
    }

    #[test]
    fn tmp_path_convention() {
        let path = Path::new("/tmp/checkpoint.bin");
        let tmp = tmp_path_for(path);
        assert_eq!(tmp, PathBuf::from("/tmp/checkpoint.bin.tmp"));
    }

    #[test]
    fn checksum_is_first_four_bytes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let store = BincodeCheckpoint::new();
        let state = sample_checkpoint();

        store.save(&state, &path).unwrap();

        let data = fs::read(&path).unwrap();
        let stored = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        let payload = &data[4..];
        let computed = crc32fast::hash(payload);

        assert_eq!(stored, computed);
    }

    #[test]
    fn atomic_rename_removes_tmp_on_failure() {
        // Create a read-only directory to force rename failure
        let tmp = TempDir::new().unwrap();
        let readonly_dir = tmp.path().join("readonly");
        fs::create_dir(&readonly_dir).unwrap();

        // Make the parent dir read-only so rename into it fails
        // This test verifies cleanup of .tmp file on rename failure
        let store = BincodeCheckpoint::new();
        let state = CrawlCheckpoint::new();

        // Try saving to a path where rename will fail (read-only parent)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o444)).unwrap();

            let bad_path = readonly_dir.join("checkpoint.bin");
            let result = store.save(&state, &bad_path);

            // Should return error
            assert!(result.is_err());

            // .tmp file should be cleaned up
            let tmp_path = tmp_path_for(&bad_path);
            assert!(
                !tmp_path.exists(),
                ".tmp file should be cleaned up after failed rename"
            );

            // Restore permissions for cleanup
            fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn checkpoint_display() {
        let cp = sample_checkpoint();
        let display = format!("{cp}");
        assert!(display.contains("pages=42"));
        assert!(display.contains("visited=2"));
        assert!(
            display.contains("queued=2"),
            "the bounded frontier is part of the shape"
        );
    }

    // ── Phase 1 RED: Tests for new behavior (types don't exist yet) ──────

    #[test]
    fn test_banned_domains_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        let banned = vec![
            BannedDomain {
                domain: "waf.example.com".into(),
                banned_until: None,
                reason: "WAF challenge".into(),
            },
            BannedDomain {
                domain: "rate.example.com".into(),
                banned_until: Some("2026-12-31T23:59:59Z".parse().unwrap()),
                reason: "rate limit exceeded".into(),
            },
        ];

        let mut original = sample_checkpoint();
        original.banned_domains = banned;

        let store = BincodeCheckpoint::new();
        store.save(&original, &path).unwrap();
        let loaded = store.load(&path).unwrap();

        assert_eq!(loaded.banned_domains.len(), 2);
        assert_eq!(loaded.banned_domains[0].domain, "waf.example.com");
        assert!(loaded.banned_domains[0].banned_until.is_none());
        assert_eq!(loaded.banned_domains[0].reason, "WAF challenge");
        assert_eq!(loaded.banned_domains[1].domain, "rate.example.com");
        assert!(loaded.banned_domains[1].banned_until.is_some());
        assert_eq!(original, loaded);
    }

    #[test]
    fn test_old_format_json_loads() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("checkpoint.json");

        // Write a pure-JSON old-schema file (visited as Vec, no CRC32 header)
        let old_json = r#"{
            "visited": ["https://a.com", "https://b.com"],
            "queued": ["https://c.com"],
            "pages_crawled": 10,
            "version": 1,
            "banned_domains": [
                {
                    "domain": "old.example.com",
                    "banned_until": null,
                    "reason": "WAF challenge"
                }
            ]
        }"#;
        fs::write(&path, old_json).unwrap();

        let store = BincodeCheckpoint::new();
        let loaded = store.load(&path);

        assert!(
            loaded.is_none(),
            "a pre-v2 checkpoint must be discarded, not resumed (#1234)",
        );
    }

    #[test]
    fn test_checkpoint_path_helper() {
        let tmp = TempDir::new().unwrap();
        let cp_path = CheckpointPath::new(tmp.path());
        let file = cp_path.file();
        assert!(file.to_string_lossy().contains("crawl_checkpoint.json"));
        assert!(file.starts_with(tmp.path()));
    }

    #[test]
    fn test_seed_hash_is_stable_and_hex() {
        let a = super::seed_hash("https://example.com/seed");
        let b = super::seed_hash("https://example.com/seed");
        assert_eq!(a, b, "same seed must map to the same scope");
        assert_eq!(a.len(), 16, "8 bytes hex-encoded");
        assert!(
            a.chars().all(|c| c.is_ascii_hexdigit()),
            "scope hash must be hex, got {a}"
        );
    }

    #[test]
    fn test_file_for_seed_scopes_concurrent_sites() {
        // F-01 (a): two seeds sharing one base dir must get disjoint files so
        // concurrent --resume runs cannot collide.
        let tmp = TempDir::new().unwrap();
        let cp_path = CheckpointPath::new(tmp.path());
        let fa = cp_path.file_for_seed("https://a.example/seed");
        let fb = cp_path.file_for_seed("https://b.example/seed");
        assert_ne!(fa, fb, "different seeds must scope to different files");
        assert_eq!(fa, cp_path.file_for_seed("https://a.example/seed"));
        for f in [&fa, &fb] {
            assert!(f.starts_with(tmp.path()));
            let name = f
                .file_name()
                .expect("scoped file has a name")
                .to_string_lossy();
            assert!(
                name.starts_with("crawl_checkpoint_") && name.ends_with(".json"),
                "unexpected scoped name {name}"
            );
        }
    }

    #[test]
    fn test_checkpoint_path_ensure_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deep").join("nested");
        let cp_path = CheckpointPath::new(&nested);
        cp_path.ensure_dir().unwrap();
        assert!(nested.exists());
    }

    /// `ensure_dir` must surface a filesystem failure as `Err` instead of
    /// panicking, so `Engine::with_checkpoint` can disable checkpointing and
    /// keep crawling (#393). Here the base directory sits *under a regular
    /// file*, a location that can never be created as a directory.
    #[test]
    fn test_ensure_dir_returns_err_when_base_dir_is_under_a_file() {
        // Arrange: a regular file where a directory component would need to be.
        let tmp = TempDir::new().expect("tempdir should be created");
        let blocker = tmp.path().join("not_a_dir");
        fs::write(&blocker, "i am a file, not a directory")
            .expect("blocker file should be written");

        // Act: try to ensure a directory nested under the file.
        let cp_path = CheckpointPath::new(blocker.join("nested"));
        let result = cp_path.ensure_dir();

        // Assert: the failure is reported as Err (not a panic), with context.
        let err = result.expect_err("ensure_dir must fail when the path is under a regular file");
        assert!(
            err.contains("failed to create checkpoint dir"),
            "error should carry creation context, got: {err}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        #[cfg_attr(miri, ignore)]
        #[test]
        fn prop_roundtrip_save_load(state in arb_checkpoint()) {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("checkpoint.json");
            let store = BincodeCheckpoint::new();
            store.save(&state, &path).unwrap();
            let loaded = store.load(&path).unwrap();
            prop_assert_eq!(state, loaded);
        }

        #[cfg_attr(miri, ignore)]
        #[test]
        fn prop_corruption_tamper_crc32(
            state in arb_checkpoint(),
            xor_byte in 1u8..=255,
            crc_offset in 0u8..4,
        ) {
            let tmp = TempDir::new().unwrap();
            let path = tmp.path().join("checkpoint.json");
            let store = BincodeCheckpoint::new();
            store.save(&state, &path).unwrap();

            let mut data = fs::read(&path).unwrap();
            data[crc_offset as usize] ^= xor_byte;
            fs::write(&path, &data).unwrap();

            prop_assert!(store.load(&path).is_none());
        }
    }
}
