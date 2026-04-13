//! HTTP-specific errors with status code information
//!
//! Variants provide specific handling hints:
//! - `Forbidden`: 403 - retry with different UA
//! - `RateLimited`: 429 - respect Retry-After header
//! - `ClientError` / `ServerError`: other 4xx/5xx codes

/// Result type for HttpClient operations
pub type HttpResult<T> = Result<T, HttpError>;

/// HTTP-specific errors with status code information
///
/// Variants provide specific handling hints:
/// - `Forbidden`: 403 - retry with different UA
/// - `RateLimited`: 429 - respect Retry-After header
/// - `ClientError` / `ServerError`: other 4xx/5xx codes
#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    /// 403 Forbidden - site blocking
    Forbidden,
    /// 429 Rate Limited - contains retry-after seconds
    RateLimited(u64),
    /// Other 4xx errors - contains status code
    ClientError(u16),
    /// 5xx server errors - contains status code
    ServerError(u16),
    /// Request timeout
    Timeout,
    /// Connection error - contains error message
    Connection(String),
    /// Request building/error - contains error message
    Request(String),
    /// WAF/CAPTCHA challenge detected in HTTP 200 (false positive)
    WafChallenge(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Forbidden => write!(f, "403 Forbidden - site blocking"),
            HttpError::RateLimited(retry_after) => {
                write!(f, "429 Rate Limited - retry after {} seconds", retry_after)
            },
            HttpError::ClientError(code) => write!(f, "Client Error {}", code),
            HttpError::ServerError(code) => write!(f, "Server Error {}", code),
            HttpError::Timeout => write!(f, "Request Timeout"),
            HttpError::Connection(msg) => write!(f, "Connection Error: {}", msg),
            HttpError::Request(msg) => write!(f, "Request Error: {}", msg),
            HttpError::WafChallenge(provider) => {
                write!(f, "WAF/CAPTCHA challenge detected ({})", provider)
            },
        }
    }
}

impl std::error::Error for HttpError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_error_forbidden() {
        let err = HttpError::Forbidden;
        assert_eq!(err, HttpError::Forbidden);
    }

    #[test]
    fn test_http_error_rate_limited() {
        let err = HttpError::RateLimited(60);
        assert_eq!(err, HttpError::RateLimited(60));

        let err2 = HttpError::RateLimited(30);
        assert_ne!(err, err2);
    }

    #[test]
    fn test_http_error_client_error() {
        let err = HttpError::ClientError(404);
        assert_eq!(err, HttpError::ClientError(404));
    }

    #[test]
    fn test_http_error_server_error() {
        let err = HttpError::ServerError(500);
        assert_eq!(err, HttpError::ServerError(500));
    }

    #[test]
    fn test_http_error_timeout() {
        let err = HttpError::Timeout;
        assert_eq!(err, HttpError::Timeout);
    }

    #[test]
    fn test_http_error_connection() {
        let err = HttpError::Connection("Connection refused".into());
        assert_eq!(err, HttpError::Connection("Connection refused".into()));
    }

    #[test]
    fn test_http_error_request() {
        let err = HttpError::Request("Invalid URL".into());
        assert_eq!(err, HttpError::Request("Invalid URL".into()));
    }

    #[test]
    fn test_http_error_display() {
        assert_eq!(
            format!("{}", HttpError::Forbidden),
            "403 Forbidden - site blocking"
        );
        assert_eq!(
            format!("{}", HttpError::RateLimited(30)),
            "429 Rate Limited - retry after 30 seconds"
        );
        assert_eq!(
            format!("{}", HttpError::ClientError(404)),
            "Client Error 404"
        );
        assert_eq!(
            format!("{}", HttpError::ServerError(500)),
            "Server Error 500"
        );
        assert_eq!(format!("{}", HttpError::Timeout), "Request Timeout");
    }
}
