//! Security: decompression-bomb containment through the CLI sitemap flow (batch 2).
//!
//! A hostile server can serve highly-compressible payloads that expand far
//! beyond their wire size. Every decompression path MUST be bounded:
//!
//! - **gzip bomb**: `gzip(gzip(120 MiB of zeros))` — small on the wire (well
//!   under the 50 MiB streamed raw-response cap), expands past the 100 MiB
//!   decompression cap. Double-wrapped so exactly one layer may be stripped by
//!   the HTTP transport while the remaining layer still exercises the
//!   handler's `take(max)` cap. Must exit with the typed failure code (69),
//!   never panic or OOM.
//! - **zstd bomb**: same shape via zstd magic bytes (`28 b5 2f fd`). The
//!   transport has no zstd auto-decoding, so the single wrapped layer reaches
//!   the handler directly.
//! - **brotli bomb**: brotli cannot be sniffed; the `.br` extension is the
//!   detection hint and must still be size-capped.
//! - **triple-layer gzip**: the handler decompresses at most TWO layers
//!   (#757 contract). Triple wrapping must terminate deterministically — no
//!   loop-decompression.
//! - **lying extension** (#757): a plain-text body behind a `.zst` URL passes
//!   through untouched and parses as a valid sitemap end-to-end.
//!
//! All scenarios run end-to-end through the `webfang` binary against a
//! wiremock server (the compression handler is `pub(crate)`).
//!
//! Run with: `cargo nextest run --test security_bombs_test`

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, BehavioralTest};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Article HTML comfortably over the 50-char minimum content guard.
const ARTICLE_HTML: &str = "<html><body><article>\
         <h1>Bomb Probe</h1>\
         <p>Substantive content from a sitemap-listed page, long enough to clear \
         the fifty character minimum content guard comfortably.</p>\
         </article></body></html>";

fn urlset_with(locs: &[String]) -> String {
    let urls: String = locs
        .iter()
        .map(|loc| format!("<url><loc>{loc}</loc></url>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>"#
    )
}

/// Decompressed payload size for bombs: past the 100 MiB decompression cap,
/// but the COMPRESSED body stays a few hundred KiB — far below the 50 MiB
/// streamed raw-response cap, so only the decompression limit can fire.
const BOMB_EXPANDED_BYTES: usize = 120 * 1024 * 1024;

/// Zero-allocation stand-in for `vec![0u8; BOMB_EXPANDED_BYTES]`.
///
/// Emits `remaining` zero bytes as an [`tokio::io::AsyncRead`], so the
/// 120 MiB expanded payload is never materialized in memory — the encoder
/// consumes it incrementally through a `BufReader`. Zero bytes compress
/// identically to the old `Vec` approach, so wire-size assertions hold.
struct ZerosReader {
    remaining: usize,
}

impl ZerosReader {
    fn new(total: usize) -> Self {
        Self { remaining: total }
    }
}

impl tokio::io::AsyncRead for ZerosReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let n = buf.remaining().min(self.remaining);
        if n > 0 {
            buf.initialize_unfilled()[..n].fill(0);
            buf.advance(n);
            self.remaining -= n;
        }
        // EOF (Ok with no fill) once the logical payload is exhausted.
        std::task::Poll::Ready(Ok(()))
    }
}

/// Async reader over an in-memory slice (small payloads: XML sitemaps,
/// intermediate compression layers). tokio has no `AsyncRead for &[u8]`.
struct SliceReader<'a> {
    data: &'a [u8],
}

impl<'a> SliceReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl tokio::io::AsyncRead for SliceReader<'_> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let n = buf.remaining().min(self.data.len());
        if n > 0 {
            buf.put_slice(&self.data[..n]);
            self.data = &self.data[n..];
        }
        std::task::Poll::Ready(Ok(()))
    }
}

async fn gzip_compress(data: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    use async_compression::tokio::bufread::GzipEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = GzipEncoder::new(BufReader::new(data));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.unwrap();
    out
}

async fn zstd_compress(data: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    use async_compression::tokio::bufread::ZstdEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = ZstdEncoder::new(BufReader::new(data));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.unwrap();
    out
}

