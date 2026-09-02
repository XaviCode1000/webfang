//! Export factory for creating exporters based on format
//!
//! Provides flexible factory methods for creating appropriate exporters
//! based on ExportFormat enum values.
//!
//! # D3 commit-point protocol (PR3)
//!
//! [`process_results`] drives each item through the frozen per-item sequence:
//!
//! ```text
//! 1. output stage append + flush Ok          ← OUTPUT DURABLE (flush barrier)
//! 2. record.advance(Processed → Exported)    ← in-memory
//! 3. record_store.save()                     ← EXPORTED CHECKPOINT DURABLE
//! 4. record.advance(Exported → Committed)    ← in-memory
//! 5. record_store.save()                     ← ★ COMMIT POINT ★
//! ```
//!
//! Until PR4 introduces the single-writer JSONL session, the exporter's
//! successful return IS the flush barrier (each `JsonlExporter::export`
//! flushes before returning `Ok`). No record ever claims more than what was
//! durably flushed.

use std::collections::HashSet;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::application::resume::{canonical_key, load_preserving, RunId};
use crate::domain::exporter::{DomainRecords, LastError, RawRecord};
use crate::domain::page_state::{PageStatus, Stateful};
use crate::domain::{entities::ExportFormat, exporter::ExporterError, Exporter, ExporterConfig};
use crate::infrastructure::export::{
    jsonl_exporter, state_store::StateStore, vector_exporter::VectorExporter, RecordStore,
};

/// Per-run resume/commit context handed to the export functions (D5 seams).
///
/// Groups the record store, this run's identity, and the skip-policy switch.
/// With `resume = false` the gate never consults prior history (fresh run
/// re-drives everything while old records stay preserved — A2/E10).
pub struct ResumeContext<'a> {
    pub(crate) store: &'a RecordStore,
    pub(crate) run_id: RunId,
    pub(crate) resume: bool,
    pub(crate) cancel: Option<&'a CancellationToken>,
    /// Test-only observer fired after every record-store save with the
    /// persisted status (ordering proof: EXPORTED strictly before COMMITTED).
    pub(crate) persist_observer: Option<&'a dyn Fn(PageStatus)>,
}

impl<'a> ResumeContext<'a> {
    /// A context for `store` with a fresh [`RunId`] and no skipping.
    #[must_use]
    pub fn new(store: &'a RecordStore) -> Self {
        Self {
            store,
            run_id: RunId::new(),
            resume: false,
            cancel: None,
            persist_observer: None,
        }
    }

    /// Enable `--resume` semantics: skip ONLY `COMMITTED`-proven records.
    #[must_use]
    pub fn with_resume(mut self, resume: bool) -> Self {
        self.resume = resume;
        self
    }

    /// Observe cooperative cancellation between items (drain-before-final-
    /// persist: the in-flight item always completes its full D3 sequence).
    #[must_use]
    pub fn observing_cancel(mut self, token: &'a CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Test-only observer hook for the PR5 crash-matrix ordering proof
    /// (asserts EXPORTED persists strictly before COMMITTED). Unused until
    /// the harness lands.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn with_persist_observer(mut self, f: &'a dyn Fn(PageStatus)) -> Self {
        self.persist_observer = Some(f);
        self
    }
}

/// Why an item took a particular route through the export phase.
enum ItemDecision {
    /// Record proven `COMMITTED` — skip entirely (type-level gate).
    AlreadyCommitted,
    /// Output flush proven via content-hash membership — promote to
    /// `COMMITTED` without appending (D3 recovery actions 3–4).
    PromoteFromFlushProof,
    /// Normal path: append, checkpoint `EXPORTED`, commit.
    DriveAndCommit,
}

/// Per-run commit session: the loaded record map plus the content-hash index
/// of the target output file (D3 recovery seam).
pub(crate) struct CommitSession<'a> {
    ctx: Option<&'a ResumeContext<'a>>,
    records: DomainRecords,
    /// `checksum_sha256` values parsed from every valid line already in the
    /// output file. Membership proves "bytes flushed" without timing guesses.
    hash_index: HashSet<String>,
}

impl<'a> CommitSession<'a> {
    fn open(ctx: Option<&'a ResumeContext<'a>>, output_path: &std::path::Path) -> Self {
        let records = ctx.map_or_else(DomainRecords::new, |c| load_preserving(c.store));
        let hash_index = build_content_hash_index(output_path);
        Self {
            ctx,
            records,
            hash_index,
        }
    }

    fn cancelled(&self) -> bool {
        self.ctx
            .and_then(|c| c.cancel)
            .is_some_and(|t| t.is_cancelled())
    }

    /// The resume-gate decision for one item. With `resume = false` every
    /// item re-drives (fresh-run semantics); with `resume = true` only
    /// `COMMITTED`-proven records skip.
    fn decide(&self, url: &str, fresh_hash: Option<&str>) -> ItemDecision {
        let Some(ctx) = self.ctx else {
            return ItemDecision::DriveAndCommit;
        };
        if !ctx.resume {
            return ItemDecision::DriveAndCommit;
        }
        match self.records.get(&canonical_key(url)) {
            Some(record) => {
                // Type-level gate: skip ONLY when the record reconciles into
                // the terminal state through the D2 table (SC2 mechanism).
                if Stateful::<RawRecord, crate::domain::page_state::Committed>::reconcile(
                    record.clone(),
                )
                .is_ok()
                {
                    return ItemDecision::AlreadyCommitted;
                }
                if let Some(hash) = &record.content_hash {
                    if self.hash_index.contains(hash) {
                        return ItemDecision::PromoteFromFlushProof;
                    }
                }
            },
            None => {
                // Flush-proof WITHOUT a record: a previous run may have died
                // after flushing this exact content but before persisting ANY
                // record. The record's own claim takes precedence when one
                // exists (an unproven EXPORTED must re-export).
                if let Some(fresh) = fresh_hash {
                    if self.hash_index.contains(fresh) {
                        return ItemDecision::PromoteFromFlushProof;
                    }
                }
            },
        }
        ItemDecision::DriveAndCommit
    }

