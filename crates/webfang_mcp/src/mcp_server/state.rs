//! MCP Server State — shared state with per-category backpressure
//!
//! McpState embeds the application Container for DI and adds
//! tokio::sync::Semaphore instances to limit concurrent operations
//! per tool category, protecting the 8GB RAM / HDD hardware.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::mcp_server::metrics::{MetricsSnapshot, ScrapeEvent, ScrapeMetrics};
use rmcp::ErrorData as McpError;
use serde_json::Value;
use webfang_core::adapters::downloader::Downloader;
use webfang_core::di::Container;
use webfang_core::domain::crawler_port::RobotsPort;
use webfang_core::domain::DomInspectorPort;
use webfang_core::infrastructure::crawler::robots_utils::RobotsFetcher;

/// Per-category semaphore limits for backpressure.
/// Tuned for Intel i5-4590 (4C), 8GB DDR3, HDD.
///
/// Every permit count is a [`NonZeroUsize`] (#1132): a zero limit used to
/// be representable and `from_limits` clamped it to 1 in silence, masking
/// the misconfiguration that without the clamp deadlocks the semaphore.
/// Zero is now unnameable, so the clamp is gone and the mapping is 1:1.
#[derive(Debug)]
pub struct CategoryLimits {
    /// AI inference tools (tract-onnx, spawn_blocking heavy)
    pub ai: NonZeroUsize,
    /// HTTP scraping tools (network I/O, WAF checks)
    pub scraping: NonZeroUsize,
    /// Export tools (file I/O, serialization)
    pub export: NonZeroUsize,
    /// Obsidian vault tools (disk scan, embeddings)
    pub obsidian: NonZeroUsize,
    /// Content processing tools (CPU-bound HTML parsing)
    pub content: NonZeroUsize,
    /// URL utility tools (lightweight, string ops)
    pub url_utils: NonZeroUsize,
    /// Security tools (WAF detection, metrics)
    pub security: NonZeroUsize,
    /// Asset download tools (file I/O, network)
    pub assets: NonZeroUsize,
}

impl Default for CategoryLimits {
    fn default() -> Self {
        // The literals are non-zero by construction; the guard is the repo
        // idiom from `domain::budget` (never `expect` in production code).
        let nz = |v: usize| {
            NonZeroUsize::new(v).unwrap_or_else(|| unreachable!("limit literal {v} is non-zero"))
        };
        Self {
            ai: nz(2),         // Heavy CPU inference — limit strictly
            scraping: nz(8),   // Network I/O — can handle more concurrent
            export: nz(4),     // File I/O — moderate limit for HDD
            obsidian: nz(3),   // Disk scan + embeddings — protect vault I/O
            content: nz(6),    // CPU-bound HTML parsing — moderate
            url_utils: nz(16), // Lightweight string ops — high limit
            security: nz(8),   // WAF detection — moderate
            assets: nz(4),     // File downloads — protect HDD
        }
    }
}

/// Shared state for the MCP server.
///
/// Embeds the Container for dependency injection and provides
/// per-category semaphores for backpressure control.
#[derive(Clone)]
pub struct McpState {
    /// Application DI container (single source of truth)
    pub container: Arc<Container>,
    /// Per-category concurrency limits
    pub limits: Arc<CategoryLimits>,
    /// Semaphores for each category
    pub semaphores: Arc<CategorySemaphores>,
    /// Shared Downloader for connection pooling across MCP tool calls
    pub downloader: Option<Arc<Downloader>>,
    /// DOM inspector for CSS selector diagnostics (None = no diagnostics)
    pub inspector: Option<Arc<dyn DomInspectorPort>>,
    /// Shared robots.txt fetcher for the scrape tools (#697). Construction
    /// failure is non-fatal: `None` leaves enforcement disabled, mirroring
    /// the rate-limiter pattern in the core container.
    pub robots_fetcher: Option<Arc<dyn RobotsPort>>,
    /// Process-lifetime scrape metrics, shared across all per-session clones
    /// (REQ-06). Locked only in short synchronous sections (REQ-07).
    pub metrics: Arc<Mutex<ScrapeMetrics>>,
    /// Allowed root directories for absolute `output_dir` paths (#696).
    ///
    /// Empty (default) = absolute `output_dir` values are REJECTED
    /// (fail-closed). When non-empty, an absolute `output_dir` must be
    /// lexically under one of these roots after normalization.
    pub allowed_export_roots: Arc<Vec<PathBuf>>,
    /// Hermetic Obsidian detection overrides for tests (#726): `(scan_root,
    /// registry_path)`. When `Some`, vault detection is injected with these
    /// paths and never reads the host's real Obsidian registry. Production
    /// leaves this `None` (real detection chain).
    pub obsidian_hermetic: Option<(PathBuf, PathBuf)>,
    /// Cancellation token for graceful shutdown propagation.
    pub cancel_token: CancellationToken,
}

