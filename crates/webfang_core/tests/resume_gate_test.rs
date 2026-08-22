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

// ---------------------------------------------------------------------------
// D3 commit-point protocol through process_results (SC2/E1/SC6/A2)
// ---------------------------------------------------------------------------

use webfang_core::application::export_factory::{process_results, ResumeContext};
use webfang_core::domain::ScrapedContent;
use webfang_core::domain::ValidUrl;
use webfang_core::ExportFormat;

fn scraped(url: &str, content: &str) -> ScrapedContent {
    ScrapedContent {
        title: "T".to_string(),
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

fn jsonl_lines(dir: &TempDir) -> usize {
    let path = dir.path().join("export.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// Rewrite one persisted record's status (kill-window simulation: the crash
/// happened after flush but before/during the checkpoint saves).
fn demote(store: &RecordStore, url: &str, status: PageStatus, also_hash: Option<String>) {
    let mut records = store.load().expect("records readable");
    let r = records.get_mut(url).expect("record exists");
    r.status = status;
    if let Some(h) = also_hash {
        r.content_hash = Some(h);
    }
    store.save(&records).unwrap();
}

#[test]
fn process_results_commits_every_item_through_the_checkpoint_sequence() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("commit.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(true);

    let processed = process_results(
        &[
            scraped("https://commit.test/a", "alpha body"),
            scraped("https://commit.test/b", "beta body"),
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .expect("export succeeds");

    assert_eq!(processed.len(), 2);
    assert_eq!(jsonl_lines(&dir), 2, "one flushed line per item");

    let records = store.load().unwrap();
    let a = &records["https://commit.test/a"];
    assert_eq!(a.status, PageStatus::Committed, "commit point reached");
    assert_eq!(a.last_error, None, "committed items carry null last_error");
    assert!(a.attempts >= 1);
    assert!(a.output_location.is_some());
    assert!(a.content_hash.is_some(), "hash recorded for dedup");
}

#[test]
fn second_resume_run_skips_committed_items_without_re_exporting() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("reskip.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let first = ResumeContext::new(&ctx_store).with_resume(true);
    let results = [scraped("https://reskip.test/a", "stable body")];

    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&first),
    )
    .unwrap();
    assert_eq!(jsonl_lines(&dir), 1);

    let ctx_store2 = store.clone();
    let second = ResumeContext::new(&ctx_store2).with_resume(true);
    let processed = process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    assert!(processed.is_empty(), "nothing left to drive");
    assert_eq!(
        jsonl_lines(&dir),
        1,
        "COMMITTED items are never written twice"
    );
}

#[test]
fn processed_record_with_flushed_output_promotes_without_duplicate_line() {
    // Kill window: flush acked (line is in the file) but the EXPORTED save
    // never landed — the record honestly stays Processed. On resume it must
    // promote straight to Committed WITHOUT appending again.
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("torn.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let first = ResumeContext::new(&ctx_store).with_resume(true);
    let results = [scraped("https://torn.test/a", "flushed body")];
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&first),
    )
    .unwrap();

    demote(&store, "https://torn.test/a", PageStatus::Processed, None);

    let ctx_store2 = store.clone();
    let second = ResumeContext::new(&ctx_store2).with_resume(true);
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(&dir),
        1,
        "already-flushed output must not be duplicated (exactly-once)"
    );
    assert_eq!(
        store.load().unwrap()["https://torn.test/a"].status,
        PageStatus::Committed,
        "promoted to COMMITTED off the durable output"
    );
}

#[test]
fn exported_record_with_hash_in_index_promotes_straight_to_committed() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("exp.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let first = ResumeContext::new(&ctx_store).with_resume(true);
    let results = [scraped("https://exp.test/a", "checkpointed body")];
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&first),
    )
    .unwrap();

    demote(&store, "https://exp.test/a", PageStatus::Exported, None);

    let ctx_store2 = store.clone();
    let second = ResumeContext::new(&ctx_store2).with_resume(true);
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(&dir),
        1,
        "no re-export when output proven flushed"
    );
    assert_eq!(
        store.load().unwrap()["https://exp.test/a"].status,
        PageStatus::Committed
    );
}

