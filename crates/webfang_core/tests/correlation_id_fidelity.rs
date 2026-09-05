//! CorrelationId fidelity tests.
//!
//! Verifies that CorrelationId is preserved through:
//! 1. Error wrapping (DomainError → ScraperError via From)
//! 2. Async boundaries (tokio::spawn)
//! 3. Serialization round-trip (serde)
//! 4. W3C traceparent format integrity
//! 5. Derivation contract — run-root identity propagation (#687 evidence)

use webfang_core::domain::value_objects::CorrelationId;
use webfang_core::domain::DomainError;
use webfang_core::error::ScraperError;

// ===========================================================================
// Error Wrapping Fidelity
// ===========================================================================

#[test]
fn correlation_id_preserved_through_domain_error_display() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    let err = DomainError::ExtractionFailed {
        url: "https://example.com".to_string(),
        reason: format!("correlation_id={traceparent}"),
    };

    let display = err.to_string();
    assert!(
        display.contains(&traceparent),
        "DomainError Display should preserve correlation ID: {display}"
    );
}

#[test]
fn correlation_id_preserved_through_scraper_error_from() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    let domain_err = DomainError::InvalidUrl(format!("bad url (cid={traceparent})"));
    let scraper_err: ScraperError = domain_err.into();

    let display = scraper_err.to_string();
    assert!(
        display.contains(&traceparent),
        "ScraperError from DomainError should preserve correlation ID: {display}"
    );
}

#[test]
fn correlation_id_preserved_through_multiple_wrapping() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    // DomainError → ScraperError
    let domain_err = DomainError::Readability(format!("parse failed (cid={traceparent})"));
    let scraper_err: ScraperError = domain_err.into();

    // ScraperError → Display → String (simulating TUI rendering)
    let rendered = format!("{scraper_err}");
    assert!(
        rendered.contains(&traceparent),
        "Multi-layer wrapping should preserve correlation ID: {rendered}"
    );
}

// ===========================================================================
// Async Boundary Fidelity
// ===========================================================================

#[tokio::test]
async fn correlation_id_survives_tokio_spawn() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    let result = tokio::spawn(async move {
        // Inside the spawned task — CorrelationId should be identical
        corr.to_traceparent()
    })
    .await
    .expect("spawn should succeed");

    assert_eq!(
        result, traceparent,
        "CorrelationId must be identical after tokio::spawn boundary"
    );
}

#[tokio::test]
async fn correlation_id_survives_multiple_spawn_chain() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    let result = tokio::spawn(async move {
        let inner = corr;
        tokio::spawn(async move {
            // Second spawn boundary
            inner.to_traceparent()
        })
        .await
        .expect("inner spawn should succeed")
    })
    .await
    .expect("outer spawn should succeed");

    assert_eq!(
        result, traceparent,
        "CorrelationId must survive chained spawn boundaries"
    );
}

#[tokio::test]
async fn correlation_id_survives_join() {
    let corr1 = CorrelationId::new();
    let corr2 = CorrelationId::new();
    let tp1 = corr1.to_traceparent();
    let tp2 = corr2.to_traceparent();

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { corr1.to_traceparent() }),
        tokio::spawn(async move { corr2.to_traceparent() }),
    );

    assert_eq!(r1.unwrap(), tp1, "first CorrelationId must survive join");
    assert_eq!(r2.unwrap(), tp2, "second CorrelationId must survive join");
}

// ===========================================================================
// Serialization Fidelity
// ===========================================================================

#[test]
fn correlation_id_json_roundtrip() {
    let corr = CorrelationId::new();
    let json = serde_json::to_string(&corr).expect("serialize should succeed");
    let deserialized: CorrelationId =
        serde_json::from_str(&json).expect("deserialize should succeed");

    assert_eq!(corr.trace_id(), deserialized.trace_id());
    assert_eq!(corr.span_id(), deserialized.span_id());
    assert_eq!(corr.to_traceparent(), deserialized.to_traceparent());
}

