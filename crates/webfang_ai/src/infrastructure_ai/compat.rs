//! Backward-compatibility layer for environment variable naming.
//!
//! Provides [`read_ai_model_id()`] which checks `WEBFANG_AI_MODEL_ID` first,
//! then falls back to `AI_MODEL_ID`. The fallback is traced at `DEBUG` level
//! so operators can audit which var was used.

/// Canonical env var for AI model selection.
const NEW_ENV_VAR: &str = "WEBFANG_AI_MODEL_ID";

/// Legacy env var (kept for backward compatibility).
const LEGACY_ENV_VAR: &str = "AI_MODEL_ID";

/// Read the AI model ID from the environment.
///
/// Checks `WEBFANG_AI_MODEL_ID` first; if unset, falls back to
/// `AI_MODEL_ID`. Emits a `tracing::debug!` log naming the variable that
/// was used (including when the legacy var is the source).
///
/// Returns `None` if neither variable is set.
#[must_use]
pub fn read_ai_model_id() -> Option<String> {
    if let Ok(val) = std::env::var(NEW_ENV_VAR) {
        if !val.is_empty() {
            tracing::debug!(
                env = NEW_ENV_VAR,
                "AI model resolved from canonical env var"
            );
            return Some(val);
        }
    }

    // Legacy fallback
    if let Ok(val) = std::env::var(LEGACY_ENV_VAR) {
        if !val.is_empty() {
            tracing::debug!(
                env = LEGACY_ENV_VAR,
                "AI model resolved from legacy env var (fallback)"
            );
            return Some(val);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== read_ai_model_id() tests ==========

    #[test]
    fn test_read_ai_model_id_new_var_wins() {
        // WEBFANG_AI_MODEL_ID takes precedence
        let guard = EnvGuard::set("WEBFANG_AI_MODEL_ID", "granite-97m");
        let result = read_ai_model_id();
        drop(guard);
        assert_eq!(result, Some("granite-97m".to_string()));
    }

    #[test]
    fn test_read_ai_model_id_legacy_fallback() {
        // Only AI_MODEL_ID set → fallback fires
        let guard = EnvGuard::set("AI_MODEL_ID", "granite-311m");
        let result = read_ai_model_id();
        drop(guard);
        assert_eq!(result, Some("granite-311m".to_string()));
    }

    #[test]
    fn test_read_ai_model_id_both_set_new_wins() {
        // Both set → new var wins, no fallback
        let _guard_new = EnvGuard::set("WEBFANG_AI_MODEL_ID", "granite-311m");
        let _guard_old = EnvGuard::set("AI_MODEL_ID", "granite-97m");
        let result = read_ai_model_id();
        assert_eq!(result, Some("granite-311m".to_string()));
    }

    #[test]
    fn test_read_ai_model_id_neither_set() {
        // Neither var set → None (caller applies silent default)
        let _guard_new = EnvGuard::unset("WEBFANG_AI_MODEL_ID");
        let _guard_old = EnvGuard::unset("AI_MODEL_ID");
        let result = read_ai_model_id();
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_ai_model_id_empty_new_var_falls_back() {
        // New var is empty string → treat as unset → fallback fires
        let _guard_new = EnvGuard::set("WEBFANG_AI_MODEL_ID", "");
        let _guard_old = EnvGuard::set("AI_MODEL_ID", "granite-97m");
        let result = read_ai_model_id();
        assert_eq!(result, Some("granite-97m".to_string()));
    }

    /// RAII guard that temporarily sets (or unsets) a process env var.
    struct EnvGuard {
        var: String,
        prior: Option<String>,
    }

    impl EnvGuard {
        fn set(var: &str, val: &str) -> Self {
            let prior = std::env::var(var).ok();
            std::env::set_var(var, val);
            Self {
                var: var.to_string(),
                prior,
            }
        }

        fn unset(var: &str) -> Self {
            let prior = std::env::var(var).ok();
            std::env::remove_var(var);
            Self {
                var: var.to_string(),
                prior,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prior {
                Some(ref v) => std::env::set_var(&self.var, v),
                None => std::env::remove_var(&self.var),
            }
        }
    }
}
