//! Scrape metrics — process-lifetime accumulator for MCP scraping activity.
//!
//! Pure types + logic only (DD-2): no DI, no locking, fully unit-testable.
//! Locking/tracing wiring lives on `McpState` (see `state.rs`). One structured
//! tracing event is emitted per recorded scrape (REQ-09) INSIDE
//! [`ScrapeMetrics::record`] (DD-3), keeping the critical section synchronous.
//!
//! Serialization is snapshot-safe (REQ-08): domains/tools use [`BTreeMap`] for
//! deterministic key order (DD-6) and timing collapses to a single flat
//! `average_duration_ms` field (DD-5) that is trivially redactable.

use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// Outcome bucket for a scrape event (A1: success/error, NOT raw HTTP codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The underlying scraping operation returned `Ok`.
    Success,
    /// The underlying scraping operation returned `Err`.
    Error,
    /// Partial success — some URLs succeeded, some failed (batch operations).
    Partial,
}

impl Outcome {
    /// Whether this outcome is a full success.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// Whether this outcome is partial (some successes, some failures).
    #[must_use]
    pub fn is_partial(self) -> bool {
        matches!(self, Self::Partial)
    }
}

/// One recorded scrape event.
#[derive(Debug, Clone)]
pub struct ScrapeEvent {
    /// Handler/tool name literal (e.g. `"scrape_url"`).
    pub tool: &'static str,
    /// Target host (`host_str()`) or `"unknown"` when unparseable (REQ-01).
    pub domain: String,
    /// Success/error bucket (A1).
    pub outcome: Outcome,
    /// Page/result count per the per-tool mapping.
    pub count: usize,
    /// Wall-clock duration of the operation.
    pub duration: Duration,
    /// Run-trace UUID (hex, dashed) of the tool call that produced this event;
    /// `Some` for handlers that mint a run-root identity (#698), `None` when
    /// the operation has no identity (e.g. synthetic test events).
    pub trace_id: Option<String>,
    /// W3C traceparent of the tool call's run-root identity (#698); pairs with
    /// `trace_id` so the metric event is reconstructable with the run trace.
    pub correlation_id: Option<String>,
}

impl ScrapeEvent {
    /// Build a scrape event without run identity (test / synthetic events).
    #[must_use]
    pub fn new(
        tool: &'static str,
        domain: String,
        outcome: Outcome,
        count: usize,
        duration: Duration,
    ) -> Self {
        Self {
            tool,
            domain,
            outcome,
            count,
            duration,
            trace_id: None,
            correlation_id: None,
        }
    }

    /// Build a scrape event stamped with the tool call's run-root identity
    /// (#698): the run-trace UUID as `trace_id` plus the W3C traceparent as
    /// `correlation_id`, so the metric event is reconstructable with the run
    /// trace. An MCP tool call IS an operation (#501) — handlers mint one
    /// identity at entry and share it across the call's success/error events
    /// through this single constructor, which is the only place that wires a
    /// [`CorrelationId`](webfang_core::domain::CorrelationId) into a
    /// [`ScrapeEvent`].
    #[must_use]
    pub fn identified(
        tool: &'static str,
        domain: String,
        outcome: Outcome,
        count: usize,
        duration: Duration,
        correlation: &webfang_core::domain::CorrelationId,
    ) -> Self {
        let mut event = Self::new(tool, domain, outcome, count, duration);
        event.trace_id = Some(correlation.trace_id().to_string());
        event.correlation_id = Some(correlation.to_traceparent());
        event
    }
}

/// Maximum number of distinct domains tracked in the per-domain breakdown
/// (REQ-05). Beyond this cap, new domains aggregate into [`OVERFLOW_DOMAIN_KEY`].
///
/// #1130: the value moved to `webfang_core::domain::budget` — the single
/// source shared with the `DomainSessionPool` cap — and is re-exported here
/// so existing `metrics::MAX_TRACKED_DOMAINS` paths keep working.
pub use webfang_core::domain::budget::MAX_TRACKED_DOMAINS;

/// Overflow bucket key for domains recorded past [`MAX_TRACKED_DOMAINS`]
/// (REQ-05). A domain literally named `"otros"` merges into this same bucket —
/// documented, accepted.
pub const OVERFLOW_DOMAIN_KEY: &str = "otros";

/// Per-domain aggregate (REQ-02).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DomainStats {
    /// Number of scrape events recorded for this domain.
    pub events: usize,
    /// Total pages/results accumulated across this domain's events.
    pub pages: usize,
    /// Number of error-outcome events for this domain.
    pub errors: usize,
}

