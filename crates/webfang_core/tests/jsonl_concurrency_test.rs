//! SC4 — single-writer JSONL session concurrency proof (PR4).
//!
//! 100 tokio tasks × 10 items each share ONE [`JsonlSession`] (one writer
//! thread per output file per run). After every task joins and the session is
//! closed, the file must contain exactly 1000 valid JSON lines: 0 corrupt,
//! 0 truncated, 0 interleaved.

use std::fs;

use tempfile::TempDir;
use webfang_core::infrastructure::export::jsonl_writer::JsonlSession;

const TASKS: u32 = 100;
const ITEMS_PER_TASK: u32 = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_hundred_tasks_share_one_session_without_corruption() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("sc4.jsonl");

    let (session, _hash_index) = JsonlSession::open(&path).expect("session opens");

    let mut handles = Vec::with_capacity(TASKS as usize);
    for task in 0..TASKS {
        let shared = session.clone();
        handles.push(tokio::spawn(async move {
            for item in 0..ITEMS_PER_TASK {
                let line = format!(
                    r#"{{"task":{task},"item":{item},"checksum_sha256":"hash-{task}-{item}"}}"#
                );
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                shared.append(&bytes).await.expect("append accepted");
            }
        }));
    }
    for handle in handles {
        handle.await.expect("task joins");
    }

    session.close().await.expect("clean shutdown");

    let content = fs::read_to_string(&path).expect("output readable");
    assert!(
        content.ends_with('\n'),
        "file must end on a newline boundary"
    );

    let mut count = 0usize;
    for line in content.lines() {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("corrupt line {count:?}: {e}"));
        assert!(
            value.get("checksum_sha256").is_some(),
            "line {count:?} lost its payload"
        );
        count += 1;
    }
    assert_eq!(
        count,
        (TASKS * ITEMS_PER_TASK) as usize,
        "exactly 1000 valid lines"
    );
}
