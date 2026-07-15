//! Progress TUI Integration Tests
//!
//! These tests verify the reactive behavior of the progress TUI view,
//! specifically timing requirements for progress updates and error display.

use std::time::{Duration, Instant};

use webfang::application::progress_types::{ProgressState, ScrapeError, ScrapeProgress};
use tokio::sync::mpsc;

/// Test that progress events are processed within 200ms.
///
/// This test verifies that the ProgressState can handle updates
/// and make them available for rendering within the required timeframe.
#[test]
fn test_progress_updates_within_200ms() {
    let url_strings = vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
        "https://example.com/3".to_string(),
    ];

    let mut state = ProgressState::new(url_strings);

    // Simulate rapid progress updates
    let start = Instant::now();

    for i in 1..=3 {
        state.update(ScrapeProgress::Started {
            url: format!("https://example.com/{}", i),
        });

        state.update(ScrapeProgress::Completed {
            url: format!("https://example.com/{}", i),
            chars: 1000 * i,
        });
    }

    let elapsed = start.elapsed();

    // Verify updates happen within 200ms
    assert!(
        elapsed < Duration::from_millis(200),
        "Progress updates took {}ms, expected < 200ms",
        elapsed.as_millis()
    );

    // Verify state is correct
    assert_eq!(state.completed, 3);
    assert_eq!(state.percentage(), 100.0);
}

/// Test that error appears in widget within 200ms.
///
/// This test verifies that errors added to the state are immediately
/// available for display (no async delay).
#[test]
fn test_error_appears_in_widget_within_200ms() {
    let url_strings = vec!["https://example.com/1".to_string()];

    let mut state = ProgressState::new(url_strings);

    // Add Started event
    state.update(ScrapeProgress::Started {
        url: "https://example.com/1".to_string(),
    });

    // Add Failed event with error
    let start = Instant::now();

    state.update(ScrapeProgress::Failed {
        url: "https://example.com/1".to_string(),
        error: ScrapeError::Network("Connection refused".to_string()),
    });

    let elapsed = start.elapsed();

    // Verify error is recorded within 200ms (should be immediate)
    assert!(
        elapsed < Duration::from_millis(200),
        "Error recording took {}ms, expected < 200ms",
        elapsed.as_millis()
    );

    // Verify error is in state
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.failed, 1);
}

/// Test progress state with mock channel timing.
///
/// This test simulates a realistic scenario where progress events
/// arrive over a channel and verifies processing latency.
#[tokio::test]
async fn test_progress_channel_timing() {
    let url_strings = vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ];

    // Create channel for progress updates
    let (tx, mut rx) = mpsc::channel::<ScrapeProgress>(10);

    // Spawn task to track processing time
    let mut state = ProgressState::new(url_strings);
    let mut processing_times = Vec::new();

    // Send first progress event
    let send_time = Instant::now();
    let progress1 = ScrapeProgress::Started {
        url: "https://example.com/1".to_string(),
    };
    tx.send(progress1).await.unwrap();

    // Receive and process
    if let Some(progress) = rx.recv().await {
        let process_time = send_time.elapsed();
        processing_times.push(process_time);
        state.update(progress);
    }

    // Send second progress event
    let send_time = Instant::now();
    let progress2 = ScrapeProgress::Completed {
        url: "https://example.com/1".to_string(),
        chars: 1000,
    };
    tx.send(progress2).await.unwrap();

    // Receive and process
    if let Some(progress) = rx.recv().await {
        let process_time = send_time.elapsed();
        processing_times.push(process_time);
        state.update(progress);
    }

    // Verify all processing times are within 200ms
    for (i, time) in processing_times.iter().enumerate() {
        assert!(
            *time < Duration::from_millis(200),
            "Progress event {} took {}ms, expected < 200ms",
            i + 1,
            time.as_millis()
        );
    }

    // Verify state reflects both events
    assert_eq!(state.completed, 1);
}