#[test]
fn correlation_id_json_contains_traceparent() {
    let corr = CorrelationId::new();
    let json = serde_json::to_string(&corr).expect("serialize");

    // JSON should contain the trace_id and span_id fields
    assert!(json.contains("trace_id"));
    assert!(json.contains("span_id"));
}

// ===========================================================================
// W3C Traceparent Format Integrity
// ===========================================================================

#[test]
fn traceparent_format_is_w3c_compliant() {
    let corr = CorrelationId::new();
    let tp = corr.to_traceparent();

    // W3C format: 00-{trace_id}-{span_id}-{trace_flags}
    let parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
    assert_eq!(parts[0], "00", "version should be 00");
    assert_eq!(parts[1].len(), 32, "trace_id should be 32 hex chars");
    assert_eq!(parts[2].len(), 16, "span_id should be 16 hex chars");
    assert_eq!(parts[3], "01", "trace_flags should be 01 (sampled)");

    // All parts should be valid hex
    assert!(
        u128::from_str_radix(parts[1], 16).is_ok(),
        "trace_id should be valid hex"
    );
    assert!(
        u64::from_str_radix(parts[2], 16).is_ok(),
        "span_id should be valid hex"
    );
}

#[test]
fn traceparent_is_deterministic_for_same_ids() {
    let trace_id = uuid::Uuid::now_v7();
    let span_id: u64 = 0xDEAD_BEEF_CAFE_BABE;

    let corr1 = CorrelationId::new_with_ids(trace_id, span_id);
    let corr2 = CorrelationId::new_with_ids(trace_id, span_id);

    assert_eq!(
        corr1.to_traceparent(),
        corr2.to_traceparent(),
        "same IDs should produce identical traceparent"
    );
}

#[test]
fn tracestate_format() {
    let corr = CorrelationId::new();
    let ts = corr.to_tracestate();

    assert!(
        ts.starts_with("webfang=v1:"),
        "tracestate should use vendor format"
    );
    // Total length: "webfang=v1:" (11) + 32 hex = 43
    assert_eq!(ts.len(), 43);
}

// ===========================================================================
// Concurrency Safety (compile-time)
// ===========================================================================

#[test]
fn correlation_id_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CorrelationId>();
}

#[tokio::test]
async fn correlation_id_shared_across_tasks() {
    let corr = CorrelationId::new();
    let traceparent = corr.to_traceparent();

    // Clone and send to multiple tasks
    let corr1 = corr.clone();
    let corr2 = corr.clone();

    let (r1, r2, r3) = tokio::join!(
        tokio::spawn(async move { corr1.to_traceparent() }),
        tokio::spawn(async move { corr2.to_traceparent() }),
        tokio::spawn(async move { corr.to_traceparent() }),
    );

    // All should produce the same traceparent
    assert_eq!(r1.unwrap(), traceparent);
    assert_eq!(r2.unwrap(), traceparent);
    assert_eq!(r3.unwrap(), traceparent);
}

// ===========================================================================
// Derivation Contract — run-root identity propagation (issue #687 / #704)
// ===========================================================================
//
// The discovery engine and the scrape flow must NOT mint their own trace
// UUIDs: every page derives `.child()` from the run-root identity, so a
// whole run stays reconstructable under ONE trace_id (audit F14). Without
// this contract each subsystem that called `CorrelationId::new()` per page
// would fragment the run into N unrelated traces — the exact symptom of
// #687 ("discovery acuña un trace UUID propio"). `scrape_single_url`
// (discovery.rs) and `scrape_urls` (scrape_flow.rs) take the identity as a
// REQUIRED parameter (#501); these tests pin the derivation semantics that
// make that parameter sufficient.

