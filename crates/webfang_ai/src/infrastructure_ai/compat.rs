//! Backward-compatibility layer for environment variable naming.
//!
//! Provides `read_ai_model_id` (the public function in this module) which checks
//! `WEBFANG_AI_MODEL_ID` first, then falls back to `AI_MODEL_ID`. The
//! fallback is traced at `DEBUG` level so operators can audit which var
//! was used.
//!
//! # Concurrency
//!
//! `read_ai_model_id` takes an injected environment accessor instead of
//! reading the process env directly. Production code passes the
//! `std_env_var` accessor (a thin wrapper over [`std::env::var`]) so behavior
//! is unchanged in production. Tests pass a snapshot closure backed by a
//! `HashMap`, removing the racy `std::env::set_var`/`remove_var` calls that
//! used to flake under parallel `cargo nextest` runs (#992).

/// Canonical env var for AI model selection.
const NEW_ENV_VAR: &str = "WEBFANG_AI_MODEL_ID";

/// Legacy env var (kept for backward compatibility).
const LEGACY_ENV_VAR: &str = "AI_MODEL_ID";

/// Default accessor: read from the process environment via [`std::env::var`].
///
/// Empty values are treated as unset, matching the prior semantics. Exposed
/// as `pub(crate)` so production callers (`cache_config::AiModel::from_env`)
/// can pass it explicitly to [`read_ai_model_id_with`]; tests inject their
/// own snapshot accessor instead.
pub(crate) fn std_env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Read the AI model ID using an injected environment accessor.
///
/// Checks `WEBFANG_AI_MODEL_ID` first; if unset, falls back to
/// `AI_MODEL_ID`. Emits a `tracing::debug!` log naming the variable that
/// was used (including when the legacy var is the source).
///
/// `env_var` should return `None` when the requested variable is unset OR
/// set to an empty string (the public callers treat an empty value as
/// absent). Production code passes `std_env_var`; tests pass a
/// snapshot closure over a `HashMap` so no real env mutation occurs.
///
/// Returns `None` if neither variable is set.
#[must_use]
pub fn read_ai_model_id_with(env_var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(val) = env_var(NEW_ENV_VAR) {
        tracing::debug!(
            env = NEW_ENV_VAR,
            "AI model resolved from canonical env var"
        );
        return Some(val);
    }

    if let Some(val) = env_var(LEGACY_ENV_VAR) {
        tracing::debug!(
            env = LEGACY_ENV_VAR,
            "AI model resolved from legacy env var (fallback)"
        );
        return Some(val);
    }

    None
}

/// Read the AI model ID from the process environment.
///
/// Convenience wrapper over [`read_ai_model_id_with`] using the default
/// `std_env_var` accessor.
#[must_use]
pub fn read_ai_model_id() -> Option<String> {
    read_ai_model_id_with(&std_env_var)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ========== read_ai_model_id_with() tests ==========
    //
    // These tests no longer mutate process env via set_var/remove_var.
    // Each test builds its own `HashMap` snapshot and passes a closure
    // that reads from it. This makes parallel `cargo nextest` runs
    // race-free by construction (#992).

    fn snapshot(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + Clone {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn test_read_ai_model_id_new_var_wins() {
        let env = snapshot(&[("WEBFANG_AI_MODEL_ID", "granite-97m")]);
        assert_eq!(read_ai_model_id_with(&env), Some("granite-97m".to_string()));
    }

    #[test]
    fn test_read_ai_model_id_legacy_fallback() {
        let env = snapshot(&[("AI_MODEL_ID", "granite-311m")]);
        assert_eq!(
            read_ai_model_id_with(&env),
            Some("granite-311m".to_string())
        );
    }

    #[test]
    fn test_read_ai_model_id_both_set_new_wins() {
        let env = snapshot(&[
            ("WEBFANG_AI_MODEL_ID", "granite-311m"),
            ("AI_MODEL_ID", "granite-97m"),
        ]);
        assert_eq!(
            read_ai_model_id_with(&env),
            Some("granite-311m".to_string())
        );
    }

    #[test]
    fn test_read_ai_model_id_neither_set() {
        let env = snapshot(&[]);
        assert_eq!(read_ai_model_id_with(&env), None);
    }

    #[test]
    fn test_read_ai_model_id_empty_new_var_falls_back() {
        // Accessor contract: None == absent (the default `std_env_var`
        // filters empty strings to None; tests model the same shape
        // explicitly). Empty canonical → legacy wins.
        let env = snapshot(&[("WEBFANG_AI_MODEL_ID", ""), ("AI_MODEL_ID", "granite-97m")]);
        // Wrap the raw snapshot so empty values map to None — this is
        // what std_env_var does in production.
        let filtered = move |key: &str| -> Option<String> {
            let v = env(key)?;
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        };
        assert_eq!(
            read_ai_model_id_with(&filtered),
            Some("granite-97m".to_string())
        );
    }
}
