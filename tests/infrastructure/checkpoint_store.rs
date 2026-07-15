//! Integration tests for BincodeCheckpoint — save/load roundtrip, corrupt files,
//! atomic writes, banned domains, and large checkpoints.

use webfang::infrastructure::checkpoint::store::{BannedDomain, CheckpointPath};
use webfang::BincodeCheckpoint;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

// ── Save/load roundtrip ───────────────────────────────────────────────────

#[test]
fn roundtrip_preserves_all_fields() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    let mut visited = HashSet::new();
    visited.insert("https://a.com".to_string());
    visited.insert("https://b.com".to_string());

    let queued = vec!["https://c.com".to_string(), "https://d.com".to_string()];
    let cp = BincodeCheckpoint::from_state(&visited, &queued, 42, vec![]);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 2);
    assert!(loaded.visited.contains(&"https://a.com".to_string()));
    assert!(loaded.visited.contains(&"https://b.com".to_string()));
    assert_eq!(loaded.queued.len(), 2);
    assert_eq!(loaded.pages_crawled, 42);
    assert_eq!(loaded.version, 1);
}

#[test]
fn roundtrip_with_banned_domains() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    let banned = vec![
        BannedDomain {
            domain: "waf.example.com".into(),
            banned_until: None,
            reason: "WAF challenge".into(),
        },
        BannedDomain {
            domain: "rate.example.com".into(),
            banned_until: Some("2026-12-31T23:59:59Z".parse().unwrap()),
            reason: "rate limit".into(),
        },
    ];

    let cp = BincodeCheckpoint::from_state(&HashSet::new(), &[], 0, banned);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.banned_domains.len(), 2);
    assert_eq!(loaded.banned_domains[0].domain, "waf.example.com");
    assert!(loaded.banned_domains[0].banned_until.is_none());
    assert_eq!(loaded.banned_domains[1].reason, "rate limit");
    assert!(loaded.banned_domains[1].banned_until.is_some());
}

// ── Corrupt file handling ─────────────────────────────────────────────────

#[test]
fn corrupt_json_returns_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("corrupt.json");
    fs::write(&path, b"{not valid json!!!").unwrap();

    let result = BincodeCheckpoint::load(&path);
    assert!(result.is_err(), "corrupt file should return Err");
}

#[test]
fn empty_file_returns_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("empty.json");
    fs::write(&path, b"").unwrap();

    let result = BincodeCheckpoint::load(&path);
    assert!(result.is_err(), "empty file should return Err");
}

#[test]
fn partial_json_returns_error() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("partial.json");
    fs::write(&path, br#"{"visited":["#).unwrap();

    let result = BincodeCheckpoint::load(&path);
    assert!(result.is_err());
}

// ── Non-existent path ─────────────────────────────────────────────────────

#[test]
fn load_nonexistent_returns_default() {
    let cp =
        BincodeCheckpoint::load(std::path::Path::new("/nonexistent/path/checkpoint.json")).unwrap();
    assert!(cp.visited.is_empty());
    assert!(cp.queued.is_empty());
    assert_eq!(cp.pages_crawled, 0);
}

// ── Atomic write (overwrite) ──────────────────────────────────────────────

#[test]
fn save_overwrites_previous_checkpoint() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    // First save with 5 URLs
    let mut v1 = HashSet::new();
    for i in 0..5 {
        v1.insert(format!("https://page{i}.com"));
    }
    BincodeCheckpoint::from_state(&v1, &[], 5, vec![])
        .save(&path)
        .unwrap();

    // Second save with 2 URLs
    let mut v2 = HashSet::new();
    v2.insert("https://x.com".to_string());
    v2.insert("https://y.com".to_string());
    BincodeCheckpoint::from_state(&v2, &[], 2, vec![])
        .save(&path)
        .unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.visited.len(), 2, "should have 2, not 5+2");
    assert_eq!(loaded.pages_crawled, 2);
}

#[test]
fn save_replaces_arbitrary_file_content() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");
    fs::write(&path, b"some old garbage content").unwrap();

    let cp = BincodeCheckpoint::from_state(&HashSet::new(), &["q1".into()], 1, vec![]);
    cp.save(&path).unwrap();

    let loaded = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(loaded.pages_crawled, 1);
    assert_eq!(loaded.queued.len(), 1);
}

// ── Resume from checkpoint ────────────────────────────────────────────────

#[test]
fn resume_adds_new_data_to_loaded_checkpoint() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

    // Phase 1: initial crawl
    let mut v1 = HashSet::new();
    v1.insert("https://page1.com".to_string());
    BincodeCheckpoint::from_state(&v1, &[], 1, vec![])
        .save(&path)
        .unwrap();

    // Phase 2: resume
    let mut loaded = BincodeCheckpoint::load(&path).unwrap();
    loaded.visited.push("https://page2.com".to_string());
    loaded.queued.push("https://page3.com".to_string());
    loaded.pages_crawled = 2;
    loaded.save(&path).unwrap();

    // Phase 3: verify
    let final_cp = BincodeCheckpoint::load(&path).unwrap();
    assert_eq!(final_cp.visited.len(), 2);
    assert!(final_cp.visited.contains(&"https://page1.com".to_string()));
    assert!(final_cp.visited.contains(&"https://page2.com".to_string()));
    assert_eq!(final_cp.queued, vec!["https://page3.com"]);
    assert_eq!(final_cp.pages_crawled, 2);
}

// ── Large checkpoint ──────────────────────────────────────────────────────

#[test]
fn large_checkpoint_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("checkpoint.json");

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

// ── Backward compatibility ────────────────────────────────────────────────

#[test]
fn old_format_without_banned_domains_loads() {
    let json = r#"{"visited":[],"queued":[],"pages_crawled":0,"version":1}"#;
    let cp: BincodeCheckpoint = jzon_serde::from_str(json).unwrap();
    assert!(cp.banned_domains.is_empty());
}

// ── CheckpointPath helper ─────────────────────────────────────────────────

#[test]
fn checkpoint_path_helper_returns_correct_file() {
    let tmp = TempDir::new().unwrap();
    let cp = CheckpointPath::new(tmp.path());
    cp.ensure_dir().unwrap();

    let file = cp.file();
    assert!(file.to_string_lossy().contains("crawl_checkpoint.json"));
    assert!(file.starts_with(tmp.path()));
}

#[test]
fn checkpoint_path_ensure_dir_creates_directory() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("deep").join("nested");
    let cp = CheckpointPath::new(&nested);
    cp.ensure_dir().unwrap();

    assert!(nested.exists());
}
