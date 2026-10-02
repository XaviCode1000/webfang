//! Scrape metrics — process-lifetime accumulator for MCP scraping activity.
//!
//! Pure types + logic only (DD-2): no DI, no locking, fully unit-testable.
//! Locking/tracing wiring lives on `McpState` (see `state.rs`). One structured
//! tracing event is emitted per recorded scrape (REQ-09) INSIDE
//! [`ScrapeMetrics::record`] (DD-3), keeping the critical section synchronous.
//!
//! Serialization is snapshot-safe (REQ-08): domains/tools use [`BTreeMap`] for
//! deterministic key order (DD-6) and the mean stays a single flat
//! `average_duration_ms` field (DD-5) that is trivially redactable.
//!
//! # SLO answerability (issue #1610 slice 4)
//!
//! `average_duration_ms` alone cannot answer a percentile question — a mean
//! destroys the distribution it summarizes, so p50/p95/p99 are not functions
//! of it. Two additive members close that gap, and both are *in-process
//! summaries* of a record the operator already owns in full: the structured
//! `scrape recorded` event emitted inside [`ScrapeMetrics::record`] is the
//! lossless record, the snapshot is the bounded index over it.
//!
//! * [`DurationReservoir`] (OBS-M4) keeps the last [`DURATION_SAMPLE_CAPACITY`]
//!   millisecond samples in a ring buffer and publishes p50/p95/p99 by
//!   nearest-rank.
//! * [`ToolDomainStats`] (OBS-M6) adds the tool x domain cross-product, which
//!   is what "which tool is slow for this domain" needs.
//!
//! The percentiles are reproducible from the trace file with one `jq` pass —
//! see [`DurationReservoir`] for the query and the exact nearest-rank rule.

use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// Outcome bucket for a scrape event (A1: success/error, NOT raw HTTP codes).
///
/// `#[non_exhaustive]` (issue #1614): this set is not complete by design — it
/// has grown once per new outcome concept (admission control, #1611, adds its
/// own dispositions), so it is a growing vocabulary. The attribute keeps the
/// next bucket an S1 / minor change instead of an S2 / major one, and it is
/// free here: the only exhaustive matches are in this crate (`ScrapeMetrics::
/// record`, `DefaultOverrides` derivation), and no test crate matches on it,
/// so `non_exhaustive` binds nobody. See
/// `docs/src/mcp-public-surface-policy.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Capacity of the process-lifetime duration reservoir (OBS-M4).
///
/// 1024 samples is a deliberate trade-off, not a default that fell out of a
/// type. One `u64` per sample is 8 KiB for the whole accumulator — negligible
/// beside the 500-domain map it sits next to — and it is large enough that a
/// busy process keeps minutes of history rather than seconds. A 24-hour run
/// will roll the window: past this many events the percentiles describe the
/// RECENT tail, which is the half an SLO cares about, and
/// [`DurationPercentiles::truncated`] plus the `observed` count say so
/// out loud rather than letting a reader assume process-lifetime coverage.
pub const DURATION_SAMPLE_CAPACITY: usize = 1024;

/// Capacity of EACH tool x domain duration reservoir (OBS-M6).
///
/// Smaller than the global reservoir on purpose: the cross-product multiplies
/// the number of reservoirs by the number of tracked cells, so a per-cell cap
/// equal to the global one would make the bound quadratic in practice. 32
/// samples still pins p50 and p95 for a pair (the ranks land inside it for any
/// pair with >= 4 events) and keeps a full cross-product in the low hundreds of
/// kilobytes.
pub const TOOL_DOMAIN_DURATION_CAPACITY: usize = 32;

/// Maximum number of tracked tool x domain cells (OBS-M6).
///
/// The per-domain map is bounded by [`MAX_TRACKED_DOMAINS`], an ACCEPTED
/// residual this slice does not re-open. The cross-product is not bounded by
/// that alone: 36 tools x 500 domains is 18 000 possible cells, ~26x the
/// per-domain map, so it needs its own stated bound. Past this many tracked
/// cells a new `(tool, domain)` pair folds into `(tool, [`OVERFLOW_DOMAIN_KEY`])`
/// — the same overflow convention, giving at most one overflow cell per tool.
pub const MAX_TRACKED_TOOL_DOMAIN_PAIRS: usize = 512;

