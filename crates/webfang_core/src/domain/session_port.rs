//! Domain-layer port for session health management.
//!
//! [`SessionPort`] abstracts the session pool so the application layer
//! can gate requests through per-domain health tracking without depending
//! on infrastructure types. The sealed [`SessionManager`] in infrastructure
//! implements this port via [`DomainSessionPool`].

pub use crate::infrastructure::network::session_pool::SessionId;

/// Port trait for per-domain session health management.
///
/// Implemented by [`DomainSessionPool`] in infrastructure.
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
