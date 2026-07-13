//! Item processing pipeline.
//!
//! Provides [`PipelineStage`] trait and [`PipelineExecutor`] for composing
//! sequential processing steps on [`ScrapedItem`]s.

mod executor;
pub mod stages;

pub use executor::PipelineExecutor;
pub use stages::{
    CleanStage, JsonlOutputStage, MultiSinkOutput, OutputError, OutputStage, ValidateStage,
};

// Re-export domain types used by the pipeline API
pub use crate::domain::pipeline_item::{PipelineStage, ScrapedItem, StageOutcome};

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[cfg_attr(miri, ignore)] // runs CleanStage -> legible/servo_arc (Tree-Borrows UB), same as stages/clean.rs
    #[tokio::test]
    async fn test_validate_then_clean_pipeline() {
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(ValidateStage));
        executor.add_stage(Box::new(CleanStage));

        let item = ScrapedItem {
            url: "https://example.com".into(),
            raw_html: r#"<html><body><p>This is a substantial paragraph with enough text content to verify that the clean stage properly extracts and processes the readable content from the HTML document.</p></body></html>"#.into(),
            status_code: 200,
            ..Default::default()
        };

        let result = executor.execute(item).await;
        match result {
            StageOutcome::Continue(item) => {
                assert!(item.text_content.is_some());
                let text = item.text_content.as_ref().unwrap();
                assert!(text.contains("substantial paragraph"));
                assert!(item.metadata.contains_key("original_size"));
                assert!(item.metadata.contains_key("cleaned_size"));
            },
            other => panic!("expected Continue, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_rejects_before_clean_runs() {
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(ValidateStage));
        executor.add_stage(Box::new(CleanStage));

        let item = ScrapedItem {
            url: "".into(),
            raw_html: "<p>hi</p>".into(),
            status_code: 200,
            ..Default::default()
        };

        let result = executor.execute(item).await;
        assert!(matches!(result, StageOutcome::Reject(_)));
    }

    #[tokio::test]
    async fn test_validate_skips_robots_txt() {
        let mut executor = PipelineExecutor::new();
        executor.add_stage(Box::new(ValidateStage));
        executor.add_stage(Box::new(CleanStage));

        let item = ScrapedItem {
            url: "https://example.com/robots.txt".into(),
            raw_html: "User-agent: *".into(),
            status_code: 200,
            ..Default::default()
        };

        let result = executor.execute(item).await;
        assert_eq!(result, StageOutcome::Skip);
    }
}
