//! Progress Integration Tests
//!
//! These tests verify the reactive behavior of the progress view:
//! the state machine's bookkeeping after each event, and the availability
//! of errors and counters for rendering.
//!
//! # Why the wall-clock bounds here are coarse
//!
//! The work under test is in-memory bookkeeping: `ProgressState::update`
//! scans a `Vec<UrlState>` (at most 100 entries in this file) and stamps a
//! `SystemTime`. That is microseconds of real work, but a loaded CI runner
//! routinely deschedules a test process for far longer than the work itself,
//! so a sub-100ms ceiling measures the runner, not the code. The bounds below
//! are therefore a stall canary, not a performance gate: they exist to catch a
//! regression that turns bookkeeping into something pathological (a lock held
//! across an update, an accidental sleep, a runaway scan), and they are set far
//! above the work's real cost. Correctness of the arithmetic is asserted on the
//! VALUE (`percentage()`), which is fully deterministic.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use webfang_core::domain::entities::progress::{
    ProgressState, ScrapeError, ScrapeProgress, ScrapeStatus,
};

/// CI-safe ceiling for the in-memory progress bookkeeping measured below.
///
/// Microseconds of actual work sit behind this number; the slack absorbs
/// process descheduling on a contended CI runner. Sized to fail only on a
/// stall, not on a loaded machine.
const CI_SAFE_BOOKKEEPING_BOUND: Duration = Duration::from_secs(5);

/// Progress events are applied and their counters exposed for rendering.
///
/// The timing check is a stall canary (see the module docs); the
/// substantive assertions are the counters.
#[test]
fn test_progress_updates_recorded_within_ci_bound() {
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
            url: format!("https://example.com/{i}"),
        });

        state.update(ScrapeProgress::Completed {
            url: format!("https://example.com/{i}"),
            chars: 1000 * i,
        });
    }

    let elapsed = start.elapsed();

    assert!(
        elapsed < CI_SAFE_BOOKKEEPING_BOUND,
        "3 progress events must not stall; took {elapsed:?}"
    );

    // Verify state is correct
    assert_eq!(state.completed, 3);
    assert_eq!(state.percentage(), 100.0);
}

/// A failed event is available for widget display as soon as it is applied
/// (no async delay, no buffering).
#[test]
fn test_error_is_immediately_available_for_widget() {
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

    assert!(
        elapsed < CI_SAFE_BOOKKEEPING_BOUND,
        "recording one error must not stall; took {elapsed:?}"
    );

    // Verify error is in state
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.failed, 1);
    // The recorded entry must be usable by the widget, not just counted.
    let entry = &state.errors[0];
    assert_eq!(entry.url, "https://example.com/1");
    assert!(
        entry.message.contains("Connection refused"),
        "the widget entry must carry the error message, got: {}",
        entry.message
    );
}

/// Progress events arriving over a channel are received and applied.
///
/// The timing check is a stall canary: a bounded channel plus a live task must
/// not leave an event parked.
#[tokio::test]
async fn test_progress_channel_timing() {
    let url_strings = vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ];

    // Create channel for progress updates
    let (tx, mut rx) = mpsc::channel::<ScrapeProgress>(10);

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

    for (i, time) in processing_times.iter().enumerate() {
        assert!(
            *time < CI_SAFE_BOOKKEEPING_BOUND,
            "progress event {} must not stall in the channel; took {time:?}",
            i + 1
        );
    }

    // Verify state reflects both events: a Started event only flips the row to
    // Fetching, so `completed` proves the Completed event was applied too.
    assert_eq!(state.completed, 1);
    assert_eq!(state.percentage(), 50.0);
}

/// Bulk progress updates do not block or lose events.
#[tokio::test]
async fn test_concurrent_progress_updates() {
    let url_strings: Vec<String> = (1..=10)
        .map(|i| format!("https://example.com/{i}"))
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

    assert!(
        elapsed < CI_SAFE_BOOKKEEPING_BOUND,
        "15 progress events must not stall; took {elapsed:?}"
    );

    assert_eq!(state.completed, 5);
    // Check that remaining 5 URLs are in Pending (not completed or failed)
    // Since we only completed 5, the rest should still be in whatever state they started
    // After Started, they become Fetching, so we check for Fetching for in-progress
    let in_progress = state
        .urls
        .iter()
        .filter(|u| u.status == ScrapeStatus::Fetching)
        .count();
    assert_eq!(in_progress, 5);
}

/// A batch of errors is recorded in full.
#[test]
fn test_batch_error_processing_timing() {
    let url_strings: Vec<String> = (1..=10)
        .map(|i| format!("https://example.com/{i}"))
        .collect();

    let mut state = ProgressState::new(url_strings);

    let start = Instant::now();

    // Add multiple errors
    for i in 1..=10 {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{i}"),
            error: ScrapeError::Other(format!("Error {i}")),
        });
    }

    let elapsed = start.elapsed();

    assert!(
        elapsed < CI_SAFE_BOOKKEEPING_BOUND,
        "a 10-error batch must not stall; took {elapsed:?}"
    );

    assert_eq!(state.errors.len(), 10);
    assert_eq!(state.failed, 10);
    assert_eq!(state.percentage(), 100.0);
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
        .map(|i| format!("https://example.com/{i}"))
        .collect();

    let mut state = ProgressState::new(url_strings);

    // Add errors in specific order
    let urls = ["3", "1", "5", "2", "4"];
    for url_num in urls {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{url_num}"),
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

/// `percentage()` is pure arithmetic over two counters, so it is asserted on
/// the VALUE it returns, not on how long it took.
///
/// The previous version timed the call against a 50ms ceiling - a bound on
/// float division, which measures the CI runner, not the code. Asserting the
/// progression (0% -> 50% -> 100%) pins the actual contract: processed is
/// `completed + failed` over `total`, so a failed item counts as progress.
#[test]
fn test_percentage_tracks_completed_and_failed() {
    let url_strings: Vec<String> = (1..=100)
        .map(|i| format!("https://example.com/{i}"))
        .collect();

    let mut state = ProgressState::new(url_strings);
    assert_eq!(state.percentage(), 0.0, "nothing processed yet");

    // Complete 50 of 100
    for i in 1..=50 {
        state.update(ScrapeProgress::Completed {
            url: format!("https://example.com/{i}"),
            chars: 1000,
        });
    }
    assert!(
        (state.percentage() - 50.0).abs() < 0.01,
        "50 of 100 processed is 50%, got {}",
        state.percentage()
    );

    // Fail the other 50 - failures are progress too, so this reaches 100%.
    for i in 51..=100 {
        state.update(ScrapeProgress::Failed {
            url: format!("https://example.com/{i}"),
            error: ScrapeError::Other("Error".to_string()),
        });
    }
    assert!(
        (state.percentage() - 100.0).abs() < 0.01,
        "50 completed + 50 failed is 100%, got {}",
        state.percentage()
    );
    assert!(state.is_complete(), "all 100 URLs are terminal");
}

/// An empty batch reports 0% rather than dividing by zero.
#[test]
fn test_percentage_of_empty_batch_is_zero() {
    let state = ProgressState::new(Vec::new());
    assert_eq!(state.percentage(), 0.0);
}
