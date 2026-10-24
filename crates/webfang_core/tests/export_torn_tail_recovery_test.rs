//! #2237 \u2014 the JSONL tail recovery reads ONLY the tail it needs.
//!
//! `recover_torn_tail` used to read the whole export file with `fs::read`, so
//! the peak memory of the process grew with the size of the accumulated
//! export. The recovery needs one thing: the LAST newline boundary. Everything
//! before it is already durable, so it is never loaded.
//!
//! These tests pin the OBSERVABLE contract through the public API
//! ([`JsonlSession::open`]):
//!
//! 1. a SHORT torn tail is cut back to the last newline,
//! 2. a torn tail LONGER than the initial window forces the window to grow
//!    backwards until it reaches the newline boundary,
//! 3. a file with NO newline at all is wiped (no durable line) and preserved,
//! 4. a tail longer than the window CAP fails with an honest error instead of
//!    a blind cut.

use std::fs;

use tempfile::TempDir;
use webfang_core::infrastructure::export::jsonl_writer::{
    JsonlSession, MAX_TAIL_WINDOW_BYTES, TAIL_WINDOW_BYTES,
};

/// The `.bak` sibling the recovery writes before a destructive rewrite.
fn backup_of(path: &std::path::Path) -> std::path::PathBuf {
    path.with_extension("jsonl.bak")
}

#[tokio::test]
async fn short_torn_tail_is_cut_back_to_the_last_newline() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("short.jsonl");
    let prefix = "{\"n\":1,\"checksum_sha256\":\"hash-1\"}\n";
    let abandoned = "{\"n\":2,\"checksum_sha256\":\"ha";
    fs::write(&path, format!("{prefix}{abandoned}")).expect("seed torn file");

    let (session, index) = JsonlSession::open(&path).expect("open recovers the torn tail");
    session.close().await.expect("clean exit");

    assert_eq!(
        fs::read_to_string(&path).expect("read recovered"),
        prefix,
        "the torn tail is cut back to the last newline"
    );
    assert_eq!(
        fs::read_to_string(backup_of(&path)).expect("read backup"),
        abandoned,
        "the backup carries the abandoned bytes"
    );
    assert!(
        index.contains("hash-1"),
        "the surviving line stays in the hash index"
    );
}

#[tokio::test]
async fn torn_tail_longer_than_the_initial_window_grows_the_window() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("long.jsonl");
    let prefix = "{\"n\":1,\"checksum_sha256\":\"hash-1\"}\n";
    // Three windows: the FIRST read cannot see the newline boundary, so the
    // window has to grow backwards to find it.
    let abandoned = "x".repeat((TAIL_WINDOW_BYTES * 3) as usize);
    fs::write(&path, format!("{prefix}{abandoned}")).expect("seed long torn file");

    let (session, _index) = JsonlSession::open(&path).expect("open recovers the long torn tail");
    session.close().await.expect("clean exit");

    assert_eq!(
        fs::read_to_string(&path).expect("read recovered"),
        prefix,
        "a tail longer than the initial window is still cut at the newline"
    );
    assert_eq!(
        fs::read_to_string(backup_of(&path)).expect("read backup"),
        abandoned,
        "the whole abandoned tail is preserved"
    );
}

#[tokio::test]
async fn file_without_any_newline_is_wiped_and_preserved() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("unterminated.jsonl");
    let content = "{\"n\":1,\"checksum_sha256\":\"hash-1\"";
    fs::write(&path, content).expect("seed unterminated file");

    let (session, index) = JsonlSession::open(&path).expect("open wipes the unterminated file");
    session.close().await.expect("clean exit");

    assert!(
        fs::read(&path).expect("read recovered").is_empty(),
        "a file with no newline boundary has no durable line"
    );
    assert_eq!(
        fs::read_to_string(backup_of(&path)).expect("read backup"),
        content,
        "the abandoned bytes (here: the whole file) are preserved"
    );
    assert!(
        index.is_empty(),
        "no line survives the wipe, so the index is empty"
    );
}

#[test]
fn tail_beyond_the_window_cap_fails_with_an_honest_error() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("beyond-cap.jsonl");
    let content = vec![b'x'; (MAX_TAIL_WINDOW_BYTES + 1) as usize];
    fs::write(&path, &content).expect("seed oversized unterminated file");

    let error = JsonlSession::open(&path).expect_err("a blind cut must not happen");

    assert!(
        error.to_string().contains("window"),
        "the error must name the cause (the window cap): {error}"
    );
    assert_eq!(
        fs::metadata(&path).expect("stat").len(),
        content.len() as u64,
        "a refused recovery destroys nothing"
    );
}
