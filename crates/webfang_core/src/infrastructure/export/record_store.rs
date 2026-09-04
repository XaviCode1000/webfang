//! JSON-per-domain v2 record store (D2, frozen contracts).
//!
//! Persists one [`RawRecord`] per canonical URL under a versioned envelope:
//!
//! ```json
//! { "version": 2, "domain_key": "example.com", "created_at": 0,
//!   "updated_at": 0, "records": { "<canonical_url>": { ... } } }
//! ```
//!
//! # Durability scope (loud non-goal)
//!
//! Writes are atomic against process death via same-directory temp file +
//! [`std::fs::rename`] (rename(2)). There is deliberately NO fsync: a power
//! loss may lose the last rename even though the process observed success.
//! Process-crash durability is the contract; power-loss durability is not.
//!

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::domain::exporter::{DomainRecords, RawRecord};
use crate::domain::page_state::PageStatus;

// Backwards-compat shim (ADR-0012-B 3.H): the record DTOs and their error
// moved to `domain::exporter`; re-export them so `infrastructure::export::*`
// paths keep resolving until the one-minor-version shim window closes.
pub use crate::domain::exporter::RecordStorePort;

/// Envelope format version this module reads and writes.
pub(crate) const CURRENT_VERSION: u32 = 2;

pub use crate::domain::page_state::MIGRATED_V1_RUN_ID;

/// Versioned on-disk envelope (D2, frozen).
#[derive(Debug, Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    domain_key: String,
    created_at: i64,
    updated_at: i64,
    records: DomainRecords,
}

/// Typed failures of the record store. Callers apply the named-path
/// fresh-start policy (`load_or_init`) on [`RecordStoreError::Corrupt`] and
/// [`RecordStoreError::UnsupportedVersion`] — never silently (Gate 2).
///
/// The enum itself lives in `domain::exporter` next to the port it guards;
/// this re-export keeps the historical `record_store::RecordStoreError` path
/// resolving during the shim window (ADR-0012-B 3.H).
pub use crate::domain::exporter::RecordStoreError;

/// Injectable filesystem surface so unit tests can fault-inject typed-error
/// plumbing without real disk state (D6: integration fidelity comes from the
/// SIGKILL harness, mocks only cover error plumbing).
pub(crate) trait StoreFs: Send + Sync {
    /// Create/truncate + write_all + flush (page cache; NO fsync by contract).
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()>;
    /// rename(2) — the atomicity and commit-point primitive.
    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()>;
}

/// Production backend over [`std::fs`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RealFs;

impl StoreFs for RealFs {
    fn write(&self, path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(bytes)?;
        file.flush()
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
}

/// RAII exclusive lock over the store's state file, mirroring the
/// `state_store::StateLock` pattern (#761): on drop the OS lock is released
/// and the `.json.lock` file is deleted so no orphan remains. SIGKILL closes
/// the fd and releases the OS lock (E3); only the empty lock file may linger.
#[must_use]
pub(crate) struct StoreLock {
    handle: fs::File,
    lock_path: PathBuf,
}

impl StoreLock {
    /// Acquire an exclusive advisory lock at `<state>.lock`.
    pub(crate) fn acquire(state_path: &std::path::Path) -> Result<Self, RecordStoreError> {
        let mut lock_path = state_path.as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        let mut lock_file =
            fs::File::create(&lock_path).map_err(|source| RecordStoreError::Io {
                path: lock_path.clone(),
                source,
            })?;
        let _ = write!(lock_file, "pid={}", std::process::id());
        lock_file
            .lock_exclusive()
            .map_err(|e| RecordStoreError::Io {
                path: lock_path.clone(),
                source: std::io::Error::other(format!("failed to acquire record-store lock: {e}")),
            })?;
        Ok(Self {
            handle: lock_file,
            lock_path,
        })
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Best-effort both steps: failure must not mask the real result.
        let _ = FileExt::unlock(&self.handle);
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// JSON-per-domain v2 persistence (D2).
///
/// Construction is deliberately lazy/infallible (#393 contract): strictness
/// lives in [`RecordStore::load`], never in the constructor.
#[derive(Clone)]
pub struct RecordStore {
    domain_key: String,
    state_dir: Option<PathBuf>,
    fs: std::sync::Arc<dyn StoreFs>,
}

impl std::fmt::Debug for RecordStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordStore")
            .field("domain_key", &self.domain_key)
            .field("state_dir", &self.state_dir)
            .finish_non_exhaustive()
    }
}

impl RecordStore {
    /// Create a store for a normalized domain key, rooted at the platform
    /// cache dir (`$CACHE/webfang/state`).
    #[must_use]
    pub fn new(domain_key: impl Into<String>) -> Self {
        Self {
            domain_key: domain_key.into(),
            state_dir: None,
            fs: std::sync::Arc::new(RealFs),
        }
    }

