//! End-to-end test for channel plumbing: observer → channel → receiver.

#[tokio::test]
async fn test_full_pipeline_event_flow() -> anyhow::Result<()> {
    use std::sync::Arc;
    use webfang_core::application::progress_observer::LiveProgressObserver;
    use webfang_core::domain::entities::progress::{ScrapeProgress, ScrapeStatus};
    use webfang_core::domain::ports::ProgressObserver;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let observer = Arc::new(LiveProgressObserver::new(Some(tx), false));

    let url = "http://mock.test".to_string();
    let url_clone = url.clone();
    let obs = Arc::clone(&observer);

    tokio::spawn(async move {
        obs.on_page_started(&url_clone).await;
        obs.on_status_changed(&url_clone, ScrapeStatus::Fetching)
            .await;
        obs.on_status_changed(&url_clone, ScrapeStatus::Extracting)
            .await;
        obs.on_page_completed(&url_clone, 1024).await;
        obs.on_finished(1, 1, 0).await;
    });

    let mut received = Vec::new();
    while let Some(msg) = rx.recv().await {
        received.push(msg);
        if matches!(received.last(), Some(ScrapeProgress::Finished { .. })) {
            break;
        }
    }

    assert_eq!(received.len(), 5);

    // Verify sequence
    assert!(matches!(
        &received[0],
        ScrapeProgress::Started { url: u } if u == &url
    ));
    assert!(matches!(
        &received[1],
        ScrapeProgress::StatusChanged {
            status: ScrapeStatus::Fetching,
            ..
        }
    ));
    assert!(matches!(
        &received[2],
        ScrapeProgress::StatusChanged {
            status: ScrapeStatus::Extracting,
            ..
        }
    ));
    assert!(matches!(
        &received[3],
        ScrapeProgress::Completed { chars: 1024, .. }
    ));
    assert!(matches!(
        &received[4],
        ScrapeProgress::Finished {
            total: 1,
            successful: 1,
            failed: 0
        }
    ));

    Ok(())
}
