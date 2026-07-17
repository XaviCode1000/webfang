//! Resource Downloader with Byte-Weighted Semaphore and Backpressure
//!
//! Implements elastic resource downloading with:
//! - Byte-weighted semaphore for memory backpressure
//! - PermitGuard for RAII permit management
//! - 25MB hard limit with chunked encoding support
//! - Concentric timeouts: 30s global, 5s per chunk (Anti-Slowloris)

use crate::error::ScraperError;
use bytes::BytesMut;
use futures::StreamExt;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use wreq::Client;

/// RAII guard for semaphore permits.
///
/// Ensures permits are released back to the semaphore even on panic,
/// preventing permit starvation in the pool.
#[derive(Debug)]
pub struct PermitGuard {
    semaphore: Arc<tokio::sync::Semaphore>,
    units_acquired: u64,
}

impl PermitGuard {
    /// Creates a new PermitGuard with the given semaphore and initial units.
    #[must_use]
    pub fn new(semaphore: Arc<tokio::sync::Semaphore>, initial_units: u64) -> Self {
        Self {
            semaphore,
            units_acquired: initial_units,
        }
    }

    /// Updates the guard with additional units acquired.
    pub fn update_reservation(&mut self, additional_units: u64) {
        self.units_acquired = self.units_acquired.saturating_add(additional_units);
    }

    /// Returns the current number of units held by this guard.
    #[must_use]
    pub fn units_acquired(&self) -> u64 {
        self.units_acquired
    }
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        if self.units_acquired > 0 {
            self.semaphore.add_permits(self.units_acquired as usize);
        }
    }
}

/// Configuration for resource downloading.
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Maximum total size in bytes (default: 25MB)
    pub max_size_bytes: u64,
    /// Global download timeout in seconds
    pub global_timeout_seconds: u64,
    /// Per-chunk timeout in seconds (Anti-Slowloris)
    pub chunk_timeout_seconds: u64,
    /// Initial permit size in bytes (4KB)
    pub initial_permit_bytes: u64,
    /// Chunk size for permit acquisition (4KB)
    pub chunk_size_bytes: u64,
    /// Buffer pre-allocation size in bytes
    pub buffer_capacity_bytes: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 25 * 1024 * 1024, // 25MB
            global_timeout_seconds: 30,
            chunk_timeout_seconds: 5,
            initial_permit_bytes: 4096,       // 4KB
            chunk_size_bytes: 4096,           // 4KB
            buffer_capacity_bytes: 16 * 1024, // 16KB
        }
    }
}

/// Result of a successful resource download.
#[derive(Debug, Clone)]
pub struct DownloadedResource {
    /// The URL that was downloaded
    pub url: String,
    /// The raw bytes of the resource
    pub bytes: Vec<u8>,
    /// Content-Type header if available
    pub content_type: Option<String>,
    /// Total size in bytes
    pub size_bytes: u64,
}

/// Resource downloader with byte-weighted semaphore backpressure.
///
/// Implements elastic resource downloading with memory backpressure
/// via a byte-weighted semaphore and Anti-Slowloris protection.
pub struct ResourceDownloader {
    semaphore: Arc<tokio::sync::Semaphore>,
    client: Client,
    config: DownloadConfig,
}

impl ResourceDownloader {
    /// Creates a new ResourceDownloader with the given semaphore and client.
    pub fn new(semaphore: Arc<tokio::sync::Semaphore>, client: Client) -> Self {
        Self {
            semaphore,
            client,
            config: DownloadConfig::default(),
        }
    }

    /// Creates a new ResourceDownloader with custom config.
    pub fn with_config(
        semaphore: Arc<tokio::sync::Semaphore>,
        client: Client,
        config: DownloadConfig,
    ) -> Self {
        Self {
            semaphore,
            client,
            config,
        }
    }

