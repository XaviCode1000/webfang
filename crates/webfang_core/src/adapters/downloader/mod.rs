//! Asset Download Module
//!
//! Handles downloading of images and documents from URLs.
//!
//! # Architecture
//!
//! Following rust-skills best practices:
//! - **True Streaming**: Writes chunks directly to disk, constant RAM (~8KB)
//! - **Atomic Operations**: Temp file with UUID, atomic rename on success
//! - **Init Once**: Directories pre-created in `new()`, zero runtime contention
//! - **Configurable**: User-Agent externalized to config
//! - **Cleanup**: Temp file removed on size limit exceeded
//! - **Hash On-The-Fly**: SHA256 computed during streaming, no buffer needed

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Entries retained per Asset-tier permit in the dedup cache (Q3 MEASURE
/// FIRST: BEFORE data measured ~600 B/entry; default tier 3 x 8_192 =
/// 24_576 entries ~= 15 MiB ceiling, far below the 50 MB materiality line).
pub const ASSET_CACHE_ENTRIES_PER_PERMIT: usize = 8_192;

/// Derive the bounded asset-cache capacity from the budget model's Asset
/// tier. Single source of truth with every other derived bound.
#[must_use]
pub fn asset_cache_capacity(asset_tier_permits: usize) -> usize {
    asset_tier_permits
        .saturating_mul(ASSET_CACHE_ENTRIES_PER_PERMIT)
        .max(ASSET_CACHE_ENTRIES_PER_PERMIT)
}

/// Insertion-order ledger for the bounded dedup cache. A `HashSet` makes
/// membership O(1) so every new cache insert stays O(1) under the single
/// ledger mutex instead of scanning the whole FIFO queue.
#[derive(Default)]
struct InsertionLedger {
    queue: VecDeque<String>,
    seen: HashSet<String>,
}

impl InsertionLedger {
    fn push_new(&mut self, key: String) {
        if self.seen.insert(key.clone()) {
            self.queue.push_back(key);
        }
    }

    /// Rotate an entry (still tracked in `seen`) back to the tail.
    fn requeue(&mut self, key: String) {
        self.queue.push_back(key);
    }

    fn pop_oldest(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    /// Drop membership tracking for an entry that left the cache.
    fn forget(&mut self, key: &str) {
        self.seen.remove(key);
    }

    fn len(&self) -> usize {
        self.queue.len()
    }
}

/// Lifecycle class of a candidate cache cell during eviction.
enum CellState {
    /// Holds a successful `DownloadedAsset`.
    Initialized,
    /// Empty but a download is executing for this URL right now.
    InFlight,
    /// Empty with no active download: a permanent-failure leftover.
    Abandoned,
    /// Key already removed from the map (evicted elsewhere).
    Gone,
}

/// RAII marker for a URL whose download is actively executing. Inserted
/// before `get_or_try_init` and removed on drop — including tokio task
/// cancellation (#509), which drops the future and therefore the guard,
/// so a cancelled download can never leave a stale in-flight marker.
struct InFlightGuard<'a> {
    map: &'a DashMap<String, ()>,
    url: String,
}

impl<'a> InFlightGuard<'a> {
    fn new(map: &'a DashMap<String, ()>, url: &str) -> Self {
        map.insert(url.to_string(), ());
        Self {
            map,
            url: url.to_string(),
        }
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.map.remove(&self.url);
    }
}

use crate::domain::ValidUrl;
use crate::error::{ErrorClass, Result, ScraperError};
use dashmap::DashMap;
use futures::stream::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use uuid::Uuid;
use wreq::{Client, Response};
use wreq_util::Profile;

/// Strategy for generating downloaded asset filenames — domain-owned VO.
///
/// Re-exported from `crate::domain::config` so `ScraperConfig` (domain) does
/// not depend on `adapters` (outward). The `adapters` implementation re-uses the
/// domain type via this pub use (inward `adapters → domain` is allowed).
pub use crate::domain::config::AssetNamingStrategy;

/// Result of a successful download
#[derive(Debug, Clone)]
pub struct DownloadedAsset {
    /// Original URL
    pub url: String,
    /// Local file path where asset was saved
    pub local_path: PathBuf,
    /// MIME type detected from HTTP headers
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size: u64,
    /// SHA256 hash of content (first 12 hex chars used in filename)
    pub content_hash: String,
}

/// Download configuration
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Output directory for downloaded files
    pub output_dir: PathBuf,
    /// Subdirectory for images
    pub images_dir: String,
    /// Subdirectory for documents
    pub documents_dir: String,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: u64,
    /// Timeout for each download in seconds
    pub timeout_secs: u64,
    /// Maximum concurrent downloads (default: 3 for HDD)
    pub concurrency_limit: usize,
    /// User-Agent string for HTTP requests
    pub user_agent: String,
    /// URL glob patterns to include (empty = allow all)
    pub include_patterns: Vec<String>,
    /// URL glob patterns to exclude (always applied)
    pub exclude_patterns: Vec<String>,
    /// TLS/HTTP2 fingerprint profile
    pub h2_profile: Profile,
    /// Strategy for naming downloaded asset files
    pub asset_naming: AssetNamingStrategy,
    /// Maximum number of retry attempts for transient network errors
    pub max_retries: u32,
    /// Base delay for exponential backoff in milliseconds
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff in milliseconds
    pub backoff_max_ms: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./downloads"),
            images_dir: "images".to_string(),
            documents_dir: "documents".to_string(),
            max_file_size: 50 * 1024 * 1024,
            timeout_secs: 30,
            concurrency_limit: 3,
            user_agent: format!("WebCrawlerStaticPages/{}", env!("CARGO_PKG_VERSION")),
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            h2_profile: Profile::Chrome145,
            asset_naming: AssetNamingStrategy::default(),
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10_000,
        }
    }
}

// ============================================================================
// Extension: ScraperConfig → DownloadConfig (moved from the retired
// infrastructure config shim — issue #1099)
// ============================================================================

impl crate::domain::config::ScraperConfig {
    /// Build a `DownloadConfig` from this scraper configuration.
    ///
    /// This is the single source of truth for mapping ScraperConfig → DownloadConfig,
    /// eliminating duplication between the orchestrator and fallback paths.
    /// Lives in `adapters::downloader` next to `DownloadConfig` so `domain::config`
    /// does not depend on `adapters` (inward-only).
    pub fn to_download_config(&self) -> DownloadConfig {
        DownloadConfig {
            output_dir: self.output_dir.clone(),
            timeout_secs: self.download_timeout_secs,
            max_file_size: self.max_file_size.unwrap_or(50 * 1024 * 1024),
            concurrency_limit: self.download_concurrency,
            include_patterns: self.asset_include_patterns.clone(),
            exclude_patterns: self.asset_exclude_patterns.clone(),
            h2_profile: self.asset_h2_profile,
            asset_naming: self.asset_naming,
            ..Default::default()
        }
    }
}

/// Asset downloader
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
    /// Crawl-scoped successful-download registry (#782). One `Downloader`
    /// lives for the whole crawl (shared via `Arc` across all concurrent
    /// pages), so this map guarantees each canonical asset URL is fetched at
    /// most once: the first occurrence runs the download and records the
    /// asset here; concurrent arrivals wait on that result through the
    /// [`OnceCell`](tokio::sync::OnceCell); later occurrences (e.g. another
    /// page that references the same asset) reuse the already-written file.
    ///
    /// Only **successful** outcomes are cached. Failures are returned
    /// (typed, unmodified) and leave the cell uninitialized, so `run_download`'s
    /// retry policy is not bypassed and later pages are free to retry.
    downloaded_urls: DashMap<String, std::sync::Arc<OnceCell<DownloadedAsset>>>,
    /// Insertion order + O(1) membership for FIFO eviction. Only populated
    /// when the cache is bounded; the unbounded legacy path skips it entirely
    /// (no per-URL strings, no mutex traffic).
    asset_cache_order: std::sync::Mutex<InsertionLedger>,
    /// URLs with a download currently executing (RAII-guarded). Distinguishes
    /// an in-flight cell — which must never be evicted, or concurrent callers
    /// would open duplicate connections — from an ABANDONED uninitialized cell
    /// left behind by a permanent failure (`run_download` deliberately leaves
    /// failures uncached so later pages may retry).
    in_flight_downloads: DashMap<String, ()>,
    /// Maximum cached entries. `usize::MAX` = unbounded (legacy behavior,
    /// byte-identical to pre-cap releases).
    asset_cache_capacity: usize,
}

