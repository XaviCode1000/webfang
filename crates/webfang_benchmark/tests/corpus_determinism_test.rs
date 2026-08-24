//! AC-1.1 / AC-1.2 — Tier A corpus determinism and local-only guarantees.
//!
//! Requests use raw one-shot TCP HTTP/1.1 reads so the test observes the exact
//! wiremock responses without any HTTP-client stack (no reqwest anywhere;
//! core's own HttpClient would auto-retry 403/429 and mask the sequence).
//! Run with: cargo nextest run -p webfang_benchmark corpus_determinism_test

use std::io::{Read, Write};
use std::net::TcpStream;

use webfang_benchmark::corpus;

/// One raw HTTP/1.1 GET over a fresh connection; returns (status_code, full_header_block).
fn raw_get(base_url: &str, path: &str) -> (u16, String) {
    let url = url::Url::parse(&format!("{base_url}{path}")).expect("test url parses");
    let host = url.host_str().expect("host");
    let port = url.port().expect("port");
    let mut stream =
        TcpStream::connect((host, port)).expect("test: corpus server must be connectable");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .expect("test: write request");
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .expect("test: read response until close");
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status_line = text.lines().next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

fn header_value<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim())
}

/// AC-1.1 — a freshly served corpus answers its WAF-guarded URL with exactly
/// 403 → 429 (with Retry-After) → 200, every time, from every fresh sequence.
#[tokio::test]
async fn waf_sequence_is_deterministic_across_fresh_serves() {
    for _ in 0..3 {
        let handle = corpus::serve().await.expect("corpus serve");
        let gate = &handle.manifest.waf_guarded_path;

        let (first, _) = raw_get(&handle.base_url, gate);
        let (second, second_raw) = raw_get(&handle.base_url, gate);
        let (third, _) = raw_get(&handle.base_url, gate);

        assert_eq!(first, 403, "first request must be 403");
        assert_eq!(second, 429, "second request must be 429");
        assert_eq!(
            header_value(&second_raw, "Retry-After"),
            Some("0"),
            "429 must carry Retry-After"
        );
        assert_eq!(third, 200, "third request must be 200");
    }
}

/// AC-1.2 groundwork — the manifest references only loopback: no external
/// hosts may appear (CI-safe, no network dependency).
#[tokio::test]
async fn manifest_contains_only_local_hosts() {
    let handle = corpus::serve().await.expect("corpus serve");
    assert!(
        handle.base_url.starts_with("http://127.0.0.1"),
        "corpus must bind loopback only, got: {}",
        handle.base_url
    );
    for page in &handle.manifest.pages {
        assert!(
            page.path.starts_with('/'),
            "manifest paths must be relative: {}",
            page.path
        );
    }
}
