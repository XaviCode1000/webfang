use super::{PipelineStage, ScrapedItem, StageOutcome};
use tracing::{instrument, Instrument};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    PIPELINE_ITEMS_REJECTED, PIPELINE_ITEMS_SKIPPED, PIPELINE_ITEMS_TOTAL,
};

/// Executes a sequence of [`PipelineStage`]s on [`ScrapedItem`]s.
///
/// Stages are processed in insertion order. The first stage to return
/// [`StageOutcome::Skip`] or [`StageOutcome::Reject`] short-circuits the pipeline.
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
    /// passes. Returns early on the first [`StageOutcome::Skip`] or
    /// [`StageOutcome::Reject`].
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
                StageOutcome::Reject(reason) => {
                    #[cfg(feature = "otel-metrics")]
                    {
                        use opentelemetry::KeyValue;
                        let stage_name = stage.name().to_owned();
                        PIPELINE_ITEMS_REJECTED.add(1, &[KeyValue::new("stage", stage_name)]);
                    }
                    return StageOutcome::Reject(reason);
                },
                StageOutcome::Skip => {
                    #[cfg(feature = "otel-metrics")]
                    PIPELINE_ITEMS_SKIPPED.add(1, &[]);
                    return StageOutcome::Skip;
                },
            }
        }
        #[cfg(feature = "otel-metrics")]
        PIPELINE_ITEMS_TOTAL.add(1, &[]);
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

    #[tokio::test]
    async fn test_execute_emits_pipeline_spans() {
        use tracing_subscriber::fmt::format::FmtSpan;

        let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(buf.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_span_events(FmtSpan::NEW)
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(TransformStage));
        let item = ScrapedItem {
            url: "https://example.com/traced".into(),
            raw_html: "<p>content</p>".into(),
            status_code: 200,
            ..Default::default()
        };
        let _ = executor.execute(item).await;

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
            Box::pin(async move { StageOutcome::Skip })
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
            Box::pin(async move { StageOutcome::Reject("invalid content".into()) })
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
        assert_eq!(result, StageOutcome::Skip);
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
        assert_eq!(result, StageOutcome::Reject("invalid content".into()));
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
        assert_eq!(result, StageOutcome::Skip);
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