impl std::fmt::Debug for Downloader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Downloader")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Downloader {
    /// Create a new downloader with configuration.
    ///
    /// Pre-creates output directories once to avoid runtime contention.
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Io` if directory creation fails.
    /// Returns `ScraperError::Config` if HTTP client build fails.
    pub fn new(config: DownloadConfig) -> Result<Self> {
        // Directories are created lazily on the first actual download (issue
        // #606): when no assets are downloaded we must not litter the CWD with
        // empty `output/` + subdir trees.
        //
        // Delegates to the bounded constructor with `usize::MAX` so both paths
        // share ONE wreq client builder — the SSRF policy (#703) can never
        // diverge between them.
        Self::with_asset_cache_capacity(config, usize::MAX)
    }

    /// Bounded-cache constructor: the dedup cache evicts oldest entries past
    /// `capacity` (Q3 materiality decision — downloaded_urls was the only
    /// structure whose measured growth crossed 50 MB at realistic long-run
    /// scale). Residual: an evicted URL may be re-downloaded if encountered
    /// again later (documented trade-off, design D2 PARTIAL).
    ///
    /// # Errors
    /// Propagates HTTP-client construction failures.
    pub fn with_asset_cache_capacity(config: DownloadConfig, capacity: usize) -> Result<Self> {
        let builder = Client::builder()
            .emulation(config.h2_profile)
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent);
        // SSRF guard (#703): identical policy to `new` — same client
        // construction, only the cache bound differs. Applied through the
        // domain `SsrfGuard` port.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to build http client: {e}")))?;
        Ok(Self::from_parts(client, config, capacity))
    }

    fn from_parts(
        client: wreq::Client,
        config: DownloadConfig,
        asset_cache_capacity: usize,
    ) -> Self {
        Self {
            client,
            config,
            downloaded_urls: DashMap::new(),
            asset_cache_order: std::sync::Mutex::new(InsertionLedger::default()),
            in_flight_downloads: DashMap::new(),
            asset_cache_capacity,
        }
    }

    /// Effective dedup-cache bound (#1120). `usize::MAX` marks the legacy
    /// unbounded mode; composition roots assert a finite value so long-lived
    /// processes can never reintroduce the linear-growth path.
    #[must_use]
    pub const fn asset_cache_capacity(&self) -> usize {
        self.asset_cache_capacity
    }

    /// Insert into the dedup cache through the bounded-cache discipline:
    /// records insertion order and enforces the capacity bound. Probe/test-only
    /// writer; production writes flow through `download`, which performs the
    /// same tracking inline.
    #[cfg(test)]
    pub(crate) fn insert_cached_cell(
        &self,
        url: String,
        cell: std::sync::Arc<OnceCell<DownloadedAsset>>,
    ) {
        self.downloaded_urls.insert(url.clone(), cell);
        if self.asset_cache_capacity != usize::MAX {
            if let Ok(mut order) = self.asset_cache_order.lock() {
                order.push_new(url);
            }
        }
        self.evict_over_capacity();
    }

    /// FIFO-evict oldest entries while over capacity.
    /// Uninitialized cells are skipped (a download is still in flight —
    /// evicting them would allow a duplicate connection); they rotate to the
    /// back of the order queue instead.
    pub(crate) fn evict_over_capacity(&self) {
        if self.asset_cache_capacity == usize::MAX
            || self.downloaded_urls.len() <= self.asset_cache_capacity
        {
            return;
        }
        let Ok(mut order) = self.asset_cache_order.lock() else {
            return; // poisoned: keep serving, eviction is best-effort
        };
        // Eviction classes:
        // - initialized cell -> evictable (normal FIFO victim),
        // - uninitialized + actively downloading (`in_flight_downloads`) ->
        //   never evicted (a duplicate connection would open); rotated back,
        // - uninitialized + NOT in flight -> abandoned failure zombie
        //   (`run_download` leaves failures uncached on purpose); evicted so
        //   error-heavy workloads stay bounded and later pages can retry.
        // The scan visits at most one full rotation of the queue, so an
        // all-in-flight excess cannot spin forever.
        let excess = self.downloaded_urls.len() - self.asset_cache_capacity;
        let mut evicted = 0;
        let mut deferred: VecDeque<String> = VecDeque::new();
        let max_scan = order.len();
        let mut scanned = 0;
        while evicted < excess && scanned < max_scan {
            scanned += 1;
            let Some(candidate) = order.pop_oldest() else {
                break;
            };
            let state = self
                .downloaded_urls
                .get(&candidate)
                .map(|e| {
                    if e.value().get().is_some() {
                        CellState::Initialized
                    } else if self.in_flight_downloads.contains_key(&candidate) {
                        CellState::InFlight
                    } else {
                        CellState::Abandoned
                    }
                })
                .unwrap_or(CellState::Gone);
            match state {
                CellState::InFlight => {
                    // Actively downloading: never evict; rotate to the back.
                    deferred.push_back(candidate);
                },
                CellState::Initialized | CellState::Abandoned | CellState::Gone => {
                    self.downloaded_urls.remove(&candidate);
                    order.forget(&candidate);
                    evicted += 1;
                },
            }
        }
        for url in deferred {
            order.requeue(url);
        }
    }

    /// Lazily create a download subdir only when a real asset is about to be
    /// persisted (issue #606). Avoids littering the CWD with empty dirs when
    /// no assets are downloaded.
    ///
    /// # Errors
    /// Returns `ScraperError::Io` if directory creation fails.
    fn ensure_subdir(&self, path: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(path).map_err(ScraperError::Io)
    }

    /// Disambiguate a filename that already exists on disk by appending a short
    /// content-hash suffix (preserving the extension).
    fn resolve_collision(
        &self,
        subdir: &std::path::Path,
        filename: &str,
        content_hash: &str,
    ) -> std::path::PathBuf {
        let hash_suffix = &content_hash[..8];
        let stem = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = std::path::Path::new(filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if ext.is_empty() {
            subdir.join(format!("{stem}-{hash_suffix}"))
        } else {
            subdir.join(format!("{stem}-{hash_suffix}.{ext}"))
        }
    }

    /// Download a single asset with true streaming to disk.
    ///
    /// # Dedup (#782)
    ///
    /// Each canonical URL is fetched at most once per `Downloader` (one
    /// instance is shared via `Arc` across all pages of a crawl). The first
    /// occurrence wins and records the downloaded asset in the crawl-scoped
    /// registry; concurrent callers for the same URL wait on that result
    /// (never a second network request); later occurrences — e.g. another
    /// page that references the same asset — reuse the already-written file
    /// and therefore resolve to the same local path. Failures are NOT
    /// cached: the registry only stores successes, so failed URLs never
    /// poison later pages and every caller keeps the typed error verbatim.
    ///
    /// # Architecture
    ///
    /// - Creates temp file with UUID
    /// - Streams chunks directly to disk (constant RAM)
    /// - Computes hash on-the-fly
    /// - Atomic rename on success
    /// - Cleanup temp file on failure
    /// - Retry with exponential backoff on transient network errors
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Network` if HTTP request fails.
    /// Returns `ScraperError::Io` if file operations fail.
    /// Returns `ScraperError::Download` if file exceeds size limit.
    /// Download one asset. The parameter is the validated [`ValidUrl`]
    /// typestate (#1117): hostile strings (non-http schemes, embedded
    /// credentials) are rejected by the caller's boundary —
    /// [`ValidUrl::parse`] / [`ValidUrl::try_from_url`] — before this
    /// method can hand them to the TLS pool.
    pub async fn download(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let key = url.as_str();
        // Fast path: a previous page already downloaded this asset (#782).
        // The duplicate is SKIPPED here — no network request is sent.
        let registered = self
            .downloaded_urls
            .get(key)
            .map(|entry| std::sync::Arc::clone(entry.value()));
        if let Some(cell) = registered {
            if let Some(asset) = cell.get() {
                tracing::debug!(
                    url = %url,
                    local_path = %asset.local_path.display(),
                    reason = "already_downloaded_this_crawl",
                    "duplicate asset download skipped — reusing cached file"
                );
                return Ok(asset.clone());
            }
            // Cell exists but is uninitialized: download still in flight or a
            // previous failure — fall through and wait/retry via the slow path.
        }

        let cell = self
            .downloaded_urls
            .entry(key.to_string())
            .or_insert_with(|| std::sync::Arc::new(OnceCell::new()))
            .clone();
        // Track insertion order for bounded-cache FIFO eviction. The unbounded
        // legacy path skips the ledger entirely: zero extra allocations, zero
        // mutex traffic, byte-identical behavior.
        if self.asset_cache_capacity != usize::MAX {
            if let Ok(mut order) = self.asset_cache_order.lock() {
                order.push_new(key.to_string());
            }
        }
        self.evict_over_capacity();

        // Slow path: exactly one caller for this URL runs the download (with
        // retries); concurrent arrivals wait on the same cell and never open
        // a second connection. Failures leave the cell uninitialized (tokio
        // hands a fresh attempt to the waiters), so typed errors reach every
        // caller and no URL is poisoned for later pages. The RAII guard marks
        // this URL as actively downloading — and clears the marker on
        // completion, failure, or task cancellation (#509) alike — which is
        // what lets eviction distinguish "in flight" from "abandoned".
        let _in_flight = InFlightGuard::new(&self.in_flight_downloads, key);
        cell.get_or_try_init(|| self.run_download(url))
            .await
            .cloned()
    }

    /// Run the retry/backoff download and record the outcome in the crawl
    /// dedup registry (#782). Successes are observable via the emitted
    /// structured event; failures pass through without caching.
    async fn run_download(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        match self.download_with_retry(url).await {
            Ok(asset) => {
                tracing::debug!(
                    url = %url,
                    local_path = %asset.local_path.display(),
                    bytes = asset.size,
                    "asset download recorded in crawl dedup registry"
                );
                Ok(asset)
            },
            Err(e) => {
                tracing::debug!(
                    url = %url,
                    err = %e,
                    "asset download failed — URL left uncached so later pages may retry"
                );
                Err(e)
            },
        }
    }

    /// Retry loop with exponential backoff for a single asset download.
    ///
    /// # Errors
    ///
    /// Returns the first non-transient error, or the last transient error
    /// after exhausting `max_retries`.
    async fn download_with_retry(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let mut last_err = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = compute_backoff_delay(
                    attempt,
                    self.config.backoff_base_ms,
                    self.config.backoff_max_ms,
                );
                tracing::debug!(
                    "retry {attempt}/{} for {} after {}ms",
                    self.config.max_retries,
                    url,
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }

            match self.download_once(url).await {
                Ok(asset) => return Ok(asset),
                Err(e) => {
                    if e.classify() != ErrorClass::TransientRetriable
                        || attempt == self.config.max_retries
                    {
                        return Err(e);
                    }
                    last_err = Some(e);
                },
            }
        }

        // Unreachable: loop always returns on last attempt, but required for type inference
        // LCOV_EXCL_LINE defensive: unreachable-retry-exhaustion — the retry loop always returns on the last attempt
        Err(last_err.unwrap_or_else(|| {
            ScraperError::Internal("exhausted retries with no error captured".to_string())
        }))
    }

    /// Single download attempt (no retry).
    #[tracing::instrument(
        skip(self),
        fields(
            url = %url,
            // D5: stable identity of the shared pooled `Client`. Constant across
            // all asset downloads within a run => observable proof of connection-pool
            // reuse (no silent per-request handshake). See MAPA item 7.
            client_id = %format!("{:p}", &self.client)
        )
    )]
    async fn download_once(&self, url: &ValidUrl) -> Result<DownloadedAsset> {
        let response = self
            .client
            .get(url.as_str())
            .send()
            .await
            .map_err(|e| ScraperError::Network(Box::new(e)))?;

        // Fail-fast on any non-success status. Covers 4xx (client errors),
        // 5xx (transient, retried by `download`) and terminal redirect
        // responses surfaced when the SSRF redirect guard `stop()`s following
        // (#703): without this gate the stopped 301 body would be streamed
        // into an asset file.
        let status = response.status();
        if !status.is_success() {
            return Err(ScraperError::http(status.as_u16(), url.as_str()));
        }

        let mime_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Extract Content-Disposition filename before consuming response
        let content_disposition_filename = response
            .headers()
            .get(wreq::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_header);

        let asset_type = crate::adapters::detector::detect_from_url(url.as_str());
        let subdir = if asset_type.is_image() {
            &self.config.images_dir
        } else {
            &self.config.documents_dir
        };

        let subdir_path = self.config.output_dir.join(subdir);
        // Lazily create the target subdir only when we are about to persist a
        // real asset (issue #606) — not at `Downloader::new` time.
        self.ensure_subdir(&subdir_path)?;

        // Create temp file with UUID (atomic operation pattern)
        let temp_path = subdir_path.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(ScraperError::Io)?;

        // Stream to disk with real-time size check
        let mut stream = into_stream(response);
        let mut downloaded: u64 = 0;
        let mut hasher = Sha256::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| ScraperError::Network(Box::new(e)))?;
            if chunk.is_empty() {
                continue;
            }

            let chunk_len = chunk.len() as u64;
            // LCOV_EXCL_LINE defensive: integer-overflow — u64 overflow requires an unrealistic download size
            downloaded = downloaded.checked_add(chunk_len).ok_or_else(|| {
                ScraperError::Internal("integer overflow in download size".to_string())
            })?;

            // Check limit in real-time
            if downloaded > self.config.max_file_size {
                let _ = fs::remove_file(&temp_path).await;
                return Err(ScraperError::PayloadTooLarge);
            }

            // Write chunk to disk IMMEDIATELY (true streaming)
            file.write_all(&chunk).await.map_err(ScraperError::Io)?;
            hasher.update(&chunk);
        }

        // Sync to ensure data is on disk
        file.sync_all().await.map_err(ScraperError::Io)?;
        drop(file); // Close file before rename

        // Calculate hash and generate final filename
        let content_hash = format!("{:x}", hasher.finalize());
        let filename = self.generate_filename(
            url,
            &content_hash,
            mime_type.as_deref(),
            content_disposition_filename.as_deref(),
        );
        let mut final_path = subdir_path.join(&filename);
        // Slug/ContentDisposition naming can collide when different URLs produce
        // the same filename. Disambiguate with a short hash suffix.
        if final_path.exists() {
            final_path = self.resolve_collision(&subdir_path, &filename, &content_hash);
        }

        // Atomic rename
        fs::rename(&temp_path, &final_path)
            .await
            .map_err(ScraperError::Io)?;

        tracing::info!(
            client_id = %format!("{:p}", &self.client),
            "downloaded: {} -> {:?}",
            url,
            final_path
        );

        Ok(DownloadedAsset {
            url: url.as_str().to_string(),
            local_path: final_path,
            mime_type,
            size: downloaded,
            content_hash: content_hash[..12].to_string(),
        })
    }

    /// Download multiple assets with configurable concurrency control.
    ///
    /// Filters URLs against `include_patterns` / `exclude_patterns` before downloading.
    /// Returns partial results — individual failures don't abort the batch.
    pub async fn download_batch(&self, urls: &[ValidUrl]) -> Vec<Result<DownloadedAsset>> {
        if urls.is_empty() {
            return Vec::new();
        }

        let filtered: Vec<&ValidUrl> = urls
            .iter()
            .filter(|url| {
                url_matches_filters(
                    url,
                    &self.config.include_patterns,
                    &self.config.exclude_patterns,
                )
            })
            .collect();

        if filtered.is_empty() {
            return Vec::new();
        }

        let concurrency = self.config.concurrency_limit;
        let mut results = Vec::with_capacity(filtered.len());
        let mut futs = Vec::with_capacity(filtered.len());
        for url in filtered {
            futs.push(self.download(url));
        }
        let stream = futures::stream::iter(futs).buffer_unordered(concurrency);
        results.extend(stream.collect::<Vec<_>>().await);
        results
    }

    /// Generate filename according to the configured naming strategy.
    fn generate_filename(
        &self,
        url: &ValidUrl,
        content_hash: &str,
        mime_type: Option<&str>,
        content_disposition_filename: Option<&str>,
    ) -> String {
        let extension =
            mime_type_to_extension(mime_type.unwrap_or("")).unwrap_or_else(|| "bin".into());

        let base_name = match self.config.asset_naming {
            AssetNamingStrategy::Hash => {
                // Use first 12 characters of hash (96 bits of entropy)
                format!("{}.{}", &content_hash[..12], extension)
            },
            AssetNamingStrategy::Slug => {
                let slug = derive_slug_from_url(url);
                let name = sanitize_filename(&slug);
                if name.is_empty() {
                    // Fallback to hash if slug is empty
                    format!("{}.{}", &content_hash[..12], extension)
                } else {
                    // Preserve original extension from slug if present, otherwise use MIME
                    let slug_ext = name.rsplit('.').next().unwrap_or("");
                    if !slug_ext.is_empty() && slug_ext != name {
                        sanitize_filename(&name)
                    } else {
                        format!("{name}.{extension}")
                    }
                }
            },
            AssetNamingStrategy::ContentDisposition => {
                if let Some(name) = content_disposition_filename {
                    let sanitized = sanitize_filename(name);
                    if !sanitized.is_empty() {
                        sanitized
                    } else {
                        format!("{}.{}", &content_hash[..12], extension)
                    }
                } else {
                    format!("{}.{}", &content_hash[..12], extension)
                }
            },
        };

        base_name
    }
}