    /// Downloads a resource with byte-weighted semaphore backpressure.
    ///
    /// Permit lifecycle (spec: "Byte-Weighted Semaphore" + "Chunked Encoding
    /// Protection" + "Hard Resource Size Limit"):
    /// - No permits are acquired until response headers arrive, so a
    ///   `Content-Length` rejection never holds any permits.
    /// - Each streamed chunk acquires its own byte-weighted permits *before*
    ///   being buffered. Permits are `forget()`ten so the `PermitGuard` owns
    ///   the release, guaranteeing `permits released == permits acquired`
    ///   (no double-release, no permit inflation).
    ///
    /// # Errors
    /// Returns `ScraperError` if:
    /// - Global timeout exceeded (`GlobalTimeout`)
    /// - Per-chunk timeout exceeded — Anti-Slowloris (`SlowlorisTimeout`)
    /// - `Content-Length` or accumulated size exceeds the limit (`PayloadTooLarge`)
    /// - Network error occurs (`Network`)
    /// - Semaphore acquisition fails (`SemaphoreInanition`)
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ScraperError> {
        // Global timeout wraps the request (headers). Permits are acquired only
        // AFTER headers, so oversized responses are rejected permit-free.
        let response = timeout(
            Duration::from_secs(self.config.global_timeout_seconds),
            self.client.get(url).send(),
        )
        .await
        .map_err(|_| {
            tracing::error!(%url, reason = "global_timeout", "descarga abortada");
            ScraperError::GlobalTimeout
        })?
        .map_err(|e| {
            tracing::error!(%url, error = %e, "error de red al iniciar descarga");
            ScraperError::from(e)
        })?;

        let _content_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Hard Resource Size Limit: reject oversized resources BEFORE acquiring
        // any permits (spec: "no permits MUST be acquired").
        let content_length = response
            .headers()
            .get(wreq::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(cl) = content_length {
            if cl > self.config.max_size_bytes {
                tracing::error!(
                    %url,
                    content_length = cl,
                    max = self.config.max_size_bytes,
                    reason = "payload_too_large",
                    "recurso supera límite de tamaño: rechazado sin adquirir permisos"
                );
                return Err(ScraperError::PayloadTooLarge);
            }
        }

        // Byte-weighted semaphore: every chunk acquires its own permits, which
        // are `forget()`ten so the PermitGuard owns the release on Drop.
        // Invariant: permits released on Drop == permits acquired mid-download.
        let mut guard = PermitGuard::new(Arc::clone(&self.semaphore), 0);
        let mut stream = response.bytes_stream();
        let mut buffer = BytesMut::with_capacity(self.config.buffer_capacity_bytes);
        let mut total_bytes: u64 = 0;

        // Process stream with per-chunk timeout (Anti-Slowloris: 5s per chunk).
        while let Some(chunk_result) = timeout(
            Duration::from_secs(self.config.chunk_timeout_seconds),
            stream.next(),
        )
        .await
        .map_err(|_| {
            tracing::error!(
                %url,
                bytes = total_bytes,
                reason = "slowloris_timeout",
                "descarga abortada: timeout de inactividad por chunk"
            );
            ScraperError::SlowlorisTimeout
        })? {
            let chunk = chunk_result.map_err(|e| {
                tracing::error!(
                    %url,
                    bytes = total_bytes,
                    error = %e,
                    "error de red durante la descarga"
                );
                ScraperError::from(e)
            })?;
            let chunk_len = chunk.len() as u64;

            // OOM prevention: abort when accumulated size exceeds the configured
            // hard limit (default 25 MB; spec: "SHOULD be configurable").
            if total_bytes + chunk_len > self.config.max_size_bytes {
                tracing::error!(
                    %url,
                    bytes = total_bytes + chunk_len,
                    max = self.config.max_size_bytes,
                    reason = "chunked_limit_exceeded",
                    "descarga abortada: supera límite de tamaño"
                );
                return Err(ScraperError::PayloadTooLarge);
            }

            // Acquire byte-weighted permits for THIS chunk before buffering it.
            // `forget()` consumes the permit (no auto-release on Drop); the
            // PermitGuard tracks the total and releases exactly this many on
            // its own Drop — preventing the double-release / permit inflation.
            //
            // `chunk_len as u32` is safe: a network chunk is frame-sized (KB),
            // and we just verified `total_bytes + chunk_len <= max_size_bytes`
            // (≤ 25 MB), far below `u32::MAX`.
            if chunk_len > 0 {
                self.semaphore
                    .acquire_many(chunk_len as u32)
                    .await
                    .map_err(|_| {
                        tracing::error!(
                            %url,
                            bytes = total_bytes,
                            reason = "semaphore_inanition",
                            "semáforo agotado: no hay permisos disponibles"
                        );
                        ScraperError::SemaphoreInanition
                    })?
                    .forget();
                guard.update_reservation(chunk_len);
            }

            total_bytes += chunk_len;
            buffer.extend_from_slice(&chunk);
        }

        Ok(buffer.to_vec())
    }
}

#[cfg(test)]
#[cfg_attr(miri, ignore)] // tokio::time::timeout + spawn_blocking hang under Miri (entire module)
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    #[test]
    fn test_permit_guard_new() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(100));
        let guard = PermitGuard::new(Arc::clone(&semaphore), 100);
        assert_eq!(guard.units_acquired(), 100);
    }

    #[test]
    fn test_permit_guard_update() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(100));
        let mut guard = PermitGuard::new(Arc::clone(&semaphore), 100);
        guard.update_reservation(50);
        assert_eq!(guard.units_acquired(), 150);
    }

    #[test]
    fn test_permit_guard_drop() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(0));
        {
            let _guard = PermitGuard::new(Arc::clone(&semaphore), 50);
        }
        // Permits should be returned on drop
        assert_eq!(semaphore.available_permits(), 50);
    }

    #[test]
    fn test_download_config_default() {
        let config = DownloadConfig::default();
        assert_eq!(config.max_size_bytes, 25 * 1024 * 1024);
        assert_eq!(config.global_timeout_seconds, 30);
        assert_eq!(config.chunk_timeout_seconds, 5);
        assert_eq!(config.initial_permit_bytes, 4096);
        assert_eq!(config.chunk_size_bytes, 4096);
        assert_eq!(config.buffer_capacity_bytes, 16 * 1024);
    }

    // ========================================================================
    // Task 2.5 — behavioral tests (wiremock + raw-socket Slowloris server)
    //
    // Short timeouts are used in every config to keep the suite under a few
    // seconds and to avoid the 30s/5s production defaults.
    // ========================================================================

    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Builds a downloader wired to a large semaphore (no backpressure) and
    /// a fresh `wreq` client.
    fn downloader(config: DownloadConfig) -> ResourceDownloader {
        let client = wreq::Client::builder()
            .build()
            .expect("fallo construyendo cliente wreq de prueba");
        let semaphore = Arc::new(Semaphore::new(1 << 20)); // 1 MiB de permisos
        ResourceDownloader::with_config(semaphore, client, config)
    }

    /// Spawns a raw HTTP/1.1 server that sends a `Transfer-Encoding: chunked`
    /// response and then stalls between chunks for `inter_chunk_delay`.
    ///
    /// wiremock 0.6 only supports buffered bodies + a single response delay, so
    /// it cannot reproduce a per-chunk stall. A raw socket gives full control
    /// and deterministically triggers `SlowlorisTimeout` via the per-chunk
    /// `timeout` wrapping `stream.next()`.
    async fn spawn_slow_chunked_server(inter_chunk_delay: Duration) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind slowloris server");
        let port = listener
            .local_addr()
            .expect("local_addr slowloris server")
            .port();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                tokio::spawn(async move {
                    // Consume request headers (read until the terminating \r\n\r\n).
                    let mut buf = vec![0u8; 1024];
                    let mut filled = 0usize;
                    loop {
                        match sock.read(&mut buf[filled..]).await {
                            Ok(0) => return,
                            Ok(n) => {
                                filled += n;
                                if filled >= 4 && &buf[filled - 4..filled] == b"\r\n\r\n" {
                                    break;
                                }
                                if filled == buf.len() {
                                    buf.resize(buf.len() * 2, 0);
                                }
                            },
                            Err(_) => return,
                        }
                    }

                    // Chunked response: headers + one chunk ("hello"), then stall.
                    let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(head).await;
                    let _ = sock.write_all(b"5\r\nhello\r\n").await;
                    // Stall longer than the configured per-chunk timeout.
                    tokio::time::sleep(inter_chunk_delay).await;
                    // Terminating chunk (usually never reached: client times out first).
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                });
            }
        });
        port
    }

    /// Spawns a raw HTTP/1.1 chunked server that sends each provided chunk as a
    /// proper `Transfer-Encoding: chunked` frame, then waits `delay` before the
    /// next. After the last chunk it sends the terminating `0\r\n\r\n` frame.
    ///
    /// Unlike `spawn_slow_chunked_server`, every chunk is delivered promptly;
    /// only the *gap between* chunks is delayed. This lets a downloader acquire
    /// per-chunk permits and hold them mid-stream — the precondition needed to
    /// observe byte-weighted backpressure deterministically.
    async fn spawn_multi_chunk_server(chunks: Vec<(Vec<u8>, Duration)>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind multi-chunk server");
        let port = listener
            .local_addr()
            .expect("local_addr multi-chunk server")
            .port();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let parts = chunks.clone();
                tokio::spawn(async move {
                    // Consume request headers (read until the terminating \r\n\r\n).
                    let mut buf = vec![0u8; 1024];
                    let mut filled = 0usize;
                    loop {
                        match sock.read(&mut buf[filled..]).await {
                            Ok(0) => return,
                            Ok(n) => {
                                filled += n;
                                if filled >= 4 && &buf[filled - 4..filled] == b"\r\n\r\n" {
                                    break;
                                }
                                if filled == buf.len() {
                                    buf.resize(buf.len() * 2, 0);
                                }
                            },
                            Err(_) => return,
                        }
                    }

                    let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(head).await;
                    for (data, delay) in parts {
                        // Chunked frame: `<hexlen>\r\n<data>\r\n`.
                        let frame = format!("{:X}\r\n", data.len());
                        let _ = sock.write_all(frame.as_bytes()).await;
                        let _ = sock.write_all(&data).await;
                        let _ = sock.write_all(b"\r\n").await;
                        tokio::time::sleep(delay).await;
                    }
                    // Terminating chunk.
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                });
            }
        });
        port
    }

    /// Normal small download returns the exact bytes.
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_download_normal() {
        let mock_server = MockServer::start().await;
        let body = b"<html><body>hola</body></html>";
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&mock_server)
            .await;

        let config = DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        };
        let downloader = downloader(config);
        let url = format!("{}/", mock_server.uri());

        let bytes = downloader
            .download(&url)
            .await
            .expect("descarga normal falló");
        assert_eq!(bytes, body);
    }

    /// Per-chunk stall beyond `chunk_timeout_seconds` → `SlowlorisTimeout`.
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_download_slowloris_chunk_timeout() {
        let config = DownloadConfig {
            // Generous global budget so the GLOBAL timeout cannot fire first.
            global_timeout_seconds: 30,
            chunk_timeout_seconds: 1, // short per-chunk timeout
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        };
        // Server stalls 3s between chunks (> 1s chunk timeout).
        let port = spawn_slow_chunked_server(Duration::from_secs(3)).await;
        let url = format!("http://127.0.0.1:{port}/");
        let downloader = downloader(config);

        let result = downloader.download(&url).await;
        assert!(
            matches!(result, Err(ScraperError::SlowlorisTimeout)),
            "esperaba SlowlorisTimeout, obtuve: {result:?}"
        );
    }

    /// Accumulated bytes beyond `max_size_bytes` → `PayloadTooLarge`.
    ///
    /// Uses a tiny configured limit (1 KiB) instead of allocating 26 MB, which
    /// keeps the test fast and memory-light. This exercises the same abort
    /// path as the 25 MB production default.
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_download_payload_too_large() {
        let mock_server = MockServer::start().await;
        // 2 KiB body > 1 KiB configured limit.
        let body = vec![0u8; 2 * 1024];
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&mock_server)
            .await;

        let config = DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024, // small limit
            ..DownloadConfig::default()
        };
        let downloader = downloader(config);
        let url = format!("{}/", mock_server.uri());

        let result = downloader.download(&url).await;
        assert!(
            matches!(result, Err(ScraperError::PayloadTooLarge)),
            "esperaba PayloadTooLarge, obtuve: {result:?}"
        );
    }

    /// Initial response delayed beyond `global_timeout_seconds` → `GlobalTimeout`.
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_download_global_timeout() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("lento")
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&mock_server)
            .await;

        let config = DownloadConfig {
            global_timeout_seconds: 1, // fires before the 2s server delay
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        };
        let downloader = downloader(config);
        let url = format!("{}/", mock_server.uri());

        let result = downloader.download(&url).await;
        assert!(
            matches!(result, Err(ScraperError::GlobalTimeout)),
            "esperaba GlobalTimeout, obtuve: {result:?}"
        );
    }

    /// RED (TDD): proves the double-release / missing per-chunk acquire bug.
    ///
    /// After a download completes, the semaphore MUST conserve permits exactly
    /// (`available == budget`). The buggy implementation acquires only an initial
    /// 4 KB permit and releases `initial + Σchunks` on Drop — releasing permits
    /// that were never acquired and inflating the available count ABOVE the
    /// budget. This assertion fails on the buggy code (RED) and passes once
    /// per-chunk byte-weighted acquire is implemented (GREEN).
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_no_permit_inflation_after_download() {
        let mock_server = MockServer::start().await;
        // 8 KiB body: large enough to span multiple wreq frames so per-chunk
        // acquire is exercised (a single chunk would still expose the bug).
        let body = vec![0x42u8; 8 * 1024];
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&mock_server)
            .await;

        let config = DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        };
        // Budget exactly equal to the body size: the download must acquire (and
        // later release) precisely `body.len()` byte-weighted permits.
        let budget = body.len();
        let semaphore = Arc::new(Semaphore::new(budget));
        let client = wreq::Client::builder()
            .build()
            .expect("fallo construyendo cliente wreq de prueba");
        let downloader = ResourceDownloader::with_config(semaphore.clone(), client, config);
        let url = format!("{}/", mock_server.uri());

        let bytes = downloader
            .download(&url)
            .await
            .expect("la descarga debió completarse");
        assert_eq!(bytes, body);

        // Invariant: permits released on Drop == permits acquired mid-download.
        assert_eq!(
            semaphore.available_permits(),
            budget,
            "inflación de permisos: available {} != budget {} \
             (bug: double-release del permiso inicial + adquisición por chunk ausente)",
            semaphore.available_permits(),
            budget
        );
    }

    /// Per-chunk byte-weighted backpressure: while a multi-chunk download is in
    /// flight, it holds permits proportional to the bytes received so far, and a
    /// concurrent acquire that would exceed the budget blocks.
    ///
    /// This replaces the former `test_semaphore_backpressure`, which only proved
    /// the *initial* 4 KB acquire blocked. After the per-chunk fix that test
    /// would pass for the wrong reason (response-delay timeout, not backpressure),
    /// so it is superseded by this streaming-based assertion.
    #[cfg_attr(miri, ignore)] // tokio::time::sleep hangs under Miri (time-driver does not advance)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_per_chunk_backpressure_holds_permits_mid_stream() {
        // Two 4 KB chunks with a 500 ms gap between them: after the first chunk
        // the downloader holds 4 KB of permits while waiting for the second.
        let chunk = vec![0x21u8; 4 * 1024];
        let port = spawn_multi_chunk_server(vec![
            (chunk.clone(), Duration::from_millis(500)),
            (chunk.clone(), Duration::from_millis(50)),
        ])
        .await;
        let url = format!("http://127.0.0.1:{port}/");

        let config = DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        };
        // Budget exactly fits the two chunks (8 KB); the in-flight download must
        // hold permits drawn from this shared budget.
        let budget = 8 * 1024usize;
        let semaphore = Arc::new(Semaphore::new(budget));
        let client = wreq::Client::builder()
            .build()
            .expect("fallo construyendo cliente wreq de prueba");
        let downloader = Arc::new(ResourceDownloader::with_config(
            Arc::clone(&semaphore),
            client,
            config,
        ));

        let first = Arc::clone(&downloader);
        let first_url = url.clone();
        let handle = tokio::spawn(async move { first.download(&first_url).await });

        // After chunk 1 arrives the downloader holds 4 KB while it waits 500 ms
        // for chunk 2 — available must drop below the budget. This proves
        // per-chunk acquire actually happens (not just bookkeeping).
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mid = semaphore.available_permits();
        assert!(
            mid < budget,
            "no hay permisos retenidos mid-stream: available {mid} >= budget {budget}"
        );

        // Backpressure: a concurrent full-budget acquire cannot complete while
        // the in-flight download holds permits (only `budget - mid` available).
        // `acquire_many` is cancellation-safe, so dropping the timed-out future
        // releases no permits and never starves the semaphore.
        let blocked = tokio::time::timeout(
            Duration::from_millis(200),
            semaphore.acquire_many(budget as u32),
        )
        .await;
        assert!(
            blocked.is_err(),
            "el semáforo debió aplicar backpressure a una adquisición concurrente"
        );

        let result = handle.await.expect("join primera descarga");
        assert!(
            result.is_ok(),
            "la primera descarga debió completarse: {result:?}"
        );

        // Conservation: all permits returned after completion (no leak, no inflation).
        assert_eq!(
            semaphore.available_permits(),
            budget,
            "pérdida o inflación de permisos tras completar"
        );
    }

    /// Content-Length pre-check (spec: "Hard Resource Size Limit"): a response
    /// whose `Content-Length` exceeds the limit is rejected with
    /// `PayloadTooLarge` BEFORE any permits are acquired.
    #[tokio::test]
    #[cfg_attr(miri, ignore)] // tokio::time::timeout hangs under Miri
    async fn test_download_rejects_oversized_content_length() {
        let mock_server = MockServer::start().await;
        // 2 KiB body with a 1 KiB limit → Content-Length (2048) > limit (1024).
        let body = vec![0u8; 2 * 1024];
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&mock_server)
            .await;

        let config = DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024,
            ..DownloadConfig::default()
        };
        // 1 MiB budget — far more than the body, so any acquire would be visible.
        let budget = 1 << 20usize;
        let semaphore = Arc::new(Semaphore::new(budget));
        let client = wreq::Client::builder()
            .build()
            .expect("fallo construyendo cliente wreq de prueba");
        let downloader = ResourceDownloader::with_config(semaphore.clone(), client, config);
        let url = format!("{}/", mock_server.uri());

        let result = downloader.download(&url).await;
        assert!(
            matches!(result, Err(ScraperError::PayloadTooLarge)),
            "esperaba PayloadTooLarge, obtuve: {result:?}"
        );

        // Spec: "no permits MUST be acquired" on Content-Length rejection.
        assert_eq!(
            semaphore.available_permits(),
            budget,
            "se adquirieron permisos pese al rechazo por Content-Length"
        );
    }
}
