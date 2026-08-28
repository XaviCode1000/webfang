//! Domain-layer port for session health management.
//!
//! [`SessionPort`] abstracts the session pool so the application layer
//! can gate requests through per-domain health tracking without depending
//! on infrastructure types. The sealed [`crate::infrastructure::network::session_pool::SessionManager`]
//! in infrastructure implements this port via [`crate::infrastructure::network::session_pool::DomainSessionPool`].

use std::fmt;
use std::time::Duration;

use crate::domain::budget::DomainSlots;

/// Domain-owned configuration for the session pool.
///
/// `pool_size` uses the `DomainSlots` newtype (NonZero) so zero is
/// unrepresentable and values are clamped via the budget model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPoolConfig {
    /// Number of session slots per domain.
    pub pool_size: DomainSlots,
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay cap for backoff.
    pub max_delay: Duration,
    /// Maximum exponent for backoff calculation.
    pub max_exp: u32,
    /// TTL for idle sessions before eviction.
    pub ttl_duration: Duration,
}

impl Default for SessionPoolConfig {
    fn default() -> Self {
        Self {
            pool_size: DomainSlots::new(crate::domain::budget::DOMAIN_SLOTS_DEFAULT)
                .unwrap_or_else(|_| unreachable!("domain slot default is non-zero")),
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_exp: 6,
            ttl_duration: Duration::from_secs(300),
        }
    }
}

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
    /// Retrieve the pool configuration (domain DTO).
    fn config(&self) -> SessionPoolConfig {
        SessionPoolConfig::default()
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::budget::DomainSlots;
    use std::time::Duration;

    #[test]
    fn session_pool_config_default_has_domain_slots() {
        let cfg = SessionPoolConfig::default();
        assert_eq!(cfg.pool_size.get(), 8);
        assert_eq!(cfg.base_delay, Duration::from_secs(1));
        assert_eq!(cfg.max_delay, Duration::from_secs(60));

        // Second config with custom slots triangulates.
        let slots = DomainSlots::new(4).unwrap();
        let cfg2 = SessionPoolConfig {
            pool_size: slots,
            ..Default::default()
        };
        assert_eq!(cfg2.pool_size.get(), 4);
        assert_eq!(cfg2.base_delay, Duration::from_secs(1));
    }

    #[test]
    fn session_pool_config_zero_slots_unrepresentable() {
        assert!(DomainSlots::new(0).is_err());
    }

    #[test]
    fn session_id_display_and_session_port_object_safe() {
        assert_eq!(format!("{}", SessionId(42)), "session-42");
        fn assert_dyn(_: &dyn SessionPort) {}
        struct Fake;
        impl SessionPort for Fake {
            fn acquire(&self, _d: &str) -> Option<SessionId> {
                None
            }
            fn report_success(&self, _d: &str, _s: SessionId) {}
            fn report_failure(&self, _d: &str, _s: SessionId, _c: u16) {}
        }
        assert_dyn(&Fake);
    }
}
