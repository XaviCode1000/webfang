//! CorrelationId fidelity tests.
//!
//! Verifies that CorrelationId is preserved through:
//! 1. Error wrapping (DomainError → ScraperError via From)
//! 2. Async boundaries (tokio::spawn)
//! 3. Serialization round-trip (serde)
//! 4. W3C traceparent format integrity

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
    // Total length: "webfang=v1:" (16) + 32 hex = 48
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