/// Semaphore instances for each tool category.
#[derive(Debug)]
pub struct CategorySemaphores {
    /// Semaphore limiting concurrent AI tool calls
    pub ai: Arc<Semaphore>,
    /// Semaphore limiting concurrent scraping tool calls
    pub scraping: Arc<Semaphore>,
    /// Semaphore limiting concurrent export tool calls
    pub export: Arc<Semaphore>,
    /// Semaphore limiting concurrent Obsidian tool calls
    pub obsidian: Arc<Semaphore>,
    /// Semaphore limiting concurrent content processing tool calls
    pub content: Arc<Semaphore>,
    /// Semaphore limiting concurrent URL utility tool calls
    pub url_utils: Arc<Semaphore>,
    /// Semaphore limiting concurrent security & diagnostics tool calls
    pub security: Arc<Semaphore>,
    /// Semaphore limiting concurrent asset download tool calls
    pub assets: Arc<Semaphore>,
}

impl McpState {
    /// Create a new McpState with the given container and default limits.
    pub fn new(container: Container) -> Self {
        Self::from_parts(Arc::new(container), CategoryLimits::default())
    }

    /// Create with custom category limits.
    pub fn with_limits(container: Container, limits: CategoryLimits) -> Self {
        Self::from_parts(Arc::new(container), limits)
    }

    /// Share a pre-`Arc`'d container (#759).
    ///
    /// Unlike [`new`](Self::new)/[`with_limits`](Self::with_limits), which move
    /// an owned `Container` into a private `Arc`, this constructor adopts the
    /// given `Arc<Container>` as-is. The MCP binaries use it so a background
    /// task (lazy AI port wiring) and the server observe the SAME container —
    /// ports injected after construction become visible through the shared
    /// [`Container`] (lazy AI port wiring, #759).
    pub fn from_container(container: Arc<Container>) -> Self {
        Self::from_parts(container, CategoryLimits::default())
    }

    /// Shared construction for [`new`](Self::new) / [`with_limits`](Self::with_limits)
    /// / [`from_container`](Self::from_container): builds the semaphores,
    /// metrics accumulator, cancellation token, and the non-fatal robots.txt
    /// fetcher ([`build_robots_fetcher`]) from the given container and limits.
    fn from_parts(container: Arc<Container>, limits: CategoryLimits) -> Self {
        let limits = Arc::new(limits);
        let semaphores = Arc::new(CategorySemaphores::from_limits(&limits));
        let robots_fetcher = build_robots_fetcher(&container);
        Self {
            container,
            limits,
            semaphores,
            downloader: None,
            inspector: None,
            robots_fetcher,
            metrics: Arc::new(Mutex::new(ScrapeMetrics::default())),
            allowed_export_roots: Arc::new(Vec::new()),
            obsidian_hermetic: None,
            cancel_token: CancellationToken::new(),
        }
    }

    /// Trigger graceful shutdown by cancelling the token.
    ///
    /// All clones of this state share the same token (via `Arc` interior
    /// mutability), so a single call propagates to all holders.
    pub fn shutdown_signal(&self) {
        self.cancel_token.cancel();
    }

    /// Set a shared Downloader for connection pooling across tool calls.
    #[must_use]
    pub fn with_downloader(mut self, downloader: Arc<Downloader>) -> Self {
        self.downloader = Some(downloader);
        self
    }

