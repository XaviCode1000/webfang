//! Parse-time diagnostics recorded before logging exists (#1431).
//!
//! Argument normalization runs BEFORE `init_logging_dual` installs the tracing
//! subscriber (hoisted to step 6b2 in `webfang_cli::main` by #796). The burst
//! value is parsed twice on one invocation — once by the preflight budget
//! staging (`cli::preflight::stage_budget_overrides`) and once by the
//! `From<Args>` projection into `CrawlOptions` — and both happen pre-subscriber,
//! so any `tracing::warn!` emitted there is silently dropped (there is no
//! subscriber) AND would have been emitted twice. Diagnostics that must stay
//! visible (e.g. the `--rate-limit-burst` warn-and-default substitution
//! notice) are instead [`record`]ed here at parse time and replayed once by
//! the binary, immediately after `init_logging_dual`, as regular `warn!`
//! events.
//!
//! The store is a process-scoped append-only buffer behind an
//! `OnceLock<Mutex<Vec<String>>>`: writers never block on async code, a
//! poisoned mutex degrades to `into_inner()` instead of panicking, and
//! [`take`] drains the buffer so each note is replayed exactly once. There is
//! deliberately no flag, no config key, and no second logging path —
//! collection and replay are the same single pipeline, split in time.

use std::sync::{Mutex, OnceLock};

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

/// Record a parse-time diagnostic note for later replay.
///
/// Lock poisoning (a previous holder panicked while pushing) degrades to
/// pushing through `into_inner()` — losing a notice must never panic the
/// parse path.
pub fn record(note: impl Into<String>) {
    let note = note.into();
    match notes().lock() {
        Ok(mut guard) => {
            if !guard.contains(&note) {
                guard.push(note);
            }
        },
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            if !guard.contains(&note) {
                guard.push(note);
            }
        },
    }
}

/// Drain every recorded note, leaving the buffer empty.
///
/// Each note is returned exactly once; later [`record`] calls start a fresh
/// batch. Poisoning degrades to `into_inner()`, same as [`record`].
#[must_use]
pub fn take() -> Vec<String> {
    match notes().lock() {
        Ok(mut guard) => std::mem::take(&mut *guard),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    }
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
            "the binary parses the burst twice (preflight + From<Args>); the operator must see one line"
        );
    }
}
