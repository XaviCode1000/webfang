use super::{PipelineStage, ScrapedItem, StageOutcome};
use tracing::{instrument, Instrument};

/// Executes a sequence of [`PipelineStage`]s on [`ScrapedItem`]s.
///
/// Stages are processed in insertion order. The first stage to return a
/// non-`Continue` outcome (`Filtered`, `Rejected`, or `Failed`) short-circuits
/// the pipeline and the outcome is returned verbatim.
pub struct PipelineExecutor {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl PipelineExecutor {
    /// Create an empty pipeline.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append a stage to the end of the pipeline.
    pub fn add_stage(&mut self, stage: Box<dyn PipelineStage>) {
        self.stages.push(stage);
    }

    /// Run all stages on `item` in order.
    ///
    /// Returns [`StageOutcome::Continue`] with the final item if every stage
    /// passes. Returns verbatim the first non-`Continue` outcome (`Filtered`,
    /// `Rejected`, or `Failed`) without running later stages.
    #[instrument(skip(self, item), fields(stages = self.stages.len(), url = %item.url))]
    pub async fn execute(&self, mut item: ScrapedItem) -> StageOutcome {
        for stage in &self.stages {
            // Per-stage span (issue #356): makes stage-level timing visible in
            // traces. `.instrument()` is async-safe (no enter-guard across await).
            let stage_span = tracing::info_span!(
                "pipeline_stage",
                stage = %stage.name(),
                url = %item.url
            );
            let outcome = stage.process(item).instrument(stage_span).await;
            match outcome {
                StageOutcome::Continue(updated) => item = updated,
                other => return other,
            }
        }
        StageOutcome::Continue(item)
    }

