//! User-agent provider port — domain-owned DTO + trait.

/// DTO for a pool of user-agent strings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserAgentPool {
    /// Ordered list of UA strings (Chrome 131+ or fallback).
    pub agents: Vec<String>,
}

impl UserAgentPool {
    /// Create a pool from a list of agents.
    #[must_use]
    pub fn new(agents: Vec<String>) -> Self {
        Self { agents }
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Number of agents in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }
}

/// Domain port for user-agent provisioning.
///
/// Infrastructure (`UserAgentCache`) implements this trait with
/// TTL-based caching and network fetch. Application uses the port
/// via `Arc<dyn UserAgentProvider>`.
pub trait UserAgentProvider: Send + Sync {
    /// Load the current pool (cache hit or fallback).
    ///
    /// Implementations should never return an empty vec — fallback
    /// to hardcoded agents on failure.
    fn load(&self) -> Vec<String>;

    /// Hardcoded fallback agents (pure, no IO).
    fn fallback_agents(&self) -> Vec<String> {
        vec![
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36".to_string(),
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36".to_string(),
        ]
    }

    /// Pick a random UA from the pool (pure helper).
    fn pick_random(&self, pool: &[String]) -> Option<String> {
        if pool.is_empty() {
            return None;
        }
        use rand::Rng;
        let idx = rand::rng().random_range(0..pool.len());
        pool.get(idx).cloned()
    }
}

/// Free-function fallback agents (domain pure, no trait object needed).
///
/// Mirrors `infrastructure::user_agent::UserAgentCache::fallback_agents` so
/// `application` can import from `domain` without violating inward-only.
#[must_use]
pub fn fallback_agents() -> Vec<String> {
    vec![
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36".to_string(),
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:123.0) Gecko/20100101 Firefox/123.0".to_string(),
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:123.0) Gecko/20100101 Firefox/123.0".to_string(),
    ]
}

/// Pick a random UA from a slice (free function, domain pure).
#[must_use]
pub fn get_random_user_agent_from_pool(pool: &[String]) -> String {
    use rand::Rng;
    if pool.is_empty() {
        return String::new();
    }
    let idx = rand::rng().random_range(0..pool.len());
    pool[idx].clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider {
        agents: Vec<String>,
    }

    impl UserAgentProvider for FakeProvider {
        fn load(&self) -> Vec<String> {
            self.agents.clone()
        }
    }

    #[test]
    fn user_agent_pool_new_and_len() {
        let pool = UserAgentPool::new(vec!["a".into(), "b".into()]);
        assert_eq!(pool.len(), 2);
        assert!(!pool.is_empty());
        assert_eq!(pool.agents, vec!["a", "b"]);

        let empty = UserAgentPool::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn user_agent_provider_load_and_fallback() {
        let p = FakeProvider {
            agents: vec!["Chrome/131".into(), "Firefox/123".into()],
        };
        let loaded = p.load();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains(&"Chrome/131".to_string()));

        let fallback = p.fallback_agents();
        assert!(!fallback.is_empty());
        assert!(fallback.iter().any(|ua| ua.contains("Chrome")));

        // Second provider with different data.
        let p2 = FakeProvider { agents: vec![] };
        assert!(p2.load().is_empty());
        assert!(!p2.fallback_agents().is_empty());
    }

    #[test]
    fn pick_random_returns_member_or_none() {
        let p = FakeProvider { agents: vec![] };
        assert!(p.pick_random(&[]).is_none());
        let pool = vec!["a".into(), "b".into(), "c".into()];
        let picked = p.pick_random(&pool);
        assert!(picked.is_some());
        assert!(pool.contains(&picked.unwrap()));

        let single = vec!["only".into()];
        assert_eq!(p.pick_random(&single), Some("only".to_string()));
    }

    #[test]
    fn provider_is_object_safe() {
        fn assert_dyn(_: &dyn UserAgentProvider) {}
        let p = FakeProvider { agents: vec![] };
        assert_dyn(&p);
    }
}