#[test]
fn exported_record_missing_from_index_is_re_exported_exactly_once() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("reexp.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let first = ResumeContext::new(&ctx_store).with_resume(true);
    let results = [scraped("https://reexp.test/a", "orphan checkpoint")];
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&first),
    )
    .unwrap();

    // Impossible combo survived validation in a hostile timeline: EXPORTED
    // claims output whose hash is nowhere in the file → quarantine-style
    // recovery is reopen-for-reexport.
    demote(
        &store,
        "https://reexp.test/a",
        PageStatus::Exported,
        Some("sha256:not-in-index".to_string()),
    );

    let ctx_store2 = store.clone();
    let second = ResumeContext::new(&ctx_store2).with_resume(true);
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&second),
    )
    .unwrap();

    assert_eq!(
        jsonl_lines(&dir),
        2,
        "unproven EXPORTED is re-exported exactly once more"
    );
    assert_eq!(
        store.load().unwrap()["https://reexp.test/a"].status,
        PageStatus::Committed
    );
}

#[test]
fn item_failure_records_classified_error_and_crawl_continues() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("fail.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(true);

    let processed = process_results(
        &[
            scraped("https://fail.test/good", "healthy body"),
            scraped("https://fail.test/bad", ""), // empty content fails validation
        ],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .expect("item-level failures must not abort the export");

    assert_eq!(processed.len(), 1, "only the healthy item exports");
    let records = store.load().unwrap();
    assert_eq!(
        records["https://fail.test/good"].status,
        PageStatus::Committed
    );

    let bad = &records["https://fail.test/bad"];
    assert_ne!(
        bad.status,
        PageStatus::Committed,
        "failed item never advances past its recorded position"
    );
    let err = bad.last_error.as_ref().expect("failure is recorded");
    assert!(matches!(
        err.class,
        webfang_core::error::ErrorClass::DomainRecoverable
    ));
    assert_eq!(bad.attempts, 1, "attempts incremented on failure");
}

#[test]
fn fresh_run_without_resume_refetches_but_preserves_prior_records() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("fresh.test").with_state_dir(dir.path().to_path_buf());

    let ctx_store = store.clone();
    let resumed = ResumeContext::new(&ctx_store).with_resume(true);
    process_results(
        &[scraped("https://fresh.test/old", "old body")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&resumed),
    )
    .unwrap();

    // New run WITHOUT --resume: everything re-fetches, old records stay.
    let ctx_store2 = store.clone();
    let fresh = ResumeContext::new(&ctx_store2).with_resume(false);
    let processed = process_results(
        &[scraped("https://fresh.test/new", "new body")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&fresh),
    )
    .unwrap();

    assert_eq!(processed.len(), 1);
    let records = store.load().unwrap();
    assert_eq!(
        records["https://fresh.test/old"].status,
        PageStatus::Committed,
        "prior record preserved and queryable"
    );
    assert_eq!(
        records["https://fresh.test/new"].status,
        PageStatus::Committed
    );
}

#[test]
fn fresh_run_re_drives_committed_urls_because_skip_gates_only_on_resume() {
    let dir = TempDir::new().unwrap();
    let store = RecordStore::new("freskips.test").with_state_dir(dir.path().to_path_buf());
    let ctx_store = store.clone();
    let resumed = ResumeContext::new(&ctx_store).with_resume(true);
    let results = [scraped("https://freskips.test/a", "same body")];
    process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&resumed),
    )
    .unwrap();

    let ctx_store2 = store.clone();
    let fresh = ResumeContext::new(&ctx_store2).with_resume(false);
    let processed = process_results(
        &results,
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&fresh),
    )
    .unwrap();

    assert_eq!(
        processed.len(),
        1,
        "fresh run ignores history and re-drives"
    );
    assert_eq!(jsonl_lines(&dir), 2);
}