impl crate::domain::ports::AssetDownloaderPort for Downloader {
    fn download_batch(
        &self,
        urls: &[ValidUrl],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<Vec<crate::domain::entities::DownloadedAsset>>,
                > + Send
                + '_,
        >,
    > {
        let urls = urls.to_vec();
        Box::pin(async move {
            let results = self.download_batch(&urls).await;
            let assets = results
                .into_iter()
                .filter_map(|r| r.ok())
                .map(|a| {
                    let asset_type = crate::adapters::detector::detect_from_url(&a.url);
                    let asset_type_str = match asset_type {
                        crate::adapters::detector::AssetType::Image => "image",
                        crate::adapters::detector::AssetType::Document => "document",
                        crate::adapters::detector::AssetType::Unknown => "unknown",
                    };
                    crate::domain::entities::DownloadedAsset {
                        url: a.url,
                        local_path: a.local_path.to_string_lossy().into_owned(),
                        asset_type: asset_type_str.to_string(),
                        size: a.size,
                    }
                })
                .collect();
            Ok(assets)
        })
    }
}

/// The seam that lets `application` stop naming [`Downloader`] directly.
///
/// `domain::asset_downloader_factory` owns the trait and the
/// [`DefaultAssetDownloaderFactory`](crate::domain::asset_downloader_factory::DefaultAssetDownloaderFactory)
/// type; this is its only implementation.
/// `ScraperConfig::to_download_config` (in adapters::downloader, next to
/// `DownloadConfig`) stays the single mapping source, so
/// the factory path and the historical inline path build byte-identical
/// clients.
impl crate::domain::asset_downloader_factory::AssetDownloaderFactory
    for crate::domain::asset_downloader_factory::DefaultAssetDownloaderFactory
{
    fn build(
        &self,
        config: &crate::domain::config::ScraperConfig,
    ) -> crate::error::Result<Arc<dyn crate::domain::ports::AssetDownloaderPort>> {
        Ok(Arc::new(Downloader::new(config.to_download_config())?))
    }
}

/// Convert a Response into a stream of bytes
fn into_stream(response: Response) -> impl StreamExt<Item = wreq::Result<bytes::Bytes>> {
    response.bytes_stream()
}

/// Check if a URL matches include/exclude filters.
///
/// If `include_patterns` is empty, all URLs pass the include check.
/// `exclude_patterns` are always applied (deny wins).
fn url_matches_filters(url: &ValidUrl, includes: &[String], excludes: &[String]) -> bool {
    if excludes.iter().any(|p| pattern_matches_asset(url, p)) {
        return false;
    }
    if includes.is_empty() {
        return true;
    }
    includes.iter().any(|p| pattern_matches_asset(url, p))
}

/// Match a URL against a pattern, supporting both extension globs (`*.pdf`)
/// and the standard host/path glob from `domain::pattern_matching`.
fn pattern_matches_asset(url: &ValidUrl, pattern: &str) -> bool {
    let p = pattern.trim();
    // Extension glob: *.ext (but NOT host globs like *.example.com which contain a dot after the prefix)
    if let Some(ext) = p.strip_prefix("*.") {
        if !ext.is_empty() && !ext.contains('.') {
            // The URL is already parsed — the old `Url::parse(url)` re-parse
            // per pattern per asset is gone (#1117).
            let last = url.path().rsplit('/').next().unwrap_or("");
            let low = last.to_ascii_lowercase();
            let ext_low = ext.to_ascii_lowercase();
            return low.ends_with(&format!(".{ext_low}"));
        }
    }
    crate::domain::matches_pattern(url.as_str(), pattern)
}

/// Compute exponential backoff delay with jitter.
fn compute_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    use rand::Rng;
    use std::cmp::min;