/// Process-lifetime scrape accumulator (A3: no reset).
///
/// NOT `Serialize` — [`ScrapeMetrics::snapshot`] is the serializable view.
#[derive(Debug, Clone, Default)]
pub struct ScrapeMetrics {
    total_events: usize,
    success_count: usize,
    error_count: usize,
    /// Partial-success events (batch with some failures).
    partial_count: usize,
    /// Per-domain breakdown; `BTreeMap` for deterministic order (DD-6).
    domains: BTreeMap<String, DomainStats>,
    /// Per-tool event counts; `BTreeMap` for deterministic order (DD-6).
    tools: BTreeMap<String, usize>,
    duration_sum: Duration,
}

impl ScrapeMetrics {
    /// Record a single scrape event.
    ///
    /// Short synchronous critical section: emits the structured tracing event
    /// (REQ-09) then mutates the aggregates. No `.await` here, so the lock held
    /// by the caller is never held across an await point (REQ-07).
    pub fn record(&mut self, event: ScrapeEvent) {
        tracing::info!(
            tool = %event.tool,
            domain = %event.domain,
            success = event.outcome.is_success(),
            duration_ms = event.duration.as_millis() as u64,
            pages = event.count,
            trace_id = event.trace_id.clone(),
            correlation_id = event.correlation_id.clone(),
            "scrape recorded"
        );

        self.total_events += 1;
        match event.outcome {
            Outcome::Success => self.success_count += 1,
            Outcome::Error => self.error_count += 1,
            Outcome::Partial => self.partial_count += 1,
        }

        // REQ-05: cap the per-domain map. Totals above are updated
        // unconditionally; already-tracked domains keep their own key even
        // at the cap so re-aggregation stays exact.
        let key: &str = if self.domains.contains_key(&event.domain)
            || self.domains.len() < MAX_TRACKED_DOMAINS
        {
            event.domain.as_str()
        } else {
            OVERFLOW_DOMAIN_KEY
        };
        let domain = self.domains.entry(key.to_string()).or_default();
        domain.events += 1;
        domain.pages += event.count;
        if matches!(event.outcome, Outcome::Error | Outcome::Partial) {
            domain.errors += 1;
        }

        *self.tools.entry(event.tool.to_string()).or_default() += 1;
        self.duration_sum += event.duration;
    }

    /// Whether any events have been recorded (DD-12: empty ≡ `total_events == 0`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_events == 0
    }

    /// Produce a serializable point-in-time view (REQ-08).
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        let average_duration_ms = if self.total_events == 0 {
            0.0
        } else {
            self.duration_sum.as_millis() as f64 / self.total_events as f64
        };
        MetricsSnapshot {
            total_events: self.total_events,
            success_count: self.success_count,
            error_count: self.error_count,
            partial_count: self.partial_count,
            domains: self.domains.clone(),
            tools: self.tools.clone(),
            average_duration_ms,
        }
    }
}

/// Serializable point-in-time view of the accumulator (REQ-08).
///
/// Timing collapses to one flat `average_duration_ms` field (DD-5) so snapshots
/// stay deterministic and trivially redactable.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsSnapshot {
    /// Total recorded scrape events.
    pub total_events: usize,
    /// Number of success-outcome events.
    pub success_count: usize,
    /// Number of error-outcome events.
    pub error_count: usize,
    /// Number of partial-success events (batch with some failures).
    pub partial_count: usize,
    /// Per-domain breakdown (sorted keys).
    pub domains: BTreeMap<String, DomainStats>,
    /// Per-tool event counts (sorted keys).
    pub tools: BTreeMap<String, usize>,
    /// Mean wall-clock duration in milliseconds across all events.
    pub average_duration_ms: f64,
}

