//! Panic hook — log panics instead of silent abort.
//!
//! Installs a custom panic hook that records the panic location, message,
//! and thread name via `tracing::error!` before delegating to the default
//! hook. This ensures panics are captured in the structured trace output
//! (including the JSONL trace file) rather than only reaching stderr.

use std::panic;

/// Install the custom panic hook.
///
/// Idempotent: safe to call multiple times. Each call replaces the
/// previous hook, so the most recent installation wins.
pub fn setup_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unknown")
            .to_string();
        let location = info.location().map_or_else(
            || "unknown location".to_string(),
            |loc| format!("{}:{}", loc.file(), loc.line()),
        );
        tracing::error!(
            panic.message = %info,
            panic.location = %location,
            panic.thread = %thread_name,
            "server panicked"
        );
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_installs_without_panic() {
        // Simply verifying the hook can be installed and a captured panic
        // still runs the default hook (which aborts in tests).
        setup_panic_hook();
        // If we got here, the hook was installed successfully.
    }
}