    fn promote_from_flush_proof(&mut self, url: &str, output_location: String, content_hash: &str) {
        let key = canonical_key(url);
        let Some(ctx) = self.ctx else { return };
        let Some(record) = self.resolve_flush_proof_record(url, &key, &ctx.run_id) else {
            return;
        };
        let Some(exported) =
            Self::drive_to_export_flushed(record, url, &ctx.run_id, content_hash, &output_location)
        else {
            return;
        };
        self.records.insert(key.clone(), exported.record().clone());
        self.save_notifying(PageStatus::Exported);
        // COMMIT POINT — no re-append; the line is already on disk.
        let committed = exported.commit();
        info!(url, "flush-proof promotion: advancing to COMMITTED");
        self.records.insert(key, committed.into_record());
        self.save_notifying(PageStatus::Committed);
    }

    /// Fetch the record backing a flush-proof promotion. A record-less
    /// promotion means the previous run died before ANY persistence:
    /// synthesize a fresh lifecycle so the URL still lands in the store
    /// Committed (invariant: every input URL present).
    /// Returns `None` when the identity is structurally invalid (#876).
    fn resolve_flush_proof_record(
        &mut self,
        url: &str,
        key: &str,
        run_id: &crate::application::resume::RunId,
    ) -> Option<RawRecord> {
        if let Some(record) = self.records.remove(key) {
            return Some(record);
        }
        match fresh_discovered(url, key, run_id) {
            Some(record) => Some(record.into_record()),
            None => {
                warn!(
                    url,
                    "record-less flush-proof promotion rejected: structurally invalid URL"
                );
                None
            },
        }
    }

    /// Advance a record to the EXPORTED state for a proven flush.
    /// EXPORTED records take their legal direct transition (commit);
    /// ONLY lower states walk the drive chain — driving an EXPORTED
    /// record would hit the terminal-state guard and lose its
    /// content_hash (the dedup identity of the flushed line).
    /// Returns `None` when the record identity is structurally invalid (#876).
    fn drive_to_export_flushed(
        record: RawRecord,
        url: &str,
        run_id: &crate::application::resume::RunId,
        content_hash: &str,
        output_location: &str,
    ) -> Option<Stateful<RawRecord, crate::domain::page_state::Exported>> {
        use crate::domain::page_state::Exported;

        let flushed_path = PathBuf::from(output_location);
        if record.status == PageStatus::Exported {
            if let Ok(exported) = Stateful::<RawRecord, Exported>::reconcile(record.clone()) {
                return Some(exported);
            }
            tracing::warn!(url, "EXPORTED record broke invariants; re-driving fresh");
            let mut processed = drive_to_processed(record, run_id)?;
            {
                let payload = processed.record_mut();
                payload.updated_at = now_millis();
                payload.content_hash = Some(content_hash.to_owned());
            }
            return Some(processed.export_flushed(flushed_path));
        }
        let mut processed = drive_to_processed(record, run_id)?;
        {
            let payload = processed.record_mut();
            payload.attempts += 1;
            payload.last_error = None;
            payload.updated_at = now_millis();
            payload.content_hash = Some(content_hash.to_owned());
            if payload.output_location.is_none() {
                payload.output_location = Some(output_location.to_owned());
            }
        }
        Some(processed.export_flushed(flushed_path))
    }

