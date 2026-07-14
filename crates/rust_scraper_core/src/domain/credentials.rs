//! Credentials domain module — Secrets protection with zeroize
//!
//! Phase 2: Secrets Protection for auditoria-resiliencia-rust_scraper
//! Provides zeroize-based secret types that:
//! - Automatically zeroize memory on drop
//! - DON'T leak in logs (Debug shows "[REDACTED]")
//! - Support optional expiry for credentials
//!
//! # Security
//!
//! **IMPORTANT**: Secret types (ApiKey, AccessToken, SensitiveString) do NOT implement
//! Serialize/Deserialize. Secrets should NEVER be serialized to disk or logs.
//!
//! # Usage
//!
//! ```
//! use rust_scraper::domain::credentials::{ApiKey, SecretCredential};
//! use chrono::Utc;
//!
//! let api_key = ApiKey::new("sk-actual-key-here".to_string());
//! let cred = SecretCredential::new("openai", api_key);
//! ```

use std::fmt::Debug;

use chrono::{DateTime, Utc};
use secrecy::ExposeSecret;

// Re-export for external use
pub use secrecy::SecretString;

/// Errors from credentials operations
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential expired")]
    Expired,

    #[error("credential not found: {0}")]
    NotFound(String),
}

/// API Key wrapper using zeroize for secure memory handling
///
/// # Security
///
/// - Memory is zeroized on drop (secrecy trait)
/// - Debug prints "[REDACTED]" instead of actual value
/// - Does NOT implement Serialize/Deserialize (secrets should never be serialized)
#[derive(Clone)]
pub struct ApiKey(SecretString);

impl ApiKey {
    /// Create a new API key from a string
    ///
    /// # Arguments
    ///
    /// * `key` - The actual API key string
    ///
    /// # Example
    ///
    /// ```
    /// use rust_scraper::domain::credentials::ApiKey;
    ///
    /// let key = ApiKey::new("sk-abc123".to_string());
    /// ```
    pub fn new(key: impl Into<String>) -> Self {
        Self(SecretString::from(key.into()))
    }

    /// Create from secret string (for parsing from config)
    #[allow(dead_code)]
    pub fn from_secret(secret: SecretString) -> Self {
        Self(secret)
    }

    /// Get reference to the secret (use sparingly)
    #[allow(dead_code)]
    pub fn as_secret(&self) -> &SecretString {
        &self.0
    }

    /// Get the secret as string (clones internally)
    /// WARNING: Only use when necessary
    #[allow(dead_code)]
    pub fn expose_secret(&self) -> String {
        self.0.expose_secret().clone()
    }
}

impl Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl PartialEq for ApiKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose_secret() == other.0.expose_secret()
    }
}

impl Default for ApiKey {
    fn default() -> Self {
        Self(SecretString::new(String::new()))
    }
}

/// Access Token wrapper with zeroize
///
/// # Security
///
/// - Memory is zeroized on drop
/// - Debug prints "[REDACTED]"
/// - Does NOT implement Serialize/Deserialize
#[derive(Clone)]
pub struct AccessToken(SecretString);

impl AccessToken {
    /// Create a new access token
    pub fn new(token: impl Into<String>) -> Self {
        Self(SecretString::from(token.into()))
    }

    /// Create from secret string
    #[allow(dead_code)]
    pub fn from_secret(secret: SecretString) -> Self {
        Self(secret)
    }

    /// Get reference to the secret
    #[allow(dead_code)]
    pub fn as_secret(&self) -> &SecretString {
        &self.0
    }

    /// Expose the token (use sparingly)
    #[allow(dead_code)]
    pub fn expose_secret(&self) -> String {
        self.0.expose_secret().clone()
    }
}

impl Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl PartialEq for AccessToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose_secret() == other.0.expose_secret()
    }
}

impl Default for AccessToken {
    fn default() -> Self {
        Self(SecretString::new(String::new()))
    }
}

/// Secret credential with optional expiry
///
/// Combines a provider name with a secret value and optional expiration time.
/// Supports expiry checking for credentials that have limited validity.
///
/// **Security**: The secret field is NOT serialized to prevent accidental leakage.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SecretCredential {
    /// Provider name (e.g., "openai", "anthropic", "github")
    pub provider: String,
    /// The actual secret (API key or token) - NOT serialized
    #[serde(skip)]
    pub secret: ApiKey,
    /// Optional expiry timestamp
    pub expires_at: Option<DateTime<Utc>>,
}

impl Debug for SecretCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretCredential")
            .field("provider", &self.provider)
            .field("secret", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl SecretCredential {
    /// Create a new credential without expiry
    pub fn new(provider: impl Into<String>, secret: ApiKey) -> Self {
        Self {
            provider: provider.into(),
            secret,
            expires_at: None,
        }
    }

    /// Create a credential with expiry
    pub fn with_expiry(
        provider: impl Into<String>,
        secret: ApiKey,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            provider: provider.into(),
            secret,
            expires_at,
        }
    }

    /// Check if this credential is expired
    ///
    /// # Returns
    ///
    /// `true` if expiry is set and has passed
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => Utc::now() > exp,
            None => false,
        }
    }

    /// Check and return error if expired
    ///
    /// # Errors
    ///
    /// Returns CredentialError::Expired if credential is expired
    pub fn check_expiry(&self) -> Result<(), CredentialError> {
        if self.is_expired() {
            Err(CredentialError::Expired)
        } else {
            Ok(())
        }
    }

    /// Get the secret value (exposes internally - use sparingly)
    pub fn secret(&self) -> &ApiKey {
        &self.secret
    }
}