    /// Set a shared robots.txt fetcher for the scrape tools (#697).
    ///
    /// Parity with [`with_downloader`](Self::with_downloader) and
    /// [`with_inspector`](Self::with_inspector): lets tests and composition
    /// roots wire a deterministic or pre-built fetcher.
    #[must_use]
    pub fn with_robots_fetcher(mut self, fetcher: Arc<dyn RobotsPort>) -> Self {
        self.robots_fetcher = Some(fetcher);
        self
    }

    /// Set a DOM inspector for CSS selector diagnostics.
    ///
    /// When set, failed selector extractions produce a `SelectorDiagnostic`
    /// with DOM structure analysis and closest-match suggestions.
    /// When `None` (default), diagnostics are `null` in the response.
    #[must_use]
    pub fn with_inspector(mut self, inspector: Arc<dyn DomInspectorPort>) -> Self {
        self.inspector = Some(inspector);
        self
    }

    /// Set the allowed root directories for absolute `output_dir` paths (#696).
    ///
    /// When empty (default), absolute `output_dir` values are rejected.
    /// When non-empty, an absolute `output_dir` must be under one of these
    /// roots. Relative paths are always allowed (they resolve against CWD).
    ///
    /// #769: when roots are configured, also checks the container's own
    /// `output_dir` — the write target of `process_export_pipeline`, which is
    /// the ONE export path that never goes through
    /// [`validate_export_dir`](Self::validate_export_dir). If it is absolute
    /// and outside every root, a tracing warning reports the inconsistency
    /// (the operator declared a boundary the server's own pipeline would
    /// violate). Deliberately warn-only: no new failure mode was added.
    #[must_use]
    pub fn with_export_roots(mut self, roots: Vec<PathBuf>) -> Self {
        if !roots.is_empty() {
            warn_if_configured_output_dir_outside_roots(
                &self.container.config().output_dir,
                &roots,
            );
        }
        self.allowed_export_roots = Arc::new(roots);
        self
    }

    /// Inject hermetic Obsidian detection paths for tests (#726).
    ///
    /// When set, Obsidian vault detection runs through
    /// [`detect_vault_hermetic`](webfang_core::infrastructure::obsidian::vault_detector::detect_vault_hermetic)
    /// rooted at `root` with the registry read from `registry_path`, so tests
    /// never touch the host's real Obsidian registry. Production leaves this
    /// `None` and keeps the default detection chain.
    #[must_use]
    pub fn with_obsidian_hermetic(
        mut self,
        root: impl Into<PathBuf>,
        registry_path: impl Into<PathBuf>,
    ) -> Self {
        self.obsidian_hermetic = Some((root.into(), registry_path.into()));
        self
    }

    /// Validate that `dir` is an allowed export destination (#696).
    ///
    /// Relative paths are always allowed (existing behavior — they resolve
    /// against the server's CWD). Absolute paths must be under one of
    /// [`allowed_export_roots`](Self::allowed_export_roots); when no roots
    /// are configured, absolute paths are rejected (fail-closed).
    ///
    /// # Errors
    /// Returns `McpError::invalid_params` when an absolute path is outside
    /// every configured root, or when no roots are configured at all.
    pub fn validate_export_dir(&self, dir: &Path) -> Result<(), McpError> {
        if !dir.is_absolute() {
            return Ok(());
        }
        let roots = self.allowed_export_roots.as_ref();
        if roots.is_empty() {
            tracing::warn!(dir = %dir.display(), "absolute output_dir rejected: no export roots configured");
            return Err(McpError::invalid_params(
                "absolute output_dir requires server-configured export roots (none configured); \
                 set --export-roots or use a relative path"
                    .to_string(),
                Some(Value::String("output_dir".to_string())),
            ));
        }
        // Normalize the candidate lexically: resolve `.` and `..` components
        // without touching the filesystem (the dir may not exist yet).
        // #769: shared pure helper [`absolute_path_within_roots`] owns the
        // lexical prefix check for every write path against the export roots.
        if absolute_path_within_roots(dir, roots) {
            Ok(())
        } else {
            tracing::warn!(dir = %dir.display(), "absolute output_dir outside allowed export roots");
            Err(McpError::invalid_params(
                format!(
                    "output_dir '{}' is outside allowed export roots",
                    dir.display()
                ),
                Some(Value::String("output_dir".to_string())),
            ))
        }
    }

