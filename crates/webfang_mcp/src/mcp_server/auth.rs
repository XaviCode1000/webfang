//! Auth middleware — Bearer token validation.
//!
//! When an auth token is configured, every request must include an
//! `Authorization: Bearer <token>` header. Requests missing or carrying
//! an invalid token receive a `401 Unauthorized` response before any
//! downstream handler runs.

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
    /// The expected Bearer token. When `None`, all requests pass through.
    pub expected_token: Option<Arc<str>>,
}

/// Axum middleware that validates the `Authorization: Bearer` header.
///
/// # Behavior
///
/// - If no token is configured (`expected_token` is `None`), the request
///   passes through unconditionally.
/// - If a token is configured, the request must carry an
///   `Authorization: Bearer <token>` header matching exactly.
/// - Missing, malformed, or mismatched headers receive `401 Unauthorized`.
///
/// # Errors
///
/// Returns `401 Unauthorized` when a token is configured but the request
/// is missing, malformed, or carries a non-matching `Authorization` header.
pub async fn validate_auth(
    State(state): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<impl IntoResponse, StatusCode> {
    let Some(expected) = &state.expected_token else {
        return Ok(next.run(request).await);
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
        axum::Router::new()
            .route("/test", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                AuthState {
                    expected_token: token,
                },
                validate_auth,
            ))
    }

    #[tokio::test]
    async fn passes_when_no_token_configured() {
        let app = app_with_token(None);
        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
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
