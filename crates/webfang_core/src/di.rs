//! Dependency Injection Container (legacy re-export)
//!
//! The canonical Container lives in `application::container`. This module
//! provides a `Config`-based convenience constructor for callers that still
//! hold a `Config` struct (e.g., MCP server tests).
//!
//! New code should use `crate::application::container::Container` directly.

use crate::config::Config;

/// Legacy Container re-export — delegates to application container.
///
/// For new code, use `crate::application::container::Container::new()` directly.
pub type Container = crate::application::container::Container;

/// Extension trait for creating a Container from the unified `Config`.
pub trait ContainerExt: Sized {
    /// Create a Container from the unified `Config` struct.
    ///
    /// # Errors
    ///
    /// Returns an error if HTTP client creation fails.
    fn from_config(
        config: Config,
    ) -> impl std::future::Future<Output = Result<Self, Box<dyn std::error::Error + Send + Sync>>> + Send;
}

impl ContainerExt for Container {
    async fn from_config(config: Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::new(config.crawler, config.scraper).await
    }
}
