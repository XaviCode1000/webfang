//! Typed page lifecycle state machine — SC1 acceptance tests (PR1).
//!
//! Spec scenarios under test:
//! - "Full legal chain compiles and advances one step at a time"
//! - `reopen_for_reexport` is the ONLY backward transition
//! - Layer 1 [`PageStatus`] serializes as SCREAMING_SNAKE_CASE.

use std::path::PathBuf;

use webfang_core::domain::page_state::{
    Committed, Discovered, Exported, Extracted, Fetched, Fetching, PageStatus, PersistedRecord,
    Processed, Queued, ReconcileError, Stateful,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rec {
    url: String,
    status: PageStatus,
}

fn rec(url: &str) -> Rec {
    Rec {
        url: url.to_string(),
        status: PageStatus::Discovered,
    }
}

impl PersistedRecord for Rec {
    fn status(&self) -> PageStatus {
        self.status
    }

    fn output_location(&self) -> Option<&str> {
        None
    }

    fn content_hash(&self) -> Option<&str> {
        None
    }

    fn has_last_error(&self) -> bool {
        false
    }

    fn attempts(&self) -> u32 {
        0
    }

    fn set_status(&mut self, status: PageStatus) {
        self.status = status;
    }
}

#[test]
fn legal_chain_advances_one_step_at_a_time() {
    // Each binding is annotated with the EXACT next state type: the compiler
    // rejects any transition that does not land precisely there.
    let s: Stateful<Rec, Discovered> = Stateful::new(rec("https://example.com/a"));
    assert_eq!(s.status(), PageStatus::Discovered);

    let s: Stateful<Rec, Queued> = s.queue();
    assert_eq!(s.status(), PageStatus::Queued);

    let s: Stateful<Rec, Fetching> = s.start_fetch();
    assert_eq!(s.status(), PageStatus::Fetching);

    let s: Stateful<Rec, Fetched> = s.fetched();
    assert_eq!(s.status(), PageStatus::Fetched);

    let s: Stateful<Rec, Extracted> = s.extracted();
    assert_eq!(s.status(), PageStatus::Extracted);

    let s: Stateful<Rec, Processed> = s.processed();
    assert_eq!(s.status(), PageStatus::Processed);

    let s: Stateful<Rec, Exported> = s.export_flushed(PathBuf::from("out/page.jsonl"));
    assert_eq!(s.status(), PageStatus::Exported);

    let s: Stateful<Rec, Committed> = s.commit();
    assert_eq!(s.status(), PageStatus::Committed);
}

#[test]
fn transitions_move_the_record_through_intact() {
    let original = rec("https://example.com/b");
    let s: Stateful<Rec, Committed> = Stateful::new(original.clone())
        .queue()
        .start_fetch()
        .fetched()
        .extracted()
        .processed()
        .export_flushed(PathBuf::from("out/b.jsonl"))
        .commit();

    // The payload persisted status tracks the typestate position (D2
    // reconciliation requires field == marker on load).
    let mut expected = original.clone();
    expected.status = PageStatus::Committed;
    assert_eq!(s.into_record(), expected);
}

#[test]
fn reopen_for_reexport_is_the_only_backward_transition() {
    let s: Stateful<Rec, Exported> = Stateful::new(rec("https://example.com/c"))
        .queue()
        .start_fetch()
        .fetched()
        .extracted()
        .processed()
        .export_flushed(PathBuf::from("out/c.jsonl"));

    let reopened: Stateful<Rec, Processed> = s.reopen_for_reexport();
    assert_eq!(reopened.status(), PageStatus::Processed);

    // The recovery loop must be able to reach COMMITTED again from PROCESSED.
    let recommitted: Stateful<Rec, Committed> = reopened
        .export_flushed(PathBuf::from("out/c.jsonl"))
        .commit();
    assert_eq!(recommitted.status(), PageStatus::Committed);
}

#[test]
fn page_status_serializes_as_screaming_snake_case() {
    let pairs = [
        (PageStatus::Discovered, "\"DISCOVERED\""),
        (PageStatus::Queued, "\"QUEUED\""),
        (PageStatus::Fetching, "\"FETCHING\""),
        (PageStatus::Fetched, "\"FETCHED\""),
        (PageStatus::Extracted, "\"EXTRACTED\""),
        (PageStatus::Processed, "\"PROCESSED\""),
        (PageStatus::Exported, "\"EXPORTED\""),
        (PageStatus::Committed, "\"COMMITTED\""),
    ];
    for (status, wire) in pairs {
        assert_eq!(
            serde_json::to_string(&status).expect("serialize PageStatus"),
            wire
        );
        let back: PageStatus =
            serde_json::from_str(wire).expect("deserialize PageStatus from SCREAMING form");
        assert_eq!(back, status);
    }
}

#[test]
fn page_status_is_copy_and_structurally_comparable() {
    let a = PageStatus::Exported;
    let b = a;
    // Copy semantics: `a` remains usable after assignment.
    assert_eq!(a, b);
    assert_ne!(a, PageStatus::Committed);
}

/// Minimal stand-in for PR2's 9-field `RawRecord`: exposes exactly the
/// fields the D2 load-time validation table reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Raw {
    status: PageStatus,
    output_location: Option<String>,
    content_hash: Option<String>,
    last_error: Option<&'static str>,
    attempts: u32,
}