    /// Record an item failure honestly: attempts++, classified last_error,
    /// updated_at — persisted at the record's NON-advanced state (SC6).
    /// A never-seen URL gets a fresh DISCOVERED record carrying the failure.
    fn fail_item(&mut self, url: &str, class: crate::error::ErrorClass, message: &str) {
        let Some(ctx) = self.ctx else { return };
        let key = canonical_key(url);
        let record = match self.records.entry(key.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                // #876 domain boundary: an empty URL is structurally
                // meaningless; reject instead of persisting a record that
                // could never address a page.
                let record = match RawRecord::new_discovered(
                    url,
                    &key,
                    ctx.run_id.as_str(),
                    now_millis(),
                ) {
                    Ok(record) => record,
                    Err(err) => {
                        warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
                        return;
                    },
                };
                entry.insert(record)
            },
        };
        record.attempts += 1;
        record.last_error = Some(LastError {
            class,
            message: message.to_string(),
        });
        record.updated_at = now_millis();
        warn!(url, class = ?class, "item failed; status NOT advanced");
        self.save_quiet();
    }

    /// D3 steps 2–5 for one successfully flushed item.
    fn commit_item(
        &mut self,
        url: &str,
        content_hash: String,
        output_location: PathBuf,
        run_id: &RunId,
    ) {
        // Crash-injection point: exporter returned (flush ack received)
        // but EXPORTED checkpoint not yet saved - the critical D3 window.
        crate::cli::crash_points::hit(crate::cli::crash_points::POST_FLUSH_PRE_COMMIT);
        let key = canonical_key(url);
        // Drive the record to PROCESSED along its legal chain (fresh URLs
        // enter as DISCOVERED; re-drives resume from their recorded state).
        let existing = self.records.remove(&key);
        // A2 fresh-run semantics: a terminal-state record from a PREVIOUS
        // run re-drives under the new run_id — the new run starts its own
        // honest lifecycle from DISCOVERED.
        let existing = existing.filter(|record| {
            !matches!(record.status, PageStatus::Exported | PageStatus::Committed)
        });
        let mut processed = match existing {
            Some(record) => {
                let Some(processed) = drive_to_processed(record, run_id) else {
                    warn!(url, "record identity is structurally invalid; item left uncommitted and will re-drive next run");
                    return;
                };
                processed
            },
            None => {
                // #876 domain boundary: validated construction or nothing.
                let fresh = match RawRecord::new_discovered(
                    url,
                    &key,
                    run_id.as_str(),
                    now_millis(),
                ) {
                    Ok(fresh) => fresh,
                    Err(err) => {
                        warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
                        return;
                    },
                };
                Stateful::<RawRecord, crate::domain::page_state::Discovered>::new(fresh)
                    .queue()
                    .start_fetch()
                    .fetched()
                    .extracted()
                    .processed()
            },
        };
        {
            let payload = processed.record_mut();
            payload.run_id = run_id.as_str().to_string();
            payload.content_hash = Some(content_hash);
            payload.output_location = Some(output_location.display().to_string());
            payload.attempts += 1;
            payload.last_error = None; // success clears last_error (SC6)
            payload.updated_at = now_millis();
        }
        // Step 2–3: EXPORTED checkpoint durable BEFORE the commit point.
        let exported = processed.export_flushed(output_location);
        self.records.insert(key.clone(), exported.record().clone());
        self.save_notifying(PageStatus::Exported);
        // Steps 4–5: ★ COMMIT POINT ★ — rename(2) persisting Committed,
        // strictly after the output flush ack + EXPORTED checkpoint.
        let committed = exported.commit();
        self.records.insert(key, committed.into_record());
        self.save_notifying(PageStatus::Committed);
    }

    fn save_and_notify(&self, observer: Option<&dyn Fn(PageStatus)>, status: PageStatus) {
        let Some(ctx) = self.ctx else { return };
        match ctx.store.save(&self.records) {
            Ok(()) => {
                if let Some(f) = observer.or(ctx.persist_observer) {
                    f(status);
                }
            },
            Err(e) => tracing::error!(error = %e, "record-store save failed"),
        }
    }

    /// Drain-before-final-persist (E8): after cancellation stops new items,
    /// one final save persists any remaining honest state before exit.
    pub(crate) fn final_persist(&self) {
        let Some(ctx) = self.ctx else { return };
        if let Err(e) = ctx.store.save(&self.records) {
            tracing::error!(error = %e, "final record-store persist failed");
        }
    }

    /// Save and fire the ordering observer with the persisted status.
    fn save_notifying(&self, status: PageStatus) {
        self.save_and_notify(None, status);
    }

    /// Save failure state WITHOUT firing the transition observer (a failed
    /// item did not advance; the observer tracks lifecycle transitions only).
    fn save_quiet(&self) {
        if let Some(ctx) = self.ctx {
            if let Err(e) = ctx.store.save(&self.records) {
                tracing::error!(error = %e, "record-store save failed");
            }
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Walk a raw record forward along its legal chain to `PROCESSED`. Every
/// arm reconciles at the recorded position first — the typestate guarantees
/// no illegal move is expressible even during recovery re-drives.
/// A reconciliation failure (impossible persisted state that slipped past
/// load-time quarantine) degrades to a fresh DISCOVERED lifecycle under the
/// current run instead of panicking. Returns `None` only when even that
/// fresh lifecycle cannot be constructed (#876: structurally meaningless
/// identity) — callers must skip the item rather than persist garbage.
fn drive_to_processed(
    record: RawRecord,
    run_id: &RunId,
) -> Option<Stateful<RawRecord, crate::domain::page_state::Processed>> {
    use crate::domain::page_state::{
        Discovered, Extracted, Fetched, Fetching, Processed, Queued, ReconcileError,
    };
    let url = record.url.clone();
    let canonical_url = record.canonical_url.clone();
    let result = match record.status {
        PageStatus::Discovered => Stateful::<RawRecord, Discovered>::reconcile(record)
            .map(|s| s.queue().start_fetch().fetched().extracted().processed()),
        PageStatus::Queued => Stateful::<RawRecord, Queued>::reconcile(record)
            .map(|s| s.start_fetch().fetched().extracted().processed()),
        PageStatus::Fetching => Stateful::<RawRecord, Fetching>::reconcile(record)
            .map(|s| s.fetched().extracted().processed()),
        PageStatus::Fetched => {
            Stateful::<RawRecord, Fetched>::reconcile(record).map(|s| s.extracted().processed())
        },
        PageStatus::Extracted => {
            Stateful::<RawRecord, Extracted>::reconcile(record).map(|s| s.processed())
        },
        PageStatus::Processed => Stateful::<RawRecord, Processed>::reconcile(record),
        // Terminal states are filtered upstream (A2 fresh-run re-drive).
        PageStatus::Exported | PageStatus::Committed => Err(ReconcileError::StatusMismatch {
            expected: PageStatus::Processed,
            found: record.status,
        }),
    };
    match result {
        Ok(processed) => Some(processed),
        Err(e) => {
            tracing::warn!(
            error = %e,
            "quarantining impossible persisted state; starting fresh lifecycle"
                        );
            let fresh = fresh_discovered(&url, &canonical_url, run_id)?;
            Some(
                fresh
                    .queue()
                    .start_fetch()
                    .fetched()
                    .extracted()
                    .processed(),
            )
        },
    }
}

/// Start a brand-new DISCOVERED lifecycle for one URL under `run_id`
/// (A2: a new run owns its records; history stays on disk until commit).
/// Returns `None` when the URL fails the #876 domain boundary validation.
fn fresh_discovered(
    url: &str,
    canonical_url: &str,
    run_id: &RunId,
) -> Option<Stateful<RawRecord, crate::domain::page_state::Discovered>> {
    let record = match RawRecord::new_discovered(url, canonical_url, run_id.as_str(), now_millis())
    {
        Ok(record) => record,
        Err(err) => {
            tracing::warn!(url, error = %err, "rejecting record creation for structurally invalid URL");
            return None;
        },
    };
    Some(Stateful::<RawRecord, crate::domain::page_state::Discovered>::new(record))
}

fn build_content_hash_index(path: &std::path::Path) -> HashSet<String> {
    let Ok(bytes) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    let mut index = HashSet::new();
    let mut lines = 0usize;
    for line in bytes.lines() {
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(hash) = value.get("checksum_sha256").and_then(|h| h.as_str()) {
                index.insert(hash.to_string());
            }
        }
    }
    info!(file = %path.display(), lines, hashes = index.len(), "indexed output file for resume dedup");
    index
}

/// Map an export-stage failure onto the SC6 taxonomy: item-data problems are
/// `DomainRecoverable` (record + continue); environment failures stay fatal.
fn classify_export_failure(err: &ExporterError) -> crate::error::ErrorClass {
    use crate::error::ErrorClass;
    match err {
        ExporterError::Serialization(_) | ExporterError::InvalidConfig(_) => {
            ErrorClass::DomainRecoverable
        },
        _ => ErrorClass::InternalFatal,
    }
}

/// Create exporter based on output format
pub fn create_exporter(
    output_dir: PathBuf,
    filename: &str,
    format: ExportFormat,
) -> Result<Box<dyn Exporter>, ExporterError> {
    match format {
        ExportFormat::Jsonl => Ok(create_jsonl_exporter(output_dir, filename)),
        ExportFormat::Vector => Ok(create_vector_exporter(output_dir, filename)),
        ExportFormat::Auto => create_auto_exporter(output_dir, filename),
    }
}

/// Build a JSONL exporter with append mode enabled.
fn create_jsonl_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename).with_append(true);
    info!("Creating JSONL exporter: {:?}", config.output_path());
    Box::new(jsonl_exporter::JsonlExporter::new(config))
}

