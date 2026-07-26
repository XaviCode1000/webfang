//! Domain-layer port for session health management.
//!
//! [`SessionPort`] abstracts the session pool so the application layer
//! can gate requests through per-domain health tracking without depending
//! on infrastructure types. The sealed [`crate::infrastructure::network::session_pool::SessionManager`]
//! in infrastructure implements this port via [`crate::infrastructure::network::session_pool::DomainSessionPool`].

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

/// Port trait for per-domain session health management.
///
/// Implemented by [`crate::infrastructure::network::session_pool::DomainSessionPool`] in infrastructure.
/// The application layer uses this trait — never the concrete pool type.
pub trait SessionPort: Send + Sync {
    /// Acquire an available session for the given domain.
    ///
    /// Returns `None` if all sessions are banned or in cooldown.
    fn acquire(&self, domain: &str) -> Option<SessionId>;

    /// Report a successful request for the given domain's session.
    fn report_success(&self, domain: &str, session: SessionId);

    /// Report a failed request with the HTTP status code.
    ///
    /// Status codes 429, 503, and 403 trigger ban logic in the pool.
    fn report_failure(&self, domain: &str, session: SessionId, status: u16);
}
