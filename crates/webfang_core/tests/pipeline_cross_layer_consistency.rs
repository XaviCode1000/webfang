//! Cross-layer `StageOutcome` consistency (Sprint 9 roadmap DoD).
//!
//! Every layer must interpret every [`StageOutcome`] variant IDENTICALLY:
//! the variant a production stage produces is exactly what the executor
//! hands upward — no re-interpretation, no payload mutation, no variant
//! smuggling. The runner-boundary side (structured logging of the same
//! variants in `run_pipeline`) is covered by unit tests inside
//! `application::crawler::crawl_task`; this file proves the
//! domain → production-stage → executor path with the REAL stages.
//!
//! Run with: `cargo nextest run --test pipeline_cross_layer_consistency`

use std::future::Future;
use std::pin::Pin;

use webfang_core::application::pipeline::{
    CleanStage, PipelineExecutor, PipelineStage, ValidateStage,
};
use webfang_core::domain::error::ErrorClass;
use webfang_core::domain::pipeline_item::{RejectReason, ScrapedItem, StageOutcome};
use webfang_core::infrastructure::content_processing::SemanticProcessor;

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Real production CleanStage wired to the production processor.
fn clean_stage() -> CleanStage {
    CleanStage(Box::new(SemanticProcessor))
}

fn item(url: &str, html: &str, status_code: u16) -> ScrapedItem {
    ScrapedItem {
        url: url.into(),
        raw_html: html.into(),
        status_code,
        ..Default::default()
    }
}

async fn stage_outcome(stage: &dyn PipelineStage, item: ScrapedItem) -> StageOutcome {
    stage.process(item).await
}

async fn executor_outcome(stages: Vec<Box<dyn PipelineStage>>, item: ScrapedItem) -> StageOutcome {
    let mut executor = PipelineExecutor::new();
    for stage in stages {
        executor.add_stage(stage);
    }
    executor.execute(item).await
}

fn validate_then_clean() -> Vec<Box<dyn PipelineStage>> {
    vec![Box::new(ValidateStage), Box::new(clean_stage())]
}

// ── Rejected: identical reason at every layer ─────────────────────────────

#[tokio::test]
async fn rejected_reason_is_identical_at_every_layer() {
    // (url, html, status, expected reason) — one case per reachable
    // ValidateStage rejection. `MissingHost` is unreachable through
    // `url::Url` (every parseable http/https URL has a host), so it is not
    // constructible here by design.
    let cases: Vec<(&str, &str, u16, RejectReason)> = vec![
        ("", "<p>hi</p>", 200, RejectReason::EmptyUrl),
        (
            "not a url",
            "<p>hi</p>",
            200,
            RejectReason::InvalidUrl {
                url: "not a url".into(),
            },
        ),
        (
            "ftp://example.com/file",
            "<p>hi</p>",
            200,
            RejectReason::UnsupportedScheme {
                scheme: "ftp".into(),
            },
        ),
        (
            "https://example.com/page",
            "",
            200,
            RejectReason::EmptyContent,
        ),
        (
            "https://example.com/page",
            "<p>hi</p>",
            404,
            RejectReason::HttpStatus { code: 404 },
        ),
    ];

    for (url, html, status, expected_reason) in cases {
        let input = item(url, html, status);

        // Stage layer: ValidateStage itself reports exactly this variant.
        let at_stage = stage_outcome(&ValidateStage, input.clone()).await;
        assert_eq!(
            at_stage,
            StageOutcome::Rejected {
                reason: expected_reason.clone()
            },
            "stage-layer mismatch for url={url:?}"
        );

        // Executor layer: ValidateStage + real CleanStage yields the SAME
        // variant with the SAME typed payload verbatim.
        let at_executor = executor_outcome(validate_then_clean(), input).await;
        assert_eq!(
            at_executor,
            StageOutcome::Rejected {
                reason: expected_reason
            },
            "executor-layer mismatch for url={url:?}"
        );
    }
}

