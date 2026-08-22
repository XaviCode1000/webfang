//! Layer 2 — compile-time transition safety over the page lifecycle.
//!
//! Zero-sized marker structs ([`Discovered`] … [`Committed`]) implement the
//! sealed [`StateMarker`] trait; [`Stateful<R, S>`] wraps a record together
//! with its state in the type system. Transition methods exist ONLY on the
//! source-state impl, so every illegal move (skip, backward except
//! [`Stateful::reopen_for_reexport`], self-transition, anything out of
//! COMMITTED) has no method to call and fails compilation — SC1.
//!
//! The trybuild suite under `tests/compile_fail/page_state/` is the
//! executable proof that illegal transitions do not compile.

use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;

use super::status::PageStatus;

mod sealed {
    /// Sealing trait: only the eight marker structs below may implement it.
    pub trait Sealed {}
}

/// Read-only view over a raw persisted record, exposing exactly the fields
/// the load-time validation table reads (D2).
///
/// PR2's 9-field `RawRecord` implements this once; the reconciliation below
/// stays generic so the store plugs in without edits here.
pub trait PersistedRecord {
    /// Persisted lifecycle status.
    fn status(&self) -> PageStatus;

    /// Output path recorded at the EXPORTED checkpoint.
    fn output_location(&self) -> Option<&str>;

    /// Hash of the serialized payload (dedup/reconciliation key).
    fn content_hash(&self) -> Option<&str>;

    /// Whether a classified error is attached.
    fn has_last_error(&self) -> bool;

    /// True attempt count.
    fn attempts(&self) -> u32;
}

/// Why a persisted record could not be reconstructed into its typed state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconcileError {
    /// The record's persisted status does not match the target state.
    #[error("record status {found:?} cannot reconcile into state {expected:?}")]
    StatusMismatch {
        /// State being reconstructed.
        expected: PageStatus,
        /// Status actually persisted on the record.
        found: PageStatus,
    },
    /// `Exported`/`Committed` records must carry an output location.
    #[error("status {0:?} requires output_location")]
    MissingOutputLocation(PageStatus),
    /// `Exported`/`Committed` records must carry a content hash.
    #[error("status {0:?} requires content_hash")]
    MissingContentHash(PageStatus),
    /// Committed items carry null last_error by contract.
    #[error("COMMITTED record must carry no last_error")]
    CommittedWithLastError,
    /// A committed item was driven at least once.
    #[error("COMMITTED record requires attempts >= 1")]
    CommittedWithZeroAttempts,
}

/// Ties a zero-sized marker type to its persisted [`PageStatus`].
///
/// Sealed: downstream crates cannot invent new lifecycle states.
pub trait StateMarker: sealed::Sealed {
    /// The persisted status this marker represents.
    const STATUS: PageStatus;
}

macro_rules! markers {
    ($($name:ident => $variant:ident),+ $(,)?) => {$(
        #[doc = concat!("Marker for the `", stringify!($variant), "` lifecycle state.")]
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        pub struct $name;

        impl sealed::Sealed for $name {}

        impl StateMarker for $name {
            const STATUS: PageStatus = PageStatus::$variant;
        }
    )+};
}

markers!(
    Discovered => Discovered,
    Queued => Queued,
    Fetching => Fetching,
    Fetched => Fetched,
    Extracted => Extracted,
    Processed => Processed,
    Exported => Exported,
    Committed => Committed,
);

/// A record whose lifecycle position is tracked in the type system.
///
/// `S` is a phantom parameter: a `Stateful<R, Discovered>` and a
/// `Stateful<R, Committed>` hold identical data but are distinct types, and
/// only legal transitions between them exist as methods.
pub struct Stateful<R, S: StateMarker> {
    record: R,
    _marker: PhantomData<S>,
}

impl<R, S: StateMarker> fmt::Debug for Stateful<R, S>
where
    R: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stateful")
            .field("status", &S::STATUS)
            .field("record", &self.record)
            .finish()
    }
}

impl<R: Clone, S: StateMarker> Clone for Stateful<R, S> {
    fn clone(&self) -> Self {
        Self {
            record: self.record.clone(),
            _marker: PhantomData,
        }
    }
}

impl<R: PartialEq, S: StateMarker> PartialEq for Stateful<R, S> {
    fn eq(&self, other: &Self) -> bool {
        self.record == other.record
    }
}

impl<R: Eq, S: StateMarker> Eq for Stateful<R, S> {}

impl<R, S: StateMarker> Stateful<R, S> {
    /// The persisted status this value is proven to be in.
    pub fn status(&self) -> PageStatus {
        S::STATUS
    }

    /// Read-only access to the wrapped record.
    pub fn record(&self) -> &R {
        &self.record
    }

