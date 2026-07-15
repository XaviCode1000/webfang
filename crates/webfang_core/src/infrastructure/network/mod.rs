//! Network infrastructure — Session pool, connection management, retry strategies.
//!
//! Following Clean Architecture: infrastructure depends on domain, not vice versa.

pub mod session_pool;
