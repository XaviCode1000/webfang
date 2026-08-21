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

use std::convert::TryFrom;
use std::fmt;
use std::marker::PhantomData;
use std::path::PathBuf;

use super::status::PageStatus;

mod sealed {
    /// Sealing trait: only the eight marker structs below may implement it.
    pub trait Sealed {}
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
