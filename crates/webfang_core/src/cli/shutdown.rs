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

use crate::application::crawler::engine::{TerminationOutcome, TERMINATION_REGISTRATION_REJECTED};
use crate::application::crawler::ports::TerminationWait;
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
    wait_for_signal_from(token, next_termination_event()).await;
}

/// Core of [`wait_for_signal`], with the termination wait injected.
///
/// `event` resolves to the NAME of the termination event, or to `None` when
/// the OS rejected the registration. The decision is the shared one
/// ([`TerminationOutcome`]): a delivered event stops the run, a rejected
/// registration does not (#1835) — the same answer the crawl engine gives.
async fn wait_for_signal_from(token: CancellationToken, mut event: TerminationWait) {
    let signalled = tokio::select! {
        () = token.cancelled() => false,
        name = &mut event => {
            let outcome = TerminationOutcome::from_wait(name);
            match outcome {
                TerminationOutcome::Event(name) => {
                    info!("received {name} — draining in-flight work");
                }
                TerminationOutcome::RegistrationRejected => {
                    warn!(authority = "cli", "{}", TERMINATION_REGISTRATION_REJECTED);
                }
            }
            // #1835: the shared decision — a rejected registration is not a
            // termination event, so the run keeps going and waits for an
            // explicit cancel, exactly like the crawl engine's handler.
            outcome.stops_run()
        },
    };
    if signalled {
        token.cancel();
    }
}

/// The termination event this host can observe, by name — or `None` when the
/// OS rejected the registration.
///
/// Unix: SIGINT + SIGTERM + SIGHUP. SIGHUP joins the set (XP-S-03, #1608):
/// closing the terminal or dropping the connection no longer kills the run
/// abruptly — it drains like SIGINT/SIGTERM. Windows: Ctrl+C/Ctrl+Break plus
/// the console close / logoff / shutdown events `ctrl_c()` never sees
/// (XP-S-02, #1808). A rejected registration must never abort the run:
/// degrade to whatever registered and say so, matching the engine's handler
/// (#509, #1835).
fn next_termination_event() -> TerminationWait {
    #[cfg(unix)]
    {
        Box::pin(wait_for_unix_termination_signal())
    }
    #[cfg(not(unix))]
    {
        Box::pin(crate::infrastructure::platform::first_termination_event())
    }
}

/// Unix signal set: SIGINT + SIGTERM + SIGHUP, degrading per-signal when a
/// registration is rejected (never panic, always say so — #509).
#[cfg(unix)]
async fn wait_for_unix_termination_signal() -> Option<&'static str> {
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

    // SIGINT always registers through tokio's `ctrl_c`, so the set is never
    // empty: this wait always ends on a real event.
    let name = first_termination_signal(sigterm.as_mut().ok(), sighup.as_mut().ok()).await;
    Some(name)
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

    /// #1835: a REJECTED registration is not a termination event. The guard
    /// must NOT cancel the run — it waits for an explicit cancel, exactly
    /// like the crawl engine's handler, and says the same thing
    /// (`TERMINATION_REGISTRATION_REJECTED`).
    #[tokio::test]
    async fn a_rejected_registration_does_not_cancel_the_run() {
        let token = CancellationToken::new();
        let listener = tokio::spawn(wait_for_signal_from(
            token.clone(),
            Box::pin(async { None::<&'static str> }),
        ));

        tokio::time::timeout(std::time::Duration::from_secs(5), listener)
            .await
            .expect("the listener must return once the rejected registration reports itself")
            .expect("the listener task must not panic");

        assert!(
            !token.is_cancelled(),
            "a rejected registration must not cancel the run (#1835)"
        );
    }

    /// The other half of the same contract: a DELIVERED termination event
    /// still stops the run. The rejection path must not swallow real events.
    #[tokio::test]
    async fn a_delivered_termination_event_still_cancels_the_run() {
        let token = CancellationToken::new();
        let listener = tokio::spawn(wait_for_signal_from(
            token.clone(),
            Box::pin(async { Some("TEST_TERMINATION_EVENT") }),
        ));

        tokio::time::timeout(std::time::Duration::from_secs(5), listener)
            .await
            .expect("the listener must return once the event arrives")
            .expect("the listener task must not panic");

        assert!(
            token.is_cancelled(),
            "a delivered termination event must stop the run"
        );
    }
}
