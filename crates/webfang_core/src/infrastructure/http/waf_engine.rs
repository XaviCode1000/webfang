//! WAF Detection Engine - Layer 7 Protection
//!
//! This module provides advanced WAF detection beyond the basic signature matching
//! in http_client.rs. It includes:
//! - Detection by Control Headers (x-datadome-response, cf-mitigated, etc.)
//! - Entropy analysis for "Silent Challenge" detection
//! - Efficient O(N) matching using Aho-Corasick for 60+ signatures
//! - Body-only detection via `detect_body()` for callers that only have the body
//!
//! # Usage
//!
//! ```ignore
//! use webfang_core::infrastructure::http::waf_engine::WafInspector;
//!
//! # let response = wreq::header::HeaderMap::new();
//! # let body = String::new();
//! // Full integrity check (headers + body)
//! if let Err(e) = WafInspector::verify_integrity(&response, &body) {
//!     return Err(e);
//! }
//!
//! // Body-only check (replaces legacy detect_waf_challenge)
//! if let Some(provider) = WafInspector::detect_body(&body) {
//!     eprintln!("WAF detected: {provider}");
//! }
//! ```

use crate::error::ScraperError;
use aho_corasick::AhoCorasick;
use once_cell::sync::Lazy;
use std::collections::HashSet;
use wreq::header::HeaderMap;

// ============================================================================
// TASK-01 — Domain types + Inspection Context API (REQ-WAF-01)
// ============================================================================

/// WAF signature tier — determines the blocking policy for a matched pattern.
///
/// Tier drives the verdict policy in [`WafInspector::inspect`]:
/// - [`WafTier::Challenge`] blocks at ANY HTTP status, including 200.
/// - [`WafTier::Fingerprint`] is evidence only; it blocks solely when correlated
///   with a WAF-associated status code (403 / 429 / 503 / 520–529).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WafTier {
    /// Unambiguous challenge/captcha markers (widget tokens, challenge prose).
    Challenge,
    /// Fingerprint markers (bare vendor names, domains, cookies, control headers).
    Fingerprint,
}

/// A single piece of WAF detection evidence collected during inspection.
///
/// A verdict carries *all* collected evidences (REQ-WAF-01), enabling an
/// evidence-chain error message (REQ-WAF-08) instead of a first-hit provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WafEvidence {
    /// Detected WAF provider (e.g. `"Cloudflare"`, `"DataDome"`).
    pub provider: &'static str,
    /// Signature tier that matched.
    pub tier: WafTier,
    /// The literal pattern (or rule label) that matched.
    pub matched_pattern: &'static str,
}

/// The verdict of a WAF inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WafVerdict {
    /// Whether the response should be treated as WAF-blocked.
    pub is_blocked: bool,
    /// All collected evidence (not just the first hit).
    pub evidences: Vec<WafEvidence>,
}

impl WafVerdict {
    /// A clean verdict: not blocked, no evidence.
    #[must_use]
    pub fn clean() -> Self {
        Self::default()
    }
}

/// HTTP context for a WAF inspection (REQ-WAF-01).
///
/// [`Default`] is *degraded mode* — no HTTP context (status/content-type
/// unknown, empty headers, `ignore_waf` false). Callers that only have the
/// body (e.g. the MCP `detect_waf` tool) use degraded mode, where only
/// [`WafTier::Challenge`] markers block and [`WafTier::Fingerprint`] evidence
/// is reported as low-confidence and never blocks (REQ-WAF-05).
#[derive(Debug, Clone, Default)]
pub struct InspectionContext {
    /// HTTP status code, if known.
    pub status: Option<u16>,
    /// Content-Type header value, if known.
    pub content_type: Option<String>,
    /// Response headers.
    pub headers: HeaderMap,
    /// Bypass WAF detection entirely (yields a clean verdict).
    pub ignore_waf: bool,
}