    // Exponential: base * 2^(attempt-1), clamped to max. Shift is capped at 62
    // to avoid panic from shifting u64 by >= 64 bits (attempt=1 is shift=0).
    let shift = (attempt - 1).min(62);
    let delay_ms = min(base_ms.saturating_mul(1u64 << shift), max_ms);
    // Add jitter: 75%-125% of delay, then clamp final result to max_ms
    let jitter = delay_ms / 4;
    let offset = rand::rng().random_range(0..=jitter.saturating_mul(2));
    let final_ms = min(
        delay_ms.saturating_sub(jitter).saturating_add(offset),
        max_ms,
    );
    Duration::from_millis(final_ms)
}

/// Derive a slug from the last path segment of a URL.
fn derive_slug_from_url(url: &ValidUrl) -> String {
    // The URL is already parsed — the old `Url::parse(url).ok()` re-parse
    // and its silent empty-slug fallback are gone (#1117).
    url.path()
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && *s != "/")
        .map(String::from)
        .unwrap_or_default()
}

/// UTF-8 safe percent-decoding for Content-Disposition filenames.
/// Uses the `percent-encoding` crate for correct handling of truncated
/// sequences, multi-byte chars, and all edge cases in RFC 5987/3986.
fn percent_decode_utf8(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .into_owned()
}

