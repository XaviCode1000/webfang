//! Integration tests for BincodeCheckpoint — real I/O with temp dirs.
//!
//! Exercises save/load roundtrip, resume from checkpoint, corrupt file
//! handling, and TempDir cleanup per R-INT-01.

use webfang::BincodeCheckpoint;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

/// Save and load a checkpoint — data survives the roundtrip.
#[tokio::test]
async fn test_save_and_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    let mut visited = HashSet::new();
    visited.insert("https://a.com".to_string());
    visited.insert("https://b.com".to_string());
    visited.insert("https://c.com".to_string());

    let queued = vec!["https://d.com".to_string(), "https://e.com".to_string()];

    let cp = BincodeCheckpoint::from_state(&visited, &queued, 42, vec![]);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 3);
    assert!(loaded.visited.iter().any(|s| s == "https://a.com"));
    assert!(loaded.visited.iter().any(|s| s == "https://c.com"));
    assert_eq!(loaded.queued.len(), 2);
    assert_eq!(loaded.pages_crawled, 42);
    assert_eq!(loaded.version, 1);
}

/// Loading a non-existent checkpoint returns a default (empty) checkpoint.
#[tokio::test]
async fn test_load_nonexistent_returns_default() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nope.json");

    let cp = BincodeCheckpoint::load(&path).unwrap();
    assert!(cp.visited.is_empty());
    assert!(cp.queued.is_empty());
    assert_eq!(cp.pages_crawled, 0);
}

/// Resume from checkpoint: load → add more data → save → reload verifies append.
#[tokio::test]
async fn test_resume_from_checkpoint() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("resume.json");

    // Phase 1: initial crawl
    let mut visited = HashSet::new();
    visited.insert("https://page1.com".to_string());
    let cp1 = BincodeCheckpoint::from_state(&visited, &[], 1, vec![]);
    cp1.save(&path).unwrap();

    // Phase 2: resume and continue
    let mut loaded = BincodeCheckpoint::load(&path).unwrap();
    loaded.visited.push("https://page2.com".to_string());
    loaded.queued.push("https://page3.com".to_string());
    loaded.pages_crawled = 2;
    loaded.save(&path).unwrap();

    // Phase 3: verify final state
    let final_cp = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(final_cp.visited.len(), 2);
    assert!(final_cp.visited.iter().any(|s| s == "https://page1.com"));
    assert!(final_cp.visited.iter().any(|s| s == "https://page2.com"));
    assert_eq!(final_cp.queued.len(), 1);
    assert_eq!(final_cp.pages_crawled, 2);
}

/// Corrupt JSON file — load returns an error (not a panic).
#[tokio::test]
async fn test_corrupt_file_returns_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("corrupt.json");
    fs::write(&path, b"{not valid json!!!").unwrap();

    let result = BincodeCheckpoint::load(&path);
    assert!(result.is_err(), "corrupt file should return Err");
}

/// Save overwrites previous checkpoint (not append).
#[tokio::test]
async fn test_save_overwrites_previous() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("overwrite.json");

    // Save first checkpoint with 5 visited URLs
    let mut visited1 = HashSet::new();
    for i in 0..5 {
        visited1.insert(format!("https://page{i}.com"));
    }
    let cp1 = BincodeCheckpoint::from_state(&visited1, &[], 5, vec![]);
    cp1.save(&path).unwrap();

    // Save second checkpoint with only 2 visited URLs
    let mut visited2 = HashSet::new();
    visited2.insert("https://x.com".to_string());
    visited2.insert("https://y.com".to_string());
    let cp2 = BincodeCheckpoint::from_state(&visited2, &[], 2, vec![]);
    cp2.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 2, "should have 2 URLs, not 5+2");
    assert_eq!(loaded.pages_crawled, 2);
}

/// Save to an existing file replaces it cleanly.
#[tokio::test]
async fn test_save_replaces_existing_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("replace.json");

    // Write initial content
    fs::write(&path, b"old content").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "old content");

    // Save checkpoint over it
    let cp = BincodeCheckpoint::from_state(&HashSet::new(), &["q1".into()], 1, vec![]);
    cp.save(&path).unwrap();

    // Verify it's valid JSON checkpoint, not "old content"
    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.pages_crawled, 1);
    assert_eq!(loaded.queued.len(), 1);
}

/// Banned domains roundtrip through save/load.
#[tokio::test]
async fn test_banned_domains_roundtrip() {
    use webfang::infrastructure::checkpoint::store::BannedDomain;

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("banned.json");

    let banned = vec![
        BannedDomain {
            domain: "waf.example.com".into(),
            banned_until: None,
            reason: "WAF challenge".into(),
        },
        BannedDomain {
            domain: "rate.example.com".into(),
            banned_until: Some("2026-12-31T23:59:59Z".parse().unwrap()),
            reason: "rate limit exceeded".into(),
        },
    ];

    let cp = BincodeCheckpoint::from_state(&HashSet::new(), &[], 0, banned);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.banned_domains.len(), 2);
    assert_eq!(loaded.banned_domains[0].domain, "waf.example.com");
    assert!(loaded.banned_domains[0].banned_until.is_none());
    assert_eq!(loaded.banned_domains[1].reason, "rate limit exceeded");
    assert!(loaded.banned_domains[1].banned_until.is_some());
}

/// Large checkpoint with many URLs saves and loads correctly.
#[tokio::test]
async fn test_large_checkpoint_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("large.json");

    let mut visited = HashSet::new();
    let mut queued = Vec::new();
    for i in 0..1000 {
        visited.insert(format!("https://visited{i}.example.com/page{i}"));
        if i % 2 == 0 {
            queued.push(format!("https://queued{i}.example.com/page{i}"));
        }
    }

    let cp = BincodeCheckpoint::from_state(&visited, &queued, 1000, vec![]);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 1000);
    assert_eq!(loaded.queued.len(), 500);
    assert_eq!(loaded.pages_crawled, 1000);
}