/// Bounded, deterministic ring of duration samples in whole milliseconds
/// (OBS-M4).
///
/// # Why a ring of the last N, not a histogram
///
/// The alternative — fixed log-ish buckets — bounds memory just as well, but
/// it can only ever answer a percentile to within a bucket width, and the
/// bucket edges become part of the published contract. This accumulator exists
/// so an SLO claim ("p95 scrape latency is under 2 s") is a statement about
/// the durations that actually happened. A ring gives EXACT values over its
/// window, and the values it retains are byte-for-byte the integers the
/// `scrape recorded` tracing event already published, so the snapshot and the
/// trace file can never disagree.
///
/// # Determinism
///
/// No RNG (so it is not a statistical reservoir sample), no clock, no
/// `HashMap`: the same sequence of [`record`](Self::record) calls always
/// yields the same [`percentiles`](Self::percentiles). The eviction order is
/// arrival order, so the retained window is exactly the last `capacity`
/// samples. [`record`](Self::record) is `O(1)`, keeping the caller's critical
/// section short (REQ-07).
///
/// # Percentile definition
///
/// **Nearest-rank, 1-indexed, on the retained samples sorted ascending:**
/// `rank = ceil(p * n / 100)` clamped to `[1, n]`, and the answer is the
/// `rank`-th smallest sample. No interpolation, no averaging of neighbours.
///
/// # Reconstructing this from the trace file
///
/// The reservoir holds the same integer milliseconds the `scrape recorded`
/// event publishes as `duration_ms`, so the same numbers come back out of
/// `--trace-file`:
///
/// ```text
/// jq -s '[ .[] | select(.message? == "scrape recorded")
///            | .fields.duration_ms ] | sort
///      | . as $s | ($s|length) as $n
///      | def nr($p): if $n == 0 then null
///                    else $s[((((($p*$n)+99)/100)|floor) - 1)] end;
///        { samples: $n, p50_ms: nr(50), p95_ms: nr(95), p99_ms: nr(99) }'
///    out.jsonl
/// ```
///
/// The one difference is scope, and it is deliberate: this ring answers over
/// its last `capacity` events, while the `jq` pass answers over the whole file
/// — the trace file is the lossless record and this snapshot is the bounded
/// in-process index of it. Both are stated, neither is silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationReservoir {
    /// Ring storage, at most `capacity` entries once filled.
    samples: Vec<u64>,
    /// Next slot to overwrite (only meaningful once `samples` is full).
    next: usize,
    /// Every sample ever recorded, including evicted ones.
    observed: usize,
    /// Documented cap for this reservoir.
    capacity: usize,
}

impl DurationReservoir {
    /// Build an empty reservoir holding at most `capacity` samples.
    ///
    /// A zero capacity is clamped to one: "retain nothing" is not a mode a
    /// percentile reader can be given, it is a misconfiguration, and the
    /// alternative (an empty ring that silently reports `None` forever) hides
    /// it.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::new(),
            next: 0,
            observed: 0,
            capacity: capacity.max(1),
        }
    }

    /// Record one duration in whole milliseconds (OBS-M4).
    ///
    /// `O(1)`: below the cap it appends, at the cap it overwrites the oldest
    /// slot and advances. `next` is kept modded here rather than reset on wrap
    /// so a full ring never re-checks its length.
    pub fn record(&mut self, duration_ms: u64) {
        if self.samples.len() < self.capacity {
            self.samples.push(duration_ms);
        } else {
            self.samples[self.next] = duration_ms;
            self.next = (self.next + 1) % self.capacity;
        }
        self.observed += 1;
    }

    /// Number of samples currently retained (never above [`Self::capacity`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether no sample has been retained yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The documented cap for this reservoir.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Every sample ever recorded, including the evicted ones.
    #[must_use]
    pub fn observed(&self) -> usize {
        self.observed
    }

    /// The retained samples, sorted ascending (the percentile input).
    ///
    /// Allocates: this is the snapshot path, called at most once per
    /// `get_scrape_metrics` call, not on the [`record`](Self::record) path.
    #[must_use]
    pub fn sorted_samples(&self) -> Vec<u64> {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        sorted
    }

    /// The published p50 / p95 / p99 plus the metadata needed to read them
    /// honestly (OBS-M4).
    #[must_use]
    pub fn percentiles(&self) -> DurationPercentiles {
        let sorted = self.sorted_samples();
        let n = sorted.len();
        DurationPercentiles {
            p50_ms: Self::nearest_rank(&sorted, 50, n),
            p95_ms: Self::nearest_rank(&sorted, 95, n),
            p99_ms: Self::nearest_rank(&sorted, 99, n),
            samples: n,
            capacity: self.capacity,
            observed: self.observed,
            truncated: self.observed > n,
        }
    }

    /// Nearest-rank percentile: `rank = ceil(p * n / 100)`, 1-indexed, clamped
    /// to `[1, n]`, over `sorted` ascending. `None` when there is no sample —
    /// an empty reservoir reports no percentile rather than a fabricated `0`.
    fn nearest_rank(sorted: &[u64], p: usize, n: usize) -> Option<u64> {
        if n == 0 {
            return None;
        }
        // `div_ceil` on integers, not floats: the same arithmetic the jq
        // recipe in the type rustdoc performs, so the two cannot drift.
        let rank = (p * n).div_ceil(100).clamp(1, n);
        Some(sorted[rank - 1])
    }
}

