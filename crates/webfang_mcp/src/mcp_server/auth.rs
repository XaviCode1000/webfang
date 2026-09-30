//! Auth middleware — Bearer token validation.
//!
//! Every request must carry an `Authorization: Bearer <token>` header matching
//! the configured token, UNLESS the operator explicitly opted into anonymous
//! operation. Requests missing or carrying an invalid token receive a `401
//! Unauthorized` response before any downstream handler runs.
//!
//! # Fail-closed by default (#1611, G-18)
//!
//! With no token configured this middleware used to pass EVERY request
//! through, which made "loopback, no token" — the shipped default — mean
//! "anyone who can open a socket may use every scraper tool". An unset
//! credential is now a refusal, not a mode: anonymous operation has to be
//! asked for (`allow_anonymous`), and the composition root that builds it says
//! so out loud at startup.
//!
//! # Constant-time bearer comparison (#1615, F9 / H-4)
//!
//! The credential is compared with
//! [`webfang_core::domain::credentials::constant_time_eq`] instead
//! of `==`, which short-circuits on the first differing byte. `AV-6` in the
//! threat model is explicit that this is a marginal signal over TCP — the
//! transport dominates and a remote attacker fights jitter — but it is one
//! function, and using it here closes the class for every bearer comparison
//! rather than leaving the argument to be re-derived per call site.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use std::sync::Arc;

/// Shared state injected into the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    /// The expected Bearer token. When `None`, auth is disabled.
    pub expected_token: Option<Arc<str>>,
    /// Whether the operator explicitly accepted token-less operation (#1611,
    /// G-18).
    ///
    /// Ignored when `expected_token` is `Some`: a configured token is always
    /// required. Only the combination "no token AND `allow_anonymous`" serves
    /// an unauthenticated request — the opt-in that turns the fail-closed
    /// default into the development mode it was always meant to be.
    pub allow_anonymous: bool,
}

/// Spanish sentence an operator needs when a request is refused because no
/// token is configured: the refusal is correct and useless without the two
/// ways out, so both are named. Tracing field names and event text stay
/// English.
const NO_TOKEN_CONFIGURED_MESSAGE: &str =
    "Servidor MCP sin token de autenticación configurado. Defina --auth-token (o WEBFANG_MCP_AUTH_TOKEN), o --allow-anonymous para operar sin token en loopback.";

/// Axum middleware that validates the `Authorization: Bearer` header.
///
/// # Behavior
///
/// - If a token is configured, the request must carry an
///   `Authorization: Bearer <token>` header matching exactly.
/// - If no token is configured and [`AuthState::allow_anonymous`] is set, the
///   request passes through unconditionally (development mode; the composition
///   root restricts this to loopback binds).
/// - If no token is configured and `allow_anonymous` is NOT set — the default
///   — every request is refused. Failing closed is the point: an unset
///   credential must never silently mean "open to anyone" (#1611, G-18).
/// - Missing, malformed, or mismatched headers receive `401 Unauthorized`.
///
/// # Errors
///
/// Returns `401 Unauthorized` when a token is configured but the request
/// is missing, malformed, or carries a non-matching `Authorization` header,
/// and when no token is configured at all without the anonymous opt-in.
pub async fn validate_auth(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<impl IntoResponse, StatusCode> {
    let Some(expected) = &state.expected_token else {
        if state.allow_anonymous {
            return Ok(next.run(request).await);
        }
        tracing::warn!(
            remote = ?request.uri(),
            user_message = NO_TOKEN_CONFIGURED_MESSAGE,
            "refused a request because no auth token is configured — \
             authentication is fail-closed by default"
        );
        return Err(StatusCode::UNAUTHORIZED);
    };

    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if bearer_token_matches(value, expected) => Ok(next.run(request).await),
        _ => {
            // #1615 F9: the refusal names only WHETHER a header was presented.
            // The value is never logged, and neither is a digest of it: an
            // operator debugging a 401 loop needs to tell "my client sent
            // nothing" from "my client sent the wrong thing", and a boolean
            // answers that without turning the log into a credential oracle.
            tracing::warn!(
                remote = ?request.uri(),
                header_present = auth_header.is_some(),
                "rejected unauthenticated request"
            );
            Err(StatusCode::UNAUTHORIZED)
        },
    }
}

