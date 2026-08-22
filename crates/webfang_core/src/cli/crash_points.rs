//! Crash-injection harness for the SIGKILL recovery matrix (design D6).
//!
//! Test-only by contract: the module is inert unless the `WEBFANG_CRASH_AT`
//! environment variable is set, in which case pinned [`hit`] sites across the
//! pipeline terminate the process with `SIGKILL` at deterministic positions so
//! integration tests can prove crash-recovery invariants end to end.
//!
//! # Environment
//!
//! - `WEBFANG_CRASH_AT=point_name` — die at the FIRST occurrence of `point_name`.
//! - `WEBFANG_CRASH_AT=point_name:n` — die at the Nth occurrence (1-based).
//!
//! # Cost when unarmed
//!
//! One [`OnceLock`] read (an acquire load) per call site check; nothing else.

use std::env;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// Environment variable carrying the armed crash-point spec.
pub const ENV_VAR: &str = "WEBFANG_CRASH_AT";

/// After discovery / resume filtering, before the first record save.
pub const PRE_FIRST_PERSIST: &str = "pre_first_persist";
/// Fetch flow returned (response complete) inside the scrape pipeline.
pub const MID_FETCH: &str = "mid_fetch";
/// After fetch + cleaning, before selector extraction.
pub const POST_FETCH_PRE_EXTRACT: &str = "post_fetch_pre_extract";
/// Inside selector/content extraction: HTML cleaned + selector applied,
/// Readability/content build not yet returned.
pub const MID_EXTRACTION: &str = "mid_extraction";
/// Extraction completed (ScrapedContent built), before the record/pipeline
/// stage persists anything.
pub const POST_EXTRACTION_PRE_PIPELINE: &str = "post_extraction_pre_pipeline";
/// Inside the JSONL writer loop: half of one append payload hits disk first.
pub const MID_JSONL_LINE: &str = "mid_jsonl_line";
/// In `CommitSession::commit_item`: flush ack received, EXPORTED not saved.
pub const POST_FLUSH_PRE_COMMIT: &str = "post_flush_pre_commit";
/// Inside a record-store save transaction while the store lock is held.
pub const WHILE_HOLDING_LOCK: &str = "while_holding_lock";
/// Record-store tmp fully written, before rename(2).
pub const TMP_WRITTEN_PRE_RENAME: &str = "tmp_written_pre_rename";
/// Record-store tmp PARTIALLY written (truncated artifact).
pub const MID_STATE_FILE_WRITE: &str = "mid_state_file_write";
/// Reserve for PR5b: during cancellation drain before final persist.
pub const DURING_CANCEL_DRAIN: &str = "during_cancel_drain";

static ARMED: OnceLock<Option<(String, u32)>> = OnceLock::new();

/// Occurrences of the armed point already observed. Only one point can be
/// armed per process, so a single counter is sufficient.
static SEEN: AtomicU32 = AtomicU32::new(0);

/// Parse `WEBFANG_CRASH_AT` once, arming (or deliberately not arming) every
/// [`hit`] site for this process. Called once, early in the CLI entrypoint.
pub fn arm_from_env() {
    let spec = env::var(ENV_VAR).ok().and_then(|raw| parse_spec(&raw));
    let _ = ARMED.set(spec);
}

/// Parse `"point_name"` or `"point_name:n"` into `(name, occurrence)` with a
/// 1-based occurrence defaulting to 1.
fn parse_spec(raw: &str) -> Option<(String, u32)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.rsplit_once(':') {
        Some((name, n)) => match n.trim().parse::<u32>() {
            Ok(n) if n >= 1 => Some((name.trim().to_owned(), n)),
            _ => None,
        },
        None => Some((raw.to_owned(), 1)),
    }
}

/// True only when THIS call is the armed occurrence for `name`.
fn is_my_turn(name: &str) -> bool {
    match ARMED.get() {
        Some(Some((target, occurrence))) if target == name => {
            let seen = SEEN.fetch_add(1, Ordering::Relaxed);
            seen + 1 == *occurrence
        },
        _ => false,
    }
}

/// True when `name` is the armed target at all (any occurrence). Used by call
/// sites that must do partial work BEFORE dying (torn-artifact injection).
#[must_use]
pub fn is_armed_for(name: &str) -> bool {
    ARMED
        .get()
        .is_some_and(|armed| armed.as_ref().is_some_and(|(target, _)| target == name))
}

/// Die by `SIGKILL` when `name` is the armed crash point at its armed
/// occurrence. Returns immediately when unarmed or not the targeted site.
pub fn hit(name: &str) {
    if !is_my_turn(name) {
        return;
    }
    tracing::warn!(point = name, "crash-injection harness: killing self");
    kill_self();
}

#[cfg(unix)]
// Test-only crash injection by contract: SIGKILL to our own pid is the
// one legitimate use of unsafe here; the workspace forbids unsafe elsewhere.
#[allow(unsafe_code)]
fn kill_self() -> ! {
    // SAFETY: `kill(2)` on our own pid (always valid, always exists) with
    // `SIGKILL`, which cannot be caught, blocked, or ignored.
    unsafe {
        libc::kill(
            i32::try_from(std::process::id()).unwrap_or(-1),
            libc::SIGKILL,
        );
    }
    unreachable!("SIGKILL cannot be caught; the process must already be gone");
}

#[cfg(not(unix))]
fn kill_self() -> ! {
    tracing::error!("crash-injection harness requires unix; continuing without crash");
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_point_defaults_to_first_occurrence() {
        assert_eq!(parse_spec("mid_fetch"), Some(("mid_fetch".to_owned(), 1)));
    }

    #[test]
    fn numbered_point_parses_occurrence() {
        assert_eq!(
            parse_spec("mid_jsonl_line:3"),
            Some(("mid_jsonl_line".to_owned(), 3))
        );
    }

    #[test]
    fn empty_and_invalid_specs_arm_nothing() {
        assert_eq!(parse_spec(""), None);
        assert_eq!(parse_spec("   "), None);
        assert_eq!(parse_spec("point:0"), None);
        assert_eq!(parse_spec("point:abc"), None);
        assert_eq!(parse_spec("point:-2"), None);
    }

    #[test]
    fn unarmed_process_never_matches() {
        // ARMED was never initialized in this test process.
        assert!(!is_armed_for(PRE_FIRST_PERSIST));
        assert!(!is_my_turn(PRE_FIRST_PERSIST));
    }
}