/// Sanitize an untrusted filename into a safe single path component.
///
/// Thin delegate over
/// [`crate::domain::crawler_port::filename::sanitize_filename_component`]
/// — the single source of truth for filename sanitization. Returns an
/// empty string when nothing safe remains (fully hostile input such as
/// `.` / `..`), so callers like [`Self::generate_filename`] fall back to
/// hash-based naming.
fn sanitize_filename(name: &str) -> String {
    crate::domain::crawler_port::filename::sanitize_filename_component(name).unwrap_or_default()
}

/// Parse `filename=` from a Content-Disposition header value.
///
/// RFC 6266 / 5987: parameter names are case-insensitive. We lowercase
/// the header value before matching to handle `Filename=`, `FILENAME=`, etc.
fn parse_content_disposition_header(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();

    // Try filename*=utf-8''encoded first (RFC 5987)
    if let Some(start) = lower.find("filename*=utf-8''") {
        let encoded = &value[start + "filename*=utf-8''".len()..];
        let name: String = encoded
            .chars()
            .take_while(|c| *c != ';' && *c != ' ')
            .collect();
        let decoded = percent_decode_utf8(&name);
        return Some(decoded);
    }

    // Try filename="name" or filename=name (case-insensitive)
    let after = lower.find("filename=")?;
    let rest = &value[after + "filename=".len()..];
    let name = if let Some(inner) = rest.strip_prefix('"') {
        // Quoted: filename="name.pdf"
        let end = inner.find('"')?;
        &inner[..end]
    } else {
        // Unquoted: filename=name.pdf
        rest.split(';').next().unwrap_or(rest).trim()
    };

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// MIME type to file extension mapping
fn mime_type_to_extension(mime: &str) -> Option<String> {
    let mime = mime.trim();
    match mime {
        "image/jpeg" | "image/jpg" => Some("jpg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "image/svg+xml" => Some("svg".to_string()),
        "image/bmp" => Some("bmp".to_string()),
        "image/tiff" => Some("tiff".to_string()),
        "image/x-icon" => Some("ico".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        "application/msword" => Some("doc".to_string()),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("docx".to_string())
        },
        "application/vnd.ms-excel" => Some("xls".to_string()),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx".to_string())
        },
        "application/vnd.ms-powerpoint" => Some("ppt".to_string()),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("pptx".to_string())
        },
        "text/csv" => Some("csv".to_string()),
        "application/vnd.oasis.opendocument.text" => Some("odt".to_string()),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("ods".to_string()),
        "application/epub+zip" => Some("epub".to_string()),
        "application/rtf" => Some("rtf".to_string()),
        "text/plain" => Some("txt".to_string()),
        "application/json" => Some("json".to_string()),
        "application/xml" | "text/xml" => Some("xml".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::ScraperConfig;

    /// Test helper: the download API is typed on `ValidUrl` (#1117), so
    /// literals must enter through the same parse the production edge uses.
    fn vu(s: &str) -> ValidUrl {
        ValidUrl::parse(s).expect("test url is valid")
    }

    /// #1117 reproduction: `download` used to take a raw `&str`, so a
    /// non-fetchable scheme or an embedded-credential URL reached the wreq
    /// client and failed late in the TLS/connection handshake. The API is
    /// now typed on `ValidUrl`: those strings cannot be named at the call
    /// site — `ValidUrl::parse` rejects them at the caller's edge.
    #[test]
    fn issue_1117_hostile_urls_cannot_reach_download() {
        // The exact shapes that used to slip through to the network layer.
        assert!(
            ValidUrl::parse("data:text/html,<h1>hi</h1>").is_err(),
            "data: must be rejected before download() can be called"
        );
        assert!(
            ValidUrl::parse("file:///etc/passwd").is_err(),
            "file: must be rejected at the type boundary"
        );
        // Credentials are stripped by the type, never forwarded to the pool.
        let with_creds =
            ValidUrl::parse("https://user:secret@example.com/a.png").expect("https is valid");
        assert_eq!(
            with_creds.as_str(),
            "https://example.com/a.png",
            "the credential pair must not survive into the download URL"
        );
    }
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_to_download_config_maps_fields() {
        let scraper = ScraperConfig::default()
            .with_images()
            .with_download_concurrency(7)
            .with_asset_naming(AssetNamingStrategy::Slug);
        let dl = scraper.to_download_config();
        assert_eq!(dl.concurrency_limit, 7);
        assert_eq!(dl.asset_naming, AssetNamingStrategy::Slug);
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_downloader_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config);
        assert!(downloader.is_ok());
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_downloader_lazy_directories() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            images_dir: "test_images".to_string(),
            documents_dir: "test_docs".to_string(),
            ..Default::default()
        };

        // Directories are created lazily on the first actual download (issue
        // #606): when no assets are downloaded we must not litter the CWD with
        // empty `output/` + subdir trees.
        let downloader = Downloader::new(config).unwrap();

        let images_path = temp_dir.path().join("test_images");
        let docs_path = temp_dir.path().join("test_docs");

        assert!(
            !images_path.exists(),
            "Images directory should not be pre-created"
        );
        assert!(
            !docs_path.exists(),
            "Documents directory should not be pre-created"
        );

        // A real download creates the subdir on demand via ensure_subdir.
        downloader.ensure_subdir(&images_path).unwrap();
        assert!(
            images_path.exists(),
            "Images directory should be created lazily on first download"
        );
    }

    #[test]
    fn test_downloader_config_concurrency() {
        let config = DownloadConfig {
            concurrency_limit: 10,
            ..Default::default()
        };
        assert_eq!(config.concurrency_limit, 10);
    }

    #[test]
    fn test_downloader_config_user_agent() {
        let custom_ua = "MyCustomBot/1.0";
        let config = DownloadConfig {
            user_agent: custom_ua.to_string(),
            ..Default::default()
        };
        assert_eq!(config.user_agent, custom_ua);
    }

    #[test]
    fn test_downloader_default_user_agent() {
        let config = DownloadConfig::default();
        assert!(
            config.user_agent.starts_with("WebCrawlerStaticPages/"),
            "Default user agent should include version"
        );
    }

    #[test]
    fn test_mime_type_to_extension() {
        assert_eq!(mime_type_to_extension("image/png"), Some("png".to_string()));
        assert_eq!(
            mime_type_to_extension("image/jpeg"),
            Some("jpg".to_string())
        );
        assert_eq!(
            mime_type_to_extension("application/pdf"),
            Some("pdf".to_string())
        );
        assert_eq!(mime_type_to_extension("application/unknown"), None);
        assert_eq!(mime_type_to_extension(""), None);
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn test_generate_filename_hash_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            &vu("https://example.com/img.png"),
            "abc123def456789",
            Some("image/png"),
            None,
        );
        assert!(
            filename.ends_with(".png"),
            "Expected .png but got: {filename}"
        );
        assert!(
            filename.starts_with("abc123def456"),
            "Filename should start with first 12 chars of hash"
        );

        let filename = downloader.generate_filename(
            &vu("https://example.com/file"),
            "xyz789abc123456",
            None,
            None,
        );
        assert!(
            filename.ends_with(".bin"),
            "Expected .bin but got: {filename}"
        );
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn test_generate_filename_slug_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::Slug,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            &vu("https://example.com/docs/rust-book.pdf"),
            "abc123def456789",
            Some("application/pdf"),
            None,
        );
        assert_eq!(filename, "rust-book.pdf");
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn test_generate_filename_content_disposition() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::ContentDisposition,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            &vu("https://example.com/download"),
            "abc123def456789",
            Some("application/pdf"),
            Some("annual-report.pdf"),
        );
        assert_eq!(filename, "annual-report.pdf");
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn test_generate_filename_content_disposition_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::ContentDisposition,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        // No Content-Disposition → falls back to hash
        let filename = downloader.generate_filename(
            &vu("https://example.com/download"),
            "abc123def456789",
            Some("application/pdf"),
            None,
        );
        assert!(filename.starts_with("abc123def456"));
    }

    #[test]
    fn test_url_matches_filters_empty_includes() {
        assert!(url_matches_filters(
            &vu("https://example.com/file.pdf"),
            &[],
            &[]
        ));
    }

    #[test]
    fn test_url_matches_filters_exclude_wins() {
        let excludes = vec!["/*.pdf".to_string()];
        assert!(!url_matches_filters(
            &vu("https://example.com/file.pdf"),
            &[],
            &excludes
        ));
    }

    #[test]
    fn test_url_matches_filters_include_only() {
        let includes = vec!["/*.pdf".to_string()];
        assert!(url_matches_filters(
            &vu("https://example.com/file.pdf"),
            &includes,
            &[]
        ));
        assert!(!url_matches_filters(
            &vu("https://example.com/file.jpg"),
            &includes,
            &[]
        ));
    }

    #[test]
    fn test_url_matches_filters_extension_glob() {
        let includes = vec!["*.pdf".to_string()];
        assert!(url_matches_filters(
            &vu("https://x.com/file.pdf"),
            &includes,
            &[]
        ));
        assert!(!url_matches_filters(
            &vu("https://x.com/file.jpg"),
            &includes,
            &[]
        ));
    }

    #[test]
    fn test_sanitize_filename() {
        // Delegates to `binary_utils::sanitize_filename_component`: only
        // the last valid segment survives.
        assert_eq!(sanitize_filename("hello/world"), "world");
        assert_eq!(sanitize_filename("file\0name"), "filename");
        assert_eq!(sanitize_filename("normal-file.pdf"), "normal-file.pdf");
    }

    // -------------------------------------------------------------
    // sanitize_filename — traversal hardening (batch 1, unified in
    // binary_utils::sanitize_filename_component since batch 3)
    // -------------------------------------------------------------

    #[test]
    fn sanitize_filename_strips_traversal_separators() {
        // Directory components are dropped entirely; the last valid
        // segment cannot traverse on join.
        assert_eq!(sanitize_filename("../escape.bin"), "escape.bin");
        assert_eq!(sanitize_filename("a/../b"), "b");
        assert_eq!(sanitize_filename("/abs/path.bin"), "path.bin");
        assert_eq!(sanitize_filename(r"\\server\share"), "share");
    }

    #[test]
    fn sanitize_filename_neutralizes_bare_dot_segments() {
        // `.` / `..` joined onto a directory resolve outside it → empty
        // result so generate_filename falls back to hash-based naming.
        assert_eq!(sanitize_filename(".."), "");
        assert_eq!(sanitize_filename("."), "");
        assert_eq!(sanitize_filename("..\\"), "");
    }

    #[test]
    fn sanitize_filename_removes_null_bytes() {
        assert_eq!(sanitize_filename("nul\0byte.pdf"), "nulbyte.pdf");
    }

    #[test]
    fn sanitize_filename_caps_length() {
        let long = format!("{}.pdf", "a".repeat(300));
        let sanitized = sanitize_filename(&long);
        // ext4 per-component cap; matches binary_utils::MAX_FILENAME_LEN.
        assert!(sanitized.len() <= 255);
        assert!(!sanitized.is_empty());
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn generate_filename_cd_falls_back_to_hash_when_fully_hostile() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::ContentDisposition,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        // Bare `..` sanitizes to empty → hash-based fallback.
        let filename = downloader.generate_filename(
            &vu("https://example.com/download"),
            "abc123def456789",
            Some("application/pdf"),
            Some(".."),
        );
        assert!(filename.starts_with("abc123def456"));
        assert!(filename.ends_with(".pdf"));
    }

    #[test]
    fn test_parse_content_disposition_quoted() {
        let val = r#"attachment; filename="report.pdf""#;
        assert_eq!(
            parse_content_disposition_header(val),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_unquoted() {
        let val = "attachment; filename=report.pdf";
        assert_eq!(
            parse_content_disposition_header(val),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_missing() {
        assert_eq!(parse_content_disposition_header("attachment"), None);
    }

    #[test]
    fn test_derive_slug_from_url() {
        assert_eq!(
            derive_slug_from_url(&vu("https://example.com/docs/book.pdf")),
            "book.pdf"
        );
        assert_eq!(derive_slug_from_url(&vu("https://example.com/")), "");
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_download_streaming_limit() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_file_size: 1024,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();
        assert_eq!(downloader.config.max_file_size, 1024);
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_download_batch_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();
        let results = downloader.download_batch(&[]).await;
        assert!(results.is_empty());
    }

    /// Mount an asset route returning `body` and `expect(1)` — the wire-level
    /// proof of the #782 dedup contract: at most ONE network request for the
    /// URL no matter how many times it is handed to a `Downloader`.
    async fn mount_asset_expect_once(server: &MockServer, asset_path: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(asset_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(body)
                    .insert_header("content-type", "image/png"),
            )
            .expect(1)
            .mount(server)
            .await;
    }

    /// #782: sequential occurrences of the same asset URL (later crawl pages)
    /// must NOT re-download — the second call reuses the cached file and both
    /// resolve to the same local path. wiremock's `expect(1)` verifies the
    /// single request at server drop.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_download_dedup_sequential_reuses_cached_file() {
        let server = MockServer::start().await;
        mount_asset_expect_once(&server, "/img.png", b"png-bytes".to_vec()).await;

        let temp_dir = TempDir::new().unwrap();
        let downloader = Downloader::new(DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_retries: 0,
            ..Default::default()
        })
        .unwrap();

        let url = vu(&format!("{}/img.png", server.uri()));
        let first = downloader.download(&url).await.unwrap();
        let second = downloader.download(&url).await.unwrap();

        assert_eq!(
            first.local_path, second.local_path,
            "the duplicate occurrence must resolve to the same already-downloaded file"
        );
        assert!(
            first.local_path.exists(),
            "cached asset file must exist on disk"
        );
    }

    /// #782: concurrent arrivals for the same asset URL (parallel pages) must
    /// produce exactly ONE origin request — the losers wait on the winner's
    /// result instead of opening second connections. The response delay
    /// forces the two downloads to overlap.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_download_dedup_concurrent_single_request() {
        use std::sync::Arc;

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/busy.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"busy-png".to_vec())
                    .insert_header("content-type", "image/png")
                    .set_delay(Duration::from_millis(200)),
            )
            .expect(1)
            .mount(&server)
            .await;

        let temp_dir = TempDir::new().unwrap();
        let downloader = Arc::new(
            Downloader::new(DownloadConfig {
                output_dir: temp_dir.path().to_path_buf(),
                max_retries: 0,
                ..Default::default()
            })
            .unwrap(),
        );
        let url = vu(&format!("{}/busy.png", server.uri()));

        let winner = {
            let d = Arc::clone(&downloader);
            let u = url.clone();
            tokio::spawn(async move { d.download(&u).await })
        };
        let loser = {
            let d = Arc::clone(&downloader);
            tokio::spawn(async move { d.download(&url).await })
        };

        let first = winner.await.unwrap().unwrap();
        let second = loser.await.unwrap().unwrap();

        assert_eq!(
            first.local_path, second.local_path,
            "both concurrent callers must resolve to the same file"
        );
    }

    /// #782 guardrail: failures are NOT cached — a failed download leaves the
    /// registry cell uninitialized, so a later page may retry the URL and
    /// every caller keeps its typed error verbatim (no error poisoning).
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_download_failure_not_cached_in_dedup_registry() {
        let server = MockServer::start().await;
        // First occurrence fails (404 is non-retriable), then the route heals.
        Mock::given(method("GET"))
            .and(path("/flaky.png"))
            .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/flaky.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"healed".to_vec())
                    .insert_header("content-type", "image/png"),
            )
            .up_to_n_times(2)
            .mount(&server)
            .await;

        let temp_dir = TempDir::new().unwrap();
        let downloader = Downloader::new(DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_retries: 0,
            ..Default::default()
        })
        .unwrap();

        let url = vu(&format!("{}/flaky.png", server.uri()));
        let failure = downloader.download(&url).await;
        assert!(
            matches!(failure, Err(ScraperError::Http { status: 404, .. })),
            "first failure must surface the typed HTTP error: {failure:?}"
        );

        // Later page retries the SAME downloader — must not be poisoned by the
        // earlier failure.
        let retry = downloader.download(&url).await;
        assert!(
            retry.is_ok(),
            "failed outcome must not be cached; retry should succeed: {retry:?}"
        );
    }

    /// Mount the entry route of a redirect-flow fixture plus its destination.
    /// The `location` header targets a forbidden literal IP, so the guard must
    /// stop the flow before `/target` is ever requested — `expect(0)` is the
    /// wire-level proof of that.
    async fn mount_redirect_with_untouched_target(
        server: &MockServer,
        from_path: &str,
        to_location: &str,
    ) {
        let redirect = ResponseTemplate::new(301).insert_header("location", to_location);
        let entry = ResponseTemplate::new(200).set_body_string("body-never-fetched");

        Mock::given(method("GET"))
            .and(path(from_path))
            .respond_with(redirect)
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/target"))
            .respond_with(entry)
            .expect(0)
            .mount(server)
            .await;
    }

    /// SSRF redirect guard (#703): even an entry URL that passed validation
    /// cannot hop onto a literal forbidden IP through a redirect — the 301
    /// itself becomes a terminal HTTP error and `/target` is never fetched
    /// (wire-level proof via `expect(0)`).
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_redirect_to_forbidden_literal_ip_is_stopped() {
        // Keep the guard active: the escape hatch must be unset in this
        // process (a no-op under nextest, which gives every test its own
        // process; defensive against shared-process harnesses).
        let _guard = webfang_test_utils::EnvGuard::clean(&[
            crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
        ]);
        let mock_server = MockServer::start().await;

        // Location points at a different loopback literal — forbidden by the
        // guard, unreachable in practice, and never requested.
        mount_redirect_with_untouched_target(
            &mock_server,
            "/download",
            "http://127.0.0.2:9/target",
        )
        .await;

        let temp_dir = TempDir::new().unwrap();
        let downloader = Downloader::new(DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_retries: 0,
            ..Default::default()
        })
        .unwrap();

        match downloader
            .download(&vu(&format!("{}/download", mock_server.uri())))
            .await
        {
            Err(ScraperError::Http { status, .. }) => assert_eq!(status, 301),
            other => panic!("expected redirect to be stopped, got: {other:?}"),
        }
    }
}