async fn brotli_compress(data: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    use async_compression::tokio::bufread::BrotliEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = BrotliEncoder::new(BufReader::new(data));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.unwrap();
    out
}

async fn mock_sitemap_bytes(server: &MockServer, sitemap_path: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(sitemap_path))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(server)
        .await;
}

/// Assert the typed-failure contract for a bomb response: exit 69
/// (EX_UNAVAILABLE — fetch/parse failure family), no panic, bounded stderr.
fn assert_typed_bomb_failure(result: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(
        result.status.code(),
        Some(69),
        "a decompression bomb must fail with the typed error exit 69, got {:?}; stderr:\n{stderr}",
        result.status.code()
    );
    assert!(
        !stderr.contains("panicked"),
        "bomb handling must never panic; stderr:\n{stderr}"
    );
}

/// Snapshot stderr with nondeterministic content redacted (wiremock port,
/// temp-dir path, timestamps) so the snapshot is stable across runs and
/// machines while still pinning the exact typed failure message.
fn assert_snapshot_redacted(name: &str, dir: &std::path::Path, value: impl Into<String>) {
    let redacted = common::redact_nondeterministic(dir, &value.into());
    insta::assert_snapshot!(name, redacted);
}

// ---------------------------------------------------------------------------
// 1. gzip bomb via the sitemap flow → typed failure, no crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gzip_bomb_via_sitemap_fails_typed_not_crash() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;

    // Double-gzip so whichever single layer the HTTP transport strips, the
    // handler still sees a gzip layer whose expansion is capped at 100 MiB.
    // Streamed zeros: no 120 MiB intermediate allocation.
    let inner = gzip_compress(ZerosReader::new(BOMB_EXPANDED_BYTES)).await;
    let bomb = gzip_compress(SliceReader::new(&inner)).await;
    assert!(
        bomb.len() < 50 * 1024 * 1024,
        "wire body must stay under the raw-response cap"
    );
    mock_sitemap_bytes(server, "/sitemap.xml.gz", bomb).await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml.gz", server.uri()))
        .arg("--output")
        .arg(harness.out.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_typed_bomb_failure(&result);

    // The exact typed size-cap failure message is pinned via snapshot
    // (ports/temp paths redacted) instead of brittle substring matching;
    // this proves the failure comes from the decompression cap itself (the
    // handler's `take(max)` + post-check), not from a downstream XML-parse
    // error on an already-expanded payload.
    assert_snapshot_redacted(
        "gzip_bomb_size_cap_stderr",
        harness.out.path(),
        String::from_utf8_lossy(&result.stderr),
    );
}

// ---------------------------------------------------------------------------
// 2. zstd bomb via magic bytes → typed failure, no crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn zstd_bomb_via_sitemap_fails_typed_not_crash() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;

    // wreq has no zstd transport decoding: this layer reaches the handler.
    let bomb = zstd_compress(ZerosReader::new(BOMB_EXPANDED_BYTES)).await;
    assert!(bomb.first() == Some(&0x28), "zstd magic bytes expected");
    mock_sitemap_bytes(server, "/sitemap.xml.zst", bomb).await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml.zst", server.uri()))
        .arg("--output")
        .arg(harness.out.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_typed_bomb_failure(&result);
}

// ---------------------------------------------------------------------------
// 3. brotli bomb via extension hint → typed failure, no crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brotli_bomb_via_sitemap_fails_typed_not_crash() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;

    // Brotli is not sniffable: the .br extension hint drives detection, and
    // that decoder path must be size-capped too.
    let bomb = brotli_compress(ZerosReader::new(BOMB_EXPANDED_BYTES)).await;
    mock_sitemap_bytes(server, "/sitemap.xml.br", bomb).await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml.br", server.uri()))
        .arg("--output")
        .arg(harness.out.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_typed_bomb_failure(&result);
}

// ---------------------------------------------------------------------------
// 4. triple-layer gzip: bounded at two layers, terminates deterministically
// ---------------------------------------------------------------------------

