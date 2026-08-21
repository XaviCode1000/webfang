//! PR3 resume-gate contract tests (SC2, A2/E10).
//!
//! The gate in [`webfang_core::application::resume`] is THE single skip
//! decision point shared by the scrape path (`scrape_flow`) and the batch
//! path (`orchestrator`). Skip-on-resume fires ONLY on records proven
//! `COMMITTED` through the typed reconciliation boundary; every other
//! status re-drives from its recorded position.
//!
//! Fresh-run semantics (A2): a new run always gets a fresh `run_id`; prior
//! records are PRESERVED and remain queryable regardless of `--resume`.

use std::collections::BTreeMap;

use tempfile::TempDir;
use url::Url;
use webfang_core::application::resume::{filter_committed, normalize_domain_key};
use webfang_core::domain::page_state::PageStatus;
use webfang_core::infrastructure::export::{DomainRecords, LastError, RawRecord, RecordStore};

fn committed_record(url: &str) -> RawRecord {
    RawRecord {
        url: url.to_string(),
        canonical_url: url.to_string(),
        run_id: "018f3c1e-7a2b-4c0d-9e1f-2a3b4c5d6e7f".to_string(),
        content_hash: Some("sha256:abc".to_string()),
        attempts: 1,
        status: PageStatus::Committed,
        last_error: None,
        output_location: Some("out/export.jsonl".to_string()),
        updated_at: 1_760_000_000_000,
    }
}

fn record_at(url: &str, status: PageStatus) -> RawRecord {
    let mut r = committed_record(url);
    r.status = status;
    if status != PageStatus::Committed {
        r.last_error = Some(LastError {
            class: webfang_core::error::ErrorClass::DomainRecoverable,
            message: "previous run died here".to_string(),
        });
    }
    r
}

fn seeded_store(domain: &str, records: DomainRecords) -> (TempDir, RecordStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = RecordStore::new(domain).with_state_dir(dir.path().to_path_buf());
    store.save(&records).expect("seed records");
    (dir, store)
}

fn urls<const N: usize>(items: [&str; N]) -> Vec<Url> {
    items.iter().map(|u| Url::parse(u).unwrap()).collect()
}

// ---------------------------------------------------------------------------
// normalize_domain_key
// ---------------------------------------------------------------------------

#[test]
fn domain_key_is_lowercased() {
    assert_eq!(normalize_domain_key("EXAMPLE.COM"), "example.com");
}

#[test]
fn domain_key_strips_trailing_dot() {
    assert_eq!(normalize_domain_key("example.com."), "example.com");
}

#[test]
fn domain_key_strips_leading_www() {
    assert_eq!(normalize_domain_key("www.example.com"), "example.com");
}

#[test]
fn domain_key_combines_all_normalizations() {
    assert_eq!(normalize_domain_key("WWW.Example.COM."), "example.com");
}

// ---------------------------------------------------------------------------
// Skip matrix: COMMITTED skipped, everything else re-driven (SC2)
// ---------------------------------------------------------------------------

#[test]
fn committed_records_are_skipped_on_resume() {
    let (_dir, store) = seeded_store(
        "skip.test",
        BTreeMap::from([(
            "https://skip.test/done".to_string(),
            committed_record("https://skip.test/done"),
        )]),
    );

    let (pending, _run_id) = filter_committed(
        urls(["https://skip.test/done", "https://skip.test/new"]),
        &store,
    );

    assert_eq!(
        pending,
        urls(["https://skip.test/new"]),
        "only the COMMITTED record is skipped"
    );
}

#[test]
fn exported_but_uncommitted_records_are_not_skipped() {
    let (_dir, store) = seeded_store(
        "exported.test",
        BTreeMap::from([(
            "https://exported.test/page".to_string(),
            record_at("https://exported.test/page", PageStatus::Exported),
        )]),
    );

    let (pending, _run_id) = filter_committed(urls(["https://exported.test/page"]), &store);

    assert_eq!(
        pending.len(),
        1,
        "EXPORTED is not COMMITTED: it must re-drive (re-export + reconcile)"
    );
}

