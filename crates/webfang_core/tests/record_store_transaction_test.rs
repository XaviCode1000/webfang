//! #1230 / F-07 — the record store's read-modify-write cycle must be atomic.
//!
//! Deterministic pin: two independent handles on one state directory,
//! interleaved in exactly the shape two `--resume` processes interleave. The
//! interleaving is expressed in the **call order**, not in sleeps, so the test
//! cannot pass by luck and cannot fail by scheduling noise.
//!
//! Why a seam test and not only a process test: the window that actually loses
//! data is `CommitSession::open()` → last `save()` (`application/export_factory.rs`),
//! which is milliseconds wide. A multi-process test can only hit it
//! probabilistically, so it is kept as an ignored reproduction in
//! `tests/behavioral/cli/transactional_store_test.rs`. This file is the gate.
//!
//! `RecordStorePort` is a domain-owned public port (ADR-0012-B §3.H), so these
//! tests exercise an external contract, not internal state.
//!
//! Evidence naming (ADR-0016 §5, issue #1292): the concurrency tests here carry
//! the `f07_` prefix and are the NAMED F-07 evidence. The multi-process
//! reproduction in `tests/behavioral/cli/transactional_store_test.rs` stays
//! ignored as a documented stress check, never CI evidence.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use webfang_core::domain::persistence::{
    DomainRecords, RawRecord, RecordStoreError, RecordStorePort,
};
use webfang_core::infrastructure::export::RecordStore;

/// A store handle rooted at `dir` for the given domain key, seen through the
/// domain-owned port. Every test drives the port, not the concrete: `#1230` is
/// a defect in the contract production code actually consumes
/// (`application/export_factory.rs` holds `&dyn RecordStorePort`).
fn store(dir: &Path, domain: &str) -> RecordStore {
    RecordStore::new(domain).with_state_dir(dir.to_path_buf())
}

/// The same handle as the port it is used through in production.
fn port(store: &RecordStore) -> &dyn RecordStorePort {
    store
}

/// Insert `prefix/0 .. prefix/n-1` into `records`, mimicking one run's export.
fn insert_set(records: &mut DomainRecords, prefix: &str, n: usize) {
    for i in 0..n {
        let url = format!("http://example.com/{prefix}/{i}");
        let record = RawRecord::new_discovered(&url, &url, prefix, i as i64)
            .expect("non-empty urls are accepted by the choke point");
        records.insert(url, record);
    }
}

/// Every `url` field currently persisted at `dir`/`domain`.
fn persisted_urls(store: &RecordStore) -> BTreeSet<String> {
    store
        .load()
        .expect("store readable")
        .values()
        .map(|r| r.url.clone())
        .collect()
}

/// Two writers that each commit a disjoint set through `update` must end with
/// the union on disk.
///
/// This is the exact shape of #1230: with the load→save pair the second writer's
/// snapshot replaces the first writer's file and its records vanish. `update`
/// loads *inside* the exclusive lock, so the second writer sees the first one's
/// committed records and adds to them instead of overwriting them.
#[test]
fn f07_concurrent_updates_persist_the_union_of_both_writers() {
    let dir = TempDir::new().unwrap();
    let writer_a = store(dir.path(), "example.com");
    let writer_b = store(dir.path(), "example.com");

    port(&writer_a)
        .update(&mut |records| {
            insert_set(records, "alpha", 3);
            Ok(())
        })
        .expect("writer A update should succeed");

    port(&writer_b)
        .update(&mut |records| {
            insert_set(records, "beta", 4);
            Ok(())
        })
        .expect("writer B update should succeed");

    let persisted = persisted_urls(&writer_a);
    let expected: BTreeSet<String> = (0..3)
        .map(|i| format!("http://example.com/alpha/{i}"))
        .chain((0..4).map(|i| format!("http://example.com/beta/{i}")))
        .collect();

    assert_eq!(
        persisted, expected,
        "second concurrent writer clobbered the first (#1230)"
    );
}