/// Constant-time match of an `Authorization` header against the expected
/// bearer token (#1615, F9 / H-4).
///
/// Parses the `Bearer ` scheme prefix, then compares the credential itself
/// with the shared constant-time helper in `webfang_core`. Returns `false` for
/// any other scheme — a present-but-wrong scheme is a refusal, not a fallback
/// to a case-insensitive match. This is exactly the behaviour `==` against
/// `format!("Bearer {expected}")` had; only the comparison's timing changed.
fn bearer_token_matches(header_value: &str, expected: &str) -> bool {
    const BEARER_PREFIX: &str = "Bearer ";
    let Some(presented) = header_value.strip_prefix(BEARER_PREFIX) else {
        return false;
    };
    webfang_core::domain::credentials::constant_time_eq(presented.as_bytes(), expected.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app_with_token(token: Option<Arc<str>>) -> axum::Router {
        app_with_auth(token, false)
    }

    /// The full shape: a token, and whether the operator opted into anonymous
    /// operation (#1611, G-18).
    fn app_with_auth(token: Option<Arc<str>>, allow_anonymous: bool) -> axum::Router {
        axum::Router::new()
            .route("/test", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                AuthState {
                    expected_token: token,
                    allow_anonymous,
                },
                validate_auth,
            ))
    }

    /// One GET to `/test` with an optional header, returning only the status:
    /// the middleware's observable contract is the status code.
    async fn status_of(app: axum::Router, header: Option<(&str, &str)>) -> StatusCode {
        let mut builder = Request::builder().uri("/test");
        if let Some((name, value)) = header {
            builder = builder.header(name, value);
        }
        let req = builder.body(Body::empty()).unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    /// #1611 G-18: the shipped default — no token, no opt-in — REFUSES. This
    /// is the regression row for the finding itself: it used to return 200
    /// here, which is what made "loopback by default" mean "open to anything
    /// that can reach the socket".
    #[tokio::test]
    async fn refuses_every_request_when_no_token_is_configured_and_none_was_asked_for() {
        assert_eq!(
            status_of(app_with_auth(None, false), None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// A bearer header does not buy a way in when nothing is configured to
    /// compare it against: there is no credential, so every presented value is
    /// wrong.
    #[tokio::test]
    async fn a_presented_token_is_still_refused_when_none_is_configured() {
        assert_eq!(
            status_of(
                app_with_auth(None, false),
                Some(("Authorization", "Bearer anything"))
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// The opt-in restores the development mode, and ONLY on its own: it is
    /// ignored while a token is configured, because "anonymous allowed" must
    /// never mean "the configured token is optional".
    #[tokio::test]
    async fn passes_when_anonymous_operation_was_explicitly_allowed() {
        assert_eq!(
            status_of(app_with_auth(None, true), None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_configured_token_is_required_even_when_anonymous_is_allowed() {
        assert_eq!(
            status_of(app_with_auth(Some(Arc::from("secret")), true), None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rejects_missing_auth_header() {
        assert_eq!(
            status_of(app_with_token(Some(Arc::from("secret"))), None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rejects_wrong_token() {
        assert_eq!(
            status_of(
                app_with_token(Some(Arc::from("secret"))),
                Some(("Authorization", "Bearer wrong"))
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn accepts_correct_token() {
        assert_eq!(
            status_of(
                app_with_token(Some(Arc::from("secret"))),
                Some(("Authorization", "Bearer secret"))
            )
            .await,
            StatusCode::OK
        );
    }

    /// Run `f` on the current thread under an in-memory `tracing` subscriber and
    /// return the log text.
    ///
    /// Async version of the `capture_logs` in the HTTP binary: the subscriber
    /// guard is thread-local, and a single-threaded tokio runtime keeps the
    /// middleware's events on THIS thread, so the capture sees them.
    async fn capture_logs_async<F, Fut>(f: F) -> String
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        use std::sync::{Arc, Mutex};

        /// Shared sink behind the subscriber's writer.
        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("capture buffer lock is not poisoned")
                    .write(bytes)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                self.0
                    .lock()
                    .expect("capture buffer lock is not poisoned")
                    .flush()
            }
        }

        let buffer = Buffer::default();
        let sink = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || sink.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        f().await;
        let bytes = buffer
            .0
            .lock()
            .expect("capture buffer lock is not poisoned")
            .clone();
        String::from_utf8(bytes).expect("tracing writes UTF-8")
    }

    /// #1615 F9: a rejection log must not carry EITHER credential.
    ///
    /// The expected token is the one an operator must never be able to read out
    /// of a log — it is the live secret. The presented one is the one they
    /// least expect to see echoed, because it is attacker-supplied and a log
    /// that echoes it is a log that has been fed whatever someone chose to
    /// type. The refusal log names only whether a header arrived, so neither
    /// string may appear. The TRACE subscriber level is deliberate: this
    /// asserts the whole span, not just the `warn!`.
    #[tokio::test]
    async fn a_rejection_log_carries_neither_the_expected_nor_the_presented_token() {
        const EXPECTED: &str = "sk-expected-secret-1615";
        const PRESENTED: &str = "sk-attacker-supplied-1615";

        let logs = capture_logs_async(|| async {
            let app = app_with_token(Some(Arc::from(EXPECTED)));
            let req = Request::builder()
                .uri("/test")
                .header("Authorization", format!("Bearer {PRESENTED}"))
                .body(Body::empty())
                .expect("request builds");
            let response = app.oneshot(req).await.expect("oneshot drives the router");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        })
        .await;

        assert!(
            !logs.contains(EXPECTED),
            "the configured token must never be logged: {logs}"
        );
        assert!(
            !logs.contains(PRESENTED),
            "the presented token must never be echoed: {logs}"
        );
        assert!(
            logs.contains("rejected unauthenticated request"),
            "the refusal must still be observable — this test is about what the \
             log says, not whether it fires: {logs}"
        );
        assert!(
            logs.contains("header_present=true"),
            "presence is the only credential fact the refusal may report: {logs}"
        );
    }

    /// #1615 F9 / H-4: the scheme prefix is part of what is compared, and a
    /// near-miss in it is still a refusal. This is the behavioural half of the
    /// constant-time swap — the helper must agree with the `==` it replaced, or
    /// the hardening has become an authentication bypass.
    #[test]
    fn the_bearer_match_accepts_exactly_what_the_prefixed_equality_did() {
        for (header, expected, should_pass) in [
            ("Bearer secret", "secret", true),
            ("Bearer  secret", "secret", false),
            ("bearer secret", "secret", false),
            ("BEARER secret", "secret", false),
            ("Bearer secret ", "secret", false),
            ("Bearer  secret ", "secret", false),
            ("Bearer", "secret", false),
            ("secret", "secret", false),
            ("", "secret", false),
            ("Basic c2VjcmV0", "secret", false),
        ] {
            assert_eq!(
                bearer_token_matches(header, expected),
                should_pass,
                "header {header:?} against expected {expected:?}"
            );
            // The invariant the fix must preserve, stated as a check rather
            // than as a comment: identical to what `header == "Bearer {expected}"`
            // decided, for every one of these shapes.
            assert_eq!(
                bearer_token_matches(header, expected),
                header == format!("Bearer {expected}"),
                "behaviour drifted from the `==` it replaced for {header:?}"
            );
        }
    }
}
