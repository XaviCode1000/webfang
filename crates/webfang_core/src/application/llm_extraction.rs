//! LLM structured extraction — configuration, SSRF gate, and application
//! orchestration (#789).

use crate::domain::credentials::ApiKey;
use crate::error::{Result, ScraperError};
use url::Url;

/// Configuration for one LLM extraction run.
///
/// `Debug` is safe: [`ApiKey`] prints `[REDACTED]` — the key value never
/// appears in logs or traces.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// OpenAI-compatible endpoint base (`POST {base_url}/chat/completions`).
    pub base_url: Url,
    /// Provider API key — `Authorization: Bearer` only, never logged.
    pub api_key: ApiKey,
    /// Per-completion output token budget; sub-chunk char budget is
    /// `max_tokens * 8` (≈8 chars/token estimate).
    pub max_tokens: usize,
}

impl LlmConfig {
    /// Build the config from the `LLM_API_KEY` environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] when the base URL is malformed or
    /// `LLM_API_KEY` is missing/empty.
    pub fn from_env(base_url: &str, max_tokens: usize) -> Result<Self> {
        let base_url = Url::parse(base_url)
            .map_err(|e| ScraperError::Config(format!("URL base del LLM inválida: {e}")))?;
        match std::env::var("LLM_API_KEY") {
            Ok(key) if !key.is_empty() => Ok(Self {
                base_url,
                api_key: ApiKey::new(key),
                max_tokens,
            }),
            _ => Err(ScraperError::Config(
                "LLM_API_KEY no está definida: configure la clave del proveedor LLM".into(),
            )),
        }
    }
}

/// SSRF gate for the LLM base URL (#703): scheme allow-list http/https plus
/// literal-host rejection via [`crate::infrastructure::ssrf::is_forbidden_ip`]
/// (loopback / private / link-local / CGNAT / IPv6-ULA). A blocked URL never
/// produces a request.
///
/// Tests against wiremock (127.0.0.1) may set `WEBFANG_DISABLE_SSRF=1`.
/// Hostname-level DNS validation lives at the MCP entry point (slice B,
/// `validate_url_no_ssrf` precedent) alongside the client-side redirect guard.
///
/// # Errors
///
/// Returns [`ScraperError::Config`] when the scheme is not http(s) or the
/// host is a forbidden literal IP.
pub fn ssrf_gate(url: &Url) -> Result<()> {
    if std::env::var("WEBFANG_DISABLE_SSRF").is_ok() {
        return Ok(());
    }
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ScraperError::Config(format!(
            "esquema '{scheme}' no permitido para el LLM (solo http/https)"
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ScraperError::Config("el URL del LLM no tiene host".to_string()))?;
    if crate::infrastructure::ssrf::is_forbidden_literal_host(host) {
        return Err(ScraperError::Config(format!(
            "URL del LLM bloqueada por SSRF: el host '{host}' pertenece a una red interna (#703)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_gate_blocks_forbidden_internal_ips() {
        for host in [
            "127.0.0.1",
            "169.254.169.254",
            "10.0.0.1",
            "100.64.1.1",
            "[fc00::1]",
        ] {
            let url = Url::parse(&format!("http://{host}/v1")).expect("literal parses");
            let err = ssrf_gate(&url).expect_err("forbidden host must be blocked");
            assert!(
                matches!(err, ScraperError::Config(_)),
                "host {host}: {err:?}"
            );
        }
    }

    #[test]
    fn ssrf_gate_allows_public_ip() {
        let url = Url::parse("https://8.8.8.8/v1").expect("literal parses");
        ssrf_gate(&url).expect("public host must pass");
    }

    #[test]
    fn ssrf_gate_rejects_non_http_scheme() {
        let url = Url::parse("ftp://8.8.8.8/v1").expect("parses");
        assert!(matches!(
            ssrf_gate(&url).expect_err("ftp must fail"),
            ScraperError::Config(_)
        ));
    }

    #[test]
    fn ssrf_gate_bypass_env_for_tests() {
        std::env::set_var("WEBFANG_DISABLE_SSRF", "1");
        let url = Url::parse("http://127.0.0.1:9/v1").expect("parses");
        let ok = ssrf_gate(&url).is_ok();
        std::env::remove_var("WEBFANG_DISABLE_SSRF");
        assert!(ok);
    }

    #[test]
    fn from_env_missing_key_is_config_error() {
        std::env::remove_var("LLM_API_KEY");
        let err = LlmConfig::from_env("https://8.8.8.8/v1", 100).expect_err("missing key fails");
        assert!(matches!(err, ScraperError::Config(_)));
    }

    #[test]
    fn from_env_redacts_key_in_debug() {
        std::env::set_var("LLM_API_KEY", "sk-super-secret");
        let cfg = LlmConfig::from_env("https://8.8.8.8/v1", 100).expect("key present");
        std::env::remove_var("LLM_API_KEY");
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("[REDACTED]"), "debug must redact: {dbg}");
        assert!(
            !dbg.contains("sk-super-secret"),
            "debug leaked secret: {dbg}"
        );
    }
}