/// Published percentiles for one [`DurationReservoir`] (OBS-M4).
///
/// `samples` / `capacity` / `observed` / `truncated` travel WITH the numbers
/// on purpose: a percentile without its sample count is how a reader ends up
/// trusting p99 computed from four events. `samples` is the denominator that
/// tells them, and `truncated` says whether the window is the whole lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DurationPercentiles {
    /// Median, or `None` when no sample was retained.
    pub p50_ms: Option<u64>,
    /// 95th percentile, or `None` when no sample was retained.
    pub p95_ms: Option<u64>,
    /// 99th percentile, or `None` when no sample was retained.
    pub p99_ms: Option<u64>,
    /// Samples currently retained in the reservoir (the percentile input).
    pub samples: usize,
    /// The reservoir's documented cap.
    pub capacity: usize,
    /// Samples ever recorded, including the evicted ones.
    pub observed: usize,
    /// `true` when older samples were evicted, so the percentiles describe
    /// the recent window rather than the whole process lifetime.
    pub truncated: bool,
}

impl DurationPercentiles {
    /// The no-sample projection: every rank `None`, nothing retained, nothing
    /// truncated. Used for a cross-product cell that somehow has counters but
    /// no reservoir, so a reader sees "no measurement" instead of `0 ms`.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            p50_ms: None,
            p95_ms: None,
            p99_ms: None,
            samples: 0,
            capacity: 0,
            observed: 0,
            truncated: false,
        }
    }
}

/// Accumulator-side counters for one (tool, domain) cell (OBS-M6).
///
/// Private: the published shape is [`ToolDomainStats`], which adds the
/// reservoir projection. Kept separate so the hot path mutates three integers
/// while the reservoir lives in its own ring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ToolDomainCounters {
    events: usize,
    pages: usize,
    errors: usize,
}

/// Per (tool, domain) aggregate in the cross-product (OBS-M6).
///
/// The counters are deliberately the same three the per-domain map publishes
/// (and deliberately NOT re-classified: separating Partial from Error is
/// OBS-M5 and out of this slice), so a reader can move between the two views
/// without a translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ToolDomainStats {
    /// Number of events recorded for this tool on this domain.
    pub events: usize,
    /// Total pages/results accumulated across this pair's events.
    pub pages: usize,
    /// Number of events bucketed as Error or Partial (see OBS-M5).
    pub errors: usize,
    /// Percentiles over this pair's own bounded reservoir. This is what turns
    /// "which tool is slow for this domain" from a guess into a lookup: the
    /// per-domain map can say `a.com` got slow, and only this says WHICH tool.
    pub duration_percentiles: DurationPercentiles,
}

impl ToolDomainStats {
    /// Join one cell's counters with the percentiles of its reservoir.
    #[must_use]
    fn new(counters: &ToolDomainCounters, duration_percentiles: DurationPercentiles) -> Self {
        Self {
            events: counters.events,
            pages: counters.pages,
            errors: counters.errors,
            duration_percentiles,
        }
    }
}

/// Process-lifetime scrape accumulator (A3: no reset).
///
/// NOT `Serialize` — [`ScrapeMetrics::snapshot`] is the serializable view.
#[derive(Debug, Clone)]
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
    /// OBS-M6: tool x domain cross-product, `BTreeMap` on BOTH axes so the
    /// published key order is total and never hash-dependent.
    tool_domains: BTreeMap<String, BTreeMap<String, ToolDomainCounters>>,
    /// OBS-M6: per-pair duration reservoirs, in the SAME key space as
    /// `tool_domains` by construction. They live apart from the counters
    /// because a reservoir is mutable ring state and the published
    /// [`ToolDomainStats`] is an immutable projection of it — merging the two
    /// would mean either recomputing percentiles on every record (O(n log n)
    /// on the hot path) or serializing the ring itself (a large, order-stable
    /// but useless blob). The snapshot joins them once, at read time.
    tool_domain_durations: BTreeMap<String, BTreeMap<String, DurationReservoir>>,
    /// Live count of occupied cross-product cells, so the cap is a count and
    /// not a per-insert scan of the whole tree.
    tool_domain_pairs: usize,
    duration_sum: Duration,
    /// OBS-M4: bounded ring of the last [`DURATION_SAMPLE_CAPACITY`]
    /// millisecond samples. The SUM above is kept untouched: `average_duration_ms`
    /// remains a whole-lifetime mean and must not silently become a
    /// windowed one.
    durations: DurationReservoir,
}

impl Default for ScrapeMetrics {
    fn default() -> Self {
        Self {
            total_events: 0,
            success_count: 0,
            error_count: 0,
            partial_count: 0,
            domains: BTreeMap::new(),
            tools: BTreeMap::new(),
            tool_domains: BTreeMap::new(),
            tool_domain_durations: BTreeMap::new(),
            tool_domain_pairs: 0,
            duration_sum: Duration::ZERO,
            durations: DurationReservoir::with_capacity(DURATION_SAMPLE_CAPACITY),
        }
    }
}