    /// Override the base directory (tests / explicit state dirs).
    #[must_use]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = Some(dir);
        self
    }

    /// Test-only constructor with an injected I/O backend.
    #[cfg(test)]
    pub(crate) fn with_fs(domain_key: &str, dir: PathBuf, fs: std::sync::Arc<dyn StoreFs>) -> Self {
        Self {
            domain_key: domain_key.to_string(),
            state_dir: Some(dir),
            fs,
        }
    }

    /// Full path of this domain's state file.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        let mut path = match &self.state_dir {
            Some(dir) => dir.clone(),
            None => {
                let mut p = dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".cache"));
                p.push("webfang");
                p.push("state");
                p
            },
        };
        path.push(format!("{}.json", self.domain_key));
        path
    }

    fn tmp_path(&self, final_path: &std::path::Path) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let mut name = final_path.file_name().map_or_else(
            || "state.json".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        name.push_str(&format!(".tmp-{}-{}", std::process::id(), nonce));
        final_path.with_file_name(name)
    }

    /// Delete stale `.tmp-*` siblings left by killed processes. Best-effort;
    /// temp files are NEVER read as state.
    fn gc_tmp_files(&self, state_path: &std::path::Path) {
        let prefix = format!(
            "{}.tmp-",
            state_path.file_name().unwrap_or_default().to_string_lossy()
        );
        if let Some(parent) = state_path.parent() {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with(&prefix) {
                        debug!(file = %name.to_string_lossy(), "GC stale record-store tmp file");
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }

    /// Persist records atomically: serialize → same-dir tmp (`<pid>-<nonce>`)
    /// → write_all + flush → rename(2). No fsync — documented non-goal.
    /// An exclusive lock is held across the whole write.
    ///
    /// # Errors
    ///
    /// Returns [`RecordStoreError`] if directory creation, serialization,
    /// writing, or the final rename fails. The previous state file (if any)
    /// is never partially modified.
    pub fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RecordStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let _lock = StoreLock::acquire(&path)?;
        crate::cli::crash_points::hit(crate::cli::crash_points::WHILE_HOLDING_LOCK);
        self.save_locked(records, now_millis())
    }

    /// Save while a lock is already held (shared by [`Self::update`]).
    fn save_locked(
        &self,
        records: &DomainRecords,
        updated_at: i64,
    ) -> Result<(), RecordStoreError> {
        let path = self.state_path();
        let envelope = StoreFile {
            version: CURRENT_VERSION,
            domain_key: self.domain_key.clone(),
            created_at: Self::existing_created_at(&path).unwrap_or_else(now_millis),
            updated_at,
            records: records.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&envelope)
            .map_err(|_| RecordStoreError::Corrupt { path: path.clone() })?;
        let tmp = self.tmp_path(&path);
        // Crash-injection: leave a TRUNCATED tmp before dying (surviving
        // occurrences overwrite with the full payload below).
        if crate::cli::crash_points::is_armed_for(crate::cli::crash_points::MID_STATE_FILE_WRITE) {
            let _ = self.fs.write(&tmp, &bytes[..bytes.len() / 2]);
            crate::cli::crash_points::hit(crate::cli::crash_points::MID_STATE_FILE_WRITE);
        }
        self.fs
            .write(&tmp, &bytes)
            .map_err(|source| RecordStoreError::Io {
                path: tmp.clone(),
                source,
            })?;
        crate::cli::crash_points::hit(crate::cli::crash_points::TMP_WRITTEN_PRE_RENAME);
        if let Err(source) = self.fs.rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(RecordStoreError::Io { path, source });
        }
        debug!(domain = %self.domain_key, records = records.len(), "saved record store");
        Ok(())
    }

    fn existing_created_at(path: &std::path::Path) -> Option<i64> {
        let bytes = fs::read(path).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        value.get("created_at")?.as_i64()
    }

    /// Load records for this domain.
    ///
    /// * Missing file → fresh empty store (nothing to discard).
    /// * Stale `.tmp-*` siblings are garbage-collected, never read.
    /// * Corrupt JSON → [`RecordStoreError::Corrupt`].
    /// * Version 1 → explicit backup-first migration to v2 (D2/SC5).
    /// * Any other version → [`RecordStoreError::UnsupportedVersion`]; no
    ///   silent discard exists (Gate 2). Callers wanting the policy apply
    ///   [`RecordStore::load_or_init`], which warns with the named path.
    ///
    /// # Errors
    ///
    /// See above; per-record invariant violations never error — they are
    /// quarantined with a `warn!` naming url + invariant + path.
    pub fn load(&self) -> Result<DomainRecords, RecordStoreError> {
        let path = self.state_path();
        if !path.exists() {
            debug!(domain = %self.domain_key, "no record store yet; starting fresh");
            return Ok(DomainRecords::new());
        }
        self.gc_tmp_files(&path);
        let bytes = fs::read(&path).map_err(|source| RecordStoreError::Io {
            path: path.clone(),
            source,
        })?;
        let envelope: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| RecordStoreError::Corrupt { path: path.clone() })?;
        match envelope
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u32
        {
            CURRENT_VERSION => {
                let file: StoreFile = serde_json::from_value(envelope)
                    .map_err(|_| RecordStoreError::Corrupt { path: path.clone() })?;
                Ok(Self::validate_and_quarantine(file.records, &path))
            },
            1 => Self::migrate_v1(self, &path, &bytes),
            found => Err(RecordStoreError::UnsupportedVersion { path, found }),
        }
    }

    /// E5 policy wrapper: corrupt or unsupported-version state files are
    /// converted to a fresh empty store — WITH a warning naming the file,
    /// never silently (Gate 2). The original bytes stay on disk for
    /// forensics; only the caller decides to overwrite them later.
    #[must_use]
    pub fn load_or_init(&self) -> DomainRecords {
        match self.load() {
            Ok(records) => records,
            Err(
                err @ (RecordStoreError::Corrupt { .. }
                | RecordStoreError::UnsupportedVersion { .. }),
            ) => {
                tracing::warn!(error = %err, "record store unreadable; starting FRESH per explicit policy — original file preserved for inspection");
                DomainRecords::new()
            },
            Err(RecordStoreError::Io { path, source }) => {
                tracing::warn!(error = %source, file = %path.display(), "record store I/O failure; starting FRESH per explicit policy");
                DomainRecords::new()
            },
            Err(RecordStoreError::Backup { path, source }) => {
                tracing::warn!(error = %source, file = %path.display(), "record store backup failure; starting FRESH per explicit policy");
                DomainRecords::new()
            },
            // Unreachable today: load() only surfaces Io/Corrupt/
            // UnsupportedVersion/Backup; InvalidRecord is produced by the
            // writer-side constructor. Matched exhaustively so a future
            // error variant can never silently fall through (fail-closed).
            Err(err @ RecordStoreError::InvalidRecord { .. }) => {
                tracing::warn!(error = %err, "record store rejected an invalid record; starting FRESH per explicit policy");
                DomainRecords::new()
            },
        }
    }

    /// D2/E6 invariant table applied at the single persistence seam.
    /// Violations are QUARANTINED (dropped from the in-memory view and
    /// re-drivable next run), never panics: `warn!` names url + invariant +
    /// file path. The rule itself lives in the pure ADR-0014 state machine
    /// ([`crate::infrastructure::export::record_transition`]); this wrapper
    /// contributes only the file-path context and the `warn!` I/O.
    fn validate_and_quarantine(records: DomainRecords, path: &std::path::Path) -> DomainRecords {
        let (kept, quarantined) = super::record_transition::partition_valid(records);
        for entry in &quarantined {
            tracing::warn!(
                url = %entry.url,
                invariant = entry.invariant,
                file = %path.display(),
                "quarantining record-store entry with impossible state"
            );
        }
        kept
    }

    /// Explicit v1→v2 migration (Gate 2/SC5): backup FIRST, map every
    /// `processed_urls[i]` to a Committed record under
    /// [`MIGRATED_V1_RUN_ID`], then save v2 atomically. The original v1
    /// bytes stay untouched until the rename succeeds; the backup covers
    /// even that window.
    fn migrate_v1(
        &self,
        path: &std::path::Path,
        original_bytes: &[u8],
    ) -> Result<DomainRecords, RecordStoreError> {
        let legacy: serde_json::Value =
            serde_json::from_slice(original_bytes).map_err(|_| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?;
        let urls: Vec<String> = legacy
            .get("processed_urls")
            .cloned()
            .map(serde_json::from_value::<Vec<String>>)
            .transpose()
            .map_err(|_| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?
            .ok_or_else(|| RecordStoreError::Corrupt {
                path: path.to_path_buf(),
            })?;

        let updated_at = Self::existing_created_at(path).unwrap_or_else(now_millis);
        let mut migrated_mtime = None;
        // File mtime is the design-preferred updated_at when readable.
        if let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) {
            if let Ok(secs) = modified.duration_since(std::time::UNIX_EPOCH) {
                migrated_mtime = Some(i64::try_from(secs.as_millis()).unwrap_or_default());
            }
        }
        let updated_at = migrated_mtime.unwrap_or(updated_at);

        let mut records = DomainRecords::new();
        for url in urls {
            // #876: an empty legacy URL is malformed input of the same
            // class as other per-record invariant violations — quarantined
            // (dropped + warned), never promoted to Committed state. The
            // whole-file Corrupt policy stays reserved for unparseable
            // structure, so one bad entry never discards good neighbors.
            if super::record_transition::is_meaningless_identity(&url) {
                tracing::warn!(
                    file = %path.display(),
                    "v1 migration: dropping empty-string URL from legacy processed_urls (malformed legacy entry)"
                );
                continue;
            }
            records.insert(
                url.clone(),
                RawRecord {
                    url: url.clone(),
                    canonical_url: url,
                    run_id: MIGRATED_V1_RUN_ID.to_string(),
                    content_hash: None,
                    attempts: 1,
                    status: PageStatus::Committed,
                    last_error: None,
                    output_location: None,
                    updated_at,
                },
            );
        }

        // (1) Backup BEFORE anything else touches the live file.
        let stamp = now_millis();
        let backup_path = path.with_file_name(format!(
            "{}.v1.bak.{stamp}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::copy(path, &backup_path).map_err(|source| RecordStoreError::Backup {
            path: backup_path.clone(),
            source,
        })?;

        // (2)+(3) Save the upgraded envelope atomically.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| RecordStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let _lock = StoreLock::acquire(path)?;
        self.save_locked(&records, updated_at)?;
        tracing::info!(
            domain = %self.domain_key,
            migrated = records.len(),
            backup = %backup_path.display(),
            "v1 state file migrated to v2 record store"
        );
        Ok(Self::validate_and_quarantine(records, path))
    }

    /// Count of `COMMITTED` records — the compat replacement for v1's stored
    /// `total_exported` counter (A3): derived, never authoritative. The
    /// rule lives in the pure ADR-0014 state machine; this associated
    /// function is a delegating alias kept for its existing callers.
    #[must_use]
    pub fn derived_total_exported(records: &DomainRecords) -> u64 {
        super::record_transition::derived_total_exported(records)
    }
}