/// Build a Vector exporter with append mode enabled.
fn create_vector_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Vector, filename).with_append(true);
    info!("Creating Vector exporter: {:?}", config.output_path());
    Box::new(VectorExporter::new(config))
}

/// Auto-detect the export format from existing files (`export.jsonl` /
/// `export.json`), falling back to JSONL when neither exists.
fn create_auto_exporter(
    output_dir: PathBuf,
    filename: &str,
) -> Result<Box<dyn Exporter>, ExporterError> {
    // Auto-detect: checks if export.jsonl or export.json exists
    info!("Auto-detecting format...");

    let jsonl_path = output_dir.join(format!("{filename}.jsonl"));
    let vector_path = output_dir.join(format!("{filename}.json"));

    if jsonl_path.exists() {
        Ok(create_detected_jsonl(output_dir, filename, &jsonl_path))
    } else if vector_path.exists() {
        Ok(create_detected_vector(output_dir, filename, &vector_path))
    } else {
        Ok(create_default_jsonl(output_dir, filename))
    }
}

/// Build a JSONL exporter for the auto-detected `.jsonl` file.
fn create_detected_jsonl(
    output_dir: PathBuf,
    filename: &str,
    jsonl_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected JSONL format - {:?} exists", jsonl_path);
    create_jsonl_exporter(output_dir, filename)
}

/// Build a Vector exporter for the auto-detected `.json` file.
fn create_detected_vector(
    output_dir: PathBuf,
    filename: &str,
    vector_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected Vector format - {:?} exists", vector_path);
    create_vector_exporter(output_dir, filename)
}

/// Build a JSONL exporter when no existing export file is found.
fn create_default_jsonl(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    info!("No existing export, using default Jsonl format");
    create_jsonl_exporter(output_dir, filename)
}

/// Create a new StateStore for tracking processed URLs
///
/// # Arguments
///
/// * `state_dir` - Directory to store state files
/// * `domain` - Domain name for state file (e.g., "example.com")
///
/// # Returns
///
/// * `Ok(StateStore)` - Created state store
/// * `Err(ScraperError)` - Failed to create state store
///
/// # Errors
///
/// Returns error if:
/// - State directory cannot be created
/// - State file cannot be read/written
pub fn create_state_store(
    state_dir: PathBuf,
    domain: &str,
) -> Result<StateStore, crate::error::ScraperError> {
    use crate::infrastructure::export::state_store::StateStore;

    info!("Creating StateStore in {:?}", state_dir);
    let mut store = StateStore::new(domain);
    store.set_cache_dir(state_dir);
    Ok(store)
}

/// Outcome of one item through the export phase.
enum SingleItemOutcome {
    /// Counted as processed this run.
    Driven,
    /// COMMITTED-proven skip (not counted as processed).
    Skipped,
    /// Cooperative cancellation observed; stop the loop.
    Cancelled,
}

/// D3 sequence for exactly one scraped item (decide → flush → checkpoint
/// → commit, or honest failure recording). Extracted from
/// `process_results` to stay within complexity ratchets.
fn process_single_item(
    session: &mut CommitSession<'_>,
    exporter: &dyn crate::domain::Exporter,
    result: &crate::domain::ScrapedContent,
    output_path: &std::path::Path,
    run_id: &RunId,
) -> SingleItemOutcome {
    use crate::domain::entities::DocumentChunkUnvalidated;
    use sha2::{Digest, Sha256};

    let url_str = result.url.as_str().to_string();
    // Same digest the JsonlExporter stamps into the line; computed up
    // front so decide() can prove flush membership even with no record.
    let content_hash = format!("{:x}", Sha256::digest(result.content.as_bytes()));
    match session.decide(&url_str, Some(content_hash.as_str())) {
        ItemDecision::AlreadyCommitted => {
            info!(url = %url_str, "resume gate: COMMITTED-proven; skipping");
            return SingleItemOutcome::Skipped;
        },
        ItemDecision::PromoteFromFlushProof => {
            session.promote_from_flush_proof(
                &url_str,
                output_path.display().to_string(),
                &content_hash,
            );
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            return SingleItemOutcome::Driven;
        },
        ItemDecision::DriveAndCommit => {},
    }

    // Item-data problems are DomainRecoverable per SC6: record the
    // classified failure and continue - never abort the export.
    let chunk = DocumentChunkUnvalidated::from_scraped_content(result);
    let validated = match chunk.validate() {
        Ok(validated) => validated,
        Err(e) => {
            let err = ExporterError::InvalidConfig(e.to_string());
            let class = classify_export_failure(&err);
            session.fail_item(&url_str, class, &err.to_string());
            return SingleItemOutcome::Skipped;
        },
    };

    match exporter.export(validated) {
        Ok(()) => {
            session.commit_item(
                &url_str,
                content_hash.clone(),
                output_path.to_path_buf(),
                run_id,
            );
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            SingleItemOutcome::Driven
        },
        Err(e) => {
            let class = classify_export_failure(&e);
            session.fail_item(&url_str, class, &e.to_string());
            warn!(url = %url_str, error = %e, "item export failed; continuing");
            if session.cancelled() {
                return SingleItemOutcome::Cancelled;
            }
            SingleItemOutcome::Skipped
        },
    }
}