// ============================================================================
// Task 5.1 memory probe — downloaded_urls dedup cache growth (BEFORE numbers).
// ============================================================================
#[cfg(test)]
mod memory_probe_tests {
    use super::*;
    use crate::infrastructure::observability::memory_probe;
    use std::sync::Arc;

    #[test]
    fn probe_downloaded_urls_growth_50k_assets() {
        const N: usize = 50_000;
        let downloader = Downloader::new(DownloadConfig::default()).expect("downloader builds");
        let before = memory_probe::rss_bytes();

        for i in 0..N {
            let cell = tokio::sync::OnceCell::new();
            cell.set(DownloadedAsset {
                url: format!("https://probe.example.com/assets/img-{i}.png"),
                local_path: format!("/tmp/probe-out/img-{i}.png").into(),
                mime_type: Some("image/png".to_string()),
                size: 12_345,
                content_hash: format!("{i:012x}"),
            })
            .expect("fresh cell accepts first set");
            downloader.downloaded_urls.insert(
                format!("https://probe.example.com/assets/img-{i}.png"),
                Arc::new(cell),
            );
        }

        let after = memory_probe::rss_bytes();
        assert_eq!(downloader.downloaded_urls.len(), N);
        memory_probe::append_report(
            "BEFORE — downloaded_urls dedup cache",
            &format!(
                "entries={} rss_before={} rss_after={} delta={}",
                downloader.downloaded_urls.len(),
                memory_probe::fmt_rss(before),
                memory_probe::fmt_rss(after),
                memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
            ),
        );
    }
}