/// Collection of credentials, keyed by provider name
///
/// Only stores provider names and metadata - actual secrets are kept in memory.
/// This type CAN be serialized (stores no actual secret values).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CredentialStore {
    /// Credentials keyed by provider name
    credentials: std::collections::HashMap<String, SecretCredential>,
}

impl Debug for CredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("credentials", &self.credentials.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl CredentialStore {
    /// Create a new empty credential store
    pub fn new() -> Self {
        Self {
            credentials: std::collections::HashMap::new(),
        }
    }

    /// Add a credential to the store
    pub fn add(&mut self, credential: SecretCredential) {
        let provider = credential.provider.clone();
        self.credentials.insert(provider, credential);
    }

    /// Get a credential by provider
    ///
    /// # Errors
    ///
    /// Returns CredentialError::NotFound if provider doesn't exist
    pub fn get(&self, provider: &str) -> Result<&SecretCredential, CredentialError> {
        self.credentials
            .get(provider)
            .ok_or_else(|| CredentialError::NotFound(provider.to_string()))
    }

    /// Get a credential, checking expiry
    ///
    /// # Errors
    ///
    /// Returns CredentialError::Expired if credential is expired
    /// Returns CredentialError::NotFound if provider doesn't exist
    pub fn get_valid(&self, provider: &str) -> Result<&SecretCredential, CredentialError> {
        let cred = self.get(provider)?;
        cred.check_expiry()?;
        Ok(cred)
    }

    /// Check if a provider exists in the store
    pub fn contains(&self, provider: &str) -> bool {
        self.credentials.contains_key(provider)
    }

    /// Remove a credential by provider
    pub fn remove(&mut self, provider: &str) -> Option<SecretCredential> {
        self.credentials.remove(provider)
    }

    /// Get number of credentials
    pub fn len(&self) -> usize {
        self.credentials.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }
}

/// Sensitive string wrapper with zeroize protection
///
/// # Security
///
/// - Memory is zeroized on drop
/// - Debug prints "[REDACTED]"
/// - Does NOT implement Serialize/Deserialize
#[derive(Clone)]
pub struct SensitiveString {
    data: SecretString,
}

impl SensitiveString {
    /// Wrap sensitive data
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: SecretString::from(data.into()),
        }
    }

    /// Get reference to the secret
    pub fn as_secret(&self) -> &SecretString {
        &self.data
    }

    /// Get the data as string reference
    pub fn as_str(&self) -> &str {
        self.data.expose_secret()
    }
}

impl Debug for SensitiveString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl PartialEq for SensitiveString {
    fn eq(&self, other: &Self) -> bool {
        self.data.expose_secret() == other.data.expose_secret()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_api_key_debug_shows_redacted() {
        let key = ApiKey::new("sk-secret123");
        let debug_str = format!("{:?}", key);
        assert_eq!(debug_str, "[REDACTED]");
    }

    #[test]
    fn test_api_key_expose() {
        let key = ApiKey::new("sk-secret123");
        assert_eq!(key.expose_secret(), "sk-secret123");
    }

    #[test]
    fn test_api_key_partial_eq() {
        let key1 = ApiKey::new("sk-secret123");
        let key2 = ApiKey::new("sk-secret123");
        let key3 = ApiKey::new("sk-other");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_access_token_debug_shows_redacted() {
        let token = AccessToken::new("ghp_token123");
        let debug_str = format!("{:?}", token);
        assert_eq!(debug_str, "[REDACTED]");
    }

    #[test]
    fn test_secret_credential_without_expiry() {
        let key = ApiKey::new("sk-abc123");
        let cred = SecretCredential::new("openai", key);

        assert_eq!(cred.provider, "openai");
        assert!(!cred.is_expired());
    }

    #[test]
    fn test_secret_credential_with_expiry_not_expired() {
        let key = ApiKey::new("sk-abc123");
        let expires = Utc::now() + Duration::hours(1);
        let cred = SecretCredential::with_expiry("openai", key, Some(expires));

        assert!(!cred.is_expired());
    }

    #[test]
    fn test_secret_credential_with_expiry_expired() {
        let key = ApiKey::new("sk-abc123");
        let expires = Utc::now() - Duration::hours(1);
        let cred = SecretCredential::with_expiry("openai", key, Some(expires));

        assert!(cred.is_expired());
        assert!(matches!(cred.check_expiry(), Err(CredentialError::Expired)));
    }

    #[test]
    fn test_credential_store_operations() {
        let mut store = CredentialStore::new();

        let key = ApiKey::new("sk-test");
        let cred = SecretCredential::new("test-provider", key);
        store.add(cred.clone());

        assert!(store.contains("test-provider"));
        assert_eq!(store.len(), 1);

        let retrieved = store.get("test-provider").unwrap();
        assert_eq!(retrieved.provider, "test-provider");
    }

    #[test]
    fn test_credential_store_not_found() {
        let store = CredentialStore::new();
        let result = store.get("nonexistent");
        assert!(matches!(result, Err(CredentialError::NotFound(_))));
    }

    #[test]
    fn test_sensitive_string_debug() {
        let sensitive = SensitiveString::new("secret-data".to_string());
        assert_eq!(format!("{:?}", sensitive), "[REDACTED]");
    }

    #[test]
    fn test_credential_store_serialize_no_secrets() {
        // CredentialStore should serialize without actual secrets
        let mut store = CredentialStore::new();
        let key = ApiKey::new("sk-secret");
        let cred = SecretCredential::with_expiry(
            "test-provider",
            key,
            Some(Utc::now() + Duration::hours(1)),
        );
        store.add(cred);

        // Serialize - should work (skips the secret field)
        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains("test-provider"));
        assert!(!json.contains("sk-secret")); // Secret should NOT be in JSON
    }
}
