//! ADR-0014 record state-transition machine (pure, deterministic).
//!
//! This module owns THE RULES that govern a persisted [`RawRecord`]'s
//! lifecycle state — previously scattered across `record_store.rs`:
//!
//! * the D2/E6 invariant table (which per-record states are impossible),
//! * quarantine classification (a violating record is dropped from the
//!   in-memory view and re-drivable next run — it never errors, never panics,
//!   and is never counted),
//! * the derived `total_exported` counter (COMMITTED only, never stored),
//! * structural-meaninglessness of empty/whitespace URL identity (#876).
//!
//! The machine is PURE: no filesystem, no locks, no clock, no tracing. All
//! I/O (reading, writing, locking, warning, migrating) stays in
//! `record_store.rs`, which delegates its rule checks here. This is the
//! binding model for ADR-0014; loom model tests over the state space arrive
//! in a follow-up commit.
//!
//! No new states, verbs, or flags were invented: this is a pure extraction of
//! the checks that already existed at the v2 record-store seam.

use crate::domain::page_state::{PageStatus, MIGRATED_V1_RUN_ID};
use crate::domain::persistence::{DomainRecords, RawRecord};

/// Returns the name of the first violated invariant, if any.
///
/// D2/E6 invariant table, verbatim from the pre-extraction seam:
///
/// 1. An empty/whitespace URL is structurally meaningless identity; such a
///    record can never address a page and is quarantined like any other
///    impossible state (#876).
/// 2. `EXPORTED`/`COMMITTED` records (except v1-migrated ones, which predate
///    hash tracking by design) must carry both `output_location` and
///    `content_hash`. COMMITTED without `output_location` is QUARANTINED,
///    not rejected.
/// 3. `COMMITTED` must not carry a `last_error`.
/// 4. `COMMITTED` requires `attempts >= 1`.
#[must_use]
pub(crate) fn invariant_violation(record: &RawRecord) -> Option<&'static str> {
    // #876: an empty URL is structurally meaningless identity; such a
    // record can never address a page and is quarantined like any
    // other impossible state.
    if is_meaningless_identity(&record.url) {
        return Some("url must not be empty");
    }
    // v1-migrated records predate hash tracking by design; their
    // Committed status is exempt from the output/hash requirement.
    let migrated = record.run_id == MIGRATED_V1_RUN_ID;
    if matches!(record.status, PageStatus::Exported | PageStatus::Committed)
        && !migrated
        && (record.output_location.is_none() || record.content_hash.is_none())
    {
        return Some("exported/committed requires output_location and content_hash");
    }
    if record.status == PageStatus::Committed {
        if record.last_error.is_some() {
            return Some("committed must not carry last_error");
        }
        if record.attempts < 1 {
            return Some("committed requires attempts >= 1");
        }
    }
    None
}

/// Whether a URL is structurally meaningless identity (#876): empty or
/// whitespace-only. Used by the v1 migration path (raw legacy strings) and by
/// the invariant table (persisted `RawRecord.url`).
#[must_use]
pub(crate) fn is_meaningless_identity(url: &str) -> bool {
    url.trim().is_empty()
}

/// One invariant-violating record, kept alongside the reason it was dropped.
#[derive(Debug)]
pub(crate) struct QuarantinedEntry {
    /// Canonical URL key of the violating record.
    pub url: String,
    /// Name of the first violated invariant.
    pub invariant: &'static str,
}

/// Partition records into a kept map and the quarantined entries, in map
/// order. Quarantine is terminal-ish for the view: the record disappears from
/// the in-memory state and is re-drivable on the next run; it is never
/// counted by [`derived_total_exported`]. Logging is the caller's I/O duty —
/// this function emits nothing.
pub(crate) fn partition_valid(records: DomainRecords) -> (DomainRecords, Vec<QuarantinedEntry>) {
    let mut kept = DomainRecords::new();
    let mut quarantined = Vec::new();
    for (key, record) in records {
        if let Some(invariant) = invariant_violation(&record) {
            quarantined.push(QuarantinedEntry {
                url: record.url.clone(),
                invariant,
            });
            continue;
        }
        kept.insert(key, record);
    }
    (kept, quarantined)
}