#[test]
fn failed_records_re_drive_from_recorded_position() {
    let (_dir, store) = seeded_store(
        "failed.test",
        BTreeMap::from([(
            "https://failed.test/page".to_string(),
            record_at("https://failed.test/page", PageStatus::Extracted),
        )]),
    );

    let (pending, _run_id) = filter_committed(urls(["https://failed.test/page"]), &store);

    assert_eq!(
        pending.len(),
        1,
        "non-committed failures must re-drive from their recorded position"
    );
}

#[test]
fn unknown_urls_are_never_skipped() {
    let (_dir, store) = seeded_store("unknown.test", BTreeMap::new());

    let (pending, _run_id) = filter_committed(urls(["https://unknown.test/fresh"]), &store);

    assert_eq!(pending.len(), 1);
}

#[test]
fn www_and_apex_urls_share_one_committed_entry() {
    // canonical_url unifies www/apex: committing https://example.com/a also
    // covers https://www.example.com/a at the gate.
    let (_dir, store) = seeded_store(
        "canon.test",
        BTreeMap::from([(
            "https://canon.test/a".to_string(),
            committed_record("https://canon.test/a"),
        )]),
    );

    let (pending, _run_id) = filter_committed(urls(["https://www.canon.test/a"]), &store);

    assert!(
        pending.is_empty(),
        "www variant of a committed apex URL is the same document"
    );
}

// ---------------------------------------------------------------------------
// Unified gate: scrape-flow vs batch-flow equivalence (SC6/unify)
// ---------------------------------------------------------------------------

#[test]
fn identical_record_sets_produce_identical_skip_sets_across_paths() {
    // Both CLI paths call the SAME filter_committed against the SAME store
    // shape; equivalence is structural, but this pins it observably.
    let mut records = DomainRecords::new();
    records.insert(
        "https://equiv.test/a".to_string(),
        committed_record("https://equiv.test/a"),
    );
    records.insert(
        "https://equiv.test/b".to_string(),
        record_at("https://equiv.test/b", PageStatus::Processed),
    );
    let dir = TempDir::new().unwrap();

    let scrape_view = RecordStore::new("equiv.test").with_state_dir(dir.path().to_path_buf());
    scrape_view.save(&records).unwrap();
    let batch_view = RecordStore::new("equiv.test").with_state_dir(dir.path().to_path_buf());

    let candidates = urls(["https://equiv.test/a", "https://equiv.test/b"]);
    let (scrape_pending, _) = filter_committed(candidates.clone(), &scrape_view);
    let (batch_pending, _) = filter_committed(candidates, &batch_view);

    assert_eq!(scrape_pending, batch_pending);
    assert_eq!(scrape_pending.len(), 1, "only /a (COMMITTED) is skipped");
}

// ---------------------------------------------------------------------------
// A2 fresh-run semantics
// ---------------------------------------------------------------------------

#[test]
fn each_gate_call_issues_a_fresh_run_id() {
    let (_dir, store) = seeded_store("runid.test", BTreeMap::new());

    let (_, first) = filter_committed(urls(["https://runid.test/x"]), &store);
    let (_, second) = filter_committed(urls(["https://runid.test/x"]), &store);

    assert_ne!(first.as_str(), second.as_str(), "every run gets its own id");
    assert_eq!(first.as_str().len(), 36, "uuid v4 shape");
}

#[test]
fn prior_records_remain_queryable_after_a_later_run_loads_the_store() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("preserve.test").with_state_dir(dir.path().to_path_buf());
    let mut records = DomainRecords::new();
    records.insert(
        "https://preserve.test/old".to_string(),
        committed_record("https://preserve.test/old"),
    );
    store.save(&records).unwrap();

    // A later run opens the same domain's store and gates its URLs.
    let later = RecordStore::new("preserve.test").with_state_dir(dir.path().to_path_buf());
    let (pending, _) = filter_committed(urls(["https://preserve.test/other"]), &later);

    assert_eq!(pending.len(), 1);
    let still_there = later.load().expect("prior records stay readable");
    assert!(
        still_there.contains_key("https://preserve.test/old"),
        "A2: opening a fresh run must not discard prior records"
    );
}
