//! Batch mode: stdin and file-based URL processing.

use crate::cmd;
use crate::BehavioralTest;
use crate::{assert_spawn_within, SPAWN_LATENCY_BUDGET};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// #1215 (F-38 + F-02) fixtures: two articles that cross-link each other and a
// shared /other page. A BFS-per-seed batch run fetches /other (twice) and
// appends one export.jsonl line per fetched page, so 2 seeds yield 6 lines
// over 3 unique URLs. A scrape-per-URL run fetches exactly the 2 seeds.
// ---------------------------------------------------------------------------

/// Mount the #1215 cross-linked article graph on `server`.
async fn mount_article_graph(server: &MockServer) {
    let uri = server.uri();
    let article = format!(
        "<html><head><title>Article</title></head><body><main><article>\
         <h1>Article One</h1>\
         <p>This is the first article with enough substantive text for the extractor.</p>\
         <p><a href=\"{uri}/article/2\">Second article</a> \
         <a href=\"{uri}/other\">Other page</a></p>\
         </article></main></body></html>"
    );
    let article2 = format!(
        "<html><head><title>Article Two</title></head><body><main><article>\
         <h1>Article Two</h1>\
         <p>This is the second article with enough substantive text for the extractor.</p>\
         <p><a href=\"{uri}/article\">First article</a> \
         <a href=\"{uri}/other\">Other page</a></p>\
         </article></main></body></html>"
    );
    for (route, body) in [
        ("/article", article),
        ("/article/2", article2),
        (
            "/other",
            "<html><head><title>Other</title></head><body><main><article>\
             <h1>Other Page</h1>\
             <p>An unrelated page reachable from both articles, causing BFS overlap.</p>\
             </article></main></body></html>"
                .to_string(),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(server)
            .await;
    }
}

/// Read export.jsonl as parsed JSON values (one per non-empty line).
fn read_jsonl_lines(out: &std::path::Path) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(out.join("export.jsonl"))
        .expect("export.jsonl must exist after a successful --batch run");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every export.jsonl line must be valid JSON"))
        .collect()
}

/// Map every .md file under `out` by its path relative to `out`.
fn md_files_by_relative_path(out: &std::path::Path) -> HashMap<String, Vec<u8>> {
    walkdir::WalkDir::new(out)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| {
            let rel = e
                .path()
                .strip_prefix(out)
                .expect("walked path must be under out")
                .to_string_lossy()
                .into_owned();
            let bytes = std::fs::read(e.path()).expect("read .md file");
            (rel, bytes)
        })
        .collect()
}

/// The identity export.jsonl must be idempotent on: same URL + same content
/// hash is one record, no matter how many times it was fetched.
fn record_identity(v: &serde_json::Value) -> (String, String) {
    (
        v["url"].as_str().unwrap_or_default().to_string(),
        v["checksum_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

// ---------------------------------------------------------------------------
// --batch (stdin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_processes_urls() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Stdin Test</h1>\
                 <p>Content from batch stdin processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--batch")
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(30))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch stdin should fetch exactly the provided URL, got {} requests",
        requests.len()
    );
}