    /// Record a scrape event into the shared accumulator.
    ///
    /// REQ-07: short synchronous critical section — the lock is acquired and
    /// dropped entirely within this call (the tracing event emitted inside
    /// `record` is synchronous), so it is never held across an `.await`.
    /// REQ-10: a poisoned mutex is recovered via `into_inner`, never a panic.
    pub fn record_scrape(&self, event: ScrapeEvent) {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .record(event);
    }

    /// Shared robots.txt gate for tools that fetch content directly (#749).
    ///
    /// Delegates to the single policy source
    /// ([`enforce_robots_policy`](webfang_core::application::scraper_service::enforce_robots_policy)):
    /// fail-open when the fetcher is absent, `WafBlocked(url, "robots.txt")`
    /// on denial. Callers receive the denial as an error payload and report it
    /// through their tool error envelope.
    pub async fn robots_denied_for(
        &self,
        url: &url::Url,
    ) -> Option<webfang_core::error::ScraperError> {
        let robots = self.robots_fetcher.as_deref();
        webfang_core::application::scraper_service::enforce_robots_policy(url, robots, false)
            .await
            .err()
    }

    /// Record a scrape event stamped with the tool call's run-root identity.
    ///
    /// The sole call-side helper for identified scrape metrics (#698): it owns
    /// the [`ScrapeEvent::identified`] construction so each MCP tool handler is
    /// a one-line record call. `start` is the operation's start instant and is
    /// converted to an elapsed [`Duration`](std::time::Duration) here, keeping
    /// timing bookkeeping in one place.
    pub fn record_scrape_identity(
        &self,
        tool: &'static str,
        domain: String,
        outcome: crate::mcp_server::metrics::Outcome,
        count: usize,
        start: std::time::Instant,
        correlation: &webfang_core::domain::CorrelationId,
    ) {
        let event =
            ScrapeEvent::identified(tool, domain, outcome, count, start.elapsed(), correlation);
        self.record_scrape(event);
    }

    /// Produce a point-in-time metrics snapshot from the shared accumulator.
    ///
    /// REQ-07: lock → clone snapshot → release. REQ-10: poison recovery, never
    /// a panic.
    #[must_use]
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }
}

/// Normalize a path lexically: resolve `.` and `..` components without
/// filesystem access. Used by [`absolute_path_within_roots`] so the prefix
/// check cannot be defeated by redundant components (#696).
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {},
            Component::ParentDir => {
                out.pop();
            },
            other => out.push(other),
        }
    }
    out
}

/// True when `path` is lexically under one of `roots` (#696).
///
/// Both sides are normalized via [`normalize_lexical`] before the check, so
/// redundant `.`/`..` components cannot defeat it. The match is
/// component-based ([`Path::starts_with`]), never a string prefix, so
/// sibling directories (`/srv/exports_evil` vs `/srv/exports`) do not match.
///
/// Empty `roots` always yields `false` — the CALLER owns the empty-roots
/// policy: [`McpState::validate_export_dir`] rejects absolute paths
/// (fail-closed) and [`McpState::with_export_roots`] skips its #769 startup
/// consistency check.
fn absolute_path_within_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let normalized = normalize_lexical(path);
    roots
        .iter()
        .any(|root| normalized.starts_with(normalize_lexical(root)))
}