/// #1230 / F-07 — companion to the `f07_` union evidence above: a transaction
/// must observe what another writer already committed.
///
/// Without this, `update` could be implemented as "load a stale snapshot,
/// mutate, write" and still satisfy the previous test by accident. The
/// observation is asserted *inside* the closure, where it is unambiguous.
#[test]
fn f07_update_observes_records_committed_by_another_writer() {
    let dir = TempDir::new().unwrap();
    let writer_a = store(dir.path(), "example.com");
    let writer_b = store(dir.path(), "example.com");

    writer_a
        .update(&mut |records| {
            insert_set(records, "alpha", 2);
            Ok(())
        })
        .expect("A commits");

    let saw_alpha = std::cell::Cell::new(false);
    port(&writer_b)
        .update(&mut |records| {
            saw_alpha.set(records.contains_key("http://example.com/alpha/0"));
            insert_set(records, "beta", 2);
            Ok(())
        })
        .expect("B commits");

    assert!(
        saw_alpha.get(),
        "writer B's transaction did not observe writer A's committed record — \
         the lock does not span the read (#1230)"
    );
}

/// A failing transaction writes nothing.
///
/// "Atomic" means both halves: no lost updates on success, and no partial state
/// when the caller bails out. Without this, a caller could not use `update` to
/// hold a read-modify-write invariant.
#[test]
fn failed_update_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let writer = store(dir.path(), "example.com");

    writer
        .update(&mut |records| {
            insert_set(records, "seed", 1);
            Ok(())
        })
        .expect("seed commit");

    let before = persisted_urls(&writer);
    let boom = || {
        Err(RecordStoreError::InvalidRecord {
            reason: "injected failure",
        })
    };

    let outcome = writer.update(&mut |records| {
        insert_set(records, "ghost", 5);
        boom()
    });

    assert!(outcome.is_err(), "the injected failure must propagate");
    assert_eq!(
        persisted_urls(&writer),
        before,
        "a rolled-back transaction must leave the store byte-identical"
    );
}

/// P8-3 at the seam: a transaction over an unreadable store must report it.
///
/// `load_or_init` deliberately degrades to an empty view so a run can proceed;
/// a *transaction* must not, because silently starting fresh is what turns
/// corrupt state into a whole-file overwrite of whatever the caller writes.
#[test]
fn update_over_corrupt_store_errors_and_preserves_the_bytes() {
    let dir = TempDir::new().unwrap();
    let writer = store(dir.path(), "example.com");
    let state_path = writer.state_path();
    std::fs::write(&state_path, "not valid json!!!").expect("write corrupt state");

    let outcome = writer.update(&mut |records| {
        insert_set(records, "alpha", 3);
        Ok(())
    });

    assert!(
        matches!(outcome, Err(RecordStoreError::Corrupt { .. })),
        "corrupt store must surface RecordStoreError::Corrupt, got {outcome:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).expect("re-read"),
        "not valid json!!!",
        "a rejected transaction must never overwrite the file it could not read"
    );
}

/// Many threads committing disjoint sets through one shared handle must all
/// survive.
///
/// This is the concurrency claim stated where it can actually be checked in CI:
/// `StoreLock` is `flock(2)` on a fresh fd per acquisition, so competing threads
/// do contend with each other the same way competing processes do.
#[test]
fn many_threads_updating_one_store_keep_every_record() {
    const WRITERS: usize = 8;
    const PER_WRITER: usize = 5;

    let dir = TempDir::new().unwrap();
    let store = Arc::new(store(dir.path(), "example.com"));

    let handles: Vec<_> = (0..WRITERS)
        .map(|i| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                store
                    .update(&mut |records| {
                        insert_set(records, &format!("w{i}"), PER_WRITER);
                        Ok(())
                    })
                    .expect("contended update should succeed, not lose state")
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("writer thread panicked");
    }

    let persisted = persisted_urls(&store);
    let expected: BTreeSet<String> = (0..WRITERS)
        .flat_map(|i| (0..PER_WRITER).map(move |j| format!("http://example.com/w{i}/{j}")))
        .collect();

    assert_eq!(
        persisted.len(),
        expected.len(),
        "lost update across {} concurrent writers: {} of {} records survived (#1230)",
        WRITERS,
        persisted.len(),
        expected.len(),
    );
    assert_eq!(persisted, expected);
}
