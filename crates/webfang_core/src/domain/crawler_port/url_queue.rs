//! URL queue port — domain seam for the discovery queue (ADR-0012-B unit 8).
//!
//! `application::crawler::*` consumed the concrete
//! `infrastructure::crawler::url_queue::UrlQueue` through struct fields, an
//! `Arc`-shared wiring, and a dead re-export shim. This port inverts the
//! edge (ADR-0012-B §2.1): application consumes [`UrlQueuePort`], the
//! concrete (enqueue-time dedup + priority machinery) stays in
//! infrastructure, and the composition root
//! (`application::container::build_url_queue`) names it.
//!
//! The surface is exactly the production consumption set
//! (`push_prioritized`, `snapshot_urls`, `drain_all`); test-only methods
//! (`push`, `pop`, `len`, `is_empty`, `peek`, `seen_count`, `clear`,
//! `get_all`) stay on the concrete.
//!
//! # Async desugaring
//!
//! Manual `BoxFuture` desugaring per the repo's frozen decision #1 (see
//! [`crate::domain::downloader_port::Downloader`]).

use std::collections::VecDeque;

use futures::future::BoxFuture;

use crate::domain::crawler_port::UrlSource;
use crate::domain::DiscoveredUrl;

/// Asynchronous discovery-queue surface.
///
/// Implemented in `infrastructure::crawler::url_queue` by
/// [`UrlQueue`](crate::infrastructure::crawler::UrlQueue); `Arc`-shared
/// between the crawl scheduler and the per-page tasks exactly as the
/// concrete was, so dedup state stays shared across the crawl.
pub trait UrlQueuePort: Send + Sync {
    /// Enqueue a URL with its discovery source. Enqueue-time dedup applies:
    /// returns `false` when the URL was already seen, `true` when newly
    /// enqueued.
    fn push_prioritized<'a>(&'a self, url: DiscoveredUrl, source: UrlSource)
        -> BoxFuture<'a, bool>;

    /// Snapshot the URLs currently visible to the scheduler.
    fn snapshot_urls<'a>(&'a self) -> BoxFuture<'a, Vec<String>>;

    /// Drain the whole queue (leaves it empty), preserving priority order.
    fn drain_all<'a>(&'a self) -> BoxFuture<'a, VecDeque<DiscoveredUrl>>;

    /// Number of pending (not-yet-drained) entries.
    fn len<'a>(&'a self) -> BoxFuture<'a, usize>;

    /// True when the queue has no pending entries.
    fn is_empty<'a>(&'a self) -> BoxFuture<'a, bool>;
}