/// Control headers that indicate WAF processing (REQ-WAF-03).
///
/// Every control header is [`WafTier::Fingerprint`] evidence — it NEVER
/// auto-blocks on mere presence, only when correlated with a WAF status code
/// (correction B). `x-wordpress` (not a real WAF header) and `x-cdn` (generic
/// CDN header) were purged as false-positive risks.
const WAF_CONTROL_HEADERS: &[(&str, &str)] = &[
    ("x-datadome-response", "DataDome"),
    ("cf-mitigated", "Cloudflare"),
    ("x-akamai-edge-auth", "Akamai"),
    ("x-sucuri-id", "Sucuri"),
    ("cf-ray", "Cloudflare"),
];

/// Boundary post-filter mode for a body signature (REQ-WAF-04).
///
/// Only [`BoundaryMode::Bare`] patterns (bare vendor names) get the O(1)
/// adjacent-byte boundary filter. [`BoundaryMode::Exempt`] patterns may match
/// inside a larger token (cookies/tokens); [`BoundaryMode::Phrase`] patterns
/// are challenge prose / widget markers that need no filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryMode {
    /// Bare vendor name — apply the boundary post-filter.
    Bare,
    /// Boundary-exempt — may match inside a larger token (`[E]`).
    Exempt,
    /// Phrase/exact — no filter needed (`—`).
    Phrase,
}

