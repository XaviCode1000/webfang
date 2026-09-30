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
        Some(value) if value == format!("Bearer {expected}") => Ok(next.run(request).await),
        _ => {
            tracing::warn!(
                remote = ?request.uri(),
                "rejected unauthenticated request"
            );
            Err(StatusCode::UNAUTHORIZED)
        },
    }
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

    /// #1611 G-18: the shipped default — no token, no opt-in — REFUSES. This
    /// is the regression row for the finding itself: it used to return 200
    /// here, which is what made "loopback by default" mean "open to anything
    /// that can reach the socket".
    #[tokio::test]
    async fn refuses_every_request_when_no_token_is_configured_and_none_was_asked_for() {
        let app = app_with_auth(None, false);
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A bearer header does not buy a way in when nothing is configured to
    /// compare it against: there is no credential, so every presented value is
    /// wrong.
    #[tokio::test]
    async fn a_presented_token_is_still_refused_when_none_is_configured() {
        let app = app_with_auth(None, false);
        let req = Request::builder()
            .uri("/test")
            .header("Authorization", "Bearer anything")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// The opt-in restores the development mode, and ONLY on its own: it is
    /// ignored while a token is configured, because "anonymous allowed" must
    /// never mean "the configured token is optional".
    #[tokio::test]
    async fn passes_when_anonymous_operation_was_explicitly_allowed() {
        let app = app_with_auth(None, true);
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_configured_token_is_required_even_when_anonymous_is_allowed() {
        let app = app_with_auth(Some(Arc::from("secret")), true);
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_missing_auth_header() {
        let app = app_with_token(Some(Arc::from("secret")));
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_wrong_token() {
        let app = app_with_token(Some(Arc::from("secret")));
        let req = Request::builder()
            .uri("/test")
            .header("Authorization", "Bearer wrong")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_correct_token() {
        let app = app_with_token(Some(Arc::from("secret")));
        let req = Request::builder()
            .uri("/test")
            .header("Authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