/// Warn at startup when the container's configured `output_dir` lies outside
/// the declared export roots (#769).
///
/// `process_export_pipeline` exports to `container.scraper_config.output_dir`
/// and never goes through [`McpState::validate_export_dir`] (the #696 gate),
/// so this is the sole check for the operator's own `--output`-style config.
/// A relative `output_dir` (the `ScraperConfig::default()` `"output"`) is
/// skipped: like [`validate_export_dir`], it resolves against the server's
/// CWD and the server's own boundary does not apply. Warn-only by design —
/// no new rejection mode for a non-exploitable consistency gap.
fn warn_if_configured_output_dir_outside_roots(output_dir: &Path, roots: &[PathBuf]) {
    if !output_dir.is_absolute() {
        return;
    }
    if absolute_path_within_roots(output_dir, roots) {
        return;
    }
    tracing::warn!(
        output_dir = %output_dir.display(),
        roots = ?roots,
        "configured output_dir is outside the configured export roots: the server's own \
         export pipeline (process_export_pipeline) writes there without going through the \
         #696 export-root gate, violating the boundary the operator declared"
    );
}

/// Build the shared robots.txt fetcher for the scrape tools (#697).
///
/// Uses the container's configured download timeout. Construction failure is
/// NON-FATAL — the fetcher stays `None` and robots enforcement degrades to
/// "no fetcher available", mirroring the rate-limiter pattern in the core
/// container.
fn build_robots_fetcher(container: &Container) -> Option<Arc<dyn RobotsPort>> {
    match RobotsFetcher::with_default_profile(container.config().download_timeout_secs) {
        Ok(fetcher) => Some(Arc::new(fetcher)),
        Err(e) => {
            tracing::warn!(error = %e, "robots_fetcher_init_failed_non_fatal");
            None
        },
    }
}