impl RecordStorePort for RecordStore {
    fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError> {
        RecordStore::save(self, records)
    }

    fn load(&self) -> Result<DomainRecords, RecordStoreError> {
        RecordStore::load(self)
    }

    fn load_or_init(&self) -> DomainRecords {
        RecordStore::load_or_init(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::ErrorClass;
    use crate::domain::exporter::LastError;
    use std::fs;
    use tempfile::tempdir;

    fn temp_store(domain: &str) -> (tempfile::TempDir, RecordStore) {
        let dir = tempdir().expect("tempdir");
        let store = RecordStore::new(domain).with_state_dir(dir.path().to_path_buf());
        (dir, store)
    }

    /// Full nine-field fixture; every field populated incl. classified error.
    /// Status is `Extracted` so carrying a `last_error` stays within the
    /// D2 invariant table (only Committed forbids one).
    fn full_record(url: &str) -> RawRecord {
        RawRecord {
            url: url.to_string(),
            canonical_url: url.to_string(),
            run_id: "018f3c1e-7a2b-4c0d-9e1f-2a3b4c5d6e7f".to_string(),
            content_hash: Some("sha256:deadbeef".to_string()),
            attempts: 3,
            status: crate::domain::page_state::PageStatus::Extracted,
            last_error: Some(LastError {
                class: crate::domain::error::ErrorClass::DomainRecoverable,
                message: "chunk exceeded --max-tokens".to_string(),
            }),
            output_location: Some("out/example.md".to_string()),
            updated_at: 1_760_000_000_000,
        }
    }

    // --- SC5: record roundtrip preserves all nine fields -------------------

    #[test]
    fn roundtrip_preserves_all_nine_fields() {
        let (_dir, store) = temp_store("roundtrip.test");
        let mut records = DomainRecords::new();
        records.insert(
            "https://roundtrip.test/a".to_string(),
            full_record("https://roundtrip.test/a"),
        );

        store.save(&records).expect("save must succeed");

        let loaded = store.load().expect("load must succeed");
        assert_eq!(loaded.len(), 1);
        let original = &records["https://roundtrip.test/a"];
        let restored = &loaded["https://roundtrip.test/a"];
        assert_eq!(original.url, restored.url);
        assert_eq!(original.canonical_url, restored.canonical_url);
        assert_eq!(original.run_id, restored.run_id);
        assert_eq!(original.content_hash, restored.content_hash);
        assert_eq!(original.attempts, restored.attempts);
        assert_eq!(original.status, restored.status);
        assert_eq!(original.last_error, restored.last_error);
        assert_eq!(original.output_location, restored.output_location);
        assert_eq!(original.updated_at, restored.updated_at);
    }

    #[test]
    fn serialized_envelope_is_version_2_with_all_fields_present() {
        let (_dir, store) = temp_store("envelope.test");
        let mut records = DomainRecords::new();
        records.insert(
            "https://envelope.test/a".to_string(),
            full_record("https://envelope.test/a"),
        );

        store.save(&records).expect("save must succeed");

        let bytes = fs::read_to_string(store.state_path()).expect("state file readable");
        let value: serde_json::Value = serde_json::from_str(&bytes).expect("valid json");
        assert_eq!(value["version"], 2);
        assert_eq!(value["domain_key"], "envelope.test");
        assert!(value["created_at"].is_i64());
        assert!(value["updated_at"].is_i64());
        let record = &value["records"]["https://envelope.test/a"];
        for field in [
            "url",
            "canonical_url",
            "run_id",
            "content_hash",
            "attempts",
            "status",
            "last_error",
            "output_location",
            "updated_at",
        ] {
            assert!(record.get(field).is_some(), "missing field `{field}`");
        }
        // Frozen field-set: exactly nine keys on the record object.
        let record_obj = record.as_object().expect("record is an object");
        assert_eq!(
            record_obj.len(),
            9,
            "RawRecord must serialize exactly 9 fields"
        );
        assert_eq!(
            record["status"], "EXTRACTED",
            "PageStatus serde is SCREAMING_SNAKE_CASE"
        );
        assert_eq!(record["last_error"]["class"], "domain_recoverable");
    }

    #[test]
    fn unknown_field_is_rejected_on_deserialize() {
        let json = r#"{
            "url": "https://x.test/", "canonical_url": "https://x.test/",
            "run_id": "r", "content_hash": null, "attempts": 1,
            "status": "DISCOVERED", "last_error": null,
            "output_location": null, "updated_at": 1, "bogus": true
        }"#;
        let result: Result<RawRecord, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deny_unknown_fields must reject bogus key");
    }

    #[test]
    fn load_missing_file_returns_empty_records() {
        let (_dir, store) = temp_store("missing.test");
        let loaded = store
            .load()
            .expect("missing file must yield fresh empty store");
        assert!(loaded.is_empty());
    }

    #[test]
    fn new_discovered_rejects_structurally_meaningless_identity() {
        for (url, canonical) in [
            ("", "https://x.test/"),
            ("https://x.test/", ""),
            ("   ", "https://x.test/"),
        ] {
            let err = RawRecord::new_discovered(url, canonical, "run", 0)
                .expect_err("empty identity must be rejected at the domain boundary");
            assert!(
                matches!(err, RecordStoreError::InvalidRecord { .. }),
                "typed InvalidRecord expected, got {err:?}"
            );
        }
    }

    #[test]
    fn new_discovered_yields_honest_fresh_shape_for_valid_url() {
        let record = RawRecord::new_discovered(
            "https://fresh.test/a",
            "https://fresh.test/a",
            "run-1",
            1_760_000_000_000,
        )
        .expect("valid URL constructs");
        assert_eq!(record.url, "https://fresh.test/a");
        assert_eq!(record.canonical_url, "https://fresh.test/a");
        assert_eq!(record.run_id, "run-1");
        assert_eq!(record.status, PageStatus::Discovered);
        assert_eq!(record.attempts, 0);
        assert_eq!(record.content_hash, None);
        assert_eq!(record.last_error, None);
        assert_eq!(record.output_location, None);
        assert_eq!(record.updated_at, 1_760_000_000_000);
    }

    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let (_dir, store) = temp_store("atomic.test");
        let records = DomainRecords::new();
        store.save(&records).expect("save must succeed");

        let state_path = store.state_path();
        let parent = state_path.parent().unwrap();
        let strays: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(
            strays.is_empty(),
            "no tmp files may survive a save: {strays:?}"
        );
    }

