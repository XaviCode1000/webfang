//! Windows console lifecycle source (#1808 — XP-S-02, XP-K-03).
//!
//! On Windows, `tokio::signal::ctrl_c()` only ever observes
//! `CTRL_C_EVENT`/`CTRL_BREAK_EVENT`. Closing the console window
//! (`CTRL_CLOSE_EVENT`), logging off (`CTRL_LOGOFF_EVENT`) and system
//! shutdown (`CTRL_SHUTDOWN_EVENT`) reach nothing, so a crawl is killed
//! with no drain. This module installs ONE process-wide
//! `SetConsoleCtrlHandler` that observes the three missing events and fans
//! them out through a [`tokio::sync::broadcast`] channel carrying the event
//! NAME, so every awaiting shutdown site keeps its own log line and its own
//! cancellation authority.
//!
//! Design constraints, all deliberate:
//!
//! - **One handler for the process.** The registration is guarded by a
//!   [`std::sync::OnceLock`], so four awaiting sites install one handler,
//!   not four.
//! - **Never a second authority.** [`console_event`] only *fires*; the
//!   `CancellationToken`/atomic of the awaiting site stays the single
//!   decision (ADR-0016).
//! - **No double-fire.** [`console_event_name`] maps ONLY 2/5/6. The
//!   handler returns 0 for Ctrl+C/Ctrl+Break, so the event keeps flowing to
//!   tokio's own handler.
//! - **Never panic.** A rejected registration warns once and degrades to
//!   "the console event source stays pending", matching the existing #509
//!   signal fallbacks.
//! - **The callback cannot block.** The broadcast `send` is synchronous and
//!   non-blocking; it never awaits and never runs user code.
//!
//! # Known Windows limit
//!
//! The OS gives a `CTRL_CLOSE_EVENT` handler roughly five seconds before it
//! kills the process, so the drain this triggers is best-effort on console
//! close. Logoff and shutdown are generous. Nothing here can extend that
//! window — it is a property of the console host.

/// `CTRL_C_EVENT` — owned by tokio's `ctrl_c()`, never by this source.
pub const CTRL_C_EVENT: u32 = 0;
/// `CTRL_BREAK_EVENT` — owned by tokio's `ctrl_c()`.
pub const CTRL_BREAK_EVENT: u32 = 1;
/// `CTRL_CLOSE_EVENT` — the console window was closed.
pub const CTRL_CLOSE_EVENT: u32 = 2;
/// `CTRL_LOGOFF_EVENT` — the user logged off.
pub const CTRL_LOGOFF_EVENT: u32 = 5;
/// `CTRL_SHUTDOWN_EVENT` — the system is shutting down.
pub const CTRL_SHUTDOWN_EVENT: u32 = 6;

// Name logged for the Ctrl+C path, so a log line names the event that
// actually arrived rather than a bare "interrupt". Owned by the application
// layer (`application::crawler::ports::CTRL_C_EVENT_NAME`) and imported here,
// so the crawl engine, the CLI shutdown guard and the MCP server cannot drift
// apart on what a Ctrl+C interruption is called.
use crate::application::crawler::ports::CTRL_C_EVENT_NAME;

/// Buffer for the console-event fan-out.
///
/// Four awaiting sites, each draining on its own schedule. Overflow drops
/// the oldest event for the slowest receiver (logged, not fatal) — the
/// shutdown decision belongs to the awaiting site, not to this buffer.
#[cfg(windows)]
const EVENT_CHANNEL_CAPACITY: usize = 16;

/// Name of the console event this source owns, or `None` when the event is
/// tokio's to handle.
///
/// Pure and cross-platform on purpose: the mapping is the part worth
/// testing, and it is testable on Linux. `CTRL_C_EVENT` and
/// `CTRL_BREAK_EVENT` return `None` so the console handler passes them to
/// the next handler in the chain instead of stealing them from tokio.
#[must_use]
pub fn console_event_name(ctrl_type: u32) -> Option<&'static str> {
    match ctrl_type {
        CTRL_CLOSE_EVENT => Some("CTRL_CLOSE_EVENT"),
        CTRL_LOGOFF_EVENT => Some("CTRL_LOGOFF_EVENT"),
        CTRL_SHUTDOWN_EVENT => Some("CTRL_SHUTDOWN_EVENT"),
        _ => None,
    }
}

/// Resolve with the NAME of the first Windows console termination event.
///
/// Every awaiting site gets its own subscription to the single process-wide
/// handler, so awaiting from four places registers one handler and delivers
/// one event to each of them.
///
/// On non-Windows this waits forever, by design: there is no console-event
/// source to observe, so a `select!` arm over it simply never wins. That is
/// what makes it safe to wire unconditionally at a shutdown site.
///
/// # Degradation
///
/// A rejected `SetConsoleCtrlHandler` registration warns once and leaves
/// this pending forever, so the awaiting site keeps whatever other sources
/// it has (`tokio`'s Ctrl+C, an explicit `cancel()`). It never resolves to
/// a synthetic event and never panics.
pub async fn console_event() -> &'static str {
    console_event_source().await
}