impl CategorySemaphores {
    /// Build semaphores from per-category concurrency limits.
    ///
    /// The mapping is 1:1 (#1132): the old `max(1)` clamp is gone because
    /// `CategoryLimits` fields are [`NonZeroUsize`] — a zero-permit
    /// semaphore cannot be named, so there is nothing left to clamp.
    pub fn from_limits(limits: &CategoryLimits) -> Self {
        Self {
            ai: Arc::new(Semaphore::new(limits.ai.get())),
            scraping: Arc::new(Semaphore::new(limits.scraping.get())),
            export: Arc::new(Semaphore::new(limits.export.get())),
            obsidian: Arc::new(Semaphore::new(limits.obsidian.get())),
            content: Arc::new(Semaphore::new(limits.content.get())),
            url_utils: Arc::new(Semaphore::new(limits.url_utils.get())),
            security: Arc::new(Semaphore::new(limits.security.get())),
            assets: Arc::new(Semaphore::new(limits.assets.get())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;
    use tracing::field::{Field, Visit};
    use tracing::Level;
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::*;
    use webfang_core::domain::config::ScraperConfig;
    use webfang_core::domain::CrawlerConfig;
    use webfang_core::infrastructure::scraper::dom_inspector::NoOpInspector;

    /// Build a minimal Container for testing (async, needs a temp output dir).
    async fn test_container() -> (TempDir, Container) {
        let tmp = TempDir::new().expect("create temp dir");
        let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
        let scraper_config = ScraperConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let container = Container::new(crawler_config, scraper_config)
            .await
            .expect("create test container");
        (tmp, container)
    }

    #[test]
    fn test_default_limits_are_reasonable() {
        let limits = CategoryLimits::default();
        assert!(
            limits.ai.get() >= 1,
            "AI limit must allow at least 1 concurrent"
        );
        assert!(
            limits.scraping.get() >= 1,
            "Scraping limit must allow at least 1"
        );
        assert!(
            limits.ai < limits.scraping,
            "AI should be more restricted than scraping"
        );
    }

    #[test]
    fn test_semaphores_created_with_correct_permits() {
        let limits = CategoryLimits::default();
        let semaphores = CategorySemaphores::from_limits(&limits);
        assert_eq!(semaphores.ai.available_permits(), limits.ai.get());
        assert_eq!(
            semaphores.scraping.available_permits(),
            limits.scraping.get()
        );
        assert_eq!(
            semaphores.obsidian.available_permits(),
            limits.obsidian.get()
        );
    }

    #[tokio::test]
    async fn test_new_state_has_no_inspector() {
        let (_tmp, container) = test_container().await;
        let state = McpState::new(container);
        assert!(state.inspector.is_none(), "inspector must default to None");
    }

    #[tokio::test]
    async fn test_with_inspector_sets_inspector() {
        let (_tmp, container) = test_container().await;
        let inspector: Arc<dyn DomInspectorPort> = Arc::new(NoOpInspector);
        let state = McpState::new(container).with_inspector(inspector);
        assert!(
            state.inspector.is_some(),
            "inspector must be set after with_inspector"
        );
    }

    #[tokio::test]
    async fn test_with_limits_has_no_inspector() {
        let (_tmp, container) = test_container().await;
        let state = McpState::with_limits(container, CategoryLimits::default());
        assert!(
            state.inspector.is_none(),
            "inspector must default to None in with_limits"
        );
    }

    /// REQ-06: the metrics accumulator is shared across per-session clones
    /// (`server.rs` clones `McpState` per session). Mirrors the `Arc::ptr_eq`
    /// pattern from `server.rs::test_mcp_state_with_downloader`.
    #[tokio::test]
    async fn metrics_shared_across_clones() {
        use crate::mcp_server::metrics::Outcome;
        use std::time::Duration;

        let (_tmp, container) = test_container().await;
        let state = McpState::new(container);
        let state2 = state.clone();

        assert!(
            Arc::ptr_eq(&state.metrics, &state2.metrics),
            "cloned McpState must share the same metrics Arc"
        );

        // A scrape recorded through one clone is visible from the other.
        state.record_scrape(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            2,
            Duration::from_millis(10),
        ));

        let snap = state2.metrics_snapshot();
        assert_eq!(snap.total_events, 1, "record-through-clone is visible");
        assert_eq!(snap.success_count, 1, "success bucket recorded");
    }
    // --- validate_export_dir (#696) ---

    #[tokio::test]
    async fn export_dir_relative_always_allowed() {
        let (_tmp, container) = test_container().await;
        let state = McpState::new(container); // no roots configured
        assert!(
            state.validate_export_dir(Path::new("./output")).is_ok(),
            "relative paths must pass through with no roots configured"
        );
    }

    #[tokio::test]
    async fn export_dir_absolute_rejected_without_roots() {
        let (_tmp, container) = test_container().await;
        let state = McpState::new(container); // fail-closed default
        let err = state
            .validate_export_dir(Path::new("/etc"))
            .expect_err("absolute path must be rejected with no roots");
        let msg = err.to_string();
        assert!(
            msg.contains("export roots"),
            "error must name the missing export roots, got: {msg}"
        );
    }

    #[tokio::test]
    async fn export_dir_absolute_allowed_under_root() {
        let (tmp, container) = test_container().await;
        let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
        let target = tmp.path().join("exports");
        assert!(
            state.validate_export_dir(&target).is_ok(),
            "path under a configured root must be allowed"
        );
    }

    #[tokio::test]
    async fn export_dir_absolute_rejected_outside_root() {
        let (tmp, container) = test_container().await;
        let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
        let err = state
            .validate_export_dir(Path::new("/etc"))
            .expect_err("path outside roots must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("outside allowed export roots"),
            "error must name the root violation, got: {msg}"
        );
    }

    #[tokio::test]
    async fn export_dir_sibling_prefix_is_not_a_root_match() {
        // `/tmp/rootX` must NOT satisfy root `/tmp/root` — the prefix check
        // is component-based (`starts_with`), not string-based.
        let (tmp, container) = test_container().await;
        let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
        let mut sibling = tmp.path().to_path_buf();
        let mut name = sibling
            .file_name()
            .expect("temp dir has a name")
            .to_os_string();
        name.push("_evil");
        sibling.set_file_name(name);
        assert!(
            state.validate_export_dir(&sibling).is_err(),
            "string-prefix sibling dir must not match the root"
        );
    }

    #[tokio::test]
    async fn export_dir_dotdot_cannot_escape_root() {
        // Lexical normalization resolves `..` before the prefix check, so
        // `<root>/../etc` is rejected even though it starts with the root.
        let (tmp, container) = test_container().await;
        let state = McpState::new(container).with_export_roots(vec![tmp.path().to_path_buf()]);
        let mut escape = tmp.path().to_path_buf();
        escape.push("..");
        escape.push("etc");
        assert!(
            state.validate_export_dir(&escape).is_err(),
            "`..` traversal out of the root must be rejected"
        );
    }

    // --- absolute_path_within_roots (#769 pure helper) ---

    /// Empty `roots` is handled BY THE CALLER (fail-closed rejection in
    /// `validate_export_dir`; the #769 startup check skips it) — the helper
    /// itself must never match.
    #[test]
    fn within_roots_empty_roots_never_matches() {
        assert!(
            !absolute_path_within_roots(Path::new("/srv/exports/file.txt"), &[]),
            "empty roots must yield false — the caller owns the empty-roots policy"
        );
    }

    #[test]
    fn within_roots_absolute_outside_returns_false() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            !absolute_path_within_roots(Path::new("/etc/passwd"), &roots),
            "a path outside every root must yield false"
        );
    }