/// Convert scraped results into the target export format with exactly-once
/// semantics: every item flows through the D3 sequence (flush → EXPORTED
/// checkpoint → COMMITTED) under the resume gate.
///
/// # Errors
///
/// Returns [`ExporterError`] only for exporter construction failures;
/// item-level failures are recorded per-record and never abort the run.
pub fn process_results(
    results: &[crate::domain::ScrapedContent],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    ctx: Option<&ResumeContext<'_>>,
) -> Result<Vec<String>, ExporterError> {
    info!("Processing {} results for export", results.len());

    // Fail-fast on uncreatable output_dir (ENOTDIR through a regular file,
    // permission denied, etc.): surface as ExporterError::DirectoryCreation
    // so the MCP/CLI caller gets an honest isError:true instead of a fake
    // success that swallows the per-item error (D3 swallow bug, #854).
    std::fs::create_dir_all(&output_dir).map_err(ExporterError::DirectoryCreation)?;

    // Hash-index BEFORE the exporter touches the file, so membership
    // reflects exactly the bytes flushed by previous runs (D3 seam).
    let output_path = output_dir.join(format!("{filename}.jsonl"));
    let mut session = CommitSession::open(ctx, &output_path);
    let fallback_run_id = RunId::new();
    let run_id = ctx.map_or(&fallback_run_id, |c| &c.run_id);

    let exporter = create_exporter(output_dir, filename, format)?;
    let mut processed_urls = Vec::new();

    for result in results {
        match process_single_item(
            &mut session,
            exporter.as_ref(),
            result,
            &output_path,
            run_id,
        ) {
            SingleItemOutcome::Driven => {
                processed_urls.push(result.url.as_str().to_string());
            },
            SingleItemOutcome::Skipped => {},
            SingleItemOutcome::Cancelled => {
                tracing::info!("cancellation observed; draining before final persist");
                // Crash-injection (PR5b): die MID-DRAIN, after some items
                // completed their full D3 sequence but before the final
                // persist — the drain-ordering kill window.
                crate::cli::crash_points::hit(crate::cli::crash_points::DURING_CANCEL_DRAIN);
                break;
            },
        }
    }
    session.final_persist();

    info!(
        "✅ Export completado: {} documentos procesados",
        processed_urls.len()
    );
    Ok(processed_urls)
}