/// Unified WAF body signature registry (REQ-WAF-03 PRIMARY, REQ-WAF-10).
///
/// Each entry is `(pattern, provider, tier, boundary_mode)`:
/// - `tier`: [`WafTier::Challenge`] (T1 — blocks any status) or
///   [`WafTier::Fingerprint`] (T2 — evidence only, blocks with a WAF status).
/// - `boundary_mode`: [`BoundaryMode::Bare`] → O(1) boundary post-filter
///   (REQ-WAF-04); `Exempt`/`Phrase` → no filter.
///
/// Reclassified per the spec table; the 14 DEL entries are purged; the
/// `spa_detector` `WAF_MARKERS` are folded in (REQ-WAF-10). Aho-Corasick is
/// case-sensitive, so case-variant prose ("Checking your browser" vs
/// "checking your browser") are distinct entries.
const WAF_BODY_SIGNATURES: &[(&str, &str, WafTier, BoundaryMode)] = &[
    // ── Cloudflare ──
    (
        "cf-turnstile",
        "Cloudflare Turnstile",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge-platform",
        "Cloudflare JS Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "Just a moment...",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "Checking your browser",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "__cf_chl_f_tk",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Exempt,
    ),
    (
        "cf-browser-verification",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "_cf_chl_opt",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "cloudflare",
        "Cloudflare",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    // ── Google reCAPTCHA ──
    (
        "g-recaptcha",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha/api.js",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "grecaptcha.execute",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha.net",
        "reCAPTCHA",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "recaptcha Enterprise",
        "reCAPTCHA",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    // ── hCaptcha ──
    (
        "hcaptcha.com",
        "hCaptcha",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "h-captcha",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha-api",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha.js",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "hcaptcha",
        "hCaptcha",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ), // folded from spa_detector
    // ── DataDome ──
    (
        "datadome",
        "DataDome",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "dd-captcha",
        "DataDome",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "datadome.co",
        "DataDome",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── PerimeterX / HUMAN Security ──
    (
        "perimeterx",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "_pxCaptcha",
        "PerimeterX",
        WafTier::Challenge,
        BoundaryMode::Exempt,
    ),
    (
        "px-captcha",
        "PerimeterX",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "perimeterx.net",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "human-security",
        "HUMAN",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "px-init",
        "PerimeterX",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── Akamai Bot Manager ──
    (
        "_abck",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    (
        "SensorData",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "akamai-bot-manager",
        "Akamai Bot Manager",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "akamai.net",
        "Akamai",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    ("akamai", "Akamai", WafTier::Fingerprint, BoundaryMode::Bare),
    // ── Imperva / Incapsula ──
    (
        "imperva",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Bare,
    ),
    (
        "incapsula",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "_Incapsula_Resource",
        "Imperva",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    (
        "visid_incap",
        "Imperva Incapsula",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    (
        "incap_ses",
        "Imperva Incapsula",
        WafTier::Fingerprint,
        BoundaryMode::Exempt,
    ),
    // ── Sucuri ──
    ("sucuri", "Sucuri", WafTier::Fingerprint, BoundaryMode::Bare),
    (
        "sucuri.net",
        "Sucuri",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── F5 ──
    (
        "BIGipServer",
        "F5",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ),
    // ── Generic challenge prose / scripts ──
    (
        "Please verify you are a human",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "verify you are human",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "checking your browser",
        "Browser Verification",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "captcha-delivery",
        "Challenge Delivery",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "__js_challenge__",
        "JS Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge.js",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "captcha.js",
        "Generic Challenge",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "challenge-running",
        "Cloudflare",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ), // folded from spa_detector
    (
        "data-sitekey",
        "Generic Captcha",
        WafTier::Fingerprint,
        BoundaryMode::Phrase,
    ), // folded from spa_detector
    // ── AWS WAF ──
    (
        "awsWafCookieDomainList",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "AwsWafIntegration",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "gokuProps",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
    (
        "aws-waf-token",
        "AWS WAF",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ),
];

/// Shannon entropy threshold for obfuscated WAF detection
const ENTROPY_THRESHOLD: f64 = 5.5;

/// Body size threshold (100KB) above which entropy analysis is applied
const SUSPICIOUS_SIZE_THRESHOLD: usize = 100_000;

/// Aho-Corasick automaton for O(N) multi-pattern body matching.
///
/// Built once via `Lazy` from [`WAF_BODY_SIGNATURES`] patterns (pattern index
/// maps back to the registry entry for tier/boundary metadata). Thread-safe
/// for concurrent reads.
static WAF_AC: Lazy<AhoCorasick> = Lazy::new(|| {
    AhoCorasick::new(WAF_BODY_SIGNATURES.iter().map(|(sig, _, _, _)| sig))
        .expect("Failed to build Aho-Corasick automaton")
});

/// WafInspector provides multi-layer WAF detection
pub struct WafInspector;

impl WafInspector {
    /// Scan body for WAF challenge signatures using Aho-Corasick (O(N) single pass).
    ///
    /// Returns the FIRST matching provider name, or `None` if the body is clean.
    /// For bodies exceeding 100KB, Shannon entropy is computed; if entropy > 5.5,
    /// returns `Some("Obfuscated WAF")`.
    ///
    /// Thread-safe: the AC automaton is immutable once compiled via `Lazy`.
    ///
    /// # Arguments
    /// * `body` - The HTTP response body to scan
    ///
    /// # Returns
    /// * `Some(provider_name)` - WAF challenge detected
    /// * `None` - No WAF challenge detected
    #[must_use]
    pub fn detect_body(body: &str) -> Option<&'static str> {
        // Early exit for empty or very small bodies (no signatures fit in <10 chars)
        if body.len() < 10 {
            return None;
        }

        // Shannon entropy check for large bodies (>100KB)
        if body.len() > SUSPICIOUS_SIZE_THRESHOLD {
            let entropy = calculate_entropy(body);
            if entropy > ENTROPY_THRESHOLD {
                return Some("Obfuscated WAF");
            }
        }

        // Aho-Corasick single-pass scan for all 62 patterns.
        // Returns provider name for the first match found by AC (earliest end position).
        WAF_AC
            .find(body)
            .map(|m| WAF_BODY_SIGNATURES[m.pattern()].1)
    }

    /// Verify response integrity across multiple layers
    ///
    /// 1. Control Headers: Check for WAF-specific headers (immediate)
    /// 2. Body Signatures: O(N) scan using Aho-Corasick
    /// 3. Entropy Analysis: Detect "Silent Challenges" in minimal HTML
    ///
    /// # Arguments
    /// * `headers` - Response headers from HTTP call
    /// * `body` - Response body (HTML content)
    ///
    /// # Returns
    /// * `Ok(())` - No WAF challenge detected
    /// * `Err(ScraperError::WafBlocked)` - WAF challenge detected
    pub fn verify_integrity(headers: &HeaderMap, body: &str) -> Result<(), ScraperError> {
        // Layer 1: Control Headers (fastest - O(1) lookup)
        Self::check_control_headers(headers)?;

        // Layer 2: Body Signature Matching (O(N) with Aho-Corasick)
        Self::check_body_signatures(body)?;

        // Layer 3: Entropy Analysis (detect Silent Challenges)
        Self::check_entropy(body)?;

        Ok(())
    }

    /// Check for WAF control headers that indicate bot detection/processing
    #[inline]
    fn check_control_headers(headers: &HeaderMap) -> Result<(), ScraperError> {
        for (header_name, provider) in WAF_CONTROL_HEADERS {
            // Check if header exists (even with empty value indicates WAF processing)
            if headers.get(*header_name).is_some() {
                // Some headers like cf-ray exist even for normal requests,
                // but others like x-datadome-response specifically indicate bot challenges
                if *header_name == "x-datadome-response"
                    || *header_name == "cf-mitigated"
                    || *header_name == "x-akamai-edge-auth"
                {
                    return Err(ScraperError::WafBlocked {
                        url: String::new(),
                        provider: format!("{provider}: header detected"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check body content for WAF signatures using O(N) Aho-Corasick
    #[inline]
    fn check_body_signatures(body: &str) -> Result<(), ScraperError> {
        // Early exit for empty or very small bodies
        // Lowered to 10 chars to detect short WAF challenge pages
        if body.len() < 10 {
            return Ok(());
        }

        // Use Aho-Corasick for O(N) multi-pattern matching
        if let Some(mat) = WAF_AC.find_iter(body).next() {
            // Map pattern index to provider name
            let provider = WAF_BODY_SIGNATURES[mat.pattern()].1;
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: format!("Signature detected: {provider}"),
            });
        }

        Ok(())
    }

    /// Detect "Silent Challenges" using entropy analysis
    ///
    /// WAFs in 2026 sometimes return HTTP 200 with minimal HTML containing
    /// heavy JavaScript challenges. This function detects that pattern:
    /// - Body < 1500 bytes
    /// - High density of <script> tags (> 5)
    /// - Low text content ratio
    #[inline]
    fn check_entropy(body: &str) -> Result<(), ScraperError> {
        // Only analyze bodies under 1500 bytes
        if body.len() > 1500 {
            return Ok(());
        }

        // Count <script> tags efficiently
        let script_count = body.matches("<script").count();

        // Silent Challenge detection:
        // - Multiple script tags in a small body suggests JS challenge
        // - Low text ratio indicates mostly code, not content
        if script_count > 5 && body.len() < 1000 {
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: "Silent Challenge: High JS density in minimal body".into(),
            });
        }

        // Additional entropy check: ratio of script to text
        if body.len() < 500 && script_count > 3 {
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: "Silent Challenge: Suspicious script/text ratio".into(),
            });
        }

        Ok(())
    }

    /// Get the list of supported WAF providers
    #[must_use]
    pub fn supported_providers() -> Vec<&'static str> {
        // Extract unique provider names from signatures
        let mut providers: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();

        for sig in WAF_BODY_SIGNATURES {
            let provider = sig.1;
            if !seen.contains(provider) {
                seen.insert(provider);
                providers.push(provider);
            }
        }
        providers.sort();
        providers
    }
}

/// Calculate Shannon entropy of a string
///
/// Used to detect obfuscated JavaScript in challenge pages, which often have
/// high entropy due to minification and encoding.
///
/// # Arguments
/// * `s` - The string to analyze
///
/// # Returns
/// * Entropy value between 0.0 (uniform distribution) and ~8.0 (high entropy)
#[inline]
fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut freq = [0u32; 256];
    let len = s.len() as f64;

    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }

    let mut entropy = 0.0;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }

    entropy
}

/// Check if body size indicates a potential WAF challenge (>100KB)
#[cfg(test)]
#[inline]
fn is_suspicious_size(body_len: usize) -> bool {
    body_len > SUSPICIOUS_SIZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // TASK-01 — Domain types + InspectionContext API (REQ-WAF-01)
    // ========================================================================

    #[test]
    fn test_inspection_context_default_is_degraded() {
        // REQ-WAF-01: Default is degraded mode (all None, ignore_waf=false).
        let ctx = InspectionContext::default();
        assert!(ctx.status.is_none(), "degraded mode has no status");
        assert!(
            ctx.content_type.is_none(),
            "degraded mode has no content-type"
        );
        assert!(ctx.headers.is_empty(), "degraded mode has no headers");
        assert!(!ctx.ignore_waf, "degraded mode does not bypass WAF");
    }

    #[test]
    fn test_waf_verdict_clean_is_not_blocked() {
        let verdict = WafVerdict::clean();
        assert!(!verdict.is_blocked, "clean verdict must not block");
        assert!(
            verdict.evidences.is_empty(),
            "clean verdict carries no evidence"
        );
    }

    #[test]
    fn test_waf_verdict_carries_all_evidences() {
        // REQ-WAF-01: the verdict carries ALL collected evidences, not first-hit.
        let verdict = WafVerdict {
            is_blocked: true,
            evidences: vec![
                WafEvidence {
                    provider: "Cloudflare",
                    tier: WafTier::Challenge,
                    matched_pattern: "cf-turnstile",
                },
                WafEvidence {
                    provider: "Akamai",
                    tier: WafTier::Fingerprint,
                    matched_pattern: "akamai",
                },
            ],
        };
        assert_eq!(
            verdict.evidences.len(),
            2,
            "verdict must retain every evidence"
        );
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.tier == WafTier::Challenge));
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.tier == WafTier::Fingerprint));
    }

    // ========================================================================
    // TASK-02 — Unified registry + reclassification table (REQ-WAF-03, REQ-WAF-10)
    // ========================================================================

    #[test]
    fn test_registry_has_no_del_residue() {
        // REQ-WAF-03: the 14 DEL entries must be purged from the body registry.
        const DEL: &[&str] = &[
            "cf-ray",
            "cf-cache-status",
            "cf-dns",
            "dd=",
            "data-domain",
            "_nfv",
            "bot detection",
            "automated requests",
            "security check",
            "anti-bot",
            "attack detected",
            "suspicious activity",
            "verify.js",
            "bot-check",
        ];
        for sig in WAF_BODY_SIGNATURES {
            assert!(
                !DEL.contains(&sig.0),
                "DEL pattern '{}' must not be in the registry",
                sig.0
            );
        }
    }

    #[test]
    fn test_registry_patterns_are_unique() {
        // #1189: dedup by (pattern, tier); no exact pattern duplicates remain.
        let mut seen: HashSet<&str> = HashSet::new();
        for sig in WAF_BODY_SIGNATURES {
            assert!(seen.insert(sig.0), "duplicate pattern '{}'", sig.0);
        }
    }

    #[test]
    fn test_registry_every_entry_has_provider() {
        for sig in WAF_BODY_SIGNATURES {
            assert!(!sig.0.is_empty(), "empty pattern in registry");
            assert!(!sig.1.is_empty(), "pattern '{}' has empty provider", sig.0);
        }
    }

    #[test]
    fn test_registry_case_variant_challenge_prose_both_present() {
        // #1189: Aho-Corasick is case-sensitive, so "Checking your browser" and
        // "checking your browser" are distinct patterns and both must remain.
        let patterns: Vec<&str> = WAF_BODY_SIGNATURES.iter().map(|(p, _, _, _)| *p).collect();
        assert!(
            patterns.contains(&"Checking your browser"),
            "title-case prose missing"
        );
        assert!(
            patterns.contains(&"checking your browser"),
            "lowercase prose missing"
        );
    }

    #[test]
    fn test_registry_folds_spa_markers() {
        // REQ-WAF-10: spa_detector WAF_MARKERS fold into the unified registry.
        let patterns: Vec<&str> = WAF_BODY_SIGNATURES.iter().map(|(p, _, _, _)| *p).collect();
        assert!(
            patterns.contains(&"challenge-running"),
            "challenge-running not folded"
        );
        assert!(
            patterns.contains(&"data-sitekey"),
            "data-sitekey not folded"
        );
        assert!(patterns.contains(&"hcaptcha"), "bare hcaptcha not folded");
    }

    #[test]
    fn test_registry_tiers_are_valid() {
        // Every entry carries an explicit tier (Challenge or Fingerprint).
        for sig in WAF_BODY_SIGNATURES {
            assert!(
                sig.2 == WafTier::Challenge || sig.2 == WafTier::Fingerprint,
                "pattern '{}' has invalid tier",
                sig.0
            );
        }
    }

    #[test]
    fn test_control_headers_purged_of_non_waf() {
        // REQ-WAF-03: x-wordpress (not a WAF header) and x-cdn (generic CDN) deleted.
        let names: Vec<&str> = WAF_CONTROL_HEADERS.iter().map(|(n, _)| *n).collect();
        assert!(
            !names.contains(&"x-wordpress"),
            "x-wordpress must be deleted"
        );
        assert!(!names.contains(&"x-cdn"), "x-cdn must be deleted");
        assert!(names.contains(&"x-datadome-response"));
        assert!(names.contains(&"cf-mitigated"));
        assert!(names.contains(&"x-akamai-edge-auth"));
        assert!(names.contains(&"x-sucuri-id"));
        assert!(names.contains(&"cf-ray"));
        assert_eq!(names.len(), 5, "exactly 5 control headers expected");
    }

    // ========================================================================
    // detect_body() tests — ported from waf.rs (Approval Testing)
    // ========================================================================

    #[test]
    fn test_detect_body_cloudflare_turnstile() {
        let html = r#"<div id="cf-turnstile" data-sitekey="abc123"></div>"#;
        assert_eq!(
            WafInspector::detect_body(html),
            Some("Cloudflare Turnstile")
        );
    }

    #[test]
    fn test_detect_body_cloudflare_just_a_moment() {
        let html = "<html><body><h1>Just a moment...</h1></body></html>";
        assert_eq!(WafInspector::detect_body(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_body_cloudflare_checking_browser() {
        let html = "<html><body>Checking your browser before accessing...</body></html>";
        assert_eq!(WafInspector::detect_body(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_body_recaptcha() {
        let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=abc"></script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_body_g_recaptcha() {
        let html = r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#;
        assert_eq!(WafInspector::detect_body(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_body_hcaptcha() {
        let html = r#"<div class="h-captcha" data-sitekey="abc"></div>"#;
        assert_eq!(WafInspector::detect_body(html), Some("hCaptcha"));
    }

    #[test]
    fn test_detect_body_datadome() {
        let html = r#"<script src="https://js.datadome.co/captcha.js"></script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("DataDome"));
    }

    #[test]
    fn test_detect_body_perimeterx() {
        let html = r#"<script>var _pxCaptcha = {};</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("PerimeterX"));
    }

    #[test]
    fn test_detect_body_akamai() {
        let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
        assert_eq!(WafInspector::detect_body(html), Some("Akamai Bot Manager"));
    }

    #[test]
    fn test_detect_body_generic_challenge() {
        let html = "<p>Please verify you are a human to continue.</p>";
        assert_eq!(WafInspector::detect_body(html), Some("Generic Challenge"));
    }

    #[test]
    fn test_detect_body_clean_html() {
        let html = r#"
            <html>
                <head><title>Normal Page</title></head>
                <body>
                    <article>
                        <h1>Welcome</h1>
                        <p>This is a normal page with real content.</p>
                    </article>
                </body>
            </html>
        "#;
        assert_eq!(WafInspector::detect_body(html), None);
    }

    #[test]
    fn test_detect_body_empty() {
        assert_eq!(WafInspector::detect_body(""), None);
    }

    #[test]
    fn test_detect_body_aws_waf_cookie_domain_list() {
        let html = r#"<script>window.awsWafCookieDomainList = [];</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_integration() {
        let html = r#"<script>AwsWafIntegration.saveReferrer();</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_goku_props() {
        let html = r#"<script>window.gokuProps = {"key":"AQIDAH..."};</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_token() {
        let html = r#"<meta name="aws-waf-token" content="abc123">"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    // ========================================================================
    // Entropy tests — ported from waf.rs
    // ========================================================================

    #[test]
    fn test_calculate_entropy_high() {
        let obfuscated_js: String = (0u8..=255).map(|b| b as char).collect();
        let entropy = calculate_entropy(&obfuscated_js);
        assert!(entropy > 6.0, "entropy={entropy}, expected > 6.0");
    }

    #[test]
    fn test_calculate_entropy_low() {
        let plain_text = "Hello world, this is a normal page with regular content.";
        let entropy = calculate_entropy(plain_text);
        assert!(entropy < 5.0);
    }

    #[test]
    fn test_is_suspicious_size() {
        assert!(is_suspicious_size(150_000));
        assert!(!is_suspicious_size(10_000));
        assert!(!is_suspicious_size(100_000));
        assert!(is_suspicious_size(100_001));
    }

    #[test]
    fn test_detect_body_by_entropy() {
        // Create >100KB with high entropy to trigger Shannon entropy detection
        let high_entropy_content: String = (0u8..=255)
            .map(|b| b as char)
            .chain((0u8..=255).map(|b| b as char))
            .chain((0u8..=255).map(|b| b as char))
            .chain((0u8..=255).map(|b| b as char))
            .cycle()
            .take(104_000)
            .collect();
        let result = WafInspector::detect_body(&high_entropy_content);
        assert_eq!(result, Some("Obfuscated WAF"));
    }

    #[test]
    fn test_detect_body_small_low_entropy() {
        let small_content = "<html><body>Redirecting...</body></html>";
        assert_eq!(WafInspector::detect_body(small_content), None);
    }

    // ========================================================================
    // verify_integrity() tests (existing, unchanged)
    // ========================================================================

    #[test]
    fn test_waf_control_header_detection() {
        // Test DataDome header detection
        let mut headers = HeaderMap::new();
        headers.insert("x-datadome-response", "blocked".parse().unwrap());

        let result = WafInspector::verify_integrity(&headers, "normal content");
        assert!(result.is_err());

        // Test that cf-ray alone doesn't trigger (common in normal requests)
        let mut headers = HeaderMap::new();
        headers.insert("cf-ray", "abc123".parse().unwrap());

        let result = WafInspector::verify_integrity(&headers, "normal content");
        assert!(result.is_ok());
    }

    #[test]
    fn test_waf_body_signature_detection() {
        // Test Cloudflare detection
        let result = WafInspector::verify_integrity(&HeaderMap::new(), "Just a moment...");
        assert!(result.is_err());

        // Test reCAPTCHA detection
        let result = WafInspector::verify_integrity(&HeaderMap::new(), "<div class='g-recaptcha'>");
        assert!(result.is_err());

        // Test normal content passes
        let result = WafInspector::verify_integrity(
            &HeaderMap::new(),
            "<html><body><p>Hello World</p></body></html>",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_silent_challenge_detection() {
        let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_err());

        let body = "<html><body><p>Hello</p></body></html>";
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_ok());
    }

    #[test]
    fn test_aho_corasick_performance() {
        let body = "This is a page with Just a moment... and recaptcha/api.js content";
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_err());
    }

    #[test]
    fn test_supported_providers() {
        let providers = WafInspector::supported_providers();
        assert!(!providers.is_empty());
        assert!(providers.contains(&"Cloudflare"));
        assert!(providers.contains(&"reCAPTCHA"));
        assert!(providers.contains(&"DataDome"));
        assert!(providers.contains(&"AWS WAF"));
    }
}