/// Count of `COMMITTED` records — the compat replacement for v1's stored
/// `total_exported` counter (A3): derived, never authoritative.
#[must_use]
pub(crate) fn derived_total_exported(records: &DomainRecords) -> u64 {
    records
        .values()
        .filter(|r| r.status == PageStatus::Committed)
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::ErrorClass;
    use crate::domain::persistence::LastError;

    fn record(url: &str) -> RawRecord {
        RawRecord {
            url: url.to_string(),
            canonical_url: url.to_string(),
            run_id: "018f3c1e-7a2b-4c0d-9e1f-2a3b4c5d6e7f".to_string(),
            content_hash: Some("sha256:deadbeef".to_string()),
            attempts: 3,
            status: PageStatus::Extracted,
            last_error: None,
            output_location: Some("out/example.md".to_string()),
            updated_at: 1_760_000_000_000,
        }
    }

    fn committed(url: &str) -> RawRecord {
        let mut r = record(url);
        r.status = PageStatus::Committed;
        r
    }

    // --- invariant 1: empty/whitespace URL is meaningless identity --------

    #[test]
    fn empty_and_whitespace_url_violates_identity() {
        for bad in ["", "   "] {
            assert_eq!(
                invariant_violation(&record(bad)),
                Some("url must not be empty"),
                "empty/whitespace URL must violate the identity invariant"
            );
        }
        assert_eq!(invariant_violation(&record("https://x.test/")), None);
    }

    #[test]
    fn is_meaningless_identity_classifies_whitespace_only_as_empty() {
        assert!(is_meaningless_identity(""));
        assert!(is_meaningless_identity("   "));
        assert!(is_meaningless_identity("\t\n"));
        assert!(!is_meaningless_identity("https://x.test/a"));
    }

    // --- invariant 2: exported/committed requires output + hash -----------

    #[test]
    fn committed_without_output_location_is_quarantined_not_rejected() {
        let mut r = committed("https://x.test/bad");
        r.output_location = None;
        assert_eq!(
            invariant_violation(&r),
            Some("exported/committed requires output_location and content_hash"),
            "committed without output_location is an invariant violation, not a typed rejection"
        );
    }

    #[test]
    fn committed_without_content_hash_violates() {
        let mut r = committed("https://x.test/bad");
        r.content_hash = None;
        assert_eq!(
            invariant_violation(&r),
            Some("exported/committed requires output_location and content_hash")
        );
    }

    #[test]
    fn exported_without_output_location_violates() {
        let mut r = record("https://x.test/bad");
        r.status = PageStatus::Exported;
        r.output_location = None;
        assert_eq!(
            invariant_violation(&r),
            Some("exported/committed requires output_location and content_hash")
        );
    }

    #[test]
    fn fully_populated_committed_record_satisfies_the_table() {
        assert_eq!(invariant_violation(&committed("https://x.test/ok")), None);
    }

    #[test]
    fn migrated_v1_committed_records_are_exempt_from_output_and_hash_rule() {
        let mut migrated = committed("https://x.test/migrated");
        migrated.run_id = MIGRATED_V1_RUN_ID.to_string();
        migrated.content_hash = None;
        migrated.output_location = None;
        assert_eq!(
            invariant_violation(&migrated),
            None,
            "v1-migrated records predate hash tracking by design"
        );
    }

    // --- invariant 3: committed must not carry last_error ------------------

    #[test]
    fn committed_with_last_error_violates() {
        let mut r = committed("https://x.test/bad");
        r.last_error = Some(LastError {
            class: ErrorClass::InternalFatal,
            message: "impossible state".to_string(),
        });
        assert_eq!(
            invariant_violation(&r),
            Some("committed must not carry last_error")
        );
    }

    #[test]
    fn non_committed_record_may_carry_last_error() {
        let mut r = record("https://x.test/ok");
        r.last_error = Some(LastError {
            class: ErrorClass::DomainRecoverable,
            message: "chunk exceeded --max-tokens".to_string(),
        });
        assert_eq!(invariant_violation(&r), None);
    }

    // --- invariant 4: committed requires attempts >= 1 ---------------------

    #[test]
    fn committed_with_zero_attempts_violates() {
        let mut r = committed("https://x.test/bad");
        r.attempts = 0;
        assert_eq!(
            invariant_violation(&r),
            Some("committed requires attempts >= 1")
        );
    }

    #[test]
    fn first_violation_wins_when_several_apply() {
        let mut r = committed("");
        r.last_error = Some(LastError {
            class: ErrorClass::InternalFatal,
            message: "multiple violations".to_string(),
        });
        assert_eq!(
            invariant_violation(&r),
            Some("url must not be empty"),
            "the identity invariant is checked first"
        );
    }

    // --- partition_valid: quarantine drops, neighbors survive --------------

    #[test]
    fn partition_keeps_valid_neighbors_and_reports_quarantined_entries_in_map_order() {
        let mut bad_first = committed("https://x.test/bad");
        bad_first.output_location = None;
        let good = record("https://x.test/good");
        let mut bad_last = committed("https://x.test/bad2");
        bad_last.last_error = Some(LastError {
            class: ErrorClass::InternalFatal,
            message: "impossible state".to_string(),
        });
        let mut records = DomainRecords::new();
        records.insert(bad_first.canonical_url.clone(), bad_first);
        records.insert(good.canonical_url.clone(), good.clone());
        records.insert(bad_last.canonical_url.clone(), bad_last);

        let (kept, quarantined) = partition_valid(records);

        assert_eq!(kept.len(), 1, "only the valid neighbor survives");
        assert_eq!(kept["https://x.test/good"], good);
        assert_eq!(
            quarantined
                .iter()
                .map(|q| (q.url.as_str(), q.invariant))
                .collect::<Vec<_>>(),
            vec![
                (
                    "https://x.test/bad",
                    "exported/committed requires output_location and content_hash"
                ),
                ("https://x.test/bad2", "committed must not carry last_error"),
            ],
            "quarantine entries preserve map order and name the violated invariant"
        );
    }

    #[test]
    fn partition_of_all_valid_records_yields_no_quarantine() {
        let mut records = DomainRecords::new();
        records.insert("https://x.test/a".to_string(), record("https://x.test/a"));
        records.insert(
            "https://x.test/b".to_string(),
            committed("https://x.test/b"),
        );
        let (kept, quarantined) = partition_valid(records);
        assert_eq!(kept.len(), 2);
        assert!(quarantined.is_empty());
    }

    // --- derived counter: COMMITTED only ------------------------------------

    #[test]
    fn derived_total_exported_counts_only_committed() {
        let mut records = DomainRecords::new();
        let committed_record = committed("https://x.test/committed");
        let mut exported = record("https://x.test/exported");
        exported.status = PageStatus::Exported;
        records.insert(committed_record.url.clone(), committed_record);
        records.insert(exported.url.clone(), exported);
        assert_eq!(derived_total_exported(&records), 1);
    }

    #[test]
    fn derived_total_exported_counts_nothing_but_committed() {
        let mut records = DomainRecords::new();
        // Exported/Discovered states are never counted, whatever else is set.
        let mut exported = record("https://x.test/exported");
        exported.status = PageStatus::Exported;
        records.insert(exported.url.clone(), exported);
        records.insert("https://x.test/d".to_string(), record("https://x.test/d"));
        assert_eq!(derived_total_exported(&records), 0);
    }

    #[test]
    fn derived_total_exported_counts_every_committed_record() {
        let mut records = DomainRecords::new();
        for url in ["https://x.test/a", "https://x.test/b", "https://x.test/c"] {
            let r = committed(url);
            records.insert(url.to_string(), r);
        }
        assert_eq!(derived_total_exported(&records), 3);
    }
}
