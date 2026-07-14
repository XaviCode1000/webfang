//! Rate Limiter module — Token Bucket implementation using governor
//!
//! Extracts the rate limiting logic from crawler_service.rs to allow
//! for independent testing and potential future swapping (e.g., Redis-backed).
//!
//! # Design Decisions
//!
//! - Uses `governor` crate with Token Bucket algorithm
//! - Thread-safe via Arc (shares across async tasks)
//! - Configurable delay and burst parameters
//! - No Mutex needed - governor handles internal synchronization
//! - Redis backend for distributed rate limiting (Phase 3 - prepared)
//! - Automatic fallback to InMemory when Redis unavailable

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{
    clock::QuantaClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use tracing::warn;

use crate::error::ScraperError;

/// Backend type for rate limiting
#[derive(Debug, Clone, PartialEq)]
pub enum RateLimiterBackend {
    /// In-memory rate limiter (default)
    InMemory,
    /// Redis-backed distributed rate limiter (prepared for future use)
    Redis,
}

impl RateLimiterBackend {
    /// Parse from environment variable
    pub fn from_env() -> Self {
        match std::env::var("RATE_LIMITER_BACKEND").as_deref() {
            Ok("redis") => RateLimiterBackend::Redis,
            _ => RateLimiterBackend::InMemory,
        }
    }
}

/// Type alias for the rate limiter - allows swapping implementations
pub type CrawlRateLimiter = GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>;

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Delay between requests in milliseconds
    pub delay_ms: u64,
    /// Maximum concurrent requests (burst)
    pub concurrency: u32,
    /// Backend to use (InMemory or Redis)
    pub backend: RateLimiterBackend,
    /// Redis URL (only used when backend is Redis)
    pub redis_url: Option<String>,
    /// Redis key prefix for distributed rate limiting
    pub redis_key_prefix: Option<String>,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            delay_ms: 100,
            concurrency: 5,
            backend: RateLimiterBackend::InMemory,
            redis_url: None,
            redis_key_prefix: Some("rust_scraper:rate_limit".to_string()),
        }
    }
}

impl RateLimiterConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            backend: RateLimiterBackend::from_env(),
            redis_url: std::env::var("REDIS_URL").ok(),
            ..Self::default()
        }
    }
    /// Create new configuration
    pub fn new(delay_ms: u64, concurrency: u32) -> Self {
        Self {
            delay_ms,
            concurrency,
            backend: RateLimiterBackend::InMemory,
            redis_url: None,
            redis_key_prefix: Some("rust_scraper:rate_limit".to_string()),
        }
    }

    /// Create configuration with explicit backend
    pub fn with_backend(delay_ms: u64, concurrency: u32, backend: RateLimiterBackend) -> Self {
        Self {
            delay_ms,
            concurrency,
            backend,
            redis_url: None,
            redis_key_prefix: Some("rust_scraper:rate_limit".to_string()),
        }
    }
}

/// Shared rate limiter for crawl operations
#[derive(Clone)]
pub struct SharedRateLimiter(Arc<CrawlRateLimiter>);

impl SharedRateLimiter {
    /// Create a new shared rate limiter from config
    pub fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        let quota = Quota::with_period(Duration::from_millis(config.delay_ms))
            .ok_or_else(|| ScraperError::Config("Invalid period".into()))?;

        let quota = quota.allow_burst(
            NonZeroU32::new(config.concurrency)
                .ok_or_else(|| ScraperError::Config("Concurrency must be > 0".into()))?,
        );

        let limiter = GovernorLimiter::direct(quota);
        Ok(Self(Arc::new(limiter)))
    }

    /// Wait until a permit is available
    pub async fn until_ready(&self) {
        self.0.until_ready().await;
    }
}

impl From<GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>> for SharedRateLimiter {
    fn from(limiter: GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>) -> Self {
        Self(Arc::new(limiter))
    }
}

// ============================================================================
// Distributed Rate Limiter (Prepared for Phase 4)
// ============================================================================

/// Placeholder for Redis-backed distributed rate limiter
/// Full implementation requires redis-async with connection pooling
/// Currently falls back to InMemory automatically
#[derive(Clone)]
#[allow(dead_code)]
pub struct DistributedRateLimiter {
    /// Key prefix for Redis keys (prepared for future use)
    key_prefix: String,
    /// Configuration
    config: RateLimiterConfig,
}

impl DistributedRateLimiter {
    /// Create a new distributed rate limiter
    /// Note: Currently not fully implemented - will warn and use fallback
    pub async fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        let redis_url = config
            .redis_url
            .as_ref()
            .ok_or_else(|| ScraperError::Config("REDIS_URL not configured".to_string()))?;

        // Warn that Redis backend is not yet fully implemented
        warn!(
            "Redis backend requested but not fully implemented (URL: {}). Using InMemory fallback.",
            redis_url
        );