impl ScrapeMetrics {
    /// Record a single scrape event.
    ///
    /// Short synchronous critical section: emits the structured tracing event
    /// (REQ-09) then mutates the aggregates. No `.await` here, so the lock held
    /// by the caller is never held across an await point (REQ-07). Every step
    /// is `O(1)` amortized or a single `BTreeMap` probe, so the section stays
    /// short even as the accumulators fill.
    pub fn record(&mut self, event: ScrapeEvent) {
        // The integer the event publishes is the integer the reservoirs keep
        // (OBS-M4). Computed once so the JSONL and the snapshot cannot drift.
        let duration_ms = event.duration.as_millis() as u64;
        tracing::info!(
            tool = %event.tool,
            domain = %event.domain,
            success = event.outcome.is_success(),
            duration_ms,
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
        let domain_key = self.resolve_domain_key(&event.domain);
        {
            let domain = self.domains.entry(domain_key.clone()).or_default();
            domain.events += 1;
            domain.pages += event.count;
            if matches!(event.outcome, Outcome::Error | Outcome::Partial) {
                domain.errors += 1;
            }
        }

        *self.tools.entry(event.tool.to_string()).or_default() += 1;
        self.duration_sum += event.duration;
        self.durations.record(duration_ms);
        // OBS-M6: keyed on the ALREADY-resolved domain key, so the cross-product
        // can never name a domain the per-domain map is hiding.
        self.record_tool_domain(event.tool, &domain_key, &event, duration_ms);
    }

    /// Resolve the per-domain key for `domain` (REQ-05).
    ///
    /// Returns an owned `String` rather than a borrow: the caller needs to
    /// insert into `self.domains` next, which a borrow of `self.domains`
    /// would forbid. Costs one short-string clone per event, which the
    /// per-domain `entry` allocation already pays.
    fn resolve_domain_key(&self, domain: &str) -> String {
        if self.domains.contains_key(domain) || self.domains.len() < MAX_TRACKED_DOMAINS {
            domain.to_string()
        } else {
            OVERFLOW_DOMAIN_KEY.to_string()
        }
    }

    /// Fold one event into the tool x domain cross-product (OBS-M6).
    ///
    /// Split from [`record`](Self::record) to keep both readable: `record` is
    /// the hot path and this is where the cross-product's two caps live.
    fn record_tool_domain(
        &mut self,
        tool: &str,
        domain_key: &str,
        event: &ScrapeEvent,
        duration_ms: u64,
    ) {
        let key = self.resolve_tool_domain_key(tool, domain_key);
        // Count the cell BEFORE the mutable borrows, so `tool_domain_pairs`
        // and the maps are not aliased.
        let is_new_cell = !self
            .tool_domains
            .get(tool)
            .is_some_and(|per_tool| per_tool.contains_key(&key));
        if is_new_cell {
            self.tool_domain_pairs += 1;
        }
        let counters = self
            .tool_domains
            .entry(tool.to_string())
            .or_default()
            .entry(key.clone())
            .or_default();
        counters.events += 1;
        counters.pages += event.count;
        // Same rule as the per-domain map (Partial counts as an error here);
        // splitting the two is OBS-M5 and deliberately NOT done in this slice.
        if matches!(event.outcome, Outcome::Error | Outcome::Partial) {
            counters.errors += 1;
        }
        self.tool_domain_durations
            .entry(tool.to_string())
            .or_default()
            .entry(key)
            .or_insert_with(|| DurationReservoir::with_capacity(TOOL_DOMAIN_DURATION_CAPACITY))
            .record(duration_ms);
    }

    /// Resolve the cross-product's domain key for `(tool, domain_key)`
    /// (OBS-M6).
    ///
    /// Two independent reasons to fold into [`OVERFLOW_DOMAIN_KEY`], checked
    /// in the same precedence the per-domain map uses (an already-tracked cell
    /// always wins, so re-aggregation stays exact):
    ///
    /// 1. the shared per-domain cap, applied upstream by
    ///    [`resolve_domain_key`](Self::resolve_domain_key); and
    /// 2. this slice's own [`MAX_TRACKED_TOOL_DOMAIN_PAIRS`] cap, which the
    ///    per-domain map has no reason to carry because it is not multiplied
    ///    by the tool count.
    fn resolve_tool_domain_key(&self, tool: &str, domain_key: &str) -> String {
        let already_tracked = self
            .tool_domains
            .get(tool)
            .is_some_and(|per_tool| per_tool.contains_key(domain_key));
        if already_tracked || self.tool_domain_pairs < MAX_TRACKED_TOOL_DOMAIN_PAIRS {
            domain_key.to_string()
        } else {
            OVERFLOW_DOMAIN_KEY.to_string()
        }
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
            duration_percentiles: self.durations.percentiles(),
            tool_domains: self.project_tool_domains(),
        }
    }

    /// Join the cross-product's counters with their reservoirs into the
    /// published view (OBS-M6).
    ///
    /// Walks the counter map, not the reservoir map, so a cell is emitted
    /// exactly when it exists — the two are written together in
    /// [`record_tool_domain`](Self::record_tool_domain) and a missing
    /// reservoir would be a bug, but a missing cell would be a lie in the
    /// snapshot. A cell with no reservoir yields empty percentiles (`None`
    /// ranks, `samples: 0`), which is the honest reading rather than a
    /// fabricated `0 ms`.
    fn project_tool_domains(&self) -> BTreeMap<String, BTreeMap<String, ToolDomainStats>> {
        self.tool_domains
            .iter()
            .map(|(tool, per_tool)| {
                let projected = per_tool
                    .iter()
                    .map(|(domain, counters)| {
                        let percentiles = self
                            .tool_domain_durations
                            .get(tool)
                            .and_then(|per_tool| per_tool.get(domain))
                            .map_or_else(
                                DurationPercentiles::empty,
                                DurationReservoir::percentiles,
                            );
                        (domain.clone(), ToolDomainStats::new(counters, percentiles))
                    })
                    .collect();
                (tool.clone(), projected)
            })
            .collect()
    }
}

