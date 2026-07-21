//! Multi-sink output stage — fans out writes to multiple output sinks.

use std::future::Future;
use std::pin::Pin;

use crate::application::pipeline::stages::output::{OutputError, OutputStage};
use crate::domain::pipeline_item::ScrapedItem;

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::OUTPUT_SINK_ERRORS;
#[cfg(feature = "otel-metrics")]
use opentelemetry::KeyValue;

/// Output stage that writes to all inner sinks sequentially.
///
/// If a sink fails, the error is logged and remaining sinks are still attempted.
/// Returns `Ok(())` if at least one sink succeeds, or the first error if all fail.
pub struct MultiSinkOutput {
    sinks: Vec<Box<dyn OutputStage>>,
}

impl MultiSinkOutput {
    /// Create a multi-sink from a list of output stages.
    pub fn new(sinks: Vec<Box<dyn OutputStage>>) -> Self {
        Self { sinks }
    }

    /// Returns the number of inner sinks.
    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    /// Returns `true` if no sinks are registered.
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

impl OutputStage for MultiSinkOutput {
    fn name(&self) -> &str {
        "multi_sink_output"
    }

    fn write<'a>(
        &'a self,
        item: &'a ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutputError>> + Send + 'a>> {
        Box::pin(async {
            let mut last_err: Option<OutputError> = None;
            let mut any_ok = false;

            for sink in &self.sinks {
                match sink.write(item).await {
                    Ok(()) => any_ok = true,
                    Err(e) => {
                        tracing::error!(sink = %sink.name(), error = %e, "sink failed");
                        #[cfg(feature = "otel-metrics")]
                        {
                            let sink_name = sink.name().to_owned();
                            OUTPUT_SINK_ERRORS.add(1, &[KeyValue::new("sink", sink_name)]);
                        }
                        if last_err.is_none() {
                            last_err = Some(e);
                        }
                    },
                }
            }

            if any_ok {
                Ok(())
            } else {
                Err(last_err.unwrap_or_else(|| OutputError::Backend("no sinks registered".into())))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MockOutputStage {
        name: String,
        call_count: Arc<AtomicUsize>,
        should_fail: bool,
    }

    impl MockOutputStage {
        fn new(name: &str) -> Self {
            Self {
                name: name.into(),
                call_count: Arc::new(AtomicUsize::new(0)),
                should_fail: false,
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                name: name.into(),
                call_count: Arc::new(AtomicUsize::new(0)),
                should_fail: true,
            }
        }
    }

    impl OutputStage for MockOutputStage {
        fn name(&self) -> &str {
            &self.name
        }

        fn write<'a>(
            &'a self,
            _item: &'a ScrapedItem,
        ) -> Pin<Box<dyn Future<Output = Result<(), OutputError>> + Send + 'a>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let should_fail = self.should_fail;
            Box::pin(async move {
                if should_fail {
                    Err(OutputError::Backend("mock failure".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn make_item() -> ScrapedItem {
        ScrapedItem {
            url: "https://example.com".into(),
            status_code: 200,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_writes_to_all_sinks() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let s1 = MockOutputStage {
            name: "s1".into(),
            call_count: c1.clone(),
            should_fail: false,
        };
        let s2 = MockOutputStage {
            name: "s2".into(),
            call_count: c2.clone(),
            should_fail: false,
        };

        let multi = MultiSinkOutput::new(vec![Box::new(s1), Box::new(s2)]);
        let item = make_item();
        multi.write(&item).await.unwrap();

        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_continues_on_single_failure() {
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let s1 = MockOutputStage {
            name: "s1".into(),
            call_count: c1.clone(),
            should_fail: true,
        };
        let s2 = MockOutputStage {
            name: "s2".into(),
            call_count: c2.clone(),
            should_fail: false,
        };

        let multi = MultiSinkOutput::new(vec![Box::new(s1), Box::new(s2)]);
        let item = make_item();
        let result = multi.write(&item).await;

        assert!(result.is_ok());
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_all_fail_returns_error() {
        let s1 = MockOutputStage::failing("s1");
        let s2 = MockOutputStage::failing("s2");

        let multi = MultiSinkOutput::new(vec![Box::new(s1), Box::new(s2)]);
        let item = make_item();
        let result = multi.write(&item).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            OutputError::Backend(msg) => assert!(msg.contains("mock failure")),
            other => panic!("expected Backend error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_empty_sinks_returns_error() {
        let multi = MultiSinkOutput::new(vec![]);
        let item = make_item();
        let result = multi.write(&item).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_len_and_is_empty() {
        let multi = MultiSinkOutput::new(vec![]);
        assert!(multi.is_empty());
        assert_eq!(multi.len(), 0);

        let multi = MultiSinkOutput::new(vec![Box::new(MockOutputStage::new("s1"))]);
        assert!(!multi.is_empty());
        assert_eq!(multi.len(), 1);
    }

    #[test]
    fn test_stage_name() {
        let multi = MultiSinkOutput::new(vec![]);
        assert_eq!(multi.name(), "multi_sink_output");
    }
}