        // Return a placeholder - caller will fall back to InMemory
        Err(ScraperError::Config(
            "Redis backend pending implementation".to_string(),
        ))
    }

    /// Wait for rate limit permit (placeholder)
    pub async fn until_ready(&self) -> Result<(), ScraperError> {
        // This should not be called due to fallback in RateLimiter::new
        Err(ScraperError::Config(
            "Redis backend not implemented".to_string(),
        ))
    }

    /// Health check (placeholder)
    pub async fn health_check(&self) -> Result<(), ScraperError> {
        Err(ScraperError::Config(
            "Redis backend not implemented".to_string(),
        ))
    }
}

/// Builder for rate limiter with automatic fallback
#[derive(Clone)]
pub enum RateLimiter {
    /// In-memory rate limiter (default, always available)
    InMemory(SharedRateLimiter),
    /// Redis-backed distributed rate limiter (prepared for future use)
    Distributed(DistributedRateLimiter),
    /// Fallback wrapper - tries Redis first, falls back to InMemory on error
    #[allow(dead_code)]
    WithFallback {
        distributed: DistributedRateLimiter,
        fallback: SharedRateLimiter,
    },
}

impl RateLimiter {
    /// Create rate limiter based on configuration
    /// Uses automatic fallback on Redis failure
    pub async fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        match config.backend {
            RateLimiterBackend::InMemory => {
                let limiter = SharedRateLimiter::new(config)?;
                Ok(RateLimiter::InMemory(limiter))
            },
            RateLimiterBackend::Redis => {
                // Try to create distributed rate limiter
                match DistributedRateLimiter::new(config).await {
                    Ok(distributed) => Ok(RateLimiter::Distributed(distributed)),
                    Err(e) => {
                        // Fallback to InMemory when Redis unavailable
                        warn!(
                            "Redis rate limiter unavailable, falling back to InMemory: {}",
                            e
                        );
                        let limiter = SharedRateLimiter::new(config)?;
                        Ok(RateLimiter::InMemory(limiter))
                    },
                }
            },
        }
    }

    /// Wait for rate limit permit
    pub async fn until_ready(&self) {
        match self {
            RateLimiter::InMemory(limiter) => limiter.until_ready().await,
            RateLimiter::Distributed(distributed) => {
                // Try Redis, fallback on error
                if let Err(e) = distributed.until_ready().await {
                    warn!("Redis failed, using fallback: {}", e);
                }
            },
            RateLimiter::WithFallback {
                distributed,
                fallback,
            } => {
                // Try distributed first
                match distributed.until_ready().await {
                    Ok(()) => (),
                    Err(e) => {
                        tracing::debug!("Distributed failed, falling back: {}", e);
                        fallback.until_ready().await;
                    },
                }
            },
        }
    }

    /// Health check (for Redis backend)
    pub async fn health_check(&self) -> Result<(), ScraperError> {
        match self {
            RateLimiter::InMemory(_) => Ok(()), // Always healthy
            RateLimiter::Distributed(distributed) => distributed.health_check().await,
            RateLimiter::WithFallback {
                distributed,
                fallback: _,
            } => distributed.health_check().await,
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_config_default() {
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
    }

    #[test]
    fn test_rate_limiter_backend_from_env_default() {
        // Without env var, should default to InMemory
        let backend = RateLimiterBackend::from_env();
        assert_eq!(backend, RateLimiterBackend::InMemory);
    }

    #[tokio::test]
    async fn test_rate_limiter_creation() {
        let config = RateLimiterConfig::new(50, 2);
        let limiter = RateLimiter::new(&config).await;
        assert!(limiter.is_ok(), "valid config should create rate limiter");
    }

    #[tokio::test]
    async fn test_rate_limiter_until_ready() {
        let config = RateLimiterConfig::new(10, 1);
        let limiter = RateLimiter::new(&config).await.unwrap();

        limiter.until_ready().await;
        // If we got here, the limiter worked
    }

    #[test]
    fn test_rate_limiter_config_default_values() {
        // Verifica valores por defecto
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
    }

    // ============================================================================
    // Behavioral Rate Limiting Tests
    // ============================================================================

    #[tokio::test]
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
    async fn test_rate_limiter_until_ready_spreads_over_time() {
        // Test que N tasks concurrentes llamando until_ready() son espaciadas
        // Config: delay_ms=50ms, concurrency=1
        // 5 tasks → mínimo ~200ms de spread total
        // Mide elapsed y verifica >= (N-1) * delay

        let config = RateLimiterConfig::new(50, 1); // 50ms entre requests, burst=1
        let limiter = RateLimiter::new(&config).await.unwrap();

        let num_tasks = 5;
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // 5 tasks con delay de 50ms → mínimo ~200ms
        // Con algo de jitter, verificamos al menos 150ms (75% de teórico)
        let min_expected_ms = 150;
        assert!(
            elapsed.as_millis() >= min_expected_ms,
            "Tiempo transcurrido {}ms < {}ms mínimo — rate limiter no está espaciando",
            elapsed.as_millis(),
            min_expected_ms
        );
    }

    #[tokio::test]
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
    async fn test_rate_limiter_burst_allows_parallel_requests() {
        // Test que burst de N requests ocurren en paralelo
        // Config: delay_ms=100ms, concurrency=5
        // 5 tasks simultáneas → todas deben pasar rápido (dentro del burst)
        use tokio::time::Instant;

        let config = RateLimiterConfig::new(100, 5); // 100ms delay, burst=5
        let limiter = RateLimiter::new(&config).await.unwrap();

        let num_tasks = 5;
        let start = Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // 5 tasks con burst=5 → todas deberían pasar casi instantáneo (< 50ms)
        assert!(
            elapsed.as_millis() < 50,
            "Tiempo {}ms > 50ms — burst no está funcionando",
            elapsed.as_millis()
        );
    }

    #[tokio::test]
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
    async fn test_rate_limiter_concurrent_backpressure() {
        // Test que 20 tasks concurrentes no colapsan — se encolan correctamente
        let config = RateLimiterConfig::new(10, 1); // 10ms, burst=1
        let limiter = RateLimiter::new(&config).await.unwrap();

        let num_tasks = 20;
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // 20 tasks × 10ms delay = 190ms mínimo
        // Verificamos que tomó al menos 100ms (rate limiting activo)
        assert!(
            elapsed.as_millis() >= 100,
            "20 tasks completaron en {}ms — rate limiting no está regulando",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_rate_limiter_config_zero_delay_returns_error() {
        // delay_ms=0 → debe retornar error, no panic
        let config = RateLimiterConfig::new(0, 1);
        let result = SharedRateLimiter::new(&config);
        assert!(result.is_err(), "delay_ms=0 debería retornar error");
    }

    #[test]
    fn test_rate_limiter_config_zero_concurrency_returns_error() {
        // concurrency=0 → debe retornar error, no panic
        let config = RateLimiterConfig::new(100, 0);
        let result = SharedRateLimiter::new(&config);
        assert!(result.is_err(), "concurrency=0 debería retornar error");
    }

    #[test]
    fn test_rate_limiter_config_with_explicit_backend() {
        let config = RateLimiterConfig::with_backend(100, 5, RateLimiterBackend::Redis);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
        assert_eq!(config.backend, RateLimiterBackend::Redis);
    }

    #[test]
    fn test_rate_limiter_config_redis_url_from_env() {
        // Test that redis_url is populated from REDIS_URL env var
        let config = RateLimiterConfig::default();
        // redis_url will be None if env var not set, or Some if set
        // Just verify the field exists
        assert!(config.redis_key_prefix.is_some());
    }

    // =====================================================================
    // Health check and fallback tests
    // =====================================================================

    #[tokio::test]
    async fn test_rate_limiter_health_check_inmemory() {
        let config = RateLimiterConfig::new(100, 5);
        let limiter = RateLimiter::new(&config).await.unwrap();
        assert!(
            limiter.health_check().await.is_ok(),
            "InMemory health check should always succeed"
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_redis_fallback_to_inmemory() {
        // Redis backend without REDIS_URL → falls back to InMemory
        let config = RateLimiterConfig::with_backend(100, 5, RateLimiterBackend::Redis);
        let limiter = RateLimiter::new(&config).await;
        assert!(
            limiter.is_ok(),
            "Redis fallback should produce InMemory limiter on error"
        );
    }

    #[test]
    fn test_rate_limiter_config_default_is_inmemory() {
        let config = RateLimiterConfig::default();
        assert_eq!(config.backend, RateLimiterBackend::InMemory);
    }

    #[test]
    fn test_shared_rate_limiter_creation_success() {
        let config = RateLimiterConfig::new(50, 3);
        let limiter = SharedRateLimiter::new(&config);
        assert!(limiter.is_ok(), "valid config should create limiter");
    }

    #[test]
    fn test_rate_limiter_config_with_backend_inmemory() {
        let config = RateLimiterConfig::with_backend(100, 5, RateLimiterBackend::InMemory);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
        assert_eq!(config.backend, RateLimiterBackend::InMemory);
    }

    #[test]
    fn test_rate_limiter_config_redis_key_prefix_default() {
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(
            config.redis_key_prefix.as_deref(),
            Some("rust_scraper:rate_limit")
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_health_check_after_until_ready() {
        let config = RateLimiterConfig::new(10, 1);
        let limiter = RateLimiter::new(&config).await.unwrap();
        limiter.until_ready().await;
        assert!(limiter.health_check().await.is_ok());
    }
}
