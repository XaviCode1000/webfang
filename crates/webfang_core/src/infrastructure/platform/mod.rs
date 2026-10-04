//! Platform-specific process plumbing (console lifecycle, code page).
//!
//! One module, one responsibility: make the Windows console behave like the
//! Unix terminal it replaces. Two things live here, both process-wide and
//! both idempotent:
//!
//! 1. [`windows_console::console_event`] — a single, lazily installed
//!    `SetConsoleCtrlHandler` that observes the console events `tokio`'s
//!    `ctrl_c()` never sees (`CTRL_CLOSE`, `CTRL_LOGOFF`,
//!    `CTRL_SHUTDOWN`) and fans them out to every awaiter by name.
//! 2. [`windows_console::init_console_output_encoding`] — UTF-8 for the
//!    console code page, so non-ASCII output is not mojibake (XP-K-03).
//!
//! `CTRL_C_EVENT`/`CTRL_BREAK_EVENT` are deliberately NOT routed here: the
//! handler returns 0 for them, so they keep flowing to tokio's own handler
//! and nothing double-fires.

pub mod windows_console;

pub use windows_console::{
    console_event, console_event_name, first_termination_event, init_console_output_encoding,
    CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};
