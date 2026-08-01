//! Status-specific retry loops for the hardened HTTP client.
//!
//! The hardened client's `get_inner` (see [`HttpClient`]) handles a request's
//! first response inline; when that response is a `429` or a `5xx`, it
//! delegates the bounded retry loop to [`retry_with_backoff`] in this module.
//!
//! The two historical loops (one for `429`, one for `5xx`) were near-identical
//! but differed in four load-bearing ways, captured by [`RetryPolicy`].
//! [`retry_with_backoff`] reproduces both exactly by varying only that policy:
//!
//! | Difference | `429` path | `5xx` path |
//! |---|---|---|
//! | Delay strategy | honors `Retry-After` (constant `secs * 1000` ms) when `> 0`, else exponential | always exponential backoff |
//! | Retryable status | `429` **or** any `5xx` | any `5xx` only |
//! | Exhaustion error | [`HttpError::RateLimited`] | [`HttpError::ServerError`] |
//! | Tracing label | `"429"` | `"5xx"` |
//!
//! Everything else — the attempt counter, request headers, success
//! short-circuit, non-retryable `ClientError`, and timeout handling — is
//! identical and lives here once. The 5xx path's pre-loop WAF inspection stays
//! in `get_inner` because it runs on the initial response, before any retry.
//!
//! [`HttpClient`]: crate::application::http_client::HttpClient
//! [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
//! [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError

use crate::domain::http_config::HttpClientConfig;
use crate::domain::http_error::{HttpError, HttpResult};
use std::time::Duration;
use tracing::debug;
use wreq::Client;

/// The four load-bearing differences between the `429` and `5xx` retry loops.
///
/// Every other aspect of the two loops is identical and lives in
/// [`retry_with_backoff`]; this policy carries only what varies between them so
/// the shared loop can reproduce both behaviors exactly.
pub(super) struct RetryPolicy {
    /// `Retry-After` seconds honored by the `429` path (`Some`); `None` for the
    /// `5xx` path, which always uses exponential backoff.
    pub(super) retry_after_secs: Option<u64>,
    /// Predicate deciding whether a received status code triggers another
    /// retry: the `429` path retries on `429` **or** any `5xx`, the `5xx` path
    /// retries on any `5xx` only.
    pub(super) retryable: fn(u16) -> bool,
    /// Error returned once `max_retries` attempts are exhausted:
    /// [`HttpError::RateLimited`] for the `429` path, [`HttpError::ServerError`]
    /// for the `5xx` path.
    ///
    /// [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
    /// [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError
    pub(super) exhausted: HttpError,
    /// Label tagging the tracing debug messages (`"429"` / `"5xx"`).
    pub(super) label: &'static str,
}

/// Run the bounded retry loop shared by the `429` and `5xx` handling paths.
///
/// Performs up to `config.max_retries` attempts. Each attempt sleeps for
/// [`compute_backoff_delay`], re-sends the GET with the same headers the
/// hardened client uses (Accept-Language, Accept, Referer, Cache-Control and
/// the pinned `user_agent`), then inspects the outcome:
///
/// - a success status returns the body immediately;
/// - a status satisfying `policy.retryable` continues to the next attempt;
/// - any other status returns [`HttpError::ClientError`];
/// - a transport error returns [`HttpError::Timeout`] when it is a timeout and
///   otherwise continues to the next attempt.
///
/// When every attempt is exhausted, `policy.exhausted` is returned. The `429`
/// caller passes a policy with `Some(retry_after)` and
/// [`HttpError::RateLimited`]; the `5xx` caller passes `None` (pure exponential
/// backoff) and [`HttpError::ServerError`].
///
/// # Errors
///
/// Returns the body on success, or the first terminal [`HttpError`] (a client
/// error, a timeout, or `policy.exhausted` once retries run out).
///
/// [`HttpError::ClientError`]: crate::domain::http_error::HttpError::ClientError
/// [`HttpError::Timeout`]: crate::domain::http_error::HttpError::Timeout
/// [`HttpError::RateLimited`]: crate::domain::http_error::HttpError::RateLimited
/// [`HttpError::ServerError`]: crate::domain::http_error::HttpError::ServerError
pub(super) async fn retry_with_backoff(
    client: &Client,
    url: &str,
    config: &HttpClientConfig,
    user_agent: &str,
    policy: RetryPolicy,
) -> HttpResult<String> {
    let mut attempt = 0;
    while attempt < config.max_retries {
        attempt += 1;

        let delay_ms = compute_backoff_delay(
            attempt,
            policy.retry_after_secs,
            config.backoff_base_ms,
            config.backoff_max_ms,
        );

        debug!(
            "{} retry attempt {} after {}ms",
            policy.label, attempt, delay_ms
        );
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let request = client
            .get(url)
            .header("Accept-Language", &config.accept_language)
            .header("Accept", &config.accept)
            .header("Referer", &config.referer)
            .header("Cache-Control", &config.cache_control)
            .header("User-Agent", user_agent);

        match request.send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    return resp
                        .text()
                        .await
                        .map_err(|e| HttpError::Request(e.to_string()));
                } else if (policy.retryable)(resp.status().as_u16()) {
                    continue;
                } else {
                    return Err(HttpError::ClientError(resp.status().as_u16()));
                }
            },
            Err(e) => {
                if e.is_timeout() {
                    return Err(HttpError::Timeout);
                }
                continue;
            },
        }
    }
    Err(policy.exhausted)
}

