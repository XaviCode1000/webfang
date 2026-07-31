//! Integration tests for checkpoint persistence — real I/O with temp dirs.
//!
//! Exercises save/load roundtrip, resume from checkpoint, corrupt file
//! handling, backward-compatible old-format loading, and TempDir cleanup.
//! Uses the consolidated application-layer `CheckpointStore` trait API.

use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;
use webfang_core::{BannedDomain, BincodeCheckpoint, CheckpointStore, CrawlCheckpoint};

/// Helper: build a CrawlCheckpoint from components.
fn checkpoint_from(
    visited: HashSet<String>,
    queued: Vec<String>,
    pages_crawled: u64,
    banned_domains: Vec<BannedDomain>,
) -> CrawlCheckpoint {
    CrawlCheckpoint {
        visited,
        queued,
        pages_crawled,
        banned_domains,
        version: 1,
    }
}

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

    let store = BincodeCheckpoint::new();
    let state = checkpoint_from(visited, queued, 42, vec![]);
    store.save(&state, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 3);
    assert!(loaded.visited.contains("https://a.com"));
    assert!(loaded.visited.contains("https://c.com"));
    assert_eq!(loaded.queued.len(), 2);
    assert_eq!(loaded.pages_crawled, 42);
    assert_eq!(loaded.version, 1);
}

/// Loading a non-existent checkpoint returns None.
#[tokio::test]
async fn test_load_nonexistent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nope.json");

    let store = BincodeCheckpoint::new();
    let loaded = store.load(&path);
    assert!(loaded.is_none(), "non-existent file should return None");
}

/// Resume from checkpoint: load → add more data → save → reload verifies append.
#[tokio::test]
async fn test_resume_from_checkpoint() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("resume.json");
    let store = BincodeCheckpoint::new();

    // Phase 1: initial crawl
    let mut visited = HashSet::new();
    visited.insert("https://page1.com".to_string());
    let cp1 = checkpoint_from(visited, vec![], 1, vec![]);
    store.save(&cp1, &path).unwrap();

    // Phase 2: resume and continue
    let mut loaded = store.load(&path).unwrap();
    loaded.visited.insert("https://page2.com".to_string());
    loaded.queued.push("https://page3.com".to_string());
    loaded.pages_crawled = 2;
    store.save(&loaded, &path).unwrap();

    // Phase 3: verify final state
    let final_cp = store.load(&path).unwrap();
    assert_eq!(final_cp.visited.len(), 2);
    assert!(final_cp.visited.contains("https://page1.com"));
    assert!(final_cp.visited.contains("https://page2.com"));
    assert_eq!(final_cp.queued.len(), 1);
    assert_eq!(final_cp.pages_crawled, 2);
}

/// Corrupt file — load returns None (not a panic).
#[tokio::test]
async fn test_corrupt_file_returns_none() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("corrupt.json");
    fs::write(&path, b"{not valid json!!!").unwrap();

    let store = BincodeCheckpoint::new();
    let result = store.load(&path);
    assert!(result.is_none(), "corrupt file should return None");
}

/// Save overwrites previous checkpoint (not append).
#[tokio::test]
async fn test_save_overwrites_previous() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("overwrite.json");
    let store = BincodeCheckpoint::new();

    // Save first checkpoint with 5 visited URLs
    let mut visited1 = HashSet::new();
    for i in 0..5 {
        visited1.insert(format!("https://page{i}.com"));
    }
    let cp1 = checkpoint_from(visited1, vec![], 5, vec![]);
    store.save(&cp1, &path).unwrap();

    // Save second checkpoint with only 2 visited URLs
    let mut visited2 = HashSet::new();
    visited2.insert("https://x.com".to_string());
    visited2.insert("https://y.com".to_string());
    let cp2 = checkpoint_from(visited2, vec![], 2, vec![]);
    store.save(&cp2, &path).unwrap();

    let loaded = store.load(&path).unwrap();
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
    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(HashSet::new(), vec!["q1".into()], 1, vec![]);
    store.save(&cp, &path).unwrap();

    // Verify it's valid checkpoint, not "old content"
    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.pages_crawled, 1);
    assert_eq!(loaded.queued.len(), 1);
}

/// Banned domains roundtrip through save/load.
#[tokio::test]
async fn test_banned_domains_roundtrip() {
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

    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(HashSet::new(), vec![], 0, banned);
    store.save(&cp, &path).unwrap();

    let loaded = store.load(&path).unwrap();
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

    let store = BincodeCheckpoint::new();
    let cp = checkpoint_from(visited, queued, 1000, vec![]);
    store.save(&cp, &path).unwrap();

    let loaded = store.load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 1000);
    assert_eq!(loaded.queued.len(), 500);
    assert_eq!(loaded.pages_crawled, 1000);
}

/// Old-format pure JSON checkpoint (no CRC32) loads via backward-compat fallback.
#[tokio::test]
async fn test_old_format_backward_compat() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("old_format.json");

    // Write old-format pure JSON (visited as array, no CRC32 header)
    let old_json = r#"{"visited":["https://old1.com","https://old2.com"],"queued":["https://q.com"],"pages_crawled":7,"version":1,"banned_domains":[]}"#;
    fs::write(&path, old_json).unwrap();

    let store = BincodeCheckpoint::new();
    let loaded = store.load(&path);
    assert!(loaded.is_some(), "old-format JSON should load via fallback");
    let cp = loaded.unwrap();
    assert_eq!(cp.visited.len(), 2);
    assert!(cp.visited.contains("https://old1.com"));
    assert_eq!(cp.pages_crawled, 7);
}