/// Test concurrent progress updates don't block.
///
/// Verifies that multiple simultaneous updates are handled
/// without excessive latency.
#[tokio::test]
async fn test_concurrent_progress_updates() {
    let url_strings: Vec<String> = (1..=10)
        .map(|i| format!("https://example.com/{}", i))
        .collect();

    let mut state = ProgressState::new(url_strings.clone());

    let start = Instant::now();

    // Simulate concurrent progress updates
    for url in &url_strings {
        state.update(ScrapeProgress::Started { url: url.clone() });
    }

    // Add some completions
    for (i, url) in url_strings.iter().enumerate().take(5) {
        state.update(ScrapeProgress::Completed {
            url: url.clone(),
            chars: 1000 + (i * 100),
        });
    }

    let elapsed = start.elapsed();

    // Should handle 10 updates well under 200ms
    assert!(
        elapsed < Duration::from_millis(200),
        "10 concurrent updates took {}ms, expected < 200ms",
        elapsed.as_millis()
    );

    assert_eq!(state.completed, 5);
    // Check that remaining 5 URLs are in Pending (not completed or failed)
    // Since we only completed 5, the rest should still be in whatever state they started
    // After Started, they become Fetching, so we check for Fetching for in-progress
    let in_progress = state
        .urls
        .iter()
        .filter(|u| u.status == webfang::application::progress_types::ScrapeStatus::Fetching)
        .count();
    assert_eq!(in_progress, 5);
}

/// Test error batch processing timing.
///
/// Verifies that adding multiple errors at once doesn't exceed
/// the 200ms threshold.
#[test]
fn test_batch_error_processing_timing() {
    let url_strings: Vec<String> = (1..=10)
        .map(|i| format!("https://example.com/{}", i))
        .collect();

    let mut state = ProgressState::new(url_strings);

    let start = Instant::now();

    // Add multiple errors
    for i in 1..=10 {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{}", i),
            error: ScrapeError::Other(format!("Error {}", i)),
        });
    }

    let elapsed = start.elapsed();

    // Should process batch under 200ms
    assert!(
        elapsed < Duration::from_millis(200),
        "Batch error processing took {}ms, expected < 200ms",
        elapsed.as_millis()
    );

    assert_eq!(state.errors.len(), 10);
    assert_eq!(state.failed, 10);
}

/// Test that error entries are correctly structured for widget display.
#[test]
fn test_error_entry_structure_for_widget() {
    let url_strings = vec!["https://example.com/1".to_string()];

    let mut state = ProgressState::new(url_strings);

    // Add error
    state.update(ScrapeProgress::Failed {
        url: "https://example.com/1".to_string(),
        error: ScrapeError::WafBlocked("Cloudflare".to_string()),
    });

    // Verify error entry has required fields for widget
    assert_eq!(state.errors.len(), 1);

    let entry = &state.errors[0];
    assert!(!entry.url.is_empty());
    assert!(!entry.message.is_empty());
    // Timestamp should be set (within last minute)
    let now = std::time::SystemTime::now();
    let duration = now.duration_since(entry.timestamp).unwrap();
    assert!(duration.as_secs() < 60);
}

/// Test progress state maintains correct ordering for display.
#[test]
fn test_error_ordering_for_display() {
    let url_strings: Vec<String> = (1..=5)
        .map(|i| format!("https://example.com/{}", i))
        .collect();

    let mut state = ProgressState::new(url_strings);

    // Add errors in specific order
    let urls = ["3", "1", "5", "2", "4"];
    for url_num in urls {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{}", url_num),
            error: ScrapeError::Network("Connection refused".to_string()),
        });
    }

    // Errors should be in chronological order (oldest first for display)
    // The most recent errors are at the end
    assert_eq!(state.errors.len(), 5);
    // First error should be for URL 3 (earliest)
    assert!(state.errors[0].url.contains("3"));
    // Last error should be for URL 4 (most recent)
    assert!(state.errors[4].url.contains("4"));
}

/// Test percentage calculation timing.
///
/// Verifies that percentage calculations don't add significant latency.
#[test]
fn test_percentage_calculation_timing() {
    let url_strings: Vec<String> = (1..=100)
        .map(|i| format!("https://example.com/{}", i))
        .collect();

    let mut state = ProgressState::new(url_strings);

    // Add 50 completed, 50 failed
    for i in 1..=50 {
        state.update(ScrapeProgress::Completed {
            url: format!("https://example.com/{}", i),
            chars: 1000,
        });
    }

    for i in 51..=100 {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{}", i),
            error: ScrapeError::Other("Error".to_string()),
        });
    }

    let start = Instant::now();
    let _percentage = state.percentage();
    let elapsed = start.elapsed();

    // Percentage calculation should be nearly instant
    assert!(
        elapsed < Duration::from_millis(50),
        "Percentage calculation took {}ms, expected < 50ms",
        elapsed.as_millis()
    );

    assert!((state.percentage() - 100.0).abs() < 0.01);
}
