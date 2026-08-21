//! Layer 1 — persisted truth: the eight-state page lifecycle as a plain
//! serde enum.
//!
//! This is the ONLY thing serialized. It can hold any value after a crash —
//! by design. Compile-time transition safety lives one layer up in
//! [`super::typed`]; the boundary between the two is reconciled once at load
//! time (see [`super::typed::PersistedRecord`]).

use serde::{Deserialize, Serialize};

/// The full page lifecycle: DISCOVERED → QUEUED → FETCHING → FETCHED →
/// EXTRACTED → PROCESSED → EXPORTED → COMMITTED.
#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PageStatus {
    /// URL discovered but not yet scheduled.
    Discovered,
    /// Scheduled into the fetch queue.
    Queued,
    /// Fetch in flight.
    Fetching,
    /// Raw content downloaded.
    Fetched,
    /// Content extracted from the raw payload.
    Extracted,
    /// Pipeline processing complete, awaiting export.
    Processed,
    /// Output bytes flushed; commit checkpoint not yet persisted (D3).
    Exported,
    /// Commit point reached: output flushed AND committed status persisted.
    /// The only state that permits skip-on-resume.
    Committed,
}