#[cfg(windows)]
async fn console_event_source() -> &'static str {
    use tokio::sync::broadcast::error::RecvError;
    use tracing::warn;

    let mut rx = match handler::ensure_source() {
        Some(sender) => sender.subscribe(),
        // Registration was rejected: no console events will ever arrive.
        None => return std::future::pending::<&'static str>().await,
    };
    loop {
        match rx.recv().await {
            Ok(name) => return name,
            // A lagging receiver missed events, not the one it is waiting
            // for — keep waiting rather than reporting a shutdown it never saw.
            Err(RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "console event receiver lagged — missed console events, still waiting for the next one"
                );
            },
            Err(RecvError::Closed) => {
                return std::future::pending::<&'static str>().await;
            },
        }
    }
}

/// No Windows console here: this wait never resolves, which is the correct
/// cross-platform behaviour for a source that cannot exist (see
/// [`console_event`]).
#[cfg(not(windows))]
async fn console_event_source() -> &'static str {
    std::future::pending::<&'static str>().await
}

/// Resolve with the NAME of the first process-level termination event, or
/// `None` when no source could be registered.
///
/// This is the composition every shutdown site wants: Ctrl+C — which is all
/// a Unix process has, and on Windows the only thing `tokio::signal::ctrl_c()`
/// ever delivers — raced against the console-event source `tokio` cannot
/// see. `None` means "Ctrl+C registration was rejected", and the caller
/// falls back to its own explicit-cancel path (#509).
pub async fn first_termination_event() -> Option<&'static str> {
    first_termination_source().await
}

#[cfg(windows)]
async fn first_termination_source() -> Option<&'static str> {
    use std::future::Future;
    use std::pin::Pin;

    type Wait = Pin<Box<dyn Future<Output = Option<&'static str>> + Send>>;
    let mut waits: Vec<Wait> = vec![
        Box::pin(async {
            tokio::signal::ctrl_c()
                .await
                .ok()
                .map(|()| CTRL_C_EVENT_NAME)
        }),
        Box::pin(async { Some(console_event_source().await) }),
    ];
    // Flat `select_all` ladder — no nested `select!` arms, so this stays under
    // the #516 complexity ratchet while handling every subset of sources.
    // A source that degrades to `None` is dropped and the remaining ones keep
    // waiting: a broken Ctrl+C must not hide the console source, which is the
    // one that matters on Windows.
    loop {
        let (name, _index, remaining) = futures::future::select_all(waits).await;
        waits = remaining;
        if name.is_some() || waits.is_empty() {
            return name;
        }
    }
}

/// Off Windows this is exactly the historical behaviour: tokio's Ctrl+C,
/// named. The console source is compiled out entirely.
#[cfg(not(windows))]
async fn first_termination_source() -> Option<&'static str> {
    tokio::signal::ctrl_c()
        .await
        .ok()
        .map(|()| CTRL_C_EVENT_NAME)
}

/// Switch the console code page to UTF-8 (XP-K-03).
///
/// Without it the Windows console keeps its legacy OEM code page (cp437 on
/// a US host, cp1252 on a Spanish one) and every non-ASCII character in a
/// user-facing message renders as mojibake.
///
/// # When to call it
///
/// Call this **before the first byte reaches the console** — before the
/// tracing subscriber is installed and before any progress or error output.
/// A code page set after output has started affects only what is written
/// afterwards, so a late call leaves earlier lines mangled.
///
/// On non-Windows this is a documented no-op: the console code page is a
/// Win32 concept, and Unix terminals are UTF-8 by convention.
///
/// # Degradation
///
/// A rejected code page change warns and leaves the console as it was. It
/// never panics and never fails the run — output degrades to mojibake,
/// which is strictly better than refusing to start.
pub fn init_console_output_encoding() {
    set_utf8_code_page();
}

#[cfg(windows)]
fn set_utf8_code_page() {
    use tracing::warn;
    use windows_sys::Win32::Globalization::CP_UTF8;
    use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};

    // SAFETY: `SetConsoleOutputCP` takes a `u32` code page id and mutates only
    // the calling process' console output state; `CP_UTF8` is the documented
    // 65001 constant. No pointer, no aliasing, no lifetime obligation.
    let output_cp = unsafe { SetConsoleOutputCP(CP_UTF8) };
    if output_cp == 0 {
        warn!("could not set the console output code page to UTF-8 — non-ASCII output may render incorrectly");
    }
    // SAFETY: same as above, for the console INPUT code page. A rejected
    // input code page only affects what the host reads back, never output.
    let input_cp = unsafe { SetConsoleCP(CP_UTF8) };
    if input_cp == 0 {
        warn!("could not set the console input code page to UTF-8 — non-ASCII input may render incorrectly");
    }
}

/// Documented no-op off Windows (see [`init_console_output_encoding`]).
#[cfg(not(windows))]
fn set_utf8_code_page() {}

/// The single process-wide console handler and its fan-out channel.
///
/// Private because it is process state, not an API: exactly one handler for
/// the whole process, installed once, whatever the number of awaiters.
#[cfg(windows)]
mod handler {
    use std::sync::OnceLock;

