//! Collects the pacing-wait events the batch path publishes (#1962).
//!
//! The batch-pacing tests used to assert a WALL-CLOCK budget: "4 mock
//! scrapes must finish in under 150 ms". That instrument measures the
//! runner, not the behaviour — it failed 3/3 on a loaded Windows runner
//! with a 4% overshoot (#1962). The property under test ("pacing disabled
//! ⇒ no delay") is already published as an EVENT: every pacing wait emits
//! `http.rate_limit.wait` (#1610). This module collects those events so the
//! tests can assert the FACT ("this batch waited, this batch did not")
//! instead of a latency budget.
//!
//! # Scoping
//!
//! The collector is installed as the THREAD-LOCAL default subscriber for the
//! duration of the batch only (see [`with_pacing_collector`]):
//!
//! - it never becomes the process-wide default, so it cannot observe
//!   another test's events and cannot leak into the rest of the binary;
//! - it runs its own current-thread runtime, so every event the batch
//!   publishes is emitted on the thread that owns the collector.
//!
//! # Vacuous-pass guard
//!
//! "Zero events observed" is only meaningful if the collector can observe
//! events at all. [`PacingWaitCollector::emit_control`] publishes one
//! pacing event through the PRODUCTION emitter, so a test can prove the
//! collector was armed before trusting a zero.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

use webfang_core::application::rate_limiter::{
    record_pacing_wait, PacingContext, RATE_LIMIT_WAIT_OPERATION,
};

/// `scope` of the pacing waits published by the batch path
/// (`scrape_multiple_inner`).
///
/// The production side passes this scope as a literal at the call site; it
/// has no exported constant yet, so the test mirrors it here.
pub const BATCH_SCOPE: &str = "batch_scrape";

/// `scope` of the control event published by [`PacingWaitCollector::emit_control`].
pub const CONTROL_SCOPE: &str = "test_control";

/// One observed pacing wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacingWait {
    /// Which pacing site waited (`"batch_scrape"`, `"cli_scrape"`, …).
    pub scope: String,
    /// URL the wait delayed, when the call site named one.
    pub url: Option<String>,
    /// `"granted"` or `"cancelled"`.
    pub outcome: String,
    /// Measured cost of the wait, in milliseconds.
    pub waited_ms: u64,
}

/// Collector of the pacing waits observed under its subscriber.
#[derive(Debug, Clone, Default)]
pub struct PacingWaitCollector {
    waits: Arc<Mutex<Vec<PacingWait>>>,
}

impl PacingWaitCollector {
    /// A collector with nothing observed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every pacing wait observed so far, in observation order.
    pub fn waits(&self) -> Vec<PacingWait> {
        self.waits
            .lock()
            .expect("pacing collector mutex poisoned")
            .clone()
    }

    /// The pacing waits observed for one scope.
    pub fn waits_in_scope(&self, scope: &str) -> Vec<PacingWait> {
        self.waits()
            .into_iter()
            .filter(|wait| wait.scope == scope)
            .collect()
    }

    /// Publish one control pacing event through the PRODUCTION emitter.
    ///
    /// The event is indistinguishable from a real wait except for its
    /// scope, so a test can prove the collector is armed — a collector that
    /// silently collected nothing would make "zero waits observed" a
    /// vacuous truth.
    pub fn emit_control(&self) {
        record_pacing_wait(
            &PacingContext::bare(CONTROL_SCOPE),
            Duration::ZERO,
            "granted",
        );
    }

    /// The tracing layer feeding this collector.
    fn layer(&self) -> PacingWaitLayer {
        PacingWaitLayer {
            waits: Arc::clone(&self.waits),
        }
    }
}

/// The tracing layer behind [`PacingWaitCollector`].
#[derive(Debug, Clone)]
struct PacingWaitLayer {
    waits: Arc<Mutex<Vec<PacingWait>>>,
}

impl<S> Layer<S> for PacingWaitLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        // The pacing wait is a DEBUG event (#1610): a subscriber that does
        // not enable DEBUG collects nothing and turns every assertion built
        // on this collector into a test of nothing.
        metadata.level() <= &tracing::Level::DEBUG
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = PacingWaitVisitor::default();
        event.record(&mut visitor);
        let Some(wait) = visitor.finish() else {
            return;
        };
        self.waits
            .lock()
            .expect("pacing collector mutex poisoned")
            .push(wait);
    }
}

/// Visitor that keeps only the fields of a pacing-wait event.
#[derive(Debug, Default)]
struct PacingWaitVisitor {
    operation: Option<String>,
    scope: Option<String>,
    url: Option<String>,
    outcome: Option<String>,
    waited_ms: Option<u64>,
}

impl Visit for PacingWaitVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "operation" => self.operation = Some(value.to_owned()),
            "scope" => self.scope = Some(value.to_owned()),
            "url" => self.url = Some(value.to_owned()),
            "outcome" => self.outcome = Some(value.to_owned()),
            _ => {},
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "waited_ms" {
            self.waited_ms = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {
        // The identity fields (`correlation_id`, `trace_id`) are recorded
        // through `Display`; the batch path publishes no identity, so there
        // is nothing to keep here.
    }
}

impl PacingWaitVisitor {
    /// The pacing wait this event describes, or `None` for any other event.
    fn finish(self) -> Option<PacingWait> {
        if self.operation.as_deref() != Some(RATE_LIMIT_WAIT_OPERATION) {
            return None;
        }
        Some(PacingWait {
            scope: self.scope.unwrap_or_default(),
            url: self.url,
            outcome: self.outcome.unwrap_or_default(),
            waited_ms: self.waited_ms.unwrap_or_default(),
        })
    }
}

/// Run `body` with `collector` installed as the thread-local subscriber.
///
/// The subscriber is scoped to this thread and to this call: it is never
/// registered process-wide, so it cannot observe another test's events and
/// cannot leak into the rest of the test binary. The body runs on its own
/// current-thread runtime, so every event it publishes is emitted on the
/// thread that owns the collector.
pub fn with_pacing_collector<F, Fut, T>(collector: &PacingWaitCollector, body: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.layer()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime for the pacing collector");
    tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()))
}

/// Assert the batch published exactly one granted pacing wait per URL.
///
/// The COUNT is the fact under test ("this batch paced every URL"); the URL
/// SET pins that no wait was attributed to the wrong target and that no URL
/// was paced twice.
pub fn assert_one_granted_wait_per_url(waits: &[PacingWait], expected_urls: &[&str]) {
    assert_eq!(
        waits.len(),
        expected_urls.len(),
        "a paced batch publishes one wait per URL, got: {waits:?}"
    );

    let mut paced: BTreeSet<&str> = BTreeSet::new();
    for wait in waits {
        assert_eq!(
            wait.outcome, "granted",
            "a batch wait must be granted, not cancelled: {wait:?}"
        );
        let url = wait
            .url
            .as_deref()
            .expect("a batch wait must name the URL it delayed");
        assert!(paced.insert(url), "URL paced more than once: {url}");
    }

    let expected: BTreeSet<&str> = expected_urls.iter().copied().collect();
    assert_eq!(
        paced, expected,
        "every batch URL must be paced exactly once"
    );
}
