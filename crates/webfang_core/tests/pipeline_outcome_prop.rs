//! Property tests for the evolved [`StageOutcome`] contract (Sprint 9).
//!
//! Algebraic properties of the pipeline executor over the outcome space:
//!
//! 1. **Composition** — N `Continue` stages apply their transforms in order;
//!    the final item equals sequential function composition.
//! 2. **Short-circuit** — the first non-`Continue` outcome at any position is
//!    returned verbatim and NO later stage runs (observable via call counters).
//! 3. **Failure is first-class** — `Failed { class }` obeys the same
//!    short-circuit algebra as `Rejected`, carrying its `ErrorClass` intact.
//!
//! Run with: `cargo nextest run --test pipeline_outcome_prop`

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use proptest::prelude::*;

use webfang_core::application::pipeline::{PipelineExecutor, PipelineStage};
use webfang_core::domain::error::ErrorClass;
use webfang_core::domain::pipeline_item::{FilterReason, RejectReason, ScrapedItem, StageOutcome};

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Stage that appends `tag` to `metadata["trace"]` and counts invocations.
struct TagStage {
    tag: &'static str,
    calls: Arc<AtomicUsize>,
}

impl PipelineStage for TagStage {
    fn name(&self) -> &str {
        self.tag
    }

    fn process(
        &self,
        mut item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let trace = item.metadata.entry("trace".into()).or_default();
            trace.push_str(self.tag);
            StageOutcome::Continue(item)
        })
    }
}

/// Stage that returns a fixed non-Continue outcome after counting the call.
struct FixedOutcomeStage {
    outcome: StageOutcome,
    calls: Arc<AtomicUsize>,
}

impl PipelineStage for FixedOutcomeStage {
    fn name(&self) -> &str {
        "fixed"
    }

    fn process(
        &self,
        _item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        // Only ever armed with a non-Continue outcome (see Property 2).
        let outcome = self.outcome.clone();
        let calls = Arc::clone(&self.calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            outcome
        })
    }
}

/// Static single-letter tags avoid leaking strings per proptest case.
const LETTERS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];

fn item_for(url: &str) -> ScrapedItem {
    ScrapedItem {
        url: url.into(),
        ..Default::default()
    }
}

// ── Property 1: composition in order ───────────────────────────────────────

proptest! {
    #[test]
    fn continue_stages_compose_in_order(tags in proptest::collection::vec(0u8..26, 0..=8)) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut executor = PipelineExecutor::new();
            let mut letters = Vec::new();
            for &t in &tags {
                let letter = LETTERS[usize::from(t)];
                letters.push(letter);
                executor.add_stage(Box::new(TagStage { tag: letter, calls: Arc::default() }));
            }
            let expected: String = letters.concat();
            let result = executor.execute(item_for("https://example.com")).await;
            prop_assert!(
                matches!(&result, StageOutcome::Continue(_)),
                "all-Continue pipeline must yield Continue, got {result:?}"
            );
            if let StageOutcome::Continue(item) = result {
                let trace = item.metadata.get("trace").map(String::as_str).unwrap_or("");
                prop_assert_eq!(trace, expected);
            }
            Ok(())
        })?;
    }

// ── Property 2: first non-Continue short-circuits ─────────────────────────

    #[test]
    fn first_non_continue_outcome_short_circuits(
        stages_before in 0usize..4,
        stages_after in 0usize..4,
        outcome_index in 0u8..3,
        reason_pick in 0u8..2,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            // The fixed outcome injected mid-pipeline.
            let outcome = match (outcome_index, reason_pick) {
                (0, _) => StageOutcome::Filtered { reason: FilterReason::NonContentPath },
                (1, 0) => StageOutcome::Rejected { reason: RejectReason::EmptyUrl },
                (1, _) => StageOutcome::Rejected { reason: RejectReason::HttpStatus { code: 503 } },
                (_, _) => StageOutcome::Failed { class: ErrorClass::TransientBackoff },
            };
            let expected = outcome.clone();

            let mut executor = PipelineExecutor::new();
            for _ in 0..stages_before {
                executor.add_stage(Box::new(TagStage { tag: "b", calls: Arc::default() }));
            }
            let stop_calls = Arc::new(AtomicUsize::new(0));
            executor.add_stage(Box::new(FixedOutcomeStage {
                outcome: expected.clone(),
                calls: Arc::clone(&stop_calls),
            }));
            let after_calls: Vec<Arc<AtomicUsize>> =
                (0..stages_after).map(|_| Arc::new(AtomicUsize::new(0))).collect();
            for c in &after_calls {
                executor.add_stage(Box::new(TagStage { tag: "z", calls: Arc::clone(c) }));
            }

            let result = executor.execute(item_for("https://example.com")).await;
            prop_assert_eq!(result, expected);
            prop_assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
            for c in &after_calls {
                prop_assert_eq!(c.load(Ordering::SeqCst), 0);
            }
            Ok(())
        })?;
    }

// ── Property 3: reasons render non-empty diagnostics ──────────────────────

    #[test]
    fn every_reason_renders_non_empty(url in "[a-z]{0,16}", code in 100u16..600) {
        let rejects = [
            RejectReason::EmptyUrl,
            RejectReason::InvalidUrl { url: url.clone() },
            RejectReason::UnsupportedScheme { scheme: url.clone() },
            RejectReason::MissingHost,
            RejectReason::HttpStatus { code },
            RejectReason::EmptyContent,
        ];
        for r in rejects {
            prop_assert!(!r.to_string().is_empty(), "{r:?} rendered empty");
        }
        prop_assert!(!FilterReason::NonContentPath.to_string().is_empty());
    }
}
