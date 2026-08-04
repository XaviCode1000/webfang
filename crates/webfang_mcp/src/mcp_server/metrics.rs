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
}

impl Outcome {
    /// Whether this outcome is a success.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
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
}

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
            "scrape recorded"
        );

        self.total_events += 1;
        match event.outcome {
            Outcome::Success => self.success_count += 1,
            Outcome::Error => self.error_count += 1,
        }

        let domain = self.domains.entry(event.domain.clone()).or_default();
        domain.events += 1;
        domain.pages += event.count;
        if event.outcome == Outcome::Error {
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

    /// Build the canonical fixture: 2 success events on `a.com` (counts 3, 5)
    /// and 1 error event on `b.com` (count 0), durations 100/200/300ms, all via
    /// the `scrape_url` tool. Mean duration = (100+200+300)/3 = 200ms.
    fn record_fixture(m: &mut ScrapeMetrics) {
        m.record(ScrapeEvent {
            tool: "scrape_url",
            domain: "a.com".to_string(),
            outcome: Outcome::Success,
            count: 3,
            duration: Duration::from_millis(100),
        });
        m.record(ScrapeEvent {
            tool: "scrape_url",
            domain: "a.com".to_string(),
            outcome: Outcome::Success,
            count: 5,
            duration: Duration::from_millis(200),
        });
        m.record(ScrapeEvent {
            tool: "scrape_url",
            domain: "b.com".to_string(),
            outcome: Outcome::Error,
            count: 0,
            duration: Duration::from_millis(300),
        });
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
        m.record(ScrapeEvent {
            tool: "scrape_url",
            domain: domain_of("not a url"),
            outcome: Outcome::Success,
            count: 1,
            duration: Duration::from_millis(50),
        });

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
                m.lock().expect("metrics mutex").record(ScrapeEvent {
                    tool: "scrape_url",
                    domain: "a.com".to_string(),
                    outcome: Outcome::Success,
                    count: 1,
                    duration: Duration::from_millis(1),
                });
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
    /// Captures via a SCOPED `with_default` subscriber (no global state).
    /// `#[serial]` keeps this test exclusive under `cargo test` (libtest runs
    /// tests on shared threads; nextest isolates per process, cargo test does
    /// not) so the capturing subscriber can never be shadowed by another test.
    #[test]
    #[serial]
    fn record_emits_structured_tracing_event() {
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
            ScrapeMetrics::default().record(ScrapeEvent {
                tool: "scrape_url",
                domain: "example.com".to_string(),
                outcome: Outcome::Success,
                count: 3,
                duration: Duration::from_millis(120),
            });
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
    }
}