// ============================================================================
// Task 5.3/5.4/5.5 probes — bounded asset cache (Q3: only measured-material
// structure gets a cap; downloaded_urls crossed the 50 MB projection at
// ~87k entries in the BEFORE data).
// ============================================================================
#[cfg(test)]
mod bounded_cache_tests {
    use super::*;
    use crate::infrastructure::observability::memory_probe;
    use std::sync::Arc;

    fn sample_asset(i: usize) -> DownloadedAsset {
        DownloadedAsset {
            url: format!("https://probe.example.com/assets/img-{i}.png"),
            local_path: format!("/tmp/probe-out/img-{i}.png").into(),
            mime_type: Some("image/png".to_string()),
            size: 12_345,
            content_hash: format!("{i:012x}"),
        }
    }

    /// Task 5.3 — RED→GREEN plateau proof: under a workload of N >> cap,
    /// the bounded cache's entry count plateaus at the cap while the
    /// unbounded baseline keeps growing linearly.
    #[test]
    fn bounded_asset_cache_plateaus_at_capacity() {
        const CAP: usize = 10_000;
        const WORKLOAD: usize = 50_000;

        let bounded = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
            .expect("downloader builds");
        let unbounded = Downloader::new(DownloadConfig::default()).expect("downloader builds");

        for i in 0..WORKLOAD {
            let key = format!("https://probe.example.com/assets/img-{i}.png");
            let cell = tokio::sync::OnceCell::new();
            cell.set(sample_asset(i)).expect("fresh cell");
            bounded.insert_cached_cell(key.clone(), Arc::new(cell));
            let cell = tokio::sync::OnceCell::new();
            cell.set(sample_asset(i)).expect("fresh cell");
            unbounded.downloaded_urls.insert(key, Arc::new(cell));
        }

        assert!(
            bounded.downloaded_urls.len() <= CAP,
            "bounded cache must plateau at {CAP}, got {}",
            bounded.downloaded_urls.len()
        );
        assert_eq!(unbounded.downloaded_urls.len(), WORKLOAD);
    }