/// #1697: an empty `--batch` on stdin must exit 64 with the Spanish message.
///
/// Runs under [`assert_spawn_within`] for the same reason as its
/// `batch_empty_file_exits_64` sibling. #1697 shipped the budget for that
/// test and this is the one it left behind: both do nothing but spawn the
/// binary, feed it an empty batch and assert the exit code, so they carry the
/// identical `windows-latest` spawn-latency flake. The flake is a property of
/// SPAWNING on that platform, not of which input path feeds the process —
/// which is why fixing the file-input half did not fix the stdin half.
///
/// Observed on `Tests (windows-latest)` for PR #1793 (run 37126785645): TRY 1
/// and TRY 2 each ran past nextest's 180 s `terminate-after` as TMT, and TRY 3
/// failed. The budget below bounds that at ~2x20 s instead, and the assertion
/// — code 64 AND the message — is unchanged, so a genuine regression still
/// fails and reports which half it got wrong.
#[test]
fn batch_empty_stdin_exits_64() {
    assert_spawn_within(
        SPAWN_LATENCY_BUDGET,
        "batch_empty_stdin_exits_64",
        move || {
            let output = cmd()
                .arg("--batch")
                .write_stdin("")
                .timeout(Duration::from_secs(5))
                .output()
                .map_err(|e| format!("could not spawn the binary: {e}"))?;

            if output.status.code() != Some(64) {
                return Err(format!(
                    "expected exit code 64, got {:?}; stderr: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("No URLs provided") {
                return Err(format!(
                    "stderr must name the empty batch, got: {}",
                    stderr.trim()
                ));
            }
            Ok(())
        },
    );
}

/// FIX-0 (#1235): record cardinality against the REAL binary. AUDIT-01
/// F-02 made `--batch` write every export.jsonl record TWICE (2 URLs → 4
/// lines, corrupting downstream RAG); #1225 fixed it but no test counted.
/// This pins the invariant with a NAMED test: N stdin URLs produce
/// exactly N export.jsonl records — one per unique URL, no duplicates.
#[tokio::test]
async fn batch_stdin_record_cardinality_matches_url_count() {
    let t = BehavioralTest::new().await;
    for (route, title) in [("/a", "Alpha"), ("/b", "Beta")] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article><h1>{title}</h1>\
                     <p>Substantive body text for {title} so the extractor keeps the page.</p>\
                     </article></body></html>"
            )))
            .mount(&t.server)
            .await;
    }
    let base = t.server.uri();

    cmd()
        .arg("--batch")
        .arg("--output")
        .arg(t.out.path())
        .write_stdin(format!("{base}/a\n{base}/b\n"))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    assert_eq!(
        t.record_count("export.jsonl"),
        2,
        "2 stdin URLs must produce exactly 2 export.jsonl records (F-02 duplication regression)"
    );
}

// ---------------------------------------------------------------------------
// --batch file output (#631): the full pipeline must write .md + .jsonl, not
// skip export with an early return.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_writes_markdown_and_jsonl() {
    let server = MockServer::start().await;
    let out = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Output</h1>\
                 <p>Body text that must be written to disk.</p>\
             </article></body></html>",
        ))
        .mount(&server)
        .await;

    cmd()
        .arg("--batch")
        .arg("--output")
        .arg(out.path())
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    // `save_results` nests files under a per-host directory (e.g.
    // `<output>/127.0.0.1/index.md`), so walk the tree rather than the top
    // level only.
    let md_files: Vec<_> = walk_dir(out.path())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    let jsonl_path = out.path().join("export.jsonl");

    assert!(
        !md_files.is_empty(),
        "--batch must write at least one .md file under --output (#631), dir: {:?}",
        out.path()
    );
    assert!(
        jsonl_path.exists(),
        "--batch must write export.jsonl to --output (#631), dir: {:?}",
        out.path()
    );
}

/// Recursively collect files under `root` (depth-first, errors skipped).
fn walk_dir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_dir(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// --batch --resume (#637): the run must create a resume state file.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_resume_creates_state_file() {
    let server = MockServer::start().await;
    let out = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Resume State</h1><p>This body carries enough substantive text to pass the minimum-content guard.</p></article></body></html>",
        ))
        .mount(&server)
        .await;

    cmd()
        .arg("--batch")
        .arg("--resume")
        .arg("--output")
        .arg(out.path())
        .env("XDG_CACHE_HOME", cache.path())
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let state_files: Vec<_> = walk_dir(&cache.path().join("webfang/state"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert!(
        !state_files.is_empty(),
        "--batch --resume must create a state file (#637), cache: {:?}",
        cache.path()
    );
}

// ---------------------------------------------------------------------------
// --batch-file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_file_processes_urls() {
    let server = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Test</h1>\
                 <p>Content from batch file processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .timeout(Duration::from_secs(30))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch-file should fetch exactly the URL from the file, got {} requests",
        requests.len()
    );
}