    use tokio::sync::broadcast;
    use tracing::warn;

    use super::{console_event_name, EVENT_CHANNEL_CAPACITY};

    /// Guards the one-and-only registration attempt.
    static INSTALL_ATTEMPTED: OnceLock<bool> = OnceLock::new();

    /// Fan-out channel. Published BEFORE the handler is registered, so an
    /// event arriving in the registration window is still delivered; the
    /// handler is the last thing installed.
    static EVENTS: OnceLock<broadcast::Sender<&'static str>> = OnceLock::new();

    /// Win32 console-control callback.
    ///
    /// Plain `extern "system"` fn with no captures — Win32 requires exactly
    /// that. Returns 1 for an event this source consumed, 0 to pass it on to
    /// the next handler in the chain (which is how Ctrl+C still reaches
    /// tokio's handler).
    unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
        let Some(name) = console_event_name(ctrl_type) else {
            // Ctrl+C / Ctrl+Break: not ours. Passing it on is what keeps the
            // two sources from double-firing.
            return 0;
        };
        match EVENTS.get() {
            Some(sender) => {
                // Non-blocking on the happy path; an error only means nobody
                // is awaiting any more.
                let _ = sender.send(name);
                1
            },
            // Reachable only if a handler outlived the channel, which cannot
            // happen because the channel is a `static`. Defer to the OS
            // default rather than swallow the event.
            None => 0,
        }
    }

    /// Install the handler once per process, publishing the channel.
    fn install() {
        use windows_sys::Win32::System::Console::{SetConsoleCtrlHandler, PHANDLER_ROUTINE};

        let (sender, _keepalive) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        if EVENTS.set(sender).is_err() {
            // Another thread installed first — its handler is the live one.
            return;
        }
        let handler: PHANDLER_ROUTINE = Some(ctrl_handler);
        // SAFETY: `ctrl_handler` is a plain `extern "system"` fn with no
        // captures, matching `PHANDLER_ROUTINE` exactly, so Win32 may call it
        // from any thread at any time (it only reads a `OnceLock` and does a
        // non-blocking send). `add = 1` registers it; the BOOL is not a
        // pointer and the call borrows nothing.
        let registered = unsafe { SetConsoleCtrlHandler(handler, 1) };
        if registered == 0 {
            // Graceful degradation, never a panic: the channel exists but no
            // handler feeds it, so `console_event` stays pending forever and
            // every site keeps its Ctrl+C / explicit-cancel path.
            warn!("console control handler registration failed — closing the console window will terminate the run without draining");
        }
    }

    /// The process-wide fan-out sender, installing the handler on first use.
    pub(super) fn ensure_source() -> Option<&'static broadcast::Sender<&'static str>> {
        // `set` returning `Ok` identifies the single installing caller, so a
        // rejected registration warns once instead of once per awaiter.
        if INSTALL_ATTEMPTED.set(true).is_ok() {
            install();
        }
        EVENTS.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_events_map_to_their_own_names() {
        assert_eq!(
            console_event_name(CTRL_CLOSE_EVENT),
            Some("CTRL_CLOSE_EVENT")
        );
        assert_eq!(
            console_event_name(CTRL_LOGOFF_EVENT),
            Some("CTRL_LOGOFF_EVENT")
        );
        assert_eq!(
            console_event_name(CTRL_SHUTDOWN_EVENT),
            Some("CTRL_SHUTDOWN_EVENT")
        );
    }

    #[test]
    fn ctrl_c_and_ctrl_break_stay_with_tokio() {
        // Mapping them here would make the console handler consume them and
        // `ctrl_c()` never fire — the double-fire this design avoids.
        assert_eq!(console_event_name(CTRL_C_EVENT), None);
        assert_eq!(console_event_name(CTRL_BREAK_EVENT), None);
    }

    #[test]
    fn unknown_control_types_are_not_console_events() {
        assert_eq!(console_event_name(3), None);
        assert_eq!(console_event_name(4), None);
        assert_eq!(console_event_name(u32::MAX), None);
    }

    #[tokio::test]
    async fn awaiting_the_console_source_is_safe_and_non_blocking_off_windows() {
        // Off Windows the source has nothing to observe, so the wait must
        // never resolve and must never block the runtime.
        let waiter = tokio::spawn(console_event());
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "console_event must stay pending");
        waiter.abort();
        assert!(waiter.await.is_err(), "abort must release the wait");
    }

    #[tokio::test]
    async fn awaiting_the_termination_source_is_safe_to_spawn_off_windows() {
        // `first_termination_event` waits on tokio's Ctrl+C, which no test
        // may deliver, so the contract pinned here is the safe one: spawning
        // it registers nothing that panics and the task stays cancellable.
        let waiter = tokio::spawn(first_termination_event());
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.is_err());
    }

    #[test]
    fn code_page_init_is_a_documented_no_op_off_windows() {
        // Must be safe to call from a test: no-op, no panic, no side effect a
        // later test could observe.
        init_console_output_encoding();
    }
}