// ── Filtered: excluded downstream, never resurrected ──────────────────────

#[tokio::test]
async fn filtered_item_is_excluded_downstream_identically() {
    let input = item("https://example.com/robots.txt", "User-agent: *", 200);

    let at_stage = stage_outcome(&ValidateStage, input.clone()).await;
    assert_eq!(
        at_stage,
        StageOutcome::Filtered {
            reason: webfang_core::domain::pipeline_item::FilterReason::NonContentPath
        },
        "stage layer must filter non-content paths"
    );

    // The executor returns the exact Filtered outcome — never a Continue
    // carrying cleaned text — so CleanStage cannot resurrect excluded items.
    let at_executor = executor_outcome(validate_then_clean(), input).await;
    assert_eq!(
        at_executor,
        StageOutcome::Filtered {
            reason: webfang_core::domain::pipeline_item::FilterReason::NonContentPath
        },
        "executor must hand the runner the same Filtered outcome"
    );
}

// ── Failed: same ErrorClass end-to-end ────────────────────────────────────

/// Minimal fixture emitting `Failed { class }` after a successful upstream
/// pass. No production stage emits `Failed` yet (cleaning degrades to raw
/// content per #840), so the propagation identity is exercised with this
/// fixture placed AFTER the real ValidateStage.
struct FailWithClass(ErrorClass);

impl PipelineStage for FailWithClass {
    fn name(&self) -> &str {
        "fail_with_class"
    }

    fn process(
        &self,
        _item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + '_>> {
        Box::pin(async move { StageOutcome::Failed { class: self.0 } })
    }
}

#[tokio::test]
async fn failed_carries_same_error_class_end_to_end() {
    let all_classes = [
        ErrorClass::TransientRetriable,
        ErrorClass::TransientBackoff,
        ErrorClass::PermanentFatal,
        ErrorClass::InternalFatal,
        ErrorClass::DomainRecoverable,
    ];

    for class in all_classes {
        let input = item(
            "https://example.com/page",
            "<html><body><p>Hello world</p></body></html>",
            200,
        );

        // Stage layer: ValidateStage passes the valid item upstream...
        let validated = stage_outcome(&ValidateStage, input.clone()).await;
        assert!(
            matches!(validated, StageOutcome::Continue(_)),
            "fixture precondition: valid item must Continue, got {validated:?}"
        );

        // ...and the executor hands the runner the exact same ErrorClass.
        let mut stages = validate_then_clean();
        stages.push(Box::new(FailWithClass(class)));
        let outcome = executor_outcome(stages, input).await;

        assert_eq!(
            outcome,
            StageOutcome::Failed { class },
            "ErrorClass must survive every layer boundary unchanged"
        );
    }
}

// ── Continue: flows through all layers untouched ──────────────────────────

#[cfg_attr(miri, ignore)] // CleanStage -> legible/servo_arc (Tree-Borrows UB)
#[tokio::test]
async fn continue_flows_identically_through_all_layers() {
    let html = r#"<html><body><p>This is a substantial paragraph with enough text content to verify that the clean stage properly extracts and processes the readable content from the HTML document.</p></body></html>"#;
    let input = item("https://example.com/page", html, 200);

    let cleaned = stage_outcome(&clean_stage(), input.clone()).await;
    let at_executor = executor_outcome(validate_then_clean(), input).await;

    match (cleaned, at_executor) {
        (StageOutcome::Continue(mut direct), StageOutcome::Continue(mut via_executor)) => {
            assert!(direct.text_content.is_some());
            assert!(via_executor.text_content.is_some());
            // Same cleaning metadata regardless of how many layers below.
            for key in ["original_size", "cleaned_size"] {
                let a = direct.metadata.remove(key);
                let b = via_executor.metadata.remove(key);
                assert_eq!(a, b, "metadata key {key} must be identical across layers");
            }
        },
        (a, b) => panic!("both layers must Continue; got stage={a:?} executor={b:?}"),
    }
}