    /// Returns the number of registered stages.
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// Returns `true` if no stages are registered.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }
}

impl Default for PipelineExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ─── Global subscriber guard (issue #417) ───

    static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

    /// Set a global fmt subscriber that writes to sink. This ensures every
    /// callsite registers with `Interest::always()` instead of the
    /// `Interest::never()` that `Dispatch::none()` would cache when concurrent
    /// tests hit instrumentation callsites without a subscriber (issue #417).
    fn ensure_global_subscriber() {
        GLOBAL_SUBSCRIBER_INIT.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });
    }

    // ─── Span-capture test helper (Fase 2, issue #356) ───

    /// MakeWriter that appends tracing output to a shared buffer so tests can
    /// assert which spans were emitted.
    #[derive(Clone)]
    struct SharedWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;
        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    struct SharedWriterGuard(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Span-capture test for pipeline span emission (issue #417).
    ///
    /// Root cause of flakiness: `tracing` caches per-callsite `Interest`
    /// process-wide via a one-time `compare_exchange`. When a concurrent test
    /// thread hits the `pipeline_stage` callsite with no subscriber active,
    /// `Dispatch::none()` registers `Interest::never()`, permanently disabling
    /// span creation at that callsite for ALL threads — including ours.
    ///
    /// Fix: a module-level `Once` sets a global fmt subscriber (writing to sink)
    /// that returns `Interest::always()` from `register_callsite()`. This ensures
    /// every callsite is registered as always-enabled before any test can poison
    /// it with `never()`. Per-test `with_default` still overrides the global for
    /// actual span capture.
    #[test]
    fn test_execute_emits_pipeline_spans() {
        use tracing_subscriber::fmt::format::FmtSpan;

        // Ensure the global subscriber is set (poison-proof callsite interest).
        ensure_global_subscriber();

        let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(buf.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_span_events(FmtSpan::NEW)
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            rt.block_on(async {
                let mut executor = PipelineExecutor::new();
                executor.add_stage(Box::new(TransformStage));
                let item = ScrapedItem {
                    url: "https://example.com/traced".into(),
                    raw_html: "<p>content</p>".into(),
                    status_code: 200,
                    ..Default::default()
                };
                let _ = executor.execute(item).await;
            });
        });

        let out = String::from_utf8_lossy(&buf.lock().unwrap()).to_string();
        assert!(
            out.contains("execute"),
            "execute span should be emitted, got: {out}"
        );
        assert!(
            out.contains("pipeline_stage"),
            "pipeline_stage span should be emitted, got: {out}"
        );
        assert!(
            out.contains("https://example.com/traced"),
            "url field should be recorded, got: {out}"
        );
    }

    struct CountingStage {
        name: String,
        counter: Arc<AtomicUsize>,
    }

    impl PipelineStage for CountingStage {
        fn name(&self) -> &str {
            &self.name
        }

        fn process(
            &self,
            mut item: ScrapedItem,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = StageOutcome> + Send + '_>>
        {
            Box::pin(async move {
                self.counter.fetch_add(1, Ordering::SeqCst);
                item.metadata.insert(self.name.clone(), "processed".into());
                StageOutcome::Continue(item)
            })
        }
    }

    struct SkipStage;

    impl PipelineStage for SkipStage {
        fn name(&self) -> &str {
            "skip_stage"
        }

        fn process(
            &self,
            _item: ScrapedItem,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = StageOutcome> + Send + '_>>
        {
            Box::pin(async move {
                StageOutcome::Filtered {
                    reason: crate::domain::pipeline_item::FilterReason::NonContentPath,
                }
            })
        }
    }

    struct RejectStage;

    impl PipelineStage for RejectStage {
        fn name(&self) -> &str {
            "reject_stage"
        }

        fn process(
            &self,
            _item: ScrapedItem,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = StageOutcome> + Send + '_>>
        {
            Box::pin(async move {
                StageOutcome::Rejected {
                    reason: crate::domain::pipeline_item::RejectReason::EmptyContent,
                }
            })
        }
    }

    struct TransformStage;

    impl PipelineStage for TransformStage {
        fn name(&self) -> &str {
            "transform_stage"
        }

        fn process(
            &self,
            mut item: ScrapedItem,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = StageOutcome> + Send + '_>>
        {
            Box::pin(async move {
                item.text_content = Some("cleaned".into());
                StageOutcome::Continue(item)
            })
        }
    }

    #[tokio::test]
    async fn test_empty_pipeline_returns_continue() {
        let executor = PipelineExecutor::new();
        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert!(matches!(result, StageOutcome::Continue(_)));
    }

    #[tokio::test]
    async fn test_single_stage_continues() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(CountingStage {
            name: "s1".into(),
            counter: counter.clone(),
        }));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        match result {
            StageOutcome::Continue(item) => {
                assert_eq!(item.metadata.get("s1").unwrap(), "processed");
            },
            _ => panic!("expected Continue"),
        }
    }

    #[tokio::test]
    async fn test_multiple_stages_all_run() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(CountingStage {
            name: "s1".into(),
            counter: c1.clone(),
        }));
        executor.add_stage(Box::new(CountingStage {
            name: "s2".into(),
            counter: c2.clone(),
        }));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
        assert!(matches!(result, StageOutcome::Continue(_)));
    }

    #[tokio::test]
    async fn test_skip_short_circuits() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(SkipStage));
        executor.add_stage(Box::new(CountingStage {
            name: "s2".into(),
            counter: counter.clone(),
        }));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(
            result,
            StageOutcome::Filtered {
                reason: crate::domain::pipeline_item::FilterReason::NonContentPath
            }
        );
    }

    #[tokio::test]
    async fn test_reject_short_circuits_with_reason() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(RejectStage));
        executor.add_stage(Box::new(CountingStage {
            name: "s2".into(),
            counter: counter.clone(),
        }));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(
            result,
            StageOutcome::Rejected {
                reason: crate::domain::pipeline_item::RejectReason::EmptyContent
            }
        );
    }

    #[tokio::test]
    async fn test_transform_modifies_item() {
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(TransformStage));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        match result {
            StageOutcome::Continue(item) => {
                assert_eq!(item.text_content.as_deref(), Some("cleaned"));
            },
            _ => panic!("expected Continue"),
        }
    }

    #[tokio::test]
    async fn test_skip_after_transform() {
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(TransformStage));
        executor.add_stage(Box::new(SkipStage));

        let item = ScrapedItem::default();
        let result = executor.execute(item).await;
        assert_eq!(
            result,
            StageOutcome::Filtered {
                reason: crate::domain::pipeline_item::FilterReason::NonContentPath
            }
        );
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut executor = PipelineExecutor::new();
        assert!(executor.is_empty());
        assert_eq!(executor.len(), 0);

        executor.add_stage(Box::new(SkipStage));
        assert!(!executor.is_empty());
        assert_eq!(executor.len(), 1);
    }

    #[test]
    fn test_default_is_empty() {
        let executor = PipelineExecutor::default();
        assert!(executor.is_empty());
    }
}
