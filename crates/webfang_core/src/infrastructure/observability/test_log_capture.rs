//! Shared capture harness for `tracing` events in `#[cfg(test)]` modules.
//!
//! Six test modules across this crate used to define their own copy of the
//! same thing: a `MakeWriter` writing into a shared `Vec<u8>` behind a mutex,
//! a write guard, and a `fmt` subscriber built on top. The copies had already
//! drifted — some poisoned the mutex with `unwrap()`, some with `expect()`,
//! some kept it alive — and a seventh copy in a new test pushed the workspace
//! duplication ratchet over its baseline.
//!
//! One definition lives here so a test that needs to assert on emitted
//! tracing events gets the same capture semantics everywhere.
//!
//! # Choosing an entry point
//!
//! Every capture is the same `fmt` subscriber writing into a [`LogBuffer`]
//! with ANSI off. They differ only in *scope* and in whether span events are
//! recorded, so the two axes are explicit in the constructor names:
//!
//! | Entry point | Subscriber scope | Span events | Used by |
//! | --- | --- | --- | --- |
//! | [`LogBuffer::capture`] | thread-local, for as long as the returned guard | no | tests that keep capturing after the code under test returns |
//! | [`LogBuffer::run`] | only while the closure runs | no | tests that scope the capture to an exact block and assert afterwards |
//! | [`LogBuffer::run_with_span_events`] | only while the closure runs | yes (`FmtSpan::NEW`) | tests that assert on span *creation*, not only on emitted events |
//!
//! Do not add a level filter, a custom formatter or a per-call-site writer
//! here to accommodate one test: those tests get a named variant of this
//! module instead, so the difference stays visible at the call site.

use std::sync::{Arc, Mutex};

/// Shared buffer a captured subscriber writes into.
///
/// Cheap to clone: every clone is another handle on the same bytes, which is
/// what lets a subscriber be built from a clone while the test keeps the
/// original for reading.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<u8>>>);

/// Guard handed to `fmt` so each write goes through the shared buffer.
///
/// Public only because it is the `Writer` associated type of the `MakeWriter`
/// impl below; it is not part of the helper's usable surface.
pub struct WriteGuard(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for WriteGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log capture buffer poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = WriteGuard;

    fn make_writer(&'a self) -> Self::Writer {
        WriteGuard(Arc::clone(&self.0))
    }
}

/// Build the shared `fmt` subscriber every entry point below installs.
fn capturing_subscriber(buffer: &LogBuffer, span_events: bool) -> impl tracing::Subscriber {
    let builder = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false);
    let builder = if span_events {
        builder.with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
    } else {
        builder
    };
    builder.finish()
}

impl LogBuffer {
    /// Install a capturing subscriber for the current thread and return its
    /// buffer together with the guard that keeps it installed.
    ///
    /// The subscriber lives only as long as the returned guard: it is set as
    /// the thread-local default, so a test that drops the guard restores the
    /// previous subscriber instead of leaking this one into the next test.
    ///
    /// Span events are *not* recorded — use [`LogBuffer::run_with_span_events`]
    /// for a test that asserts on span creation.
    pub fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let buffer = Self::default();
        let guard = tracing::subscriber::set_default(capturing_subscriber(&buffer, false));
        (buffer, guard)
    }

    /// Run `body` with a capturing subscriber installed for the current
    /// thread, then return the buffer with everything it captured.
    ///
    /// This is the scoped counterpart of [`LogBuffer::capture`]: the
    /// subscriber is uninstalled when `body` returns, so assertions made
    /// afterwards run with the thread's previous subscriber restored. Use it
    /// when the capture must cover exactly one block.
    pub fn run(body: impl FnOnce()) -> Self {
        let buffer = Self::default();
        tracing::subscriber::with_default(capturing_subscriber(&buffer, false), body);
        buffer
    }

    /// Run `body` with a capturing subscriber that also records span events
    /// (`FmtSpan::NEW`), then return the buffer with everything it captured.
    ///
    /// Identical to [`LogBuffer::run`] except that span *creation* and
    /// *close* are rendered as lines, which is what lets a test assert that a
    /// span was emitted at all.
    pub fn run_with_span_events(body: impl FnOnce()) -> Self {
        let buffer = Self::default();
        tracing::subscriber::with_default(capturing_subscriber(&buffer, true), body);
        buffer
    }

    /// Everything captured so far, lossy-decoded.
    ///
    /// Returns the empty string rather than panicking when the mutex is
    /// poisoned: a failed assertion is the test's business, not this helper's.
    /// A poisoned buffer therefore surfaces as an assertion failure showing an
    /// empty output, never as a second, unrelated panic.
    pub fn text(&self) -> String {
        match self.0.lock() {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => String::new(),
        }
    }
}
