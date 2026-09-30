//! Credentials domain module — Secrets protection with zeroize
//!
//! Phase 2: Secrets Protection for auditoria-resiliencia-webfang
//! Provides zeroize-based secret types that:
//! - Automatically zeroize memory on drop
//! - DON'T leak in logs (Debug shows `[REDACTED]`)
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
//! use webfang_core::domain::credentials::{ApiKey, SecretCredential};
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

/// Constant-time equality for two byte strings, for comparing SECRETS.
///
/// #1615 (F9 / H-4, `AV-6`): the ordinary `==` on `[u8]`/`&str` short-circuits
/// on the first differing byte, so its running time is a (noisy) function of
/// the shared prefix length. Over a network that is a marginal signal — `AV-6`
/// says so explicitly — but it is one function, and closing the whole class of
/// `==`-on-credentials call sites is cheaper than auditing them one at a time.
///
/// # What this does and does not buy you
///
/// - **Does**: the comparison visits every byte of both operands, always, and
///   folds each difference into an accumulator instead of branching. There is
///   no data-dependent early exit, so a wrong guess costs the same as a right
///   one at the instruction level.
/// - **Does not**: make the surrounding system timing-proof. The caller's
///   allocation, transport, and HTTP framing still dominate; a remote attacker
///   also has to fight TCP coalescing and jitter. And the length of the two
///   operands is still visible through the early `len` check below — that is
///   inherent to comparing strings of different lengths without hashing, and
///   is a separate, weaker leak than a per-byte prefix oracle.
///
/// The point is the *class*: any future secret comparison routed through this
/// helper inherits the property, so nobody has to re-derive the argument.
///
/// Comparison is over bytes, so two strings that differ only in multi-byte
/// UTF-8 encoding are correctly unequal.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // The length check is the one early exit. It leaks only "are the two
    // secrets the same length", which every fixed-format bearer token already
    // reveals by construction — see the note above.
    if a.len() != b.len() {
        return false;
    }
    // Branch-free accumulation: `black_box` keeps the optimizer from proving
    // the loop to a `memcmp` and re-introducing the early exit we just removed.
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// Errors from credentials operations
#[allow(dead_code)] // pub(crate) API for credential store — used in tests
#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialError {
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
/// - Debug prints `[REDACTED]` instead of actual value
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
    /// use webfang_core::domain::credentials::ApiKey;
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
        // #1615 F9: secret comparison goes through the shared constant-time
        // helper, never `==`.
        constant_time_eq(
            self.0.expose_secret().as_bytes(),
            other.0.expose_secret().as_bytes(),
        )
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
/// - Debug prints `[REDACTED]`
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
        // #1615 F9: same class as `ApiKey` — see `constant_time_eq`.
        constant_time_eq(
            self.0.expose_secret().as_bytes(),
            other.0.expose_secret().as_bytes(),
        )
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
    #[allow(dead_code)] // pub(crate) API for credential store — used in tests
    pub(crate) fn check_expiry(&self) -> Result<(), CredentialError> {
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
    #[allow(dead_code)] // pub(crate) API for credential store — used in tests
    pub(crate) fn get(&self, provider: &str) -> Result<&SecretCredential, CredentialError> {
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
    #[allow(dead_code)] // pub(crate) API for credential store — used in tests
    pub(crate) fn get_valid(&self, provider: &str) -> Result<&SecretCredential, CredentialError> {
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
/// - Debug prints `[REDACTED]`
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
        // #1615 F9: same class — see `constant_time_eq`.
        constant_time_eq(
            self.data.expose_secret().as_bytes(),
            other.data.expose_secret().as_bytes(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_api_key_debug_shows_redacted() {
        let key = ApiKey::new("sk-secret123");
        let debug_str = format!("{key:?}");
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
        let debug_str = format!("{token:?}");
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
        assert_eq!(format!("{sensitive:?}"), "[REDACTED]");
    }

    // #1615 F9 / H-4. The behavioural contract of `constant_time_eq`: it must
    // agree with `==` on every case, because a secret comparison that returns
    // a DIFFERENT answer than `==` would be an authentication bypass, not a
    // hardening. The timing property is asserted by construction (no early
    // exit) and is not something a unit test can measure meaningfully over a
    // CPU's branch predictors.
    #[test]
    fn constant_time_eq_agrees_with_byte_equality() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("a", "a"),
            ("a", "b"),
            ("ab", "ab"),
            ("ab", "ba"),
            ("abc", "ab"),
            ("ab", "abc"),
            ("", "a"),
            ("Bearer sk-secret", "Bearer sk-secret"),
            ("Bearer sk-secret", "Bearer sk-secrez"),
        ];
        for (a, b) in cases {
            assert_eq!(
                constant_time_eq(a.as_bytes(), b.as_bytes()),
                a.as_bytes() == b.as_bytes(),
                "constant_time_eq disagreed with == for {a:?} vs {b:?}"
            );
        }
    }

    /// The multi-byte-UTF-8 case is the one place a byte-wise comparison could
    /// plausibly have been implemented wrong (over `char`s instead of `bytes`):
    /// `é` is two bytes and must still make the strings unequal here.
    #[test]
    fn constant_time_eq_compares_bytes_not_characters() {
        assert!(constant_time_eq("café".as_bytes(), "café".as_bytes()));
        assert!(!constant_time_eq("café".as_bytes(), "cafe".as_bytes()));
    }

    /// The class closure: every secret type in this module now routes its
    /// `PartialEq` through the helper, so a fourth secret type cannot silently
    /// reintroduce `==`.
    #[test]
    fn every_secret_type_compares_without_short_circuiting() {
        let key = ApiKey::new("sk-same");
        let other_key = ApiKey::new("sk-same");
        assert!(key == other_key);
        assert!(key != ApiKey::new("sk-differs"));

        let token = AccessToken::new("ghp-same");
        assert!(token == AccessToken::new("ghp-same"));
        assert!(token != AccessToken::new("ghp-differs"));

        let sensitive = SensitiveString::new("data-same");
        assert!(sensitive == SensitiveString::new("data-same"));
        assert!(sensitive != SensitiveString::new("data-differs"));
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