impl Raw {
    fn at(status: PageStatus) -> Self {
        Self {
            status,
            output_location: None,
            content_hash: None,
            last_error: None,
            attempts: 0,
        }
    }

    fn exported() -> Self {
        let mut r = Self::at(PageStatus::Exported);
        r.output_location = Some("out/a.jsonl".into());
        r.content_hash = Some("hash-a".into());
        r
    }
}

impl PersistedRecord for Raw {
    fn status(&self) -> PageStatus {
        self.status
    }

    fn output_location(&self) -> Option<&str> {
        self.output_location.as_deref()
    }

    fn content_hash(&self) -> Option<&str> {
        self.content_hash.as_deref()
    }

    fn has_last_error(&self) -> bool {
        self.last_error.is_some()
    }

    fn attempts(&self) -> u32 {
        self.attempts
    }

    fn set_status(&mut self, status: PageStatus) {
        self.status = status;
    }
}

#[test]
fn try_from_reconciles_every_matching_state() {
    fn committed() -> Raw {
        let mut r = Raw::exported();
        r.status = PageStatus::Committed;
        r.attempts = 1;
        r
    }

    let discovered: Stateful<Raw, Discovered> =
        Stateful::<Raw, Discovered>::reconcile(Raw::at(PageStatus::Discovered))
            .expect("discovered reconciles");
    assert_eq!(discovered.status(), PageStatus::Discovered);

    let queued: Stateful<Raw, Queued> =
        Stateful::<Raw, Queued>::reconcile(Raw::at(PageStatus::Queued)).expect("queued reconciles");
    assert_eq!(queued.status(), PageStatus::Queued);

    let fetching: Stateful<Raw, Fetching> =
        Stateful::<Raw, Fetching>::reconcile(Raw::at(PageStatus::Fetching))
            .expect("fetching reconciles");
    assert_eq!(fetching.status(), PageStatus::Fetching);

    let fetched: Stateful<Raw, Fetched> =
        Stateful::<Raw, Fetched>::reconcile(Raw::at(PageStatus::Fetched))
            .expect("fetched reconciles");
    assert_eq!(fetched.status(), PageStatus::Fetched);

    let extracted: Stateful<Raw, Extracted> =
        Stateful::<Raw, Extracted>::reconcile(Raw::at(PageStatus::Extracted))
            .expect("extracted reconciles");
    assert_eq!(extracted.status(), PageStatus::Extracted);

    let processed: Stateful<Raw, Processed> =
        Stateful::<Raw, Processed>::reconcile(Raw::at(PageStatus::Processed))
            .expect("processed reconciles");
    assert_eq!(processed.status(), PageStatus::Processed);

    let exported: Stateful<Raw, Exported> =
        Stateful::<Raw, Exported>::reconcile(Raw::exported()).expect("exported reconciles");
    assert_eq!(exported.status(), PageStatus::Exported);

    let committed: Stateful<Raw, Committed> =
        Stateful::<Raw, Committed>::reconcile(committed()).expect("committed reconciles");
    assert_eq!(committed.status(), PageStatus::Committed);
}

#[test]
fn try_from_rejects_status_mismatch() {
    // A record persisted as QUEUED cannot be reconstructed at DISCOVERED.
    let err = Stateful::<Raw, Discovered>::reconcile(Raw::at(PageStatus::Queued))
        .expect_err("mismatch must fail");
    assert_eq!(
        err,
        ReconcileError::StatusMismatch {
            expected: PageStatus::Discovered,
            found: PageStatus::Queued,
        }
    );
}

#[test]
fn exported_requires_output_location_and_content_hash() {
    let mut raw = Raw::at(PageStatus::Exported);
    raw.content_hash = Some("hash".into());

    let err =
        Stateful::<Raw, Exported>::reconcile(raw).expect_err("missing output_location must fail");
    assert_eq!(
        err,
        ReconcileError::MissingOutputLocation(PageStatus::Exported)
    );

    let mut raw = Raw::at(PageStatus::Exported);
    raw.output_location = Some("out/a.jsonl".into());
    let err =
        Stateful::<Raw, Exported>::reconcile(raw).expect_err("missing content_hash must fail");
    assert_eq!(
        err,
        ReconcileError::MissingContentHash(PageStatus::Exported)
    );
}

#[test]
fn committed_requires_clean_error_state_and_one_attempt() {
    let mut raw = Raw::exported();
    raw.status = PageStatus::Committed;
    raw.last_error = Some("boom");
    raw.attempts = 1;
    let err =
        Stateful::<Raw, Committed>::reconcile(raw).expect_err("last_error on COMMITTED must fail");
    assert_eq!(err, ReconcileError::CommittedWithLastError);

    let mut raw = Raw::exported();
    raw.status = PageStatus::Committed;
    let err = Stateful::<Raw, Committed>::reconcile(raw)
        .expect_err("zero attempts on COMMITTED must fail");
    assert_eq!(err, ReconcileError::CommittedWithZeroAttempts);
}
