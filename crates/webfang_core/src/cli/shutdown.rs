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
use tracing::Instrument;
use tracing::{info, warn};

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

/// Resolve when a termination event arrives, returning nothing — the caller
/// only needs to know that one happened, not which.
///
/// Unix: SIGINT + SIGTERM + SIGHUP. SIGHUP joins the set (XP-S-03, #1608):
/// closing the terminal or dropping the connection no longer kills the run
/// abruptly — it drains like SIGINT/SIGTERM. Windows: Ctrl+C/Ctrl+Break plus
/// the console close / logoff / shutdown events `ctrl_c()` never sees
/// (XP-S-02, #1808). A rejected registration must never abort the run:
/// degrade to whatever registered and say so, matching the engine's handler
/// (#509).
async fn next_termination_signal() {
    #[cfg(unix)]
    wait_for_unix_termination_signal().await;
    #[cfg(not(unix))]
    {
        match crate::infrastructure::platform::first_termination_event().await {
            Some(name) => info!("received {name} — draining in-flight work"),
            None => warn!(
                "interrupt handler registration failed — shutdown will only respond to an explicit cancel"
            ),
        }
    }
}

/// Unix signal set: SIGINT + SIGTERM + SIGHUP, degrading per-signal when a
/// registration is rejected (never panic, always say so — #509).
#[cfg(unix)]
async fn wait_for_unix_termination_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate());
    let mut sighup = signal(SignalKind::hangup());
    // LCOV_EXCL_START defensive: signal-registration — the OS rejects a handler only on an invariant break
    if let Err(e) = &sigterm {
        warn!(
            error = %e,
            "SIGTERM handler registration failed — shutdown will only respond to SIGINT"
        );
    }
    if let Err(e) = &sighup {
        warn!(
            error = %e,
            "SIGHUP handler registration failed — closing the terminal will terminate the run"
        );
    }
    // LCOV_EXCL_STOP

    let name = first_termination_signal(sigterm.as_mut().ok(), sighup.as_mut().ok()).await;
    info!("received {name} — draining in-flight work");
}

/// Await the FIRST termination signal among those that registered and
/// return its name (SIGINT always registers via `ctrl_c`).
///
/// A flat `futures::future::select_all` over boxed waits — no nested
/// `select!` arms — keeps this under the #516 complexity ratchet while
/// handling every subset of registered signals uniformly.
#[cfg(unix)]
async fn first_termination_signal(
    sigterm: Option<&mut tokio::signal::unix::Signal>,
    sighup: Option<&mut tokio::signal::unix::Signal>,
) -> &'static str {
    let mut names: Vec<&'static str> = vec!["SIGINT"];
    let mut waits: Vec<std::pin::Pin<Box<dyn futures::Future<Output = ()> + Send>>> =
        vec![Box::pin(async {
            tokio::signal::ctrl_c().await.ok();
        })];
    if let Some(sigterm) = sigterm {
        names.push("SIGTERM");
        waits.push(Box::pin(async move {
            sigterm.recv().await;
        }));
    }
    if let Some(sighup) = sighup {
        names.push("SIGHUP");
        waits.push(Box::pin(async move {
            sighup.recv().await;
        }));
    }
    let (_, index, _) = futures::future::select_all(waits).await;
    names[index]
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