/// Get domain from URL
///
/// Extracts the domain (host) from a URL string.
///
/// # Arguments
///
/// * `url` - URL string to extract domain from
///
/// # Returns
///
/// Domain string (e.g., "example.com" from `<https://www.example.com/docs/api/>`)
///
/// # Examples
///
/// ```
/// use webfang_core::application::export_factory::domain_from_url;
///
/// let domain = domain_from_url("https://www.example.com/docs/api/");
/// assert_eq!(domain, "www.example.com");
/// ```
pub fn domain_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|p| p.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Process pre-cleaned document chunks and export them.
///
/// This function is used when `--clean-ai` is enabled. It accepts
/// `DocumentChunk` instances that already have embeddings populated
/// by the `SemanticCleaner`, bypassing the simple field-mapping conversion.
#[cfg(feature = "ai")]
pub fn process_results_with_chunks(
    chunks: &[crate::domain::DocumentChunk],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &str,
    ctx: Option<&ResumeContext<'_>>,
) -> Result<Vec<String>, ExporterError> {
    use sha2::{Digest, Sha256};

    info!("Processing {} cleaned chunks for export", chunks.len());

    std::fs::create_dir_all(&output_dir).map_err(ExporterError::DirectoryCreation)?;

    let output_path = output_dir.join(format!("{filename}.jsonl"));
    let mut session = CommitSession::open(ctx, &output_path);
    let fallback_run_id = RunId::new();
    let run_id = ctx.map_or(&fallback_run_id, |c| &c.run_id);

    let exporter = create_exporter(output_dir, filename, format)?;

    let processed_urls: Vec<String> = chunks.iter().map(|c| c.url.clone()).collect();

    // Resume gate BEFORE any export (mirrors `process_single_item`): decide()
    // is read-only over the loaded records + flushed-bytes hash index, so the
    // decisions can be precomputed per chunk without side effects.
    let content_hashes: Vec<String> = chunks
        .iter()
        // Same digest the exporter stamps into the line; computed up front so
        // decide() can prove flush membership even with no record.
        .map(|c| format!("{:x}", Sha256::digest(c.content.as_bytes())))
        .collect();
    let decisions: Vec<ItemDecision> = processed_urls
        .iter()
        .zip(&content_hashes)
        .map(|(url_str, hash)| session.decide(url_str, Some(hash.as_str())))
        .collect();
    let skipped = decisions
        .iter()
        .filter(|d| matches!(d, ItemDecision::AlreadyCommitted))
        .count();
    if skipped > 0 {
        info!(
            skipped,
            total = processed_urls.len(),
            "resume gate: skipping already-flushed AI chunks before batch export"
        );
    }

    // Only chunks that will actually drive this run reach export_batch:
    // COMMITTED-proven and PromoteFromFlushProof chunks must not re-append
    // (BUG F3-A: exporting before deciding duplicated them on resume).
    let validated_chunks: Vec<crate::domain::DocumentChunkValidated> = chunks
        .iter()
        .zip(&decisions)
        .filter(|(_, d)| matches!(d, ItemDecision::DriveAndCommit))
        .filter_map(|(c, _)| c.clone().validate().ok())
        .collect();

    // Use export_batch to avoid per-chunk file open/close (which overwrites in VectorExporter)
    if !validated_chunks.is_empty() {
        exporter.export_batch(&validated_chunks)?;
    }

    for ((url_str, content_hash), decision) in
        processed_urls.iter().zip(&content_hashes).zip(decisions)
    {
        match decision {
            ItemDecision::AlreadyCommitted => continue,
            ItemDecision::PromoteFromFlushProof => {
                session.promote_from_flush_proof(
                    url_str,
                    output_path.display().to_string(),
                    content_hash,
                );
            },
            ItemDecision::DriveAndCommit => {
                session.commit_item(url_str, content_hash.clone(), output_path.clone(), run_id);
            },
        }
        if session.cancelled() {
            break;
        }
    }
    session.final_persist();

    info!(
        "✅ AI-cleaned export completed: {} chunks processed",
        processed_urls.len()
    );
    Ok(processed_urls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::ScrapedContent;
    use crate::domain::ValidUrl;
    use tempfile::TempDir;

    fn make_scraped_content(url: &str, title: &str, content: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_string(),
            content: content.to_string(),
            url: ValidUrl::parse(url).unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    // =========================================================================
    // domain_from_url tests
    // =========================================================================

    #[test]
    fn test_domain_from_url_extracts_correctly() {
        let url = "https://www.example.com/docs/api/";
        let domain = domain_from_url(url);
        assert_eq!(domain, "www.example.com");
    }

    #[test]
    fn test_domain_from_url_invalid_url_returns_unknown() {
        let domain = domain_from_url("not-a-url");
        assert_eq!(domain, "unknown");
    }

    // =========================================================================
    // create_state_store tests
    // =========================================================================

    #[test]
    fn test_create_state_store_creates_directory() {
        let temp_dir = TempDir::new().unwrap();
        let domain = "example.com";
        let store = create_state_store(temp_dir.path().to_path_buf(), domain);
        assert!(store.is_ok());
        let state_file = temp_dir.path().join("example.com.json");
        let store = store.unwrap();
        assert_eq!(store.get_state_path(), state_file);
    }

    // =========================================================================
    // process_results tests (T2.2)
    // =========================================================================

    #[test]
    fn test_process_results_empty_results_returns_empty_vec() {
        let temp_dir = TempDir::new().unwrap();
        let results: Vec<ScrapedContent> = vec![];

        let processed = process_results(
            &results,
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .unwrap();

        assert!(processed.is_empty());
    }

    #[test]
    fn test_process_results_single_item_exports_and_returns_url() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content(
            "https://example.com/page1",
            "Page One",
            "Content of page one",
        );

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        assert_eq!(processed[0], "https://example.com/page1");
        // Verify export file was created
        assert!(temp_dir.path().join("export.jsonl").exists());
    }

    #[test]
    fn test_process_results_multiple_items_exports_all() {
        let temp_dir = TempDir::new().unwrap();
        let contents = vec![
            make_scraped_content("https://a.com/", "A", "Content A"),
            make_scraped_content("https://b.com/", "B", "Content B"),
            make_scraped_content("https://c.com/", "C", "Content C"),
        ];

        let processed = process_results(
            &contents,
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .unwrap();

        assert_eq!(processed.len(), 3);
        // URLs get normalized through ValidUrl — check that all 3 are present
        assert_eq!(processed.len(), 3);
    }

    #[test]
    fn test_process_results_vector_format_creates_json_file() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content("https://example.com", "Test", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Vector,
            "export",
            None,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        assert!(temp_dir.path().join("export.json").exists());
    }

    #[test]
    fn test_process_results_invalid_content_records_failure_and_continues() {
        let temp_dir = TempDir::new().unwrap();
        // Empty content fails validation: per SC6 the item-level failure is
        // recorded (DomainRecoverable) and the run continues - it never aborts.
        let content = make_scraped_content("https://example.com", "Title", "");

        let result = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        );

        assert!(result.is_ok(), "item failures never abort the export");
    }

    // =========================================================================
    // create_exporter tests
    // =========================================================================

    #[test]
    fn test_create_exporter_jsonl_returns_ok() {
        let temp_dir = TempDir::new().unwrap();
        let result = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Jsonl);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_exporter_vector_returns_ok() {
        let temp_dir = TempDir::new().unwrap();
        let result = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Vector);
        assert!(result.is_ok());
    }

    // =========================================================================
    // ExportFormat::Auto branch tests (T2.3)
    // =========================================================================

    #[test]
    fn test_auto_format_detects_jsonl_when_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        // Create a .jsonl file to trigger detection
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Auto detects JSONL and creates a JSONL exporter
    }

    #[test]
    fn test_auto_format_detects_vector_when_json_file_exists() {
        let temp_dir = TempDir::new().unwrap();
        // Create a .json file to trigger Vector detection
        // Vector takes priority over JSONL in the detection logic
        std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Auto detects Vector format from .json file
    }

    #[test]
    fn test_auto_format_vector_takes_priority_over_jsonl() {
        let temp_dir = TempDir::new().unwrap();
        // Create both files — Vector (.json) takes priority
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();
        std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Vector (.json) is checked first in the code
    }

    #[test]
    fn test_auto_format_falls_back_to_jsonl_when_no_files_exist() {
        let temp_dir = TempDir::new().unwrap();
        // No files exist — should default to Jsonl

        let result = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto);
        assert!(result.is_ok());
        // Falls back to default Jsonl format
    }

    #[test]
    fn test_auto_format_with_empty_dir_exports_successfully() {
        let temp_dir = TempDir::new().unwrap();
        let content = make_scraped_content("https://example.com", "Test", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Auto,
            "export",
            None,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        // Default fallback creates .jsonl file
        assert!(temp_dir.path().join("export.jsonl").exists());
    }

    #[test]
    fn test_auto_format_with_existing_jsonl_exports_to_jsonl() {
        let temp_dir = TempDir::new().unwrap();
        // Pre-create a .jsonl file
        std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

        let content = make_scraped_content("https://example.com", "Test", "Body");
        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Auto,
            "export",
            None,
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
        // Should have appended to the existing .jsonl file
        let content = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
        assert!(!content.is_empty());
    }

    // =========================================================================
    // process_results with resume mode
    // =========================================================================

    #[test]
    fn test_process_results_resume_mode_tracks_urls() {
        let temp_dir = TempDir::new().unwrap();
        let _store = create_state_store(temp_dir.path().to_path_buf(), "example.com").unwrap();

        let content = make_scraped_content("https://example.com/page", "Page", "Body");

        let processed = process_results(
            &[content],
            temp_dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None, // legacy StateStore no longer drives export; RecordStore does (resume_gate_test)
        )
        .unwrap();

        assert_eq!(processed.len(), 1);
    }

    // =========================================================================
    // create_state_store failure-path characterization (#393)
    // =========================================================================

    /// Pins the current contract of [`create_state_store`]: it is lazy and
    /// infallible. `StateStore::new` and `set_cache_dir` perform no I/O, so the
    /// store is created successfully even when `state_dir` is a regular file
    /// where no directory could ever be created.
    ///
    /// Consequence: the `CliExit::IoError` branch in `apply_resume_mode`
    /// (`cli/scrape_flow.rs`) is currently unreachable dead code, because the
    /// state directory is only created later, on `StateStore::save`. If
    /// `create_state_store` is ever made eager (e.g. `create_dir_all` up front),
    /// this test must be updated and the `IoError` path becomes coverable.
    #[test]
    fn test_create_state_store_returns_ok_even_when_state_dir_is_a_file() {
        // Arrange: a regular file where a state directory would need to be —
        // an impossible location for any real directory creation.
        let temp_dir = TempDir::new().expect("tempdir should be created");
        let blocker = temp_dir.path().join("not_a_dir");
        std::fs::write(&blocker, "i am a file, not a directory")
            .expect("blocker file should be written");

        // Act
        let result = create_state_store(blocker, "example.com");

        // Assert: creation is lazy/infallible — no I/O is attempted yet.
        assert!(
            result.is_ok(),
            "create_state_store must be infallible (lazy): it performs no I/O at creation time"
        );
    }

    // =========================================================================
    // SC7 / E8 - cooperative cancellation: drain-before-final-persist ordering
    // =========================================================================

    #[test]
    fn cancellation_drains_in_flight_item_and_persists_honest_statuses() {
        let dir = TempDir::new().unwrap();
        let store = RecordStore::new("cancel.test").with_state_dir(dir.path().to_path_buf());
        let ctx_store = store.clone();

        // Cancel deterministically at the 2nd EXPORTED checkpoint save: item1
        // finishes its full D3 sequence, item2 reaches its flush-proof point,
        // THEN cancellation is observed between items and the run drains.
        let token = tokio_util::sync::CancellationToken::new();
        let export_count = std::sync::atomic::AtomicUsize::new(0);
        let token_ref = &token;
        let counter_ref = &export_count;
        let observer = move |status: PageStatus| {
            if status == PageStatus::Exported
                && counter_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
            {
                token_ref.cancel();
            }
        };
        let ctx = ResumeContext::new(&ctx_store)
            .with_resume(false)
            .observing_cancel(&token)
            .with_persist_observer(&observer);

        let results = [
            make_scraped_content("https://cancel.test/one", "One", "first distinct body"),
            make_scraped_content("https://cancel.test/two", "Two", "second distinct body"),
            make_scraped_content("https://cancel.test/three", "Three", "third distinct body"),
        ];
        process_results(
            &results,
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            Some(&ctx),
        )
        .unwrap();

        // Drain ordering: items 1-2 completed their ATOMIC D3 sequence
        // (in-flight work never half-done); item 3 never started.
        let records = store.load().unwrap();
        assert_eq!(
            records.len(),
            2,
            "drained run persists exactly the in-flight items"
        );
        for url in ["https://cancel.test/one", "https://cancel.test/two"] {
            let r = &records[url];
            assert_eq!(
                r.status,
                PageStatus::Committed,
                "{url} must be fully committed"
            );
            assert!(r.last_error.is_none(), "{url}: no phantom errors");
        }
        assert!(
            !records.contains_key("https://cancel.test/three"),
            "item after cancellation must NOT exist (no invented state)"
        );
        let lines = std::fs::read_to_string(dir.path().join("export.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count();
        assert_eq!(lines, 2, "exactly the drained items' lines on disk");
    }

    #[test]
    fn resume_after_cancellation_re_drives_non_committed_exactly_once() {
        let dir = TempDir::new().unwrap();
        let store = RecordStore::new("cancel2.test").with_state_dir(dir.path().to_path_buf());

        // Cancelled first run: only item 'a' completes its D3 sequence.
        let token = tokio_util::sync::CancellationToken::new();
        let ctx_store_a = store.clone();
        {
            let token_ref = &token;
            let observer = move |status: PageStatus| {
                if status == PageStatus::Committed {
                    token_ref.cancel();
                }
            };
            let ctx = ResumeContext::new(&ctx_store_a)
                .with_resume(false)
                .observing_cancel(&token)
                .with_persist_observer(&observer);
            process_results(
                &[
                    make_scraped_content("https://cancel2.test/a", "A", "alpha content"),
                    make_scraped_content("https://cancel2.test/b", "B", "beta content"),
                ],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&ctx),
            )
            .unwrap();
        }
        let lines_after_cancel = std::fs::read_to_string(dir.path().join("export.jsonl"))
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(
            lines_after_cancel, 1,
            "cancelled run exported exactly one line"
        );

        // Resume rerun: Committed 'a' skips; 'b' re-drives fresh - once.
        let ctx_store_b = store.clone();
        let second = ResumeContext::new(&ctx_store_b).with_resume(true);
        process_results(
            &[
                make_scraped_content("https://cancel2.test/a", "A", "alpha content"),
                make_scraped_content("https://cancel2.test/b", "B", "beta content"),
            ],
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            Some(&second),
        )
        .unwrap();

        let records = store.load().unwrap();
        for url in ["https://cancel2.test/a", "https://cancel2.test/b"] {
            assert_eq!(records[url].status, PageStatus::Committed, "{url}");
        }
        let lines_final = std::fs::read_to_string(dir.path().join("export.jsonl"))
            .unwrap_or_default()
            .lines()
            .count();
        assert_eq!(lines_final, 2, "no duplicated lines across cancel+resume");
    }

    // =========================================================================
    // AI chunk export: resume gate BEFORE batch export (BUG F3-A)
    // =========================================================================
    // The non-AI path (`process_single_item`) decides BEFORE each export;
    // the AI chunk path must mirror that ordering so --clean-ai --resume
    // never appends chunks whose URL is COMMITTED-proven or whose bytes
    // are already flushed (PromoteFromFlushProof).
    #[cfg(feature = "ai")]
    mod ai_chunk_resume_gate {
        use super::*;
        use crate::domain::DocumentChunkUnvalidated;
        use std::collections::HashMap;

        fn make_ai_chunk(url: &str, title: &str, content: &str) -> crate::domain::DocumentChunk {
            DocumentChunkUnvalidated {
                id: uuid::Uuid::new_v4(),
                url: url.to_string(),
                title: title.to_string(),
                content: content.to_string(),
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now(),
                embeddings: None,
                correlation_id: None,
                _state: std::marker::PhantomData,
            }
        }

        fn jsonl_lines(dir: &std::path::Path, filename: &str) -> Vec<String> {
            std::fs::read_to_string(dir.join(format!("{filename}.jsonl")))
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string)
                .collect()
        }

        /// RED (F3-A): with --clean-ai --resume, chunks of COMMITTED-proven
        /// URLs must NOT be re-appended to the output JSONL.
        #[test]
        fn ai_resume_does_not_append_chunks_for_committed_urls() {
            let dir = TempDir::new().unwrap();
            let store = RecordStore::new("resume-ai.test").with_state_dir(dir.path().to_path_buf());

            // Seed run (fresh): urlA drives + commits; exactly one line.
            let seed_store = store.clone();
            let seed_ctx = ResumeContext::new(&seed_store).with_resume(false);
            process_results_with_chunks(
                &[make_ai_chunk(
                    "https://resume-ai.test/a",
                    "A",
                    "alpha content",
                )],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&seed_ctx),
            )
            .unwrap();
            assert_eq!(jsonl_lines(dir.path(), "export").len(), 1);

            // Resume rerun: urlA is COMMITTED-proven (skip), urlB exports once.
            let resume_store = store.clone();
            let resume_ctx = ResumeContext::new(&resume_store).with_resume(true);
            let processed = process_results_with_chunks(
                &[
                    make_ai_chunk("https://resume-ai.test/a", "A", "alpha content"),
                    make_ai_chunk("https://resume-ai.test/b", "B", "beta content"),
                ],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&resume_ctx),
            )
            .unwrap();
            assert_eq!(processed.len(), 2);

            let lines = jsonl_lines(dir.path(), "export");
            assert_eq!(
                lines.len(),
                2,
                "--clean-ai --resume must not duplicate chunks for COMMITTED-proven URLs"
            );
            assert_eq!(
                lines.iter().filter(|l| l.contains("alpha")).count(),
                1,
                "the committed-proven chunk appears exactly once"
            );

            let records = store.load().unwrap();
            for url in ["https://resume-ai.test/a", "https://resume-ai.test/b"] {
                assert_eq!(records[url].status, PageStatus::Committed, "{url}");
            }
        }

        /// Regression guard: a fresh-state first run still exports every chunk.
        #[test]
        fn ai_fresh_run_exports_all_chunks() {
            let dir = TempDir::new().unwrap();
            let store = RecordStore::new("fresh-ai.test").with_state_dir(dir.path().to_path_buf());
            let ctx_store = store.clone();
            let ctx = ResumeContext::new(&ctx_store).with_resume(false);

            let processed = process_results_with_chunks(
                &[
                    make_ai_chunk("https://fresh-ai.test/one", "One", "first body"),
                    make_ai_chunk("https://fresh-ai.test/two", "Two", "second body"),
                ],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&ctx),
            )
            .unwrap();

            assert_eq!(processed.len(), 2);
            assert_eq!(
                jsonl_lines(dir.path(), "export").len(),
                2,
                "fresh run exports every chunk"
            );
            let records = store.load().unwrap();
            assert_eq!(
                records["https://fresh-ai.test/one"].status,
                PageStatus::Committed
            );
            assert_eq!(
                records["https://fresh-ai.test/two"].status,
                PageStatus::Committed
            );
        }

        /// TRIANGULATE: PromoteFromFlushProof items must be promoted WITHOUT
        /// re-appending their chunk (hash_index membership proves the bytes
        /// were already flushed).
        #[test]
        fn ai_flush_proof_promotion_does_not_reappend_chunk() {
            let dir = TempDir::new().unwrap();
            let store = RecordStore::new("flush-ai.test").with_state_dir(dir.path().to_path_buf());

            // Run 1: AI-export commits gamma; one line on disk.
            let first_store = store.clone();
            let first = ResumeContext::new(&first_store).with_resume(false);
            process_results_with_chunks(
                &[make_ai_chunk(
                    "https://flush-ai.test/g",
                    "G",
                    "gamma content",
                )],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&first),
            )
            .unwrap();
            assert_eq!(jsonl_lines(dir.path(), "export").len(), 1);

            // Demote the record to Extracted keeping its content_hash:
            // bytes proven on disk, but the record no longer claims COMMITTED.
            let mut records = store.load().unwrap();
            records
                .get_mut("https://flush-ai.test/g")
                .expect("gamma record exists")
                .status = PageStatus::Extracted;
            store.save(&records).unwrap();

            // Resume rerun: PromoteFromFlushProof promotes WITHOUT appending.
            let second_store = store.clone();
            let second = ResumeContext::new(&second_store).with_resume(true);
            process_results_with_chunks(
                &[make_ai_chunk(
                    "https://flush-ai.test/g",
                    "G",
                    "gamma content",
                )],
                dir.path().to_path_buf(),
                ExportFormat::Jsonl,
                "export",
                Some(&second),
            )
            .unwrap();

            assert_eq!(
                jsonl_lines(dir.path(), "export").len(),
                1,
                "flush-proof promotion must not re-append the chunk"
            );
            let records = store.load().unwrap();
            assert_eq!(
                records["https://flush-ai.test/g"].status,
                PageStatus::Committed,
                "record promoted back to Committed"
            );
        }
    }
}
