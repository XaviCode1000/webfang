//! Parse-time diagnostics recorded before logging exists (#1431).
//!
//! Argument normalization runs BEFORE `init_logging_dual` installs the tracing
//! subscriber (hoisted to step 6b2 in `webfang_cli::main` by #796), so any
//! `tracing::warn!` emitted from argument parsing is silently dropped (there is
//! no subscriber yet). Diagnostics that must stay visible are instead
//! [`record`]ed here at parse time and replayed once by the binary,
//! immediately after `init_logging_dual`, as regular `warn!` events.
//!
//! # Vestigial since #1813
//!
//! This module was built for one caller: the `--rate-limit-burst`
//! warn-and-default substitution notice, which needed a pre-subscriber home
//! because the burst was parsed TWICE per invocation (preflight staging plus
//! the `From<Args>` projection) and had to be de-duplicated. #1813 removed
//! both: the burst now fails closed in `parse_rate_limit_burst`, and
//! `From<Args>` no longer parses it, so the replay path has no producer.
//!
//! [`record`] and the buffer are kept, not deleted, because
//! `webfang_cli::main` still calls [`take`] on every run (removing the call
//! site is outside #1813's edit surface) and a future parse-time
//! warn-and-default would need this seam again. The infrastructure tests below
//! keep the contract honest in the meantime.
//!
//! The store is a process-scoped append-only buffer behind a
//! `OnceLock<Mutex<Vec<String>>>`: writers never block on async code, a
//! poisoned mutex degrades to `into_inner()` instead of panicking, and
//! [`take`] drains the buffer so each note is replayed exactly once. There is
//! deliberately no flag, no config key, and no second logging path —
//! collection and replay are the same single pipeline, split in time.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Append-only buffer for parse-time diagnostic notes.
///
/// Initialized lazily on first use; lives for the whole process.
static NOTES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// Access the note buffer, initializing it on first call.
///
/// The `OnceLock` is only ever set here with an empty buffer, so
/// `get_or_init` always succeeds without blocking beyond the init race.
fn notes() -> &'static Mutex<Vec<String>> {
    NOTES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Acquire the note buffer, recovering deliberately from a poisoning panic.
///
/// `PoisonError::into_inner()` returns the SAME guard an uncontended lock would:
/// poisoning only records that a previous holder panicked while it was inside
/// the critical section, so this call still owns the buffer exclusively and
/// every read and write made through the returned guard stays race-free. One
/// acquisition point, one place where that is stated — `record` and `take` are
/// both ordinary guard users and differ only in what they do with it.
///
/// Recovery rather than panic-on-lock is intentional: losing one operator
/// diagnostic must never abort argument handling. The invariant is pinned by
/// `notes_survive_a_poisoned_mutex`, which poisons the mutex on purpose and
/// then asserts the note still comes back out of `take`.
fn lock_buffer() -> MutexGuard<'static, Vec<String>> {
    match notes().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Record a parse-time diagnostic note for later replay.
pub fn record(note: impl Into<String>) {
    let note = note.into();
    let mut guard = lock_buffer();
    if !guard.contains(&note) {
        guard.push(note);
    }
}

/// Drain every recorded note, leaving the buffer empty.
///
/// Each note is returned exactly once; later [`record`] calls start a fresh
/// batch.
#[must_use]
pub fn take() -> Vec<String> {
    let mut guard = lock_buffer();
    std::mem::take(&mut *guard)
}

#[cfg(test)]
mod tests {
    // NOTE: these tests mutate the process-scoped buffer, so each uses a
    // unique note payload and drains leftovers first — never assert on the
    // global length across tests (lib tests share one process).

    #[test]
    fn take_drains_recorded_notes_exactly_once() {
        let _ = super::take();
        super::record("preflight-notes-test-drain-me");
        let drained = super::take();
        assert!(drained.contains(&"preflight-notes-test-drain-me".to_string()));
        assert!(
            super::take().is_empty(),
            "a second take must see an empty buffer"
        );
    }

    /// AUDIT probe: does a POISONED mutex really lose notes, as the review
    /// finding claims? Panic while holding the lock, then record + take.
    #[test]
    fn notes_survive_a_poisoned_mutex() {
        let _ = super::take();
        let handle = std::thread::spawn(|| {
            let _guard = super::notes().lock().expect("poison probe lock");
            panic!("poison the buffer lock on purpose");
        });
        assert!(
            handle.join().is_err(),
            "the probe thread must panic to poison the lock"
        );
        assert!(
            super::notes().lock().is_err(),
            "precondition: the mutex must actually be poisoned now"
        );

        super::record("poison-probe-note-must-survive");
        let drained = super::take();
        assert!(
            drained
                .iter()
                .any(|n| n == "poison-probe-note-must-survive"),
            "a poisoned mutex must not swallow the note, got {drained:?}"
        );
    }

    #[test]
    fn identical_notes_are_stored_once() {
        let _ = super::take();
        super::record("preflight-notes-test-duplicate");
        super::record("preflight-notes-test-duplicate");
        let drained = super::take();
        assert_eq!(
            drained
                .iter()
                .filter(|n| n.as_str() == "preflight-notes-test-duplicate")
                .count(),
            1,
            "the same note recorded twice must collapse to one replay line"
        );
    }
}