/// #1697: an empty `--batch-file` must exit 64 with the Spanish message.
///
/// Runs under [`assert_spawn_within`] because this test carries the
/// `windows-latest` spawn-latency flake: its `.timeout(5s)` was not honoured
/// there and the first attempt burned nextest's whole 180 s `terminate-after`
/// before the retry passed in 3 s. The budget below bounds that at ~2x20 s
/// instead, and the assertion — code 64 AND the message — is unchanged, so a
/// genuine regression still fails and reports which half it got wrong.
#[test]
fn batch_empty_file_exits_64() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "").unwrap();

    assert_spawn_within(
        SPAWN_LATENCY_BUDGET,
        "batch_empty_file_exits_64",
        move || {
            let output = cmd()
                .arg("--batch-file")
                .arg(&batch_file)
                .timeout(Duration::from_secs(5))
                .output()
                .map_err(|e| format!("could not spawn the binary: {e}"))?;

            if output.status.code() != Some(64) {
                return Err(format!(
                    "expected exit code 64, got {:?}; stderr: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("No URLs provided") {
                return Err(format!(
                    "stderr must name the empty batch, got: {}",
                    stderr.trim()
                ));
            }
            Ok(())
        },
    );
}

// ---------------------------------------------------------------------------
// Batch timeout tests
// ---------------------------------------------------------------------------

/// A --batch-file with a slow endpoint and --timeout-secs 1 must exit 69
/// and complete well under 25s (the per-request timeout fires, not a hang).
#[tokio::test]
async fn batch_file_timeout_does_not_hang() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}/slow\n", t.server.uri())).unwrap();

    let start = Instant::now();
    let output = timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            cmd()
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(t.out.path())
                .output()
        }),
    )
    .await
    .expect("test must not hang — tokio::time::timeout fired")
    .expect("task must not panic")
    .expect("command must execute");

    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(69),
        "expected exit code 69 (partial/all failures), got {:?}",
        output.status.code()
    );
    assert!(
        elapsed < Duration::from_secs(25),
        "batch timeout test should complete in under 25s, took {elapsed:?}"
    );
}

/// A --batch-file with a slow endpoint and --timeout-secs 1 must exit 69
/// and stderr must contain the failed URL and a timeout keyword.
#[tokio::test]
async fn batch_file_timeout_reports_failures() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    let slow_url = format!("{}/slow", t.server.uri());
    std::fs::write(&batch_file, format!("{slow_url}\n")).unwrap();

    let output = timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            cmd()
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(t.out.path())
                .output()
        }),
    )
    .await
    .expect("test must not hang")
    .expect("task must not panic")
    .expect("command must execute");

    assert_eq!(
        output.status.code(),
        Some(69),
        "expected exit code 69, got {:?}",
        output.status.code()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&slow_url),
        "stderr should contain the failed URL, got: {stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("timeout") || stderr.to_lowercase().contains("timed out"),
        "stderr should mention timeout, got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// #1215 (F-38 + F-02): --batch scrapes each URL (one page per URL).
// ---------------------------------------------------------------------------

/// `--batch` over 2 cross-linked fixture URLs must yield exactly 2 export.jsonl
/// lines over 2 unique URLs: each seed is scraped once, never BFS-expanded
/// (which fetched /other and appended 6 lines over 3 unique URLs), and the
/// append-only export never holds duplicate (url, content_hash) records.
#[tokio::test]
async fn batch_scrapes_each_url_once_no_jsonl_duplicates() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    let uri = t.server.uri();
    let stdin_urls = format!("{uri}/article\n{uri}/article/2\n");

    cmd()
        .arg("--batch")
        .arg("--output")
        .arg(t.out.path())
        .arg("--no-checkpoint")
        .arg("--delay-ms")
        .arg("0")
        .write_stdin(stdin_urls)
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let lines = read_jsonl_lines(t.out.path());
    let unique_urls: HashSet<String> = lines
        .iter()
        .filter_map(|v| v["url"].as_str().map(str::to_string))
        .collect();
    let unique_records: HashSet<(String, String)> = lines.iter().map(record_identity).collect();

    assert_eq!(
        lines.len(),
        2,
        "--batch must scrape exactly the 2 seed URLs (got {} export.jsonl lines)",
        lines.len()
    );
    assert_eq!(
        unique_urls.len(),
        2,
        "export.jsonl must hold 2 unique URLs, got {unique_urls:?}"
    );
    assert_eq!(
        unique_records.len(),
        lines.len(),
        "export.jsonl must hold no duplicate (url, content_hash) records"
    );

    // The BFS expansion fetched /other; a scrape-per-URL run must never touch it.
    let requests = t.server.received_requests().await.unwrap();
    let other_hits = requests.iter().filter(|r| r.url.path() == "/other").count();
    assert_eq!(
        other_hits, 0,
        "--batch must not crawl links discovered on seed pages (/other was fetched {other_hits}x)"
    );
}

