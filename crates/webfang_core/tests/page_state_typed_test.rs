//! Typed page lifecycle state machine — SC1 acceptance tests (PR1).
//!
//! Spec scenarios under test:
//! - "Full legal chain compiles and advances one step at a time"
//! - `reopen_for_reexport` is the ONLY backward transition
//! - Layer 1 [`PageStatus`] serializes as SCREAMING_SNAKE_CASE.

use std::path::PathBuf;

use webfang_core::domain::page_state::{
    Committed, Discovered, Extracted, Exported, Fetched, Fetching, PageStatus, Processed, Queued,
    Stateful,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rec {
    url: String,
}

fn rec(url: &str) -> Rec {
    Rec {
        url: url.to_string(),
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

    assert_eq!(s.into_record(), original);
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