/// Serializable point-in-time view of the accumulator (REQ-08).
///
/// The mean stays one flat `average_duration_ms` field (DD-5) so snapshots
/// stay deterministic and trivially redactable. The two OBS-M4/OBS-M6 members
/// are appended AFTER it: they are purely additive, so every field that
/// existed before keeps both its exact bytes and its exact position
/// (`pre_existing_snapshot_fields_are_byte_identical` pins that).
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
    /// Mean wall-clock duration in milliseconds across ALL events — a
    /// whole-lifetime mean, deliberately NOT windowed by
    /// [`duration_percentiles`](Self::duration_percentiles) even after that
    /// ring rolls over. A reader comparing the two is looking at "mean over
    /// the process" versus "median over the recent tail", which is a
    /// difference of scope, not a bug.
    pub average_duration_ms: f64,
    /// OBS-M4: p50/p95/p99 plus the sample-count metadata that says whether
    /// they describe the whole lifetime or the recent window. `None` when no
    /// scrape has been recorded.
    pub duration_percentiles: DurationPercentiles,
    /// OBS-M6: tool x domain cross-product, sorted on both axes. The domain
    /// axis is drawn from the SAME key space as [`domains`](Self::domains) —
    /// a domain beyond [`MAX_TRACKED_DOMAINS`] appears here only under
    /// [`OVERFLOW_DOMAIN_KEY`], never as a key of its own. The pair count is
    /// separately bounded by [`MAX_TRACKED_TOOL_DOMAIN_PAIRS`], past which new
    /// pairs fold into `(tool, "otros")`.
    pub tool_domains: BTreeMap<String, BTreeMap<String, ToolDomainStats>>,
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
            "  \"average_duration_ms\": 200.0,",
            "  \"duration_percentiles\": {",
            "    \"p50_ms\": 200,",
            "    \"p95_ms\": 300,",
            "    \"p99_ms\": 300,",
            "    \"samples\": 3,",
            "    \"capacity\": 1024,",
            "    \"observed\": 3,",
            "    \"truncated\": false",
            "  },",
            "  \"tool_domains\": {",
            "    \"scrape_url\": {",
            "      \"a.com\": {",
            "        \"events\": 2,",
            "        \"pages\": 8,",
            "        \"errors\": 0,",
            "        \"duration_percentiles\": {",
            "          \"p50_ms\": 100,",
            "          \"p95_ms\": 200,",
            "          \"p99_ms\": 200,",
            "          \"samples\": 2,",
            "          \"capacity\": 32,",
            "          \"observed\": 2,",
            "          \"truncated\": false",
            "        }",
            "      },",
            "      \"b.com\": {",
            "        \"events\": 1,",
            "        \"pages\": 0,",
            "        \"errors\": 1,",
            "        \"duration_percentiles\": {",
            "          \"p50_ms\": 300,",
            "          \"p95_ms\": 300,",
            "          \"p99_ms\": 300,",
            "          \"samples\": 1,",
            "          \"capacity\": 32,",
            "          \"observed\": 1,",
            "          \"truncated\": false",
            "        }",
            "      }",
            "    }",
            "  }",
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

    // ---------------------------------------------------------------------
    // OBS-M4 — duration percentiles (bounded, deterministic, reconstructible)
    // ---------------------------------------------------------------------

    /// OBS-M4: recording the known distribution `1..=100` ms makes the
    /// percentiles *reconstructible*, which is the whole point — a mean
    /// (`average_duration_ms == 50.5`) cannot answer any percentile question.
    ///
    /// Nearest-rank, 1-indexed: `rank = ceil(p * n / 100)`, clamped to
    /// `[1, n]`. For `n = 100` the ranks are 50 / 95 / 99, so the answers are
    /// the 50th / 95th / 99th smallest sample — exact, zero tolerance.
    #[test]
    fn percentiles_reconstruct_a_known_distribution() {
        let mut m = ScrapeMetrics::default();
        for ms in 1..=100u64 {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(ms),
            ));
        }

        let snap = m.snapshot();
        assert_eq!(
            snap.average_duration_ms, 50.5,
            "the mean is what OBS-M4 says cannot answer SLO questions"
        );
        assert_eq!(snap.duration_percentiles.p50_ms, Some(50), "p50");
        assert_eq!(snap.duration_percentiles.p95_ms, Some(95), "p95");
        assert_eq!(snap.duration_percentiles.p99_ms, Some(99), "p99");
        assert_eq!(snap.duration_percentiles.samples, 100, "all samples kept");
        assert_eq!(snap.duration_percentiles.observed, 100, "observed total");
        assert_eq!(snap.duration_percentiles.capacity, 1024, "documented cap");
        assert!(
            !snap.duration_percentiles.truncated,
            "100 samples are far below the cap, nothing is evicted"
        );
    }

    /// OBS-M4: the reservoir BOUNDS memory. Recording ten times the cap must
    /// leave exactly `capacity` samples retained, `observed` counting every
    /// event, and `truncated` telling the reader that the percentiles
    /// describe the RECENT window rather than the whole process lifetime.
    #[test]
    fn duration_reservoir_is_bounded() {
        let mut m = ScrapeMetrics::default();
        // Cap-worth of ascending samples, then far more of a constant value
        // so the retained window's contents are unambiguous.
        for ms in 0..DURATION_SAMPLE_CAPACITY as u64 {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(ms),
            ));
        }
        for _ in 0..(DURATION_SAMPLE_CAPACITY * 10) {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(999),
            ));
        }

        let snap = m.snapshot();
        let p = snap.duration_percentiles;
        assert_eq!(p.samples, DURATION_SAMPLE_CAPACITY, "retained is capped");
        assert_eq!(
            p.observed,
            DURATION_SAMPLE_CAPACITY * 11,
            "observed counts every recorded event, not just the retained ones"
        );
        assert!(p.truncated, "an evicted sample must be declared");
        assert_eq!(
            p.p50_ms,
            Some(999),
            "the retained window is the LAST cap samples (all 999 ms)"
        );
        assert_eq!(p.p95_ms, Some(999), "p95 of the retained window");
        assert_eq!(p.p99_ms, Some(999), "p99 of the retained window");
    }

    /// OBS-M4: the ring keeps the MOST RECENT `capacity` samples, in order,
    /// and the same recorded sequence always yields the same snapshot — no
    /// wall clock, no RNG, no map-iteration order anywhere in the path.
    #[test]
    fn duration_reservoir_is_a_deterministic_last_n_ring() {
        let reservoir = || {
            let mut m = ScrapeMetrics::default();
            for ms in 1..=10u64 {
                m.record(ScrapeEvent::new(
                    "scrape_url",
                    "a.com".to_string(),
                    Outcome::Success,
                    1,
                    Duration::from_millis(ms),
                ));
            }
            m.snapshot().duration_percentiles
        };
        assert_eq!(
            reservoir(),
            reservoir(),
            "the same event sequence must yield a byte-identical percentile set"
        );

        // Ring eviction: overwrite a 4-slot reservoir with 1..=6 and assert the
        // retained window is 3,4,5,6 — the last four, not the first four.
        let mut r = DurationReservoir::with_capacity(4);
        for ms in 1..=6u64 {
            r.record(ms);
        }
        let p = r.percentiles();
        assert_eq!(p.samples, 4, "ring never exceeds its capacity");
        assert_eq!(p.observed, 6, "observed is the lifetime total");
        assert!(p.truncated, "two samples were evicted");
        assert_eq!(p.p50_ms, Some(4), "median of {{3,4,5,6}} is the 2nd = 4");
        assert_eq!(p.p95_ms, Some(6), "p95 of {{3,4,5,6}} is the 4th = 6");
        assert_eq!(p.p99_ms, Some(6), "p99 of {{3,4,5,6}} is the 4th = 6");
    }

    /// OBS-M4: an empty accumulator reports `None` percentiles, never a
    /// fabricated `0.0` — "no samples" and "every sample was instantaneous"
    /// must not serialize the same way.
    #[test]
    fn percentiles_are_none_without_samples() {
        let snap = ScrapeMetrics::default().snapshot();
        assert_eq!(snap.duration_percentiles.p50_ms, None, "p50");
        assert_eq!(snap.duration_percentiles.p95_ms, None, "p95");
        assert_eq!(snap.duration_percentiles.p99_ms, None, "p99");
        assert_eq!(snap.duration_percentiles.samples, 0, "samples");
        assert!(!snap.duration_percentiles.truncated, "nothing evicted");
    }

    /// OBS-M4: the reconstruction bridge. The sample the reservoir retains is
    /// the SAME integer millisecond value the `scrape recorded` tracing event
    /// publishes, so a `jq` pass over the `--trace-file` JSONL can recompute
    /// these numbers from the raw events. This test pins that equality: the
    /// multiset of captured `duration_ms` fields, sorted and ranked with the
    /// documented nearest-rank rule, must equal the snapshot's percentiles.
    #[test]
    #[serial]
    fn retained_samples_equal_the_published_duration_ms_events() {
        ensure_global_subscriber();
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::Layer;
        use tracing_subscriber::prelude::*;

        struct Capture(Arc<Mutex<Vec<String>>>);
        impl Visit for Capture {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "duration_ms" {
                    self.0
                        .lock()
                        .expect("capture mutex")
                        .push(format!("{value:?}"));
                }
            }
            fn record_u64(&mut self, field: &Field, value: u64) {
                if field.name() == "duration_ms" {
                    self.0
                        .lock()
                        .expect("capture mutex")
                        .push(value.to_string());
                }
            }
        }
        struct CaptureLayer {
            events: Arc<Mutex<Vec<String>>>,
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
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            events: Arc::clone(&events),
        });

        let mut m = ScrapeMetrics::default();
        tracing::subscriber::with_default(subscriber, || {
            for ms in 1..=100u64 {
                m.record(ScrapeEvent::new(
                    "scrape_url",
                    "a.com".to_string(),
                    Outcome::Success,
                    1,
                    Duration::from_millis(ms),
                ));
            }
        });

        // The jq side: `[.[] | select(.message? == "scrape recorded")
        //              | .fields.duration_ms] | sort`.
        // `message` is TOP-LEVEL in the JSONL, not inside `fields` —
        // FileTraceLayer writes `record["message"]` beside `record["fields"]`.
        // Selecting `.fields.message` yields an empty set and silently
        // reports `samples: 0`, which looks like "no scrapes" rather than
        // like a broken query.
        let mut from_jsonl: Vec<u64> = events
            .lock()
            .expect("capture mutex")
            .iter()
            .filter_map(|v| v.parse::<u64>().ok())
            .collect();
        from_jsonl.sort_unstable();
        assert_eq!(from_jsonl.len(), 100, "one event per recorded scrape");
        // Nearest-rank, exactly as the snapshot computes it.
        let near = |p: usize| from_jsonl[(p * from_jsonl.len()).div_ceil(100) - 1];
        assert_eq!(near(50), 50, "jq-side p50");
        assert_eq!(near(95), 95, "jq-side p95");
        assert_eq!(near(99), 99, "jq-side p99");

        let snap = m.snapshot();
        assert_eq!(
            snap.duration_percentiles.p50_ms,
            Some(near(50)),
            "p50 agrees"
        );
        assert_eq!(
            snap.duration_percentiles.p95_ms,
            Some(near(95)),
            "p95 agrees"
        );
        assert_eq!(
            snap.duration_percentiles.p99_ms,
            Some(near(99)),
            "p99 agrees"
        );
    }

    // ---------------------------------------------------------------------
    // OBS-M6 — tool x domain cross-product
    // ---------------------------------------------------------------------

    /// OBS-M6: the cross-product is EXACT for a fixture small enough that no
    /// cap can hide a mistake, which is what makes it answerable as
    /// "which tool is slow for this domain".
    #[test]
    fn tool_domain_cross_product_is_exact() {
        let mut m = ScrapeMetrics::default();
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            2,
            Duration::from_millis(10),
        ));
        m.record(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Error,
            0,
            Duration::from_millis(30),
        ));
        m.record(ScrapeEvent::new(
            "crawl_site",
            "a.com".to_string(),
            Outcome::Success,
            7,
            Duration::from_millis(100),
        ));
        m.record(ScrapeEvent::new(
            "scrape_url",
            "b.com".to_string(),
            Outcome::Success,
            1,
            Duration::from_millis(50),
        ));

        let snap = m.snapshot();
        assert_eq!(
            snap.tool_domains.keys().collect::<Vec<_>>(),
            vec!["crawl_site", "scrape_url"],
            "outer keys are tools, sorted"
        );
        let scrape_a = &snap.tool_domains["scrape_url"]["a.com"];
        assert_eq!(scrape_a.events, 2, "two scrape_url events on a.com");
        assert_eq!(scrape_a.pages, 2, "pages accumulate per pair");
        assert_eq!(scrape_a.errors, 1, "the Error event counts as an error");
        assert_eq!(
            scrape_a.duration_percentiles.p50_ms,
            Some(10),
            "median of {{10,30}} is the 1st nearest-rank sample"
        );
        assert_eq!(
            scrape_a.duration_percentiles.p95_ms,
            Some(30),
            "p95 of {{10,30}} is the 2nd"
        );
        assert_eq!(snap.tool_domains["crawl_site"]["a.com"].pages, 7, "crawl");
        assert_eq!(snap.tool_domains["scrape_url"]["b.com"].events, 1, "b.com");

        // Cross-check: the pairs re-sum to the flat totals already published.
        let events: usize = snap
            .tool_domains
            .values()
            .flat_map(|per_tool| per_tool.values())
            .map(|cell| cell.events)
            .sum();
        assert_eq!(events, snap.total_events, "pairs partition the events");
        let per_tool_pairs = snap.tool_domains["scrape_url"].len();
        assert_eq!(per_tool_pairs, 2, "scrape_url hit two domains");
    }

    /// OBS-M6: the cross-product NEVER invents a domain key the per-domain map
    /// does not already show. Both axes resolve through the same
    /// `MAX_TRACKED_DOMAINS` rule, so a domain past the cap is visible in
    /// `domains` only as `"otros"` and here only as `(tool, "otros")` — a
    /// reader who cannot see a domain in one map can never see it in the other.
    #[test]
    fn tool_domains_respect_the_per_domain_cap() {
        let mut m = ScrapeMetrics::default();
        for i in 0..(MAX_TRACKED_DOMAINS + 10) {
            m.record(ScrapeEvent::new(
                "scrape_url",
                format!("d{i:04}.com"),
                Outcome::Success,
                1,
                Duration::from_millis(1),
            ));
        }

        let snap = m.snapshot();
        assert!(
            !snap.tool_domains["scrape_url"].contains_key("d0500.com"),
            "a domain past the per-domain cap must not get its own cross-product cell"
        );
        assert_eq!(
            snap.tool_domains["scrape_url"][OVERFLOW_DOMAIN_KEY].events, 10,
            "the overflow domain reuses the documented bucket key"
        );
        assert_eq!(
            snap.tool_domains["scrape_url"][OVERFLOW_DOMAIN_KEY].events,
            snap.domains[OVERFLOW_DOMAIN_KEY].events,
            "overflow events are the same events in both maps"
        );
    }

    /// OBS-M6: the cross-product's OWN bound, and the fact that it is a
    /// SECOND, INDEPENDENT cap. Writing this test is what surfaced why: with
    /// one tool the per-domain cap (500) binds first and the pair cap is
    /// unreachable, so a single-tool fixture "proves" nothing about the
    /// cross-product's own bound. Two tools over 300 shared domains produce
    /// 600 pairs — under the 500-domain cap, over the 512-pair cap — which is
    /// the only shape where the two caps are distinguishable at all.
    #[test]
    fn tool_domain_pairs_are_capped_with_otros_overflow() {
        const DOMAINS: usize = 300;
        let mut m = ScrapeMetrics::default();
        for i in 0..DOMAINS {
            m.record(ScrapeEvent::new(
                "scrape_url",
                format!("p{i:05}.com"),
                Outcome::Success,
                1,
                Duration::from_millis(1),
            ));
        }
        for i in 0..DOMAINS {
            m.record(ScrapeEvent::new(
                "crawl_site",
                format!("p{i:05}.com"),
                Outcome::Success,
                1,
                Duration::from_millis(1),
            ));
        }

        let snap = m.snapshot();
        assert_eq!(
            snap.domains.len(),
            DOMAINS,
            "the per-domain cap is NOT reached: every domain keeps its own key"
        );
        assert!(
            !snap.tool_domains["scrape_url"].contains_key(OVERFLOW_DOMAIN_KEY),
            "the tool that filled the tracked cells needs no overflow bucket"
        );

        let crawl = &snap.tool_domains["crawl_site"];
        assert!(
            crawl.contains_key(OVERFLOW_DOMAIN_KEY),
            "the pair past the cap folds into the documented bucket"
        );
        let total_cells: usize = snap.tool_domains.values().map(BTreeMap::len).sum();
        assert_eq!(
            total_cells,
            MAX_TRACKED_TOOL_DOMAIN_PAIRS + 1,
            "the tracked cells plus exactly one overflow bucket"
        );
        assert_eq!(
            crawl[OVERFLOW_DOMAIN_KEY].events,
            DOMAINS * 2 - MAX_TRACKED_TOOL_DOMAIN_PAIRS,
            "600 pairs - 512 tracked = 88 events in the overflow bucket"
        );
        assert_eq!(
            snap.tool_domains
                .values()
                .flat_map(|per_tool| per_tool.values())
                .map(|cell| cell.events)
                .sum::<usize>(),
            snap.total_events,
            "no event is lost by the cap"
        );
    }

    /// OBS-M6 + the accepted `MAX_TRACKED_DOMAINS` residual: a per-tool
    /// reservoir is capped too, so the cross-product cannot grow the process
    /// footprint with `pairs * capacity` samples without bound.
    #[test]
    fn tool_domain_reservoirs_are_capped_independently() {
        let mut m = ScrapeMetrics::default();
        for ms in 0..(TOOL_DOMAIN_DURATION_CAPACITY * 5) as u64 {
            m.record(ScrapeEvent::new(
                "scrape_url",
                "a.com".to_string(),
                Outcome::Success,
                1,
                Duration::from_millis(ms),
            ));
        }
        let cell = &m.snapshot().tool_domains["scrape_url"]["a.com"];
        assert_eq!(cell.events, TOOL_DOMAIN_DURATION_CAPACITY * 5, "counters");
        assert_eq!(
            cell.duration_percentiles.samples, TOOL_DOMAIN_DURATION_CAPACITY,
            "per-pair reservoir keeps its own smaller cap"
        );
        assert_eq!(
            cell.duration_percentiles.observed,
            TOOL_DOMAIN_DURATION_CAPACITY * 5
        );
        assert!(cell.duration_percentiles.truncated, "eviction is declared");
    }

    /// Additive-stability guard: the fields that existed before OBS-M4/OBS-M6
    /// must keep their exact bytes AND their exact position, so a consumer
    /// that indexes positionally is not broken by two new members.
    #[test]
    fn pre_existing_snapshot_fields_are_byte_identical() {
        let mut m = ScrapeMetrics::default();
        record_fixture(&mut m);
        let json = serde_json::to_string_pretty(&m.snapshot()).expect("must serialize");
        let legacy_prefix = [
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
            "  \"average_duration_ms\": 200.0,",
        ]
        .join("\n");
        assert!(
            json.starts_with(&legacy_prefix),
            "OBS-M4/OBS-M6 must be purely additive; the legacy prefix drifted:\n{json}"
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