/// Production contract: at most TWO decompression layers in the handler
/// (#757). Triple wrapping must therefore NOT yield a successful parse of the
/// innermost urlset through decompression alone — the run must terminate with
/// the typed parse failure instead of looping or crashing.
///
/// Note the existing double-gzip SUCCESS test (`sitemap_exit_code_test.rs`)
/// stays valid: two layers remain inside the bounded contract.
#[tokio::test]
async fn triple_layer_gzip_terminates_without_loop_decompression() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;

    let page_url = format!("{}/article", server.uri());
    let xml = urlset_with(&[page_url]);
    let l1 = gzip_compress(SliceReader::new(xml.as_bytes())).await;
    let l2 = gzip_compress(SliceReader::new(&l1)).await;
    let l3 = gzip_compress(SliceReader::new(&l2)).await;

    mock_sitemap_bytes(server, "/sitemap.xml.gz", l3).await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml.gz", server.uri()))
        .arg("--output")
        .arg(harness.out.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_ne!(
        result.status.code(),
        Some(0),
        "triple-layer gzip must not be fully unwrapped by the 2-layer handler; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "bounded layered decompression must never panic; stderr:\n{stderr}"
    );
    // Deterministic typed termination: the leftover compressed layer reaches
    // the XML parser and fails as a parse error (exit 69 family), never an
    // unbounded hang or crash.
    assert_eq!(
        result.status.code(),
        Some(69),
        "expected the typed parse-failure exit code, got {:?}; stderr:\n{stderr}",
        result.status.code()
    );

    // Pin the typed parse-failure envelope via snapshot (redacted). The
    // leftover compressed stream can terminate through EITHER typed error
    // family depending on the garbage bytes that reach the XML parser —
    // and those bytes embed the wiremock URL (per-run port), so the cut
    // point is not reproducible across runs (issue #926):
    //   - quick-xml syntax failure: "XML parsing failed: <detail>"
    //   - structural rejection:      "invalid sitemap structure"
    // Both families normalize to the same <PARSE_FAILURE> placeholder so
    // one snapshot covers both; the exit code, source location, and
    // Spanish user-facing wrapper stay pinned above.
    let stderr = String::from_utf8_lossy(&result.stderr);
    let normalized = stderr
        .lines()
        .map(|line| {
            if let Some(i) = line.find("XML parsing failed:") {
                format!("{}<PARSE_FAILURE>", &line[..i])
            } else if let Some(i) = line.find("invalid sitemap structure") {
                format!("{}<PARSE_FAILURE>", &line[..i])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot_redacted(
        "triple_layer_gzip_parse_failure_stderr",
        harness.out.path(),
        normalized,
    );
}

// ---------------------------------------------------------------------------
// 5. lying extension pass-through (#757): plain body behind .zst parses fine
// ---------------------------------------------------------------------------

/// A plain-text (uncompressed) sitemap served from a `.zst` URL passes through
/// the compression handler untouched — no zstd decode attempt on non-magic
/// bytes — and the crawl completes successfully.
#[tokio::test]
async fn lying_zst_extension_on_plain_body_passes_through_and_parses() {
    let harness = BehavioralTest::new().await;
    let server = &harness.server;

    let page_url = format!("{}/article", server.uri());
    Mock::given(method("GET"))
        .and(path("/sitemap.xml.zst"))
        .respond_with(
            // Raw bytes avoid wiremock's implicit `text/plain` from
            // set_body_string; the explicit XML content-type satisfies the
            // sitemap fetch validation so ONLY the compression pass-through
            // behavior is under test.
            ResponseTemplate::new(200)
                .set_body_bytes(urlset_with(&[page_url]).into_bytes())
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml.zst", server.uri()))
        .arg("--output")
        .arg(harness.out.path())
        .arg("--quiet")
        .output()
        .expect("run webfang");

    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(
        result.status.code(),
        Some(0),
        "a plain sitemap behind a lying .zst extension must pass through and succeed; stderr:\n{stderr}"
    );

    let md = harness.find_files("md");
    assert!(
        !md.is_empty(),
        "the crawl should have scraped the listed article page"
    );
}
