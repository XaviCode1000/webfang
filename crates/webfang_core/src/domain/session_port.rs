//! Session pool port trait — Domain-level abstraction for session management.
//!
//! Following Hexagonal Architecture: the domain defines the contract for
//! session health tracking, and the infrastructure layer provides the
//! real `DomainSessionPool` implementation. This keeps `HttpClient`
//! decoupled from the concrete DashMap-backed pool.
//!
//! # Design Decision (Issue #176)
//!
//! The existing `SessionManager` trait in `session_pool.rs` is **sealed** —
//! only `DomainSessionPool` can implement it. This prevents us from using
//! it as a trait object from outside the infrastructure module. Instead,
//! we define `SessionPort` here in the domain layer as a public, unsealed
//! trait that `DomainSessionPool` will also implement.

use std::fmt;

/// Unique identifier for a session slot within a domain pool.
///
/// Defined in the domain layer so that `SessionPort` and its consumers
/// never depend on infrastructure types. Infrastructure converts
/// internally when needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub usize);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

/// Port trait for session health tracking — domain layer contract.
///
/// The application layer depends on this trait, not on the concrete
/// `DomainSessionPool` or the sealed `SessionManager`. This allows
/// test doubles and alternative implementations.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to work with Tokio's
/// multi-threaded runtime.
pub trait SessionPort: Send + Sync {
    /// Acquire an available session for the given domain.
    ///
    /// Returns `None` if all sessions are banned or in cooldown,
    /// meaning the domain should be treated as banned.
    fn acquire(&self, domain: &str) -> Option<SessionId>;

    /// Report a successful request for the given domain's session.
    fn report_success(&self, domain: &str, session: SessionId);

    /// Report a failed request with the HTTP status code.
    ///
    /// Status codes 429, 503, and 403 trigger ban logic with
    /// exponential backoff.
    fn report_failure(&self, domain: &str, session: SessionId, status: u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Test double for SessionPort — records all calls for assertions.
    struct MockSessionPort {
        acquire_result: Option<SessionId>,
        success_calls: std::sync::Mutex<Vec<(String, SessionId)>>,
        failure_calls: std::sync::Mutex<Vec<(String, SessionId, u16)>>,
    }

    impl MockSessionPort {
        fn new(acquire_result: Option<SessionId>) -> Self {
            Self {
                acquire_result,
                success_calls: std::sync::Mutex::new(Vec::new()),
                failure_calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn success_count(&self) -> usize {
            self.success_calls.lock().unwrap().len()
        }

        fn failure_count(&self) -> usize {
            self.failure_calls.lock().unwrap().len()
        }
    }

    impl SessionPort for MockSessionPort {
        fn acquire(&self, _domain: &str) -> Option<SessionId> {
            self.acquire_result
        }

        fn report_success(&self, domain: &str, session: SessionId) {
            self.success_calls
                .lock()
                .unwrap()
                .push((domain.to_string(), session));
        }

        fn report_failure(&self, domain: &str, session: SessionId, status: u16) {
            self.failure_calls
                .lock()
                .unwrap()
                .push((domain.to_string(), session, status));
        }
    }

    #[test]
    fn mock_acquire_returns_configured_result() {
        let mock = MockSessionPort::new(Some(SessionId(42)));
        assert_eq!(mock.acquire("example.com"), Some(SessionId(42)));

        let mock_none = MockSessionPort::new(None);
        assert_eq!(mock_none.acquire("example.com"), None);
    }

    #[test]
    fn mock_records_success_calls() {
        let mock = MockSessionPort::new(Some(SessionId(0)));
        mock.report_success("example.com", SessionId(0));
        mock.report_success("other.com", SessionId(1));
        assert_eq!(mock.success_count(), 2);
    }

    #[test]
    fn mock_records_failure_calls() {
        let mock = MockSessionPort::new(Some(SessionId(0)));
        mock.report_failure("example.com", SessionId(0), 429);
        assert_eq!(mock.failure_count(), 1);
    }

    #[test]
    fn session_port_is_object_safe() {
        let _: Box<dyn SessionPort> = Box::new(MockSessionPort::new(Some(SessionId(0))));
    }

    #[test]
    fn session_port_works_with_arc() {
        let port: Arc<dyn SessionPort> = Arc::new(MockSessionPort::new(Some(SessionId(5))));
        assert_eq!(port.acquire("test.com"), Some(SessionId(5)));
    }
}
