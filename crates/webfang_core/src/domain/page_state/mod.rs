//! Typed page lifecycle state machine (`page_state`).
//!
//! Two-layer typestate design (D1, frozen):
//!
//! - **Layer 1** ([`status::PageStatus`]) — plain serde enum, the ONLY
//!   serialized form. Persisted records may hold ANY status after a crash;
//!   that is honest durable truth.
//! - **Layer 2** ([`typed`]) — zero-sized state markers + [`typed::Stateful`]
//!   phantom wrapper. Legal transitions are methods defined only on
//!   source-state impls; illegal transitions do not compile (SC1, proven by
//!   the trybuild suite in `tests/compile_fail/page_state/`).
//!
//! Runtime logic exists ONLY at the persistence seam: reconstructing a
//! `Stateful` from raw persisted data validates the record's invariants once
//! (per-state `TryFrom`, see [`typed::PersistedRecord`] and
//! [`typed::ReconcileError`], added at the boundary task of PR1).
//! In-memory code physically cannot express an illegal move.
//!
//! # Legacy partial encodings (R1 mitigation)
//!
//! The three legacy state encodings map onto this machine one-way; none
//! remains an independent source of truth for resume decisions:
//!
//! | Legacy encoding | Mapping |
//! |---|---|
//! | `ExportState.processed_urls: Vec<String>` (persisted v1) | RETIRED. Survives only as input to the v1→v2 migration (PR2): every entry becomes a `COMMITTED` record with `run_id = "migrated-v1"`. |
//! | `ScrapeStatus` (memory-only display) | Display projection read by the TUI — `Pending → Discovered/Queued`, `Downloading → Fetched`, `Fetching → Fetching`, `Extracting → Extracted`, `Completed → Committed`, `Failed → any state + last_error set`. Never consulted for resume. |
//! | `CrawlCheckpoint.visited/queued` (engine) | UNTOUCHED (A5: Engine-API only). Maps conceptually onto `Fetched+` / `Queued`; no longer consulted for ANY resume decision after PR3. |

pub mod status;
pub mod typed;

pub use status::PageStatus;
pub use typed::{StateMarker, Stateful};
pub use typed::{
    Committed, Discovered, Extracted, Exported, Fetched, Fetching, Processed, Queued,
};