/// Compute the delay in milliseconds before retry attempt `attempt` (1-based).
///
/// When `retry_after_secs` is `Some(secs)` with `secs > 0` — the `429` path
/// honoring a `Retry-After` header — the delay is the constant
/// `secs * 1000` ms. In every other case (the `5xx` path's `None`, or a `429`
/// carrying `Retry-After: 0`) the delay is exponential backoff
/// `base_ms * 2^(attempt - 1)` capped at `max_ms`.
fn compute_backoff_delay(
    attempt: u32,
    retry_after_secs: Option<u64>,
    base_ms: u64,
    max_ms: u64,
) -> u64 {
    match retry_after_secs {
        Some(secs) if secs > 0 => secs * 1000,
        _ => {
            let exponent = attempt.saturating_sub(1);
            let delay = base_ms * 2_u64.pow(exponent);
            delay.min(max_ms)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_honored_as_constant_delay() {
        // 429 path with Retry-After: 2 → constant 2000ms regardless of attempt.
        assert_eq!(compute_backoff_delay(1, Some(2), 10, 50), 2000);
        assert_eq!(compute_backoff_delay(2, Some(2), 10, 50), 2000);
        assert_eq!(compute_backoff_delay(3, Some(2), 10, 50), 2000);
    }

    #[test]
    fn retry_after_zero_falls_back_to_exponential() {
        // 429 path with Retry-After: 0 → exponential backoff like the 5xx path.
        assert_eq!(compute_backoff_delay(1, Some(0), 10, 50), 10);
        assert_eq!(compute_backoff_delay(2, Some(0), 10, 50), 20);
        assert_eq!(compute_backoff_delay(3, Some(0), 10, 50), 40);
    }

    #[test]
    fn no_retry_after_uses_exponential_backoff() {
        // 5xx path (None) → base * 2^(attempt-1).
        assert_eq!(compute_backoff_delay(1, None, 10, 10_000), 10);
        assert_eq!(compute_backoff_delay(2, None, 10, 10_000), 20);
        assert_eq!(compute_backoff_delay(3, None, 10, 10_000), 40);
        assert_eq!(compute_backoff_delay(4, None, 10, 10_000), 80);
    }

    #[test]
    fn exponential_backoff_is_capped_at_max() {
        // base 1000, max 10000 (production defaults): 1s, 2s, 4s, 8s, then capped.
        assert_eq!(compute_backoff_delay(1, None, 1000, 10_000), 1000);
        assert_eq!(compute_backoff_delay(2, None, 1000, 10_000), 2000);
        assert_eq!(compute_backoff_delay(3, None, 1000, 10_000), 4000);
        assert_eq!(compute_backoff_delay(4, None, 1000, 10_000), 8000);
        assert_eq!(compute_backoff_delay(5, None, 1000, 10_000), 10_000);
        assert_eq!(compute_backoff_delay(6, None, 1000, 10_000), 10_000);
    }
}