/// The root identity is fixed for every test in this section so assertions
/// are exact-equality against a known trace UUID.
fn fixed_root() -> CorrelationId {
    let trace_id =
        uuid::Uuid::parse_str("01949e0e-8b8e-7000-8000-000000000001").expect("valid UUID");
    CorrelationId::new_with_ids(trace_id, 0x0000_0000_0000_0042)
}

/// A derived page identity must keep the run-root `trace_id` and get its own
/// `span_id` — the core invariant that lets the discovery engine reuse the
/// root trace instead of minting a fresh UUID per page (#687).
#[test]
fn child_reuses_root_trace_id_and_mints_fresh_span_id() {
    let root = fixed_root();
    let page = root.child();

    assert_eq!(
        page.trace_id(),
        root.trace_id(),
        "a derived identity must stay under the run-root trace_id (#687: \
         discovery may not mint its own trace UUID)"
    );
    assert_ne!(
        page.span_id(),
        root.span_id(),
        "a derived identity must carry its own span_id so the page stays distinguishable"
    );
}

/// The derived identity's W3C traceparent must embed the root's trace UUID,
/// so forensic reconstruction (`jq 'select(.trace_id == ...)'`) sees one
/// trace per run regardless of which subsystem emits the span.
#[test]
fn child_traceparent_embeds_root_trace_uuid() {
    let root = fixed_root();
    let page = root.child();

    let root_hex = root.trace_id().as_simple().to_string();
    let page_traceparent = page.to_traceparent();
    let parts: Vec<&str> = page_traceparent.split('-').collect();
    assert_eq!(parts.len(), 4, "derived traceparent must be W3C-compliant");
    assert_eq!(
        parts[1], root_hex,
        "the trace part of a derived traceparent must equal the root trace UUID"
    );
    assert_ne!(
        parts[2],
        format!("{:016x}", root.span_id()),
        "the span part must be the page's own span_id, not the root's"
    );
}

/// Discovery derives one child per discovered/fetched page: ALL of them must
/// share exactly ONE distinct `trace_id` and carry distinct `span_id`s —
/// audit F14 done criterion ("1 trace_id esperado por corrida"; the #687
/// symptom was 2+ trace_ids per run).
#[test]
fn discovery_derivation_tree_keeps_one_trace_id_across_all_pages() {
    let root = fixed_root();

    // Simulate the per-page derivation fan-out: crawl task and scrape flow
    // both call `root.child()` once per page (crawl_task.rs:99,
    // scrape_flow.rs:234).
    let pages: Vec<CorrelationId> = (0..10).map(|_| root.child()).collect();

    let trace_ids: std::collections::BTreeSet<uuid::Uuid> =
        pages.iter().map(CorrelationId::trace_id).collect();
    assert_eq!(
        trace_ids.len(),
        1,
        "every derived page identity must share the run-root trace_id; \
         got {} distinct trace_id(s)",
        trace_ids.len()
    );
    assert_eq!(
        trace_ids.first().copied(),
        Some(root.trace_id()),
        "the shared trace_id must be the run-root's"
    );

    let span_ids: std::collections::BTreeSet<u64> =
        pages.iter().map(CorrelationId::span_id).collect();
    assert_eq!(
        span_ids.len(),
        pages.len(),
        "each derived page must own a distinct span_id (64-bit random space; \
         a collision here would indicate a broken RNG, not flakiness)"
    );
}

/// A derive-of-derive chain (e.g. a page discovered FROM a page) must still
/// stay under the original run-root trace — depth never fragments the trace.
#[test]
fn chained_derivation_never_fragments_the_trace() {
    let root = fixed_root();
    let depth1 = root.child();
    let depth2 = depth1.child();

    assert_eq!(depth2.trace_id(), root.trace_id());
    assert_eq!(depth2.trace_id(), depth1.trace_id());
    let depth2_tp = depth2.to_traceparent();
    let root_tp = root.to_traceparent();
    assert_eq!(
        depth2_tp.split('-').nth(1),
        root_tp.split('-').nth(1),
        "deep-derived identities must still embed the root trace UUID"
    );
}