/// `--batch --single-page` must equal per-URL `--single-page` scrapes: every
/// .md file byte-identical, export.jsonl identical modulo the per-run
/// `timestamp_utc` (a run-time instant by design, proven to be the ONLY
/// divergence between two identical back-to-back runs).
#[tokio::test]
async fn batch_single_page_matches_per_url_single_page() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    let uri = t.server.uri();
    let urls = [format!("{uri}/article"), format!("{uri}/article/2")];

    // Batch run over both seeds with --single-page.
    cmd()
        .arg("--batch")
        .arg("--single-page")
        .arg("--output")
        .arg(t.out.path())
        .arg("--no-checkpoint")
        .arg("--delay-ms")
        .arg("0")
        .write_stdin(format!("{}\n{}\n", urls[0], urls[1]))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();
    let batch_md = md_files_by_relative_path(t.out.path());
    let batch_jsonl = read_jsonl_lines(t.out.path());

    // Per-URL --single-page reference runs, one output dir each.
    let ref1 = TempDir::new().unwrap();
    let ref2 = TempDir::new().unwrap();
    for (url, dir) in [(&urls[0], &ref1), (&urls[1], &ref2)] {
        cmd()
            .arg("--url")
            .arg(url)
            .arg("--single-page")
            .arg("--output")
            .arg(dir.path())
            .arg("--no-checkpoint")
            .arg("--delay-ms")
            .arg("0")
            .timeout(Duration::from_secs(60))
            .assert()
            .success();
    }
    let mut reference_md = md_files_by_relative_path(ref1.path());
    reference_md.extend(md_files_by_relative_path(ref2.path()));
    let mut reference_records: Vec<(String, String)> = Vec::new();
    for dir in [ref1.path(), ref2.path()] {
        reference_records.extend(read_jsonl_lines(dir).iter().map(record_identity));
    }

    assert_eq!(
        batch_md, reference_md,
        "--batch --single-page .md files must be byte-identical to per-URL --single-page"
    );

    let mut batch_records: Vec<(String, String)> =
        batch_jsonl.iter().map(record_identity).collect();
    batch_records.sort();
    reference_records.sort();
    assert_eq!(
        batch_records, reference_records,
        "--batch --single-page export.jsonl must hold the same (url, content_hash) \
         records as the per-URL --single-page runs"
    );
}

/// `--batch --max-pages 1` must still scrape EVERY seed (#1215: the report of
/// seeds silently dropped with exit 0). In scrape-per-URL mode the crawl
/// budget never gates seed fetching — each seed is one page by construction.
#[tokio::test]
async fn batch_max_pages_one_still_scrapes_every_seed() {
    let t = BehavioralTest::new().await;
    mount_article_graph(&t.server).await;
    let uri = t.server.uri();

    cmd()
        .arg("--batch")
        .arg("--max-pages")
        .arg("1")
        .arg("--output")
        .arg(t.out.path())
        .arg("--no-checkpoint")
        .arg("--delay-ms")
        .arg("0")
        .write_stdin(format!("{uri}/article\n{uri}/article/2\n"))
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let lines = read_jsonl_lines(t.out.path());
    let unique_urls: HashSet<String> = lines
        .iter()
        .filter_map(|v| v["url"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "--max-pages 1 must not starve batch seeds (got {} lines)",
        lines.len()
    );
    assert_eq!(unique_urls.len(), 2, "both seeds must be scraped");
}