    /// D6: fault-injected rename(2) — typed Io error, no partial state.
    struct RenameFailFs;

    impl StoreFs for RenameFailFs {
        fn write(&self, _path: &std::path::Path, _bytes: &[u8]) -> std::io::Result<()> {
            Ok(())
        }

        fn rename(&self, _from: &std::path::Path, _to: &std::path::Path) -> std::io::Result<()> {
            Err(std::io::Error::other("simulated rename failure"))
        }
    }

    #[test]
    fn rename_failure_is_typed_and_leaves_previous_state_intact() {
        let dir = tempdir().unwrap();
        let store = RecordStore::with_fs(
            "fail.test",
            dir.path().to_path_buf(),
            std::sync::Arc::new(RenameFailFs),
        );
        let err = store
            .save(&DomainRecords::new())
            .expect_err("rename failure surfaces");
        assert!(matches!(err, RecordStoreError::Io { .. }));
    }

    #[test]
    fn stale_tmp_files_are_garbage_collected_on_load_and_never_read() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("gc.test").with_state_dir(dir.path().to_path_buf());
        let garbage = dir.path().join("gc.test.json.tmp-123-abc");
        fs::write(&garbage, b"garbage never valid").unwrap();

        store.save(&DomainRecords::new()).unwrap();
        let loaded = store.load().expect("load must succeed and GC tmp siblings");
        assert!(loaded.is_empty());
        assert!(
            !garbage.exists(),
            "stale .tmp-* sibling must be deleted on load"
        );
    }

    // --- SC5: v1→v2 explicit migration, backup-first ---------------------

    /// Real legacy `ExportState` bytes (domain/entities/export.rs v1 shape).
    fn v1_fixture(domain: &str) -> String {
        format!(
            r#"{{"version":1,"domain":"{domain}","processed_urls":["https://{domain}/a","https://{domain}/b/c"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}}"#
        )
    }

    #[test]
    fn v1_state_file_migrates_to_committed_v2_records_with_backup() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("migrate.test").with_state_dir(dir.path().to_path_buf());
        fs::write(store.state_path(), v1_fixture("migrate.test")).unwrap();
        let before = fs::read(store.state_path()).unwrap();

        let loaded = store.load().expect("v1 file must migrate, not error");

        assert_eq!(loaded.len(), 2, "both processed_urls become records");
        for url in ["https://migrate.test/a", "https://migrate.test/b/c"] {
            let record = &loaded[url];
            assert_eq!(
                record.status,
                PageStatus::Committed,
                "v1 processed == committed history"
            );
            assert_eq!(record.run_id, MIGRATED_V1_RUN_ID);
            assert_eq!(record.attempts, 1);
            assert_eq!(record.url, url);
            assert_eq!(record.canonical_url, url);
            assert_eq!(record.last_error, None);
        }

        // Backup exists with original bytes; live file is now v2.
        let state_path = store.state_path();
        let parent = state_path.parent().unwrap();
        let backups: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name().to_string_lossy().contains(".v1.bak."))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup created");
        assert_eq!(
            fs::read(backups[0].path()).unwrap(),
            before,
            "backup preserves original bytes"
        );
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(store.state_path()).unwrap()).unwrap();
        assert_eq!(value["version"], 2, "live file upgraded in place");
    }

    /// #876: an empty-string URL in legacy state is malformed input of the
    /// same class as a per-record invariant violation — quarantined with a
    /// warning, never promoted to Committed, and never at the cost of its
    /// well-formed neighbors (file-level Corrupt stays reserved for
    /// unparseable structure).
    #[test]
    fn v1_migration_drops_empty_urls_but_keeps_neighbors() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("empty.test").with_state_dir(dir.path().to_path_buf());
        let legacy = r#"{"version":1,"domain":"empty.test","processed_urls":["","   ","https://empty.test/good"],"last_export":"2026-08-21T12:00:00Z","total_exported":3}"#.to_string();
        fs::write(store.state_path(), legacy).unwrap();

        let loaded = store
            .load()
            .expect("migration must not fail the whole file");

        assert_eq!(loaded.len(), 1, "only the well-formed neighbor survives");
        assert!(!loaded.contains_key(""), "empty key must not become state");
        assert!(!loaded.contains_key("   "));
        let good = &loaded["https://empty.test/good"];
        assert_eq!(good.status, PageStatus::Committed);
        assert_eq!(good.run_id, MIGRATED_V1_RUN_ID);
    }

    /// #876: the validated constructor is the choke point every new-record
    /// writer passes through — structurally meaningless identity must be
    /// rejected, not persisted as retrievable state.
    #[test]
    fn new_discovered_rejects_empty_and_whitespace_identity() {
        for bad in ["", "   "] {
            let err = RawRecord::new_discovered(bad, "https://x.test/a", "run", 0)
                .expect_err("empty url must be rejected");
            assert!(matches!(err, RecordStoreError::InvalidRecord { .. }));
            let err = RawRecord::new_discovered("https://x.test/a", bad, "run", 0)
                .expect_err("empty canonical_url must be rejected");
            assert!(matches!(err, RecordStoreError::InvalidRecord { .. }));
        }
        RawRecord::new_discovered("https://x.test/a", "https://x.test/a", "run", 0)
            .expect("valid identity must construct");
    }

    #[test]
    fn migrated_records_survive_invariant_validation_despite_null_hash() {
        // Migrated records are Committed with content_hash/output_location
        // = None: the MIGRATED_V1_RUN_ID exemption must keep them.
        let dir = tempdir().unwrap();
        let store = RecordStore::new("exempt.test").with_state_dir(dir.path().to_path_buf());
        fs::write(store.state_path(), v1_fixture("exempt.test")).unwrap();

        let loaded = store.load().expect("migration succeeds");
        assert_eq!(loaded.len(), 2, "no migrated record may be quarantined");
    }

    #[test]
    fn second_load_after_migration_reads_v2_without_remigrating() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("idem.test").with_state_dir(dir.path().to_path_buf());
        fs::write(store.state_path(), v1_fixture("idem.test")).unwrap();
        store.load().expect("first load migrates");
        let loaded = store.load().expect("second load reads v2");
        assert_eq!(loaded.len(), 2);
        let state_path = store.state_path();
        let parent = state_path.parent().unwrap();
        let backups = fs::read_dir(parent).unwrap().filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".v1.bak.")
        });
        assert_eq!(backups.count(), 1, "no second backup on idempotent re-read");
    }

    // --- SC5/E6: invariant table quarantines impossible states ------------

    #[test]
    fn committed_record_without_output_location_is_quarantined() {
        let (_dir, store) = temp_store("quarantine.test");
        let mut record = full_record("https://quarantine.test/bad");
        record.status = PageStatus::Committed;
        record.output_location = None;
        let mut records = DomainRecords::new();
        records.insert(record.canonical_url.clone(), record.clone());
        store
            .save(&records)
            .expect("save succeeds — corruption is logical, not structural");

        let loaded = store
            .load()
            .expect("load never panics nor errors on bad record");
        assert!(
            loaded.is_empty(),
            "invariant-violating record is quarantined"
        );
    }

    // --- #876: empty-string URL is structurally meaningless identity ----

    #[test]
    fn v1_empty_string_url_is_quarantined_never_committed() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("empty-v1.test").with_state_dir(dir.path().to_path_buf());
        let legacy = r#"{"version":1,"domain":"empty-v1.test","processed_urls":["","https://empty-v1.test/ok"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}"#;
        fs::write(store.state_path(), legacy).unwrap();

        let loaded = store
            .load()
            .expect("empty legacy URL is a per-record violation, not file corruption");

        assert_eq!(
            loaded.len(),
            1,
            "only the valid URL may become a migrated record"
        );
        assert!(
            !loaded.contains_key(""),
            "an empty URL must never be retrievable committed state"
        );
        assert!(loaded.contains_key("https://empty-v1.test/ok"));
    }

    #[test]
    fn persisted_empty_url_record_is_quarantined_on_load() {
        let (_dir, store) = temp_store("empty-v2.test");
        let mut bad = full_record("");
        bad.status = PageStatus::Committed;
        // Empty URL must be the ONLY violation here: satisfy every other
        // committed invariant so this test isolates the #876 gap.
        bad.last_error = None;
        let good = full_record("https://empty-v2.test/good");
        let mut records = DomainRecords::new();
        records.insert(bad.url.clone(), bad);
        records.insert(good.url.clone(), good);
        store
            .save(&records)
            .expect("save succeeds; rejection happens at the load boundary");

        let loaded = store
            .load()
            .expect("load never errors on per-record violations");

        assert!(
            !loaded.contains_key(""),
            "an empty-URL record must be quarantined on load"
        );
        assert_eq!(loaded.len(), 1, "the valid neighbor survives quarantine");
    }

    #[test]
    fn v1_whitespace_only_url_is_quarantined_never_committed() {
        let dir = tempdir().unwrap();
        let store = RecordStore::new("ws-v1.test").with_state_dir(dir.path().to_path_buf());
        let legacy = r#"{"version":1,"domain":"ws-v1.test","processed_urls":["   ","https://ws-v1.test/ok"],"last_export":"2026-08-21T12:00:00Z","total_exported":2}"#;
        fs::write(store.state_path(), legacy).unwrap();

        let loaded = store
            .load()
            .expect("whitespace-only URL is a per-record violation, not file corruption");

        assert_eq!(loaded.len(), 1, "only the valid URL survives");
        assert!(!loaded.contains_key("   "));
    }

    #[test]
    fn committed_record_with_last_error_is_quarantined_but_valid_neighbors_survive() {
        let (_dir, store) = temp_store("quarantine-mix.test");
        let mut bad = full_record("https://q.test/bad");
        bad.status = PageStatus::Committed;
        bad.last_error = Some(LastError {
            class: ErrorClass::InternalFatal,
            message: "impossible state".to_string(),
        });
        let good = full_record("https://q.test/good");
        let mut records = DomainRecords::new();
        records.insert(bad.canonical_url.clone(), bad);
        records.insert(good.canonical_url.clone(), good);
        store.save(&records).unwrap();

        let loaded = store.load().expect("load succeeds");
        assert_eq!(loaded.len(), 1, "only the valid neighbor survives");
        assert!(loaded.contains_key("https://q.test/good"));
    }

    // --- E5: load_or_init fresh-start policy with named-path warning ------

    #[test]
    fn corrupt_file_yields_fresh_store_via_load_or_init() {
        let (_dir, store) = temp_store("corrupt.test");
        fs::write(store.state_path(), b"{{{{not json").unwrap();

        assert!(
            matches!(store.load(), Err(RecordStoreError::Corrupt { .. })),
            "raw load surfaces the typed error"
        );
        let loaded = store.load_or_init();
        assert!(loaded.is_empty(), "policy converts to fresh start");
    }

    #[test]
    fn unsupported_version_never_silently_discards_via_load_or_init() {
        let (_dir, store) = temp_store("future.test");
        let future = r#"{"version":99,"records":{}}"#;
        fs::write(store.state_path(), future).unwrap();

        assert!(matches!(
            store.load(),
            Err(RecordStoreError::UnsupportedVersion { found: 99, .. })
        ));
        let loaded = store.load_or_init();
        assert!(loaded.is_empty(), "explicit policy applies");
        assert!(
            store.state_path().exists(),
            "original file left untouched for forensics"
        );
    }

    #[test]
    fn deterministic_serialization_btree_ordering_stable_diffs() {
        let (_dir, store) = temp_store("stable.test");
        let mut records = DomainRecords::new();
        for url in ["https://stable.test/z", "https://stable.test/a"] {
            let mut r = full_record(url);
            r.status = crate::domain::page_state::PageStatus::Discovered;
            records.insert(url.to_string(), r);
        }
        store.save(&records).unwrap();
        let first = fs::read_to_string(store.state_path()).unwrap();

        store.save(&records).unwrap();
        let second = fs::read_to_string(store.state_path()).unwrap();
        let first_value: serde_json::Value =
            serde_json::from_str(&first).expect("first serialization is valid JSON");
        let second_value: serde_json::Value =
            serde_json::from_str(&second).expect("second serialization is valid JSON");
        assert_eq!(
            first_value["records"], second_value["records"],
            "identical records must serialize identically (BTree ordering stable)"
        );
        assert!(
            first_value["updated_at"].is_i64() && second_value["updated_at"].is_i64(),
            "updated_at must be present as i64 in both serializations"
        );
        let a_pos = first.find("\"https://stable.test/a\"").expect("a present");
        let z_pos = first.find("\"https://stable.test/z\"").expect("z present");
        assert!(
            a_pos < z_pos,
            "BTreeMap ordering must be sorted for stable diffs"
        );
    }
}