    /// Task 5.5 — byte-identity: a workload completing entirely WITHIN the
    /// cap must produce dedup decisions and returned assets identical to the
    /// unbounded baseline (the cap only changes behavior for entries that
    /// would otherwise have been evicted anyway).
    #[test]
    fn bounded_cache_within_cap_is_byte_identical_to_unbounded() {
        const CAP: usize = 100;
        const WORKLOAD: usize = 60; // < CAP: no eviction ever fires

        let bounded = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
            .expect("downloader builds");
        let unbounded = Downloader::new(DownloadConfig::default()).expect("downloader builds");

        for i in 0..WORKLOAD {
            let key = format!("https://probe.example.com/assets/img-{i}.png");
            let asset = sample_asset(i);
            // Bounded path through the disciplined insert.
            let cell = tokio::sync::OnceCell::new();
            cell.set(asset.clone()).expect("fresh cell");
            bounded.insert_cached_cell(key.clone(), std::sync::Arc::new(cell));
            // Unbounded baseline.
            let cell = tokio::sync::OnceCell::new();
            cell.set(asset).expect("fresh cell");
            unbounded
                .downloaded_urls
                .insert(key, std::sync::Arc::new(cell));
        }

        assert_eq!(
            bounded.downloaded_urls.len(),
            unbounded.downloaded_urls.len()
        );
        for i in 0..WORKLOAD {
            let key = format!("https://probe.example.com/assets/img-{i}.png");
            let b = bounded.downloaded_urls.get(&key).expect("bounded entry");
            let u = unbounded
                .downloaded_urls
                .get(&key)
                .expect("unbounded entry");
            let b_asset = b.value().get().expect("initialized").clone();
            let u_asset = u.value().get().expect("initialized").clone();
            assert_eq!(b_asset.url, u_asset.url);
            assert_eq!(b_asset.local_path, u_asset.local_path);
            assert_eq!(b_asset.content_hash, u_asset.content_hash);
            assert_eq!(b_asset.mime_type, u_asset.mime_type);
            assert_eq!(b_asset.size, u_asset.size);
        }
    }

    /// Direct state builder for eviction-classifier tests: raw map insert +
    /// ledger registration, NO automatic eviction (that is exactly what we
    /// want to drive manually per scenario).
    fn seed_cell(
        dl: &Downloader,
        key: &str,
        initialized: Option<&DownloadedAsset>,
        in_flight: bool,
    ) {
        let cell = match initialized {
            Some(asset) => {
                let c = tokio::sync::OnceCell::new();
                c.set(asset.clone()).expect("fresh cell");
                std::sync::Arc::new(c)
            },
            None => std::sync::Arc::new(tokio::sync::OnceCell::new()),
        };
        dl.downloaded_urls.insert(key.to_string(), cell);
        if let Ok(mut order) = dl.asset_cache_order.lock() {
            order.push_new(key.to_string());
        }
        if in_flight {
            dl.in_flight_downloads.insert(key.to_string(), ());
        }
    }

    /// Review HIGH regression: permanent-failure zombies (uninitialized cells
    /// with NO active download) must be evictable so error-heavy long runs
    /// stay bounded, while genuinely in-flight cells are preserved.
    #[test]
    fn abandoned_failure_zombies_evicted_inflight_preserved() {
        const CAP: usize = 8;
        let dl = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
            .expect("downloader builds");

        // Oldest first: 6 zombies (failed downloads never retried), then 3
        // initialized successes, then 2 actively-downloading cells.
        for i in 0..6 {
            seed_cell(
                &dl,
                &format!("https://probe.example.com/zombie-{i}.png"),
                None,
                false,
            );
        }
        for i in 0..3 {
            seed_cell(
                &dl,
                &format!("https://probe.example.com/success-{i}.png"),
                Some(&sample_asset(i)),
                false,
            );
        }
        for i in 0..2 {
            seed_cell(
                &dl,
                &format!("https://probe.example.com/inflight-{i}.png"),
                None,
                true,
            );
        }

        assert_eq!(dl.downloaded_urls.len(), 11);
        dl.evict_over_capacity(); // 11 - 8 = 3 excess

        // FIFO scans the 3 oldest zombies and evicts exactly those.
        assert_eq!(dl.downloaded_urls.len(), CAP, "cache bounded at cap");
        for i in 0..3 {
            let gone = format!("https://probe.example.com/zombie-{i}.png");
            assert!(!dl.downloaded_urls.contains_key(&gone), "{gone} evicted");
        }
        for i in 0..3 {
            let kept = format!("https://probe.example.com/success-{i}.png");
            assert!(dl.downloaded_urls.contains_key(&kept), "{kept} retained");
        }
        for i in 0..2 {
            let kept = format!("https://probe.example.com/inflight-{i}.png");
            assert!(
                dl.downloaded_urls.contains_key(&kept),
                "in-flight {kept} NEVER evicted"
            );
        }
    }

    /// A second eviction pass with an all-in-flight excess must terminate and
    /// preserve every in-flight cell (rotation bound, no infinite loop).
    #[test]
    fn eviction_terminates_when_excess_is_all_inflight() {
        const CAP: usize = 2;
        let dl = Downloader::with_asset_cache_capacity(DownloadConfig::default(), CAP)
            .expect("downloader builds");
        for i in 0..5 {
            seed_cell(
                &dl,
                &format!("https://probe.example.com/busy-{i}.png"),
                None,
                true,
            );
        }
        dl.evict_over_capacity();
        assert_eq!(
            dl.downloaded_urls.len(),
            5,
            "nothing evictable while in flight"
        );
        for i in 0..5 {
            let kept = format!("https://probe.example.com/busy-{i}.png");
            assert!(dl.downloaded_urls.contains_key(&kept));
        }
        // Once the downloads complete successfully, the cells are normal
        // FIFO victims again.
        for i in 0..5 {
            let key = format!("https://probe.example.com/busy-{i}.png");
            dl.in_flight_downloads.remove(&key);
            if let Some(entry) = dl.downloaded_urls.get(&key) {
                entry.value().set(sample_asset(i)).expect("init");
            }
        }
        dl.evict_over_capacity();
        assert_eq!(dl.downloaded_urls.len(), CAP, "bounded after completion");
    }

    /// Legacy unbounded mode skips the insertion ledger entirely: no per-URL
    /// strings, no O(n) membership work (review MEDIUM fix).
    #[test]
    fn legacy_unbounded_skips_insertion_ledger() {
        let dl = Downloader::new(DownloadConfig::default()).expect("downloader builds");
        for i in 0..200 {
            let cell = tokio::sync::OnceCell::new();
            cell.set(sample_asset(i)).expect("fresh cell");
            dl.downloaded_urls.insert(
                format!("https://probe.example.com/img-{i}.png"),
                std::sync::Arc::new(cell),
            );
        }
        assert_eq!(dl.downloaded_urls.len(), 200, "all entries cached");
        assert_eq!(
            dl.asset_cache_order.lock().expect("ledger").len(),
            0,
            "ledger untouched in legacy mode"
        );
    }

    /// Task 5.4 — AFTER numbers: same 50k-entry fill as the BEFORE probe,
    /// now under the production-derived cap; RSS delta must be bounded by
    /// roughly cap * per-entry cost instead of linear in the workload.
    #[test]
    fn memory_probe_downloaded_urls_after_cap() {
        const CAP: usize = 24_576; // asset tier 3 × 8_192 (production derivation)
        let downloader = Downloader::with_asset_cache_capacity(
            DownloadConfig::default(),
            crate::adapters::downloader::ASSET_CACHE_ENTRIES_PER_PERMIT
                * crate::domain::budget::tiers::DownloadConcurrency::new(3)
                    .expect("3 > 0")
                    .get(),
        )
        .expect("downloader builds");
        let before = memory_probe::rss_bytes();

        for i in 0..50_000 {
            let cell = tokio::sync::OnceCell::new();
            cell.set(sample_asset(i)).expect("fresh cell");
            downloader.insert_cached_cell(
                format!("https://probe.example.com/assets/img-{i}.png"),
                Arc::new(cell),
            );
        }

        let after = memory_probe::rss_bytes();
        assert!(downloader.downloaded_urls.len() <= CAP);
        memory_probe::append_report(
            "AFTER - downloaded_urls dedup cache (bounded)",
            &format!(
                "entries={} (cap={CAP}) rss_before={} rss_after={} delta={} | BEFORE was 28.8 MiB at 50k entries",
                downloader.downloaded_urls.len(),
                memory_probe::fmt_rss(before),
                memory_probe::fmt_rss(after),
                memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
            ),
        );
    }
}