/// Extract the target host from a URL, falling back to `"unknown"` (REQ-01).
#[must_use]
pub fn domain_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

    /// Set a global fmt subscriber (sink writer) so every `tracing` callsite
    /// registers with `Interest::always()` instead of the `Interest::never()`
    /// that gets cached process-wide when a thread hits a callsite with no
    /// subscriber active (see [`record_emits_structured_tracing_event`]).
    fn ensure_global_subscriber() {
        GLOBAL_SUBSCRIBER_INIT.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });
    }

    /// Build the canonical fixture: 2 success events on `a.com` (counts 3, 5)
    /// and 1 error event on `b.com` (count 0), durations 100/200/300ms, all via
    /// the `scrape_url` tool. Mean duration = (100+200+300)/3 = 200ms.
    fn record_fixture(m: &mut ScrapeMetrics) {
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            3,
            Duration::from_millis(100),
        ));
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            5,
            Duration::from_millis(200),
        ));
        m.record(ScrapeEvent::new(
            "scrape_url",
            "b.com".to_string(),
            Outcome::Error,
            0,
            Duration::from_millis(300),
        ));
    }

    /// REQ-02: aggregates accumulate exactly (counts, per-domain, timing mean).
    #[test]
    fn aggregates_exact_numbers() {
        let mut m = ScrapeMetrics::default();
        record_fixture(&mut m);

        let snap = m.snapshot();
        assert_eq!(snap.total_events, 3, "total_events");
        assert_eq!(snap.success_count, 2, "success_count");
        assert_eq!(snap.error_count, 1, "error_count");
        assert_eq!(
            snap.domains.get("a.com"),
            Some(&DomainStats {
                events: 2,
                pages: 8,
                errors: 0
            }),
            "domain a.com aggregate"
        );
        assert_eq!(
            snap.domains.get("b.com"),
            Some(&DomainStats {
                events: 1,
                pages: 0,
                errors: 1
            }),
            "domain b.com aggregate"
        );
        assert_eq!(snap.average_duration_ms, 200.0, "average_duration_ms");
        assert!(!m.is_empty(), "non-empty after recording");
    }

    /// REQ-05: the 501st distinct domain aggregates into the `"otros"`
    /// overflow bucket while global totals (events, outcomes, duration mean)
    /// stay exact — totals are updated unconditionally before the cap.
    ///
    /// 600 distinct domains, one event each, outcome cycling
    /// Success/Error/Partial by `i % 3`, page count `(i % 5) + 1`, duration
    /// cycling 10/20/30/40 ms. The first 500 keep their own keys; domains
    /// 500..600 (100 events) merge: pages = 20 cycles × (1+2+3+4+5) = 300,
    /// errors = the 67 events whose `i % 3 != 0`. Mean duration over all 600
    /// events = exactly 25 ms.
    #[test]
    fn domains_capped_at_500_with_otros_overflow() {
        let mut m = ScrapeMetrics::default();
        for i in 0..600usize {
            let outcome = match i % 3 {
                0 => Outcome::Success,
                1 => Outcome::Error,
                _ => Outcome::Partial,
            };
            m.record(ScrapeEvent::new(
                "scrape_url",
                format!("d{i:04}.com"),
                outcome,
                (i % 5) + 1,
                Duration::from_millis(((i % 4) + 1) as u64 * 10),
            ));
        }

        let snap = m.snapshot();
        assert_eq!(snap.total_events, 600, "global total unaffected by cap");
        assert_eq!(snap.success_count, 200, "success_count exact");
        assert_eq!(snap.error_count, 200, "error_count exact");
        assert_eq!(snap.partial_count, 200, "partial_count exact");
        assert_eq!(snap.average_duration_ms, 25.0, "duration mean exact");

        assert_eq!(
            snap.domains.len(),
            501,
            "500 tracked domains + one overflow bucket"
        );
        assert_eq!(
            snap.domains.get("otros"),
            Some(&DomainStats {
                events: 100,
                pages: 300,
                errors: 67
            }),
            "overflow aggregates with identical semantics"
        );
        assert!(
            !snap.domains.contains_key("d0500.com"),
            "overflow domains must not get their own key"
        );
        assert_eq!(
            snap.domains.get("d0000.com"),
            Some(&DomainStats {
                events: 1,
                pages: 1,
                errors: 0
            }),
            "first tracked domain keeps exact stats"
        );
    }

    /// REQ-05: an already-tracked domain keeps aggregating under its own key
    /// after the cap is reached (`contains_key` takes precedence); only NEW
    /// domains past the cap land in `"otros"`.
    #[test]
    fn existing_domain_aggregates_after_cap_reached() {
        let mut m = ScrapeMetrics::default();
        for i in 0..500usize {
            m.record(ScrapeEvent::new(
                "scrape_url",
                format!("d{i:04}.com"),
                Outcome::Success,
                1,
                Duration::from_millis(1),
            ));
        }
        let snap = m.snapshot();
        assert_eq!(snap.domains.len(), 500, "cap not yet exceeded");
        assert!(!snap.domains.contains_key("otros"), "no overflow yet");

        // Domain #1 (already tracked) still aggregates under its own key.
        m.record(ScrapeEvent::new(
            "scrape_url",
            "d0000.com".to_string(),
            Outcome::Error,
            3,
            Duration::from_millis(1),
        ));
        let snap = m.snapshot();
        assert_eq!(snap.domains.len(), 500, "re-record adds no key");
        assert_eq!(
            snap.domains.get("d0000.com"),
            Some(&DomainStats {
                events: 2,
                pages: 4,
                errors: 1
            }),
            "existing domain keeps aggregating under its own key"
        );

        // A brand-new domain #502 lands in the overflow bucket.
        m.record(ScrapeEvent::new(
            "scrape_url",
            "newdomain.com".to_string(),
            Outcome::Error,
            7,
            Duration::from_millis(1),
        ));
        let snap = m.snapshot();
        assert_eq!(snap.domains.len(), 501, "cap + overflow bucket");
        assert!(
            !snap.domains.contains_key("newdomain.com"),
            "new domain past the cap must not get its own key"
        );
        assert_eq!(
            snap.domains.get("otros"),
            Some(&DomainStats {
                events: 1,
                pages: 7,
                errors: 1
            }),
            "first overflow event lands in otros"
        );

        // A second new domain keeps merging into the same bucket.
        m.record(ScrapeEvent::new(
            "scrape_url",
            "anothernew.com".to_string(),
            Outcome::Success,
            2,
            Duration::from_millis(1),
        ));
        assert_eq!(
            m.snapshot().domains.get("otros"),
            Some(&DomainStats {
                events: 2,
                pages: 9,
                errors: 1
            }),
            "overflow keeps aggregating"
        );
    }

    /// REQ-01: an unparseable domain is recorded as `"unknown"` and still counted.
    #[test]
    fn record_unknown_domain() {
        assert_eq!(domain_of("not a url"), "unknown", "unparseable → unknown");
        assert_eq!(
            domain_of("https://example.com/path?q=1"),
            "example.com",
            "parseable → host_str"
        );

        let mut m = ScrapeMetrics::default();
        m.record(ScrapeEvent::new(
            "scrape_url",
            domain_of("not a url"),
            Outcome::Success,
            1,
            Duration::from_millis(50),
        ));

        let snap = m.snapshot();
        assert_eq!(snap.total_events, 1, "unknown-domain event still counted");
        assert_eq!(
            snap.domains.get("unknown"),
            Some(&DomainStats {
                events: 1,
                pages: 1,
                errors: 0
            }),
            "unknown bucket present"
        );
    }

    /// REQ-08: fixed durations serialize to an exact, byte-stable pretty JSON
    /// string (BTreeMap sorted keys, flat `average_duration_ms`).
    #[test]
    fn snapshot_serialization_exact() {
        let mut m = ScrapeMetrics::default();
        record_fixture(&mut m);
        let snap = m.snapshot();

        let expected = [
            "{",
            "  \"total_events\": 3,",
            "  \"success_count\": 2,",
            "  \"error_count\": 1,",
            "  \"partial_count\": 0,",
            "  \"domains\": {",
            "    \"a.com\": {",
            "      \"events\": 2,",
            "      \"pages\": 8,",
            "      \"errors\": 0",
            "    },",
            "    \"b.com\": {",
            "      \"events\": 1,",
            "      \"pages\": 0,",
            "      \"errors\": 1",
            "    }",
            "  },",
            "  \"tools\": {",
            "    \"scrape_url\": 3",
            "  },",
            "  \"average_duration_ms\": 200.0",
            "}",
        ]
        .join("\n");

        assert_eq!(
            serde_json::to_string_pretty(&snap).expect("snapshot must serialize"),
            expected,
            "serialization must be byte-stable"
        );
    }

    /// REQ-07: 100 concurrent records lose nothing (no lost updates/deadlock).
    #[tokio::test]
    async fn concurrent_records_lose_nothing() {
        let metrics = Arc::new(Mutex::new(ScrapeMetrics::default()));
        let mut handles = Vec::with_capacity(100);
        for _ in 0..100 {
            let m = Arc::clone(&metrics);
            handles.push(tokio::spawn(async move {
                m.lock().expect("metrics mutex").record(ScrapeEvent::new(
                    "scrape_url",
                    "a.com".to_string(),
                    Outcome::Success,
                    1,
                    Duration::from_millis(1),
                ));
            }));
        }
        for h in handles {
            h.await.expect("task must not panic");
        }

        let snap = metrics.lock().expect("metrics mutex").snapshot();
        assert_eq!(snap.total_events, 100, "no lost updates under concurrency");
    }

    /// REQ-09: each record emits ONE structured tracing event carrying
    /// tool/domain/success/duration_ms/pages (English field names/values).
    ///
    /// `tracing` caches per-callsite `Interest` process-wide: if a NON-serial
    /// test calls [`ScrapeMetrics::record`] without a subscriber active, its
    /// thread registers the `"scrape recorded"` callsite as `Interest::never()`
    /// and that decision is cached forever — this test's scoped `with_default`
    /// subscriber would then never receive the event, even with `#[serial]`
    /// (serial_test only excludes other `#[serial]` tests).
    ///
    /// Fix: [`ensure_global_subscriber`] (invoked before capture) installs a
    /// global fmt subscriber writing to sink, so every callsite registers with
    /// `Interest::always()`; `set_global_default` also triggers a global
    /// interest rebuild that recovers already-poisoned callsites. The global is
    /// a fallback only — the per-test `with_default` below still overrides it
    /// for capture on the test thread.
    #[test]
    #[serial]
    fn record_emits_structured_tracing_event() {
        ensure_global_subscriber();
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::Layer;
        use tracing_subscriber::prelude::*;

        /// Collects `(field_name, value)` pairs as strings.
        struct Capture(Arc<Mutex<Vec<(String, String)>>>);

        impl Visit for Capture {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), format!("{value:?}")));
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), value.to_string()));
            }
            fn record_bool(&mut self, field: &Field, value: bool) {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), value.to_string()));
            }
            fn record_u64(&mut self, field: &Field, value: u64) {
                self.0
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), value.to_string()));
            }
        }

        struct CaptureLayer {
            events: Arc<Mutex<Vec<(String, String)>>>,
        }

        impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut visitor = Capture(Arc::clone(&self.events));
                event.record(&mut visitor);
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CaptureLayer {
            events: Arc::clone(&events),
        };
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let mut event = ScrapeEvent::new(
                "scrape_url",
                "example.com".to_string(),
                Outcome::Success,
                3,
                Duration::from_millis(120),
            );
            event.trace_id = Some("01949e0e-8b8e-7000-8000-000000000001".to_string());
            event.correlation_id =
                Some("00-01949e0e8b8e70008000000000000001-0000000000000042-01".to_string());
            ScrapeMetrics::default().record(event);
        });

        let captured = events.lock().expect("capture mutex");
        let field = |name: &str| -> Option<String> {
            captured
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(field("tool").as_deref(), Some("scrape_url"), "tool field");
        assert_eq!(
            field("domain").as_deref(),
            Some("example.com"),
            "domain field"
        );
        assert_eq!(field("success").as_deref(), Some("true"), "success field");
        assert_eq!(
            field("duration_ms").as_deref(),
            Some("120"),
            "duration_ms field"
        );
        assert_eq!(field("pages").as_deref(), Some("3"), "pages field");
        assert_eq!(
            field("trace_id").as_deref(),
            Some("01949e0e-8b8e-7000-8000-000000000001"),
            "trace_id field"
        );
        assert_eq!(
            field("correlation_id").as_deref(),
            Some("00-01949e0e8b8e70008000000000000001-0000000000000042-01"),
            "correlation_id field"
        );
    }

    /// #698: events with no identity (`None` trace_id/correlation_id — e.g.
    /// synthetic test events) must emit NO trace_id/correlation_id fields at
    /// all, so the presence of a key always implies a real identity.
    #[test]
    #[serial]
    fn record_omits_identity_when_none() {
        ensure_global_subscriber();
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::Layer;
        use tracing_subscriber::prelude::*;

        /// Collects field names only (values are irrelevant here).
        struct Keys(Arc<Mutex<Vec<String>>>);

        impl Visit for Keys {
            fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
                self.0
                    .lock()
                    .expect("keys mutex")
                    .push(field.name().to_string());
            }
        }

        struct KeysLayer {
            keys: Arc<Mutex<Vec<String>>>,
        }

        impl<S: tracing::Subscriber> Layer<S> for KeysLayer {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut visitor = Keys(Arc::clone(&self.keys));
                event.record(&mut visitor);
            }
        }

        let keys = Arc::new(Mutex::new(Vec::new()));
        let layer = KeysLayer {
            keys: Arc::clone(&keys),
        };
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            ScrapeMetrics::default().record(ScrapeEvent::new(
                "scrape_url",
                "example.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(10),
            ));
        });

        let keys = keys.lock().expect("keys mutex");
        assert!(!keys.contains(&"trace_id".to_string()), "no trace_id key");
        assert!(
            !keys.contains(&"correlation_id".to_string()),
            "no correlation_id key"
        );
    }
}