    #[test]
    fn within_roots_absolute_inside_root_returns_true() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            absolute_path_within_roots(Path::new("/srv/exports/sub/file.txt"), &roots),
            "a path under a root must yield true"
        );
        // The root itself is a valid destination.
        assert!(
            absolute_path_within_roots(Path::new("/srv/exports"), &roots),
            "the root path itself must yield true"
        );
    }

    #[test]
    fn within_roots_dotdot_cannot_defeat_the_prefix_check() {
        let roots = vec![PathBuf::from("/srv/exports")];
        // The raw string starts with the root, but lexical normalization
        // resolves `..` first, landing at `/srv/other`.
        assert!(
            !absolute_path_within_roots(Path::new("/srv/exports/../other"), &roots),
            "`..` traversal out of the root must yield false"
        );
        // A candidate that traverses `..` and lands back INSIDE the root
        // stays allowed.
        assert!(
            absolute_path_within_roots(Path::new("/srv/exports/sub/../../exports/file"), &roots),
            "`..` segments that resolve back inside the root must yield true"
        );
    }

    #[test]
    fn within_roots_dot_components_are_ignored() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            absolute_path_within_roots(Path::new("/srv/./exports/./file.txt"), &roots),
            "redundant `.` components must not defeat a valid match"
        );
    }

    #[test]
    fn within_roots_sibling_prefix_is_not_a_root_match() {
        // `/srv/exports_evil` must NOT satisfy root `/srv/exports` — the
        // prefix check is component-based (`starts_with`), not string-based.
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            !absolute_path_within_roots(Path::new("/srv/exports_evil/file"), &roots),
            "a string-prefix sibling dir must yield false"
        );
    }

    // --- with_export_roots startup consistency check (#769) ---

    static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

    /// Set a global fmt subscriber (sink writer) so every `tracing` callsite
    /// registers with `Interest::always()` instead of the `Interest::never()`
    /// that gets cached process-wide when a thread hits a callsite with no
    /// subscriber active — same guard as
    /// `metrics.rs::ensure_global_subscriber`.
    fn ensure_global_subscriber() {
        GLOBAL_SUBSCRIBER_INIT.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });
    }

    /// One captured tracing event: level, message, `(field, value)` pairs.
    struct CapturedEvent {
        level: Level,
        message: String,
        fields: Vec<(String, String)>,
    }

    /// Collects `(field_name, value)` pairs as strings; the message field is
    /// recorded as a plain field by `tracing`.
    struct FieldCapture {
        sink: Arc<Mutex<Vec<(String, String)>>>,
        message: Arc<Mutex<Option<String>>>,
    }

    impl Visit for FieldCapture {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                // `format!("{:?}")` on the message args yields the quoted
                // string; strip the outer quotes so `.contains()` matches the
                // raw English message verbatim.
                let raw = format!("{value:?}");
                *self.message.lock().expect("message mutex") =
                    Some(raw.trim_matches('"').to_string());
            } else {
                self.sink
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), format!("{value:?}")));
            }
        }
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                *self.message.lock().expect("message mutex") = Some(value.to_string());
            } else {
                self.sink
                    .lock()
                    .expect("capture mutex")
                    .push((field.name().to_string(), value.to_string()));
            }
        }
    }

    struct EventCaptureLayer {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl<S: tracing::Subscriber> Layer<S> for EventCaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let fields = Arc::new(Mutex::new(Vec::new()));
            let message = Arc::new(Mutex::new(None));
            let mut visitor = FieldCapture {
                sink: Arc::clone(&fields),
                message: Arc::clone(&message),
            };
            event.record(&mut visitor);
            let message = message
                .lock()
                .expect("message mutex")
                .take()
                .unwrap_or_default();
            let fields = fields.lock().expect("capture mutex").split_off(0);
            let mut events = self.events.lock().expect("capture mutex");
            events.push(CapturedEvent {
                level: *event.metadata().level(),
                message,
                fields,
            });
        }
    }

    /// Run `body` under a capturing subscriber and return `(result, events)`.
    fn capture_events_during<T>(body: impl FnOnce() -> T) -> (T, Vec<CapturedEvent>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(EventCaptureLayer {
            events: Arc::clone(&events),
        });
        let result = tracing::subscriber::with_default(subscriber, body);
        let captured = events.lock().expect("capture mutex").split_off(0);
        (result, captured)
    }

    /// Build a container whose `ScraperConfig::output_dir` is `output_dir`.
    async fn container_with_output_dir(output_dir: PathBuf) -> Container {
        let crawler_config =
            CrawlerConfig::new(url::Url::parse("https://example.com").expect("valid url"));
        let scraper_config = ScraperConfig {
            output_dir,
            ..Default::default()
        };
        Container::new(crawler_config, scraper_config)
            .await
            .expect("create test container")
    }

    /// The `#769` warning is identified by its message; unrelated warns from
    /// container construction (e.g. robots_fetcher init) never match it.
    const OUTSIDE_ROOTS_WARN: &str = "outside the configured export roots";

    #[tokio::test]
    #[serial]
    async fn with_export_roots_warns_when_output_dir_outside_roots() {
        ensure_global_subscriber();
        let root = TempDir::new().expect("create root temp dir");
        let outside = TempDir::new().expect("create outside temp dir");
        let outside_path = outside.path().display().to_string();
        let container = container_with_output_dir(outside.path().to_path_buf()).await;
        let state = McpState::new(container);

        let (_state, events) =
            capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));

        let warns: Vec<&CapturedEvent> = events
            .iter()
            .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
            .collect();
        assert_eq!(warns.len(), 1, "exactly one #769 warning must fire");
        let warn = warns[0];
        // Structured English fields, not string soup.
        let field = |name: &str| {
            warn.fields
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            field("output_dir").as_deref(),
            Some(outside_path.as_str()),
            "output_dir field must carry the configured directory"
        );
        assert!(
            field("roots")
                .expect("roots field must be present")
                .contains(
                    &*root
                        .path()
                        .file_name()
                        .expect("root has a name")
                        .to_string_lossy()
                ),
            "roots field must name the configured root"
        );
    }

    #[tokio::test]
    #[serial]
    async fn with_export_roots_no_warn_when_output_dir_inside_roots() {
        ensure_global_subscriber();
        let root = TempDir::new().expect("create temp dir");
        let container = container_with_output_dir(root.path().to_path_buf()).await;
        let state = McpState::new(container);

        let (_state, events) =
            capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));

        let stray = events
            .iter()
            .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
            .count();
        assert_eq!(stray, 0, "an output_dir under a root must not warn");
    }

    #[tokio::test]
    #[serial]
    async fn with_export_roots_no_warn_when_output_dir_relative() {
        ensure_global_subscriber();
        // Relative `output_dir` resolves against the server CWD — the same
        // policy as `validate_export_dir`: no warning, regardless of roots.
        // The container opens its crawl log lazily, so a relative path
        // performs no file I/O here (container.rs #606).
        let root = TempDir::new().expect("create root temp dir");
        let container = container_with_output_dir(PathBuf::from("output")).await;
        assert!(
            container.config().output_dir.is_relative(),
            "fixture must keep a relative output_dir"
        );
        let state = McpState::new(container);

        let (state, events) =
            capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));
        drop(state);

        let stray = events
            .iter()
            .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
            .count();
        assert_eq!(stray, 0, "a relative output_dir must not warn");
    }
}