    /// Mutable access to record *payload* fields without advancing state
    /// (e.g. persisting `last_error` / `attempts` at a non-advanced state).
    pub fn record_mut(&mut self) -> &mut R {
        &mut self.record
    }

    /// Consume the wrapper, returning the bare record.
    pub fn into_record(self) -> R {
        self.record
    }

    fn advance<S2: StateMarker>(self) -> Stateful<R, S2> {
        Stateful {
            record: self.record,
            _marker: PhantomData,
        }
    }
}

impl<R> Stateful<R, Discovered> {
    /// Start of the lifecycle: wrap a freshly discovered record.
    pub const fn new(record: R) -> Self {
        Self {
            record,
            _marker: PhantomData,
        }
    }

    /// DISCOVERED → QUEUED.
    #[must_use]
    pub fn queue(self) -> Stateful<R, Queued> {
        self.advance()
    }
}

impl<R> Stateful<R, Queued> {
    /// QUEUED → FETCHING.
    #[must_use]
    pub fn start_fetch(self) -> Stateful<R, Fetching> {
        self.advance()
    }
}

impl<R> Stateful<R, Fetching> {
    /// FETCHING → FETCHED.
    #[must_use]
    pub fn fetched(self) -> Stateful<R, Fetched> {
        self.advance()
    }
}

impl<R> Stateful<R, Fetched> {
    /// FETCHED → EXTRACTED.
    #[must_use]
    pub fn extracted(self) -> Stateful<R, Extracted> {
        self.advance()
    }
}

impl<R> Stateful<R, Extracted> {
    /// EXTRACTED → PROCESSED.
    #[must_use]
    pub fn processed(self) -> Stateful<R, Processed> {
        self.advance()
    }
}

impl<R> Stateful<R, Processed> {
    /// PROCESSED → EXPORTED, after the output flush barrier acked (D3 step 1).
    ///
    /// `output_location` is recorded on the persisted record at the
    /// infrastructure boundary; the typestate move itself carries no payload.
    #[must_use]
    pub fn export_flushed(self, _output_location: PathBuf) -> Stateful<R, Exported> {
        self.advance()
    }
}

impl<R> Stateful<R, Exported> {
    /// EXPORTED → COMMITTED — the commit point (D3 step 4); only reachable
    /// once the EXPORTED checkpoint was durably persisted.
    #[must_use]
    pub fn commit(self) -> Stateful<R, Committed> {
        self.advance()
    }

    /// EXPORTED → PROCESSED — THE one backward transition, reserved for the
    /// re-export recovery path. No other backward method exists.
    #[must_use]
    pub fn reopen_for_reexport(self) -> Stateful<R, Processed> {
        self.advance()
    }
}

// `Stateful<R, Committed>` deliberately has NO transition methods: COMMITTED
// is terminal in the type system.

mod reconcile {
    use super::{PageStatus, PersistedRecord, ReconcileError};

    /// D2 load-time invariant table — the single validation point between
    /// disk truth and the typed machine.
    pub(super) fn validate<R: PersistedRecord>(
        record: &R,
        target: PageStatus,
    ) -> Result<(), ReconcileError> {
        if record.status() != target {
            return Err(ReconcileError::StatusMismatch {
                expected: target,
                found: record.status(),
            });
        }
        if matches!(target, PageStatus::Exported | PageStatus::Committed) {
            record
                .output_location()
                .ok_or(ReconcileError::MissingOutputLocation(target))?;
            record
                .content_hash()
                .ok_or(ReconcileError::MissingContentHash(target))?;
        }
        if target == PageStatus::Committed {
            if record.has_last_error() {
                return Err(ReconcileError::CommittedWithLastError);
            }
            if record.attempts() < 1 {
                return Err(ReconcileError::CommittedWithZeroAttempts);
            }
        }
        Ok(())
    }
}

macro_rules! impl_state_reconcile {
    ($($marker:ident),+ $(,)?) => {$(
        impl<R: PersistedRecord> Stateful<R, $marker> {
            /// Boundary reconciliation (D1 crux): reconstruct this exact
            /// state from raw persisted data after validating the D2
            /// invariant table. The single validation point between disk
            /// truth and the typed machine.
            ///
            /// Deliberately NOT `TryFrom`: `impl TryFrom<R> for
            /// Stateful<R, S>` conflicts with std's reflexive
            /// `impl<T> From<T> for T` blanket under coherence.
            pub fn reconcile(record: R) -> Result<Self, ReconcileError> {
                reconcile::validate(&record, <$marker as StateMarker>::STATUS)?;
                Ok(Self {
                    record,
                    _marker: PhantomData,
                })
            }
        }
    )+};
}

impl_state_reconcile!(
    Discovered, Queued, Fetching, Fetched, Extracted, Processed, Exported, Committed,
);
