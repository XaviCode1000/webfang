//! Process-level graceful shutdown for the CLI pipeline.
//!
//! [`Engine::spawn_signal_handler`](crate::application::crawler::engine) only
//! covers a single in-process crawl. The CLI's batch and scrape paths never
//! reach it, so SIGINT/SIGTERM were ignored and the run kept fetching until it
//! finished — or the operator escalated to SIGKILL and lost every page already
//! captured (#653).
//!
//! [`ShutdownGuard`] installs ONE signal listener for the whole run and exposes
//! a [`CancellationToken`]. The pipeline stages observe the token cooperatively:
//! they stop starting new work, drain what is already in flight, and let the
//! export phase persist it. Cancellation is deliberately NOT a
//! `select!`-and-drop of the work future — dropping it mid-run is exactly the
//! data loss this fixes.

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::Instrument;

#[cfg(unix)]
use tracing::warn;

/// Owns the run's signal listener and its [`CancellationToken`].
///
/// The listener task is aborted on drop, so a completed run never leaves a
/// stray signal handler behind.
#[derive(Debug)]
pub struct ShutdownGuard {
    token: CancellationToken,
    handle: JoinHandle<()>,
}

impl ShutdownGuard {
    /// Install the SIGINT/SIGTERM listener for this run.
    #[must_use]
    pub fn install() -> Self {
        let token = CancellationToken::new();
        let handle = tokio::spawn(wait_for_signal(token.clone()).in_current_span());
        Self { token, handle }
    }

    /// Clone of the run's cancellation token.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Whether a shutdown signal has already been observed.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Await the first termination signal, then fire `token`.
///
/// Returns early — without cancelling — if the token is fired by another path
/// (e.g. a nested engine shutdown), so the task never outlives its usefulness.
async fn wait_for_signal(token: CancellationToken) {
    let signalled = tokio::select! {
        () = token.cancelled() => false,
        () = next_termination_signal() => true,
    };
    if signalled {
        token.cancel();
    }
}

/// Resolve when SIGINT (or, on unix, SIGTERM) arrives.
async fn next_termination_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // A rejected SIGTERM registration must never abort the run: degrade to
        // SIGINT-only and say so, matching the engine's handler (#509).
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = ctrl_c => info!("received SIGINT — draining in-flight work"),
                    _ = sigterm.recv() => info!("received SIGTERM — draining in-flight work"),
                }
            },
            // LCOV_EXCL_START defensive: signal-registration — the OS rejects the SIGTERM handler only on an invariant break
            Err(e) => {
                warn!(
                    error = %e,
                    "SIGTERM handler registration failed — shutdown will only respond to SIGINT"
                );
                ctrl_c.await.ok();
            },
            // LCOV_EXCL_STOP
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        info!("received interrupt — draining in-flight work");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_fresh_guard_is_not_cancelled() {
        let guard = ShutdownGuard::install();
        assert!(!guard.is_cancelled());
    }

    #[tokio::test]
    async fn firing_the_token_marks_the_guard_cancelled() {
        let guard = ShutdownGuard::install();
        guard.token().cancel();
        assert!(guard.is_cancelled());
    }

    #[tokio::test]
    async fn the_listener_stops_when_the_token_is_fired_elsewhere() {
        let token = CancellationToken::new();
        let listener = tokio::spawn(wait_for_signal(token.clone()));
        token.cancel();
        listener.await.expect("listener must exit cleanly");
    }

    #[tokio::test]
    async fn dropping_the_guard_aborts_the_listener() {
        let token = {
            let guard = ShutdownGuard::install();
            guard.token()
        };
        // The listener is gone, so nothing can cancel the token any more.
        tokio::task::yield_now().await;
        assert!(!token.is_cancelled());
    }
}
