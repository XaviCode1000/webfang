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

impl WafTier {
    /// Spanish user-facing label for the evidence chain (REQ-WAF-08).
    #[must_use]
    pub const fn label_es(self) -> &'static str {
        match self {
            WafTier::Challenge => "desafío",
            WafTier::Fingerprint => "huella",
        }
    }
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

/// Maximum body size (bytes) for silent-challenge script-density analysis (REQ-WAF-06b)
const SILENT_CHALLENGE_MAX_BYTES: usize = 1500;

/// Minimum `<script>` tag count to flag a silent challenge (REQ-WAF-06b)
const SILENT_CHALLENGE_MIN_SCRIPTS: usize = 5;

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
    /// Inspect a response body with full HTTP context and return a verdict.
    ///
    /// This is the single entry point for context-aware, evidence-based WAF
    /// detection (REQ-WAF-01). It collects ALL evidence — boundary-filtered body
    /// signatures (REQ-WAF-03/04), control headers (REQ-WAF-03), and entropy
    /// signals (REQ-WAF-06) — then applies the verdict policy (REQ-WAF-05):
    ///
    /// - [`WafTier::Challenge`] (T1) blocks at ANY status, including 200.
    /// - [`WafTier::Fingerprint`] (T2) blocks only with a correlated WAF status
    ///   (403 / 429 / 503 / 520–529); in degraded mode (no status) T2 is reported
    ///   low-confidence and never blocks.
    ///
    /// `ctx.ignore_waf` yields a clean verdict immediately (REQ-WAF-02 step 1),
    /// and the content-type denylist (REQ-WAF-02) gates body scanning.
    ///
    /// Thread-safe: the AC automaton is immutable once compiled via `Lazy`.
    #[must_use]
    pub fn inspect(body: &str, ctx: &InspectionContext) -> WafVerdict {
        // REQ-WAF-02 step 1: explicit bypass → clean verdict.
        if ctx.ignore_waf {
            return WafVerdict::clean();
        }

        let mut evidences = Vec::new();
        let scan_body = should_scan_body(ctx);

        // Body signatures (boundary-filtered), gated by content-type.
        if scan_body {
            evidences.extend(collect_body_evidence(body));
        }

        // Control headers are always inspected (independent of the body gate).
        evidences.extend(collect_header_evidence(&ctx.headers));

        // Entropy signals need full T2 awareness (rule (a) coexistence), so they
        // run after body + header evidence is collected.
        if scan_body {
            let has_fingerprint = evidences.iter().any(|e| e.tier == WafTier::Fingerprint);
            evidences.extend(entropy_evidence(body, ctx, has_fingerprint));
        }

        let is_blocked = decide(&evidences, ctx);

        // Observability (REQ-WAF-08): warn on a block with its evidence count;
        // debug on every informational (non-blocking) detection.
        if is_blocked {
            tracing::warn!(
                status = ?ctx.status,
                evidences = evidences.len(),
                "WAF/CAPTCHA challenge detected; blocking response"
            );
        } else if !evidences.is_empty() {
            tracing::debug!(
                status = ?ctx.status,
                evidences = evidences.len(),
                "WAF fingerprints observed but not blocking (informational)"
            );
        }

        WafVerdict {
            is_blocked,
            evidences,
        }
    }

    /// Scan a body for WAF challenge signatures (body-only callers).
    ///
    /// Transitional shim over [`WafInspector::inspect`] in **degraded mode**
    /// (no HTTP context): returns the first evidence's provider when the verdict
    /// blocks, or `None` otherwise. In degraded mode only [`WafTier::Challenge`]
    /// markers block; [`WafTier::Fingerprint`] evidence never blocks (REQ-WAF-05).
    /// Callers should migrate to `inspect` with a full [`InspectionContext`].
    #[must_use]
    pub fn detect_body(body: &str) -> Option<&'static str> {
        let ctx = InspectionContext::default();
        let verdict = Self::inspect(body, &ctx);
        if verdict.is_blocked {
            verdict.evidences.first().map(|e| e.provider)
        } else {
            None
        }
    }

    /// Verify response integrity across headers + body (callers with headers).
    ///
    /// Transitional shim over [`WafInspector::inspect`] in **degraded mode**
    /// (headers present, but no status/content-type): returns
    /// [`ScraperError::WafBlocked`] when the verdict blocks. Control headers are
    /// [`WafTier::Fingerprint`] evidence and never auto-block on mere presence
    /// (correction B). Callers should migrate to `inspect` with a full
    /// [`InspectionContext`].
    pub fn verify_integrity(headers: &HeaderMap, body: &str) -> Result<(), ScraperError> {
        let ctx = InspectionContext {
            headers: headers.clone(),
            ..Default::default()
        };
        let verdict = Self::inspect(body, &ctx);
        if verdict.is_blocked {
            Err(ScraperError::waf_blocked(
                String::new(),
                format_evidence_chain(&verdict.evidences),
            ))
        } else {
            Ok(())
        }
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

/// Collect all body-signature evidence in a single Aho-Corasick pass, applying
/// the boundary post-filter (REQ-WAF-04) to Fingerprint `[B]` matches.
///
/// Returns every match that survives the filter (not just the first), so the
/// verdict can carry all collected evidence (REQ-WAF-01) and reason about
/// tier coexistence (REQ-WAF-06).
fn collect_body_evidence(body: &str) -> Vec<WafEvidence> {
    let mut evidences = Vec::new();
    for mat in WAF_AC.find_iter(body) {
        let sig = &WAF_BODY_SIGNATURES[mat.pattern()];
        if !passes_boundary_filter(body, &mat, sig.2, sig.3) {
            continue;
        }
        evidences.push(WafEvidence {
            provider: sig.1,
            tier: sig.2,
            matched_pattern: sig.0,
        });
    }
    evidences
}

/// O(1) adjacent-byte boundary post-filter for Fingerprint-tier `[B]` matches
/// (REQ-WAF-04).
///
/// Rejects a bare vendor-name match when the byte immediately before its start
/// or after its end is ASCII alphanumeric or `_` — this is what stops
/// `akamai_hash` from matching bare `akamai`. UTF-8 safe: any non-ASCII byte
/// (>= 0x80) counts as a boundary. `[E]` (Exempt) patterns and all
/// Challenge-tier patterns skip the filter entirely.
#[inline]
fn passes_boundary_filter(
    body: &str,
    mat: &aho_corasick::Match,
    tier: WafTier,
    boundary: BoundaryMode,
) -> bool {
    // Only Fingerprint-tier bare vendor names get the filter.
    if tier != WafTier::Fingerprint || boundary != BoundaryMode::Bare {
        return true;
    }
    let bytes = body.as_bytes();
    if mat.start() > 0 {
        let before = bytes[mat.start() - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return false;
        }
    }
    if mat.end() < bytes.len() {
        let after = bytes[mat.end()];
        if after.is_ascii_alphanumeric() || after == b'_' {
            return false;
        }
    }
    true
}

/// Content-Type gate / denylist (REQ-WAF-02).
///
/// Decides whether the response body should be scanned for WAF signatures:
/// 1. `application/xhtml+xml` carve-out → scan (it is HTML despite `+xml`).
/// 2. Skip structured data (`+json`/`+xml` suffix, `application/json`,
///    `application/xml`, `text/json`, `text/xml`) and binary assets
///    (`image/*`, `font/*`, `application/wasm`, `application/javascript`).
/// 3. `text/*`, missing content-type, or anything else not denied → scan
///    (this is a denylist, not an allowlist).
///
/// The `ignore_waf` short-circuit (REQ-WAF-02 step 1) happens in
/// [`WafInspector::inspect`] before this gate is consulted.
fn should_scan_body(ctx: &InspectionContext) -> bool {
    // Missing content-type → scan.
    let Some(raw) = &ctx.content_type else {
        return true;
    };
    // Strip parameters (e.g. "text/html; charset=utf-8" → "text/html").
    let mime = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    // Carve-out: XHTML is HTML — scan it (checked before the +xml skip).
    if mime == "application/xhtml+xml" {
        return true;
    }
    // Denylist: structured data formats are never HTML challenges.
    let is_json = mime.ends_with("+json") || mime == "application/json" || mime == "text/json";
    let is_xml = mime.ends_with("+xml") || mime == "application/xml" || mime == "text/xml";
    if is_json || is_xml {
        return false;
    }
    // Denylist: binary assets.
    if mime.starts_with("image/") || mime.starts_with("font/") {
        return false;
    }
    if mime == "application/wasm" || mime == "application/javascript" {
        return false;
    }
    // text/* or anything else not denied → scan.
    true
}

/// Collect control-header evidence (REQ-WAF-03).
///
/// Every control header is [`WafTier::Fingerprint`] evidence — it never
/// auto-blocks on mere presence (correction B), only when correlated with a
/// WAF status code by [`decide`].
fn collect_header_evidence(headers: &HeaderMap) -> Vec<WafEvidence> {
    let mut evidences = Vec::new();
    for (name, provider) in WAF_CONTROL_HEADERS {
        if headers.get(*name).is_some() {
            evidences.push(WafEvidence {
                provider,
                tier: WafTier::Fingerprint,
                matched_pattern: name,
            });
        }
    }
    evidences
}

/// Verdict policy (REQ-WAF-05).
///
/// - [`WafTier::Challenge`] blocks at ANY status (including 200 and degraded).
/// - [`WafTier::Fingerprint`] blocks only when the status correlates with a WAF
///   response (403 / 429 / 503 / 520–529); degraded mode (no status) never
///   blocks on Fingerprint evidence (reported low-confidence).
///
/// Entropy challenges are emitted as [`WafTier::Challenge`] evidence only when
/// their own policy (REQ-WAF-06) already decided to block, so they short-circuit
/// here like any other Challenge marker.
fn decide(evidences: &[WafEvidence], ctx: &InspectionContext) -> bool {
    for ev in evidences {
        match ev.tier {
            WafTier::Challenge => return true,
            WafTier::Fingerprint => {
                if is_t2_blocking_status(ctx.status) {
                    return true;
                }
            },
        }
    }
    false
}

/// Whether an HTTP status correlates with a WAF block for Fingerprint evidence.
#[inline]
fn is_t2_blocking_status(status: Option<u16>) -> bool {
    matches!(status, Some(403 | 429 | 503 | 520..=529))
}

/// Entropy-based challenge detection (REQ-WAF-06).
///
/// Returns evidence ONLY when a rule decides to block (so its presence in a
/// verdict always means "blocked"):
/// - Rule (a): body > 100KB AND entropy > 5.5 b/B → block if status != 200
///   (unknown/degraded counts as != 200) OR a Fingerprint marker coexists.
/// - Rule (b): body < 1500B AND > 5 `<script>` tags → block if non-2xx OR
///   (200 + HTML content-type). The 200+HTML case is the "H3 fix" silent
///   challenge and MUST be kept (`discovery.rs` depends on it).
fn entropy_evidence(
    body: &str,
    ctx: &InspectionContext,
    has_fingerprint: bool,
) -> Option<WafEvidence> {
    // Rule (a): large + high entropy → obfuscated WAF.
    if body.len() > SUSPICIOUS_SIZE_THRESHOLD {
        let entropy = calculate_entropy(body);
        if entropy > ENTROPY_THRESHOLD {
            if ctx.status != Some(200) || has_fingerprint {
                return Some(WafEvidence {
                    provider: "Obfuscated WAF",
                    tier: WafTier::Challenge,
                    matched_pattern: "high-entropy body (>100KB, >5.5 b/B)",
                });
            }
            // Informational detection (REQ-WAF-08): high entropy at status 200
            // without a coexisting T2 marker is logged, not blocked.
            tracing::debug!(
                entropy,
                "high-entropy body at status 200 without a T2 marker; not blocking"
            );
        }
    }

    // Rule (b): tiny body dense with <script> tags → silent challenge.
    if body.len() < SILENT_CHALLENGE_MAX_BYTES {
        let script_count = body.matches("<script").count();
        if script_count > SILENT_CHALLENGE_MIN_SCRIPTS {
            let is_2xx = matches!(ctx.status, Some(200..=299));
            let is_html = ctx
                .content_type
                .as_deref()
                .is_some_and(is_html_content_type);
            // Unknown status (degraded) is treated as non-2xx (conservative).
            if !is_2xx || (ctx.status == Some(200) && is_html) {
                return Some(WafEvidence {
                    provider: "Silent Challenge",
                    tier: WafTier::Challenge,
                    matched_pattern: "script-density (<1500B, >5 <script>)",
                });
            }
        }
    }

    None
}

/// Whether a content-type denotes HTML (for the 200+HTML silent-challenge rule).
fn is_html_content_type(ct: &str) -> bool {
    let mime = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mime == "text/html" || mime == "application/xhtml+xml"
}

/// Format the evidence chain for the Spanish user-facing block message (REQ-WAF-08).
///
/// Each evidence renders as `provider (patrón: <pattern>, tier: <label_es>)`,
/// joined by `; `. Falls back to a generic label when the chain is empty.
fn format_evidence_chain(evidences: &[WafEvidence]) -> String {
    if evidences.is_empty() {
        return "WAF desconocido".to_string();
    }
    evidences
        .iter()
        .map(|e| {
            format!(
                "{} (patrón: {}, tier: {})",
                e.provider,
                e.matched_pattern,
                e.tier.label_es()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
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
    // TASK-03 — Boundary post-filter (REQ-WAF-04)
    // ========================================================================

    #[test]
    fn test_boundary_rejects_trailing_underscore() {
        // Fixture 1 core: bare "akamai" [B] rejected when followed by '_'.
        let evidences = collect_body_evidence(r#"{"key": "akamai_hash"}"#);
        assert!(
            !evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "bare 'akamai' must be rejected when followed by '_' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_rejects_alphanumeric_prefix() {
        // bare "akamai" [B] rejected when preceded by an alphanumeric byte.
        let evidences = collect_body_evidence("xakamai");
        assert!(
            !evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "bare 'akamai' must be rejected when preceded by 'x' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_stands_in_prose() {
        // Fixture 4 core: "cloudflare" surrounded by spaces stands (T2 evidence).
        let evidences = collect_body_evidence("an article mentioning cloudflare in prose");
        assert!(
            evidences
                .iter()
                .any(|e| e.matched_pattern == "cloudflare" && e.tier == WafTier::Fingerprint),
            "'cloudflare' with space boundaries must stand (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_exempt_pattern_matches_inside_token() {
        // Fixture 5 core: "incap_ses" [E] matches inside the larger token.
        let evidences = collect_body_evidence("cookie incap_ses_123 = abc");
        assert!(
            evidences.iter().any(|e| e.matched_pattern == "incap_ses"),
            "[E] 'incap_ses' must match inside 'incap_ses_123' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_t1_exempt() {
        // Challenge-tier patterns are exempt from the boundary filter.
        let evidences = collect_body_evidence("xxcf-turnstileyy");
        assert!(
            evidences
                .iter()
                .any(|e| e.matched_pattern == "cf-turnstile"),
            "T1 'cf-turnstile' matches regardless of adjacent bytes (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_utf8_non_ascii_is_boundary() {
        // Any non-ASCII byte counts as a boundary (UTF-8 safe).
        let evidences = collect_body_evidence("ñakamaiñ");
        assert!(
            evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "non-ASCII adjacent bytes are boundaries, 'akamai' stands (got {evidences:?})"
        );
    }

    // ========================================================================
    // TASK-04 — Content-Type gate / denylist (REQ-WAF-02)
    // ========================================================================

    #[test]
    fn test_gate_skips_application_json() {
        // Fixture 1 gate: 200 application/json is NOT scanned.
        let ctx = InspectionContext {
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        assert!(!should_scan_body(&ctx), "application/json must be skipped");
    }

    #[test]
    fn test_gate_skips_structured_suffixes() {
        for ct in [
            "application/ld+json",
            "application/vnd.api+json",
            "application/atom+xml",
        ] {
            let ctx = InspectionContext {
                content_type: Some(ct.into()),
                ..Default::default()
            };
            assert!(!should_scan_body(&ctx), "{ct} must be skipped");
        }
    }

    #[test]
    fn test_gate_scans_xhtml_carve_out() {
        // application/xhtml+xml is carved out → scanned despite the +xml suffix.
        let ctx = InspectionContext {
            content_type: Some("application/xhtml+xml".into()),
            ..Default::default()
        };
        assert!(
            should_scan_body(&ctx),
            "xhtml+xml carve-out must be scanned"
        );
    }

    #[test]
    fn test_gate_skips_binary_assets() {
        for ct in [
            "image/png",
            "image/jpeg",
            "font/woff2",
            "application/wasm",
            "application/javascript",
        ] {
            let ctx = InspectionContext {
                content_type: Some(ct.into()),
                ..Default::default()
            };
            assert!(!should_scan_body(&ctx), "{ct} must be skipped");
        }
    }

    #[test]
    fn test_gate_scans_text_and_missing() {
        // text/* and missing content-type → scanned (denylist, not allowlist).
        for ct in [Some("text/html"), Some("text/plain"), None] {
            let ctx = InspectionContext {
                content_type: ct.map(String::from),
                ..Default::default()
            };
            assert!(should_scan_body(&ctx), "{ct:?} must be scanned");
        }
    }

    #[test]
    fn test_gate_ignores_charset_parameter() {
        // Parameters after ';' are ignored when classifying the MIME type.
        let ctx = InspectionContext {
            content_type: Some("application/json; charset=utf-8".into()),
            ..Default::default()
        };
        assert!(!should_scan_body(&ctx), "json with charset must be skipped");
        let ctx = InspectionContext {
            content_type: Some("text/html; charset=utf-8".into()),
            ..Default::default()
        };
        assert!(should_scan_body(&ctx), "html with charset must be scanned");
    }

    // ========================================================================
    // TASK-05 — Verdict policy + inspect(ctx) (REQ-WAF-05)
    // ========================================================================

    fn ctx_with(status: Option<u16>, content_type: Option<&str>) -> InspectionContext {
        InspectionContext {
            status,
            content_type: content_type.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn test_inspect_t1_blocks_at_200() {
        // Fixture 3: Turnstile (T1) at 200 → BLOCK.
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(r#"<div id="cf-turnstile"></div>"#, &ctx);
        assert!(verdict.is_blocked, "T1 must block at 200");
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.tier == WafTier::Challenge));
    }

    #[test]
    fn test_inspect_cf_503_challenge_blocks() {
        // Fixture 2: Cloudflare 503 challenge (T1 prose) → BLOCK.
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect("<title>Just a moment...</title>", &ctx);
        assert!(verdict.is_blocked, "T1 prose at 503 must block");
    }

    #[test]
    fn test_inspect_t2_only_at_200_passes() {
        // Fixtures 1/4/5: T2-only evidence at 200 → PASS (but evidence collected).
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect("an article mentioning cloudflare in prose", &ctx);
        assert!(!verdict.is_blocked, "T2 at 200 must not block");
        assert!(!verdict.evidences.is_empty(), "T2 evidence still collected");
    }

    #[test]
    fn test_inspect_t2_at_503_blocks() {
        // T2 evidence with a correlated WAF status → BLOCK.
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect("served by cloudflare", &ctx);
        assert!(verdict.is_blocked, "T2 at 503 must block");
    }

    #[test]
    fn test_inspect_t2_blocking_status_set() {
        // Only 403/429/503/520-529 correlate with T2 evidence.
        for status in [403u16, 429, 503, 520, 525, 529] {
            let ctx = ctx_with(Some(status), Some("text/html"));
            let verdict = WafInspector::inspect("perimeterx protected", &ctx);
            assert!(verdict.is_blocked, "T2 at {status} must block");
        }
        for status in [200u16, 201, 301, 404, 500, 519, 530] {
            let ctx = ctx_with(Some(status), Some("text/html"));
            let verdict = WafInspector::inspect("perimeterx protected", &ctx);
            assert!(!verdict.is_blocked, "T2 at {status} must NOT block");
        }
    }

    #[test]
    fn test_inspect_degraded_t2_never_blocks() {
        // Degraded mode (no status): T2 reported low-confidence, never blocks.
        let ctx = InspectionContext::default();
        let verdict = WafInspector::inspect("served by cloudflare", &ctx);
        assert!(!verdict.is_blocked, "degraded T2 must not block");
        assert!(!verdict.evidences.is_empty(), "T2 evidence still emitted");
    }

    #[test]
    fn test_inspect_degraded_t1_blocks() {
        // Degraded mode: T1 still blocks.
        let ctx = InspectionContext::default();
        let verdict = WafInspector::inspect("<div class='g-recaptcha'></div>", &ctx);
        assert!(verdict.is_blocked, "degraded T1 must block");
    }

    #[test]
    fn test_inspect_ignore_waf_clean() {
        // REQ-WAF-02 step 1: ignore_waf → clean verdict even with T1 present.
        let ctx = InspectionContext {
            ignore_waf: true,
            content_type: Some("text/html".into()),
            ..Default::default()
        };
        let verdict = WafInspector::inspect("<div id='cf-turnstile'></div>", &ctx);
        assert!(!verdict.is_blocked, "ignore_waf must yield clean verdict");
        assert!(
            verdict.evidences.is_empty(),
            "ignore_waf collects no evidence"
        );
    }

    #[test]
    fn test_inspect_control_header_t2_never_auto_blocks() {
        // Correction B: a control header alone (T2) never blocks on mere presence.
        let mut headers = HeaderMap::new();
        headers.insert("x-datadome-response", "blocked".parse().unwrap());
        let ctx = InspectionContext {
            headers,
            ..Default::default()
        };
        let verdict = WafInspector::inspect("normal content here", &ctx);
        assert!(
            !verdict.is_blocked,
            "header alone must not block (degraded)"
        );
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.matched_pattern == "x-datadome-response"));
    }

    #[test]
    fn test_inspect_control_header_with_503_blocks() {
        // Control header (T2) + correlated status → block.
        let mut headers = HeaderMap::new();
        headers.insert("cf-mitigated", "challenge".parse().unwrap());
        let ctx = InspectionContext {
            headers,
            status: Some(503),
            content_type: Some("text/html".into()),
            ..Default::default()
        };
        let verdict = WafInspector::inspect("normal content here", &ctx);
        assert!(verdict.is_blocked, "cf-mitigated + 503 must block");
    }

    #[test]
    fn test_inspect_json_gate_not_scanned() {
        // Fixture 1: 200 application/json with akamai_hash → not scanned → clean.
        let ctx = ctx_with(Some(200), Some("application/json"));
        let verdict = WafInspector::inspect(r#"{"key": "akamai_hash"}"#, &ctx);
        assert!(!verdict.is_blocked, "json body must not be scanned");
        assert!(verdict.evidences.is_empty(), "no evidence from gated body");
    }

    #[test]
    fn test_inspect_collects_all_evidences() {
        // REQ-WAF-01: the verdict carries ALL evidences, not first-hit.
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect(
            "Just a moment... protected by cloudflare and datadome",
            &ctx,
        );
        assert!(verdict.is_blocked);
        assert!(
            verdict.evidences.len() >= 2,
            "multiple evidences collected (got {:?})",
            verdict.evidences
        );
    }

    // ========================================================================
    // TASK-06 — Entropy rules (REQ-WAF-06)
    //
    // The entropy mechanism landed in inspect() in TASK-05 (inseparable: the
    // silent-challenge / obfuscated-WAF guard rails must stay green the moment
    // the shims delegate to inspect). These tests provide the dedicated
    // REQ-WAF-06 status-aware policy coverage — each distinguishes the new
    // policy from the old "always block on entropy" behavior.
    // ========================================================================

    #[test]
    fn test_entropy_silent_challenge_200_html_blocks() {
        // REQ-WAF-06b: 200 + HTML, <1500B, 6 scripts → BLOCK (silent challenge).
        // This is the "H3 fix" case discovery.rs depends on — MUST be kept.
        let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(body, &ctx);
        assert!(verdict.is_blocked, "200+HTML silent challenge must block");
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.provider == "Silent Challenge"));
    }

    #[test]
    fn test_entropy_silent_challenge_200_non_html_passes() {
        // REQ-WAF-06b: 200 + non-HTML, <1500B, 6 scripts → NOT blocked.
        let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
        let ctx = ctx_with(Some(200), Some("text/plain"));
        let verdict = WafInspector::inspect(body, &ctx);
        assert!(
            !verdict.is_blocked,
            "200+non-HTML script density must not block"
        );
    }

    #[test]
    fn test_entropy_silent_challenge_non_2xx_blocks() {
        // REQ-WAF-06b: non-2xx + <1500B + >5 scripts → BLOCK.
        let body = "<html><script></script><script></script><script></script><script></script><script></script><script></script></html>";
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect(body, &ctx);
        assert!(verdict.is_blocked, "non-2xx silent challenge must block");
    }

    #[test]
    fn test_entropy_obfuscated_200_no_t2_passes() {
        // REQ-WAF-06a: 200, >100KB, >5.5 b/B, no T2 marker → NOT blocked
        // (old behavior blocked unconditionally — this is the policy change).
        let high_entropy: String = (0u8..=255)
            .map(|b| b as char)
            .cycle()
            .take(104_000)
            .collect();
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&high_entropy, &ctx);
        assert!(
            !verdict.is_blocked,
            "200 high-entropy without T2 must not block"
        );
    }

    #[test]
    fn test_entropy_obfuscated_non_2xx_blocks() {
        // REQ-WAF-06a: non-2xx + >100KB + >5.5 b/B → BLOCK.
        let high_entropy: String = (0u8..=255)
            .map(|b| b as char)
            .cycle()
            .take(104_000)
            .collect();
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect(&high_entropy, &ctx);
        assert!(verdict.is_blocked, "non-2xx high-entropy must block");
        assert!(verdict
            .evidences
            .iter()
            .any(|e| e.provider == "Obfuscated WAF"));
    }

    #[test]
    fn test_entropy_obfuscated_200_with_t2_blocks() {
        // REQ-WAF-06a: 200 + >100KB + >5.5 b/B + coexisting T2 marker → BLOCK.
        // The ascending byte cycle contains no signature, so append a
        // boundary-clean bare vendor name to force T2 coexistence.
        let mut high_entropy: String = (0u8..=255)
            .map(|b| b as char)
            .cycle()
            .take(104_000)
            .collect();
        high_entropy.push_str(" cloudflare ");
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&high_entropy, &ctx);
        assert!(
            verdict.is_blocked,
            "200 high-entropy with T2 coexistence must block"
        );
    }

    // ========================================================================
    // TASK-07 — Error UX (Spanish evidence chain) + observability (REQ-WAF-08)
    // ========================================================================

    #[test]
    fn test_evidence_chain_spanish_lists_all_evidences() {
        let evidences = vec![
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
        ];
        let chain = format_evidence_chain(&evidences);
        // Every evidence's provider + pattern + Spanish tier label is listed.
        assert!(chain.contains("Cloudflare"), "chain: {chain}");
        assert!(chain.contains("cf-turnstile"), "chain: {chain}");
        assert!(
            chain.contains("desafío"),
            "Challenge Spanish label: {chain}"
        );
        assert!(chain.contains("Akamai"), "chain: {chain}");
        assert!(chain.contains("akamai"), "chain: {chain}");
        assert!(
            chain.contains("huella"),
            "Fingerprint Spanish label: {chain}"
        );
    }

    #[test]
    fn test_evidence_chain_empty_fallback() {
        assert_eq!(format_evidence_chain(&[]), "WAF desconocido");
    }

    #[test]
    fn test_verify_integrity_error_carries_evidence_chain() {
        // REQ-WAF-08: the block error message lists each evidence in Spanish.
        let result = WafInspector::verify_integrity(&HeaderMap::new(), "Just a moment...");
        let err = result.expect_err("T1 prose must block");
        let msg = err.to_string();
        assert!(
            msg.contains("WAF/CAPTCHA detectado"),
            "Spanish prefix: {msg}"
        );
        assert!(msg.contains("Cloudflare"), "chain provider: {msg}");
        assert!(msg.contains("patrón:"), "chain pattern label: {msg}");
        assert!(msg.contains("tier:"), "chain tier label: {msg}");
    }

    #[test]
    fn test_block_error_stays_permanent_fatal() {
        // REQ-WAF-08: ErrorClass stays PermanentFatal (exit 69).
        let err = ScraperError::waf_blocked(
            "https://example.com",
            format_evidence_chain(&[WafEvidence {
                provider: "Cloudflare",
                tier: WafTier::Challenge,
                matched_pattern: "cf-turnstile",
            }]),
        );
        assert_eq!(err.classify(), crate::error::ErrorClass::PermanentFatal);
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
        // `_abck` is a Fingerprint-tier ([E]) marker. In degraded mode (the
        // detect_body shim) T2 evidence never blocks, so this is no longer
        // reported as a block — the false positive that motivated issue #346.
        // (Migrated from old first-hit behavior; TASK-14 verifies the full policy.)
        let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
        assert_eq!(WafInspector::detect_body(html), None);
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
        // Control headers are Fingerprint-tier evidence — mere presence never
        // auto-blocks (correction B). verify_integrity runs in degraded mode (no
        // status), so a T2 header alone is clean.
        // (Migrated from old presence-blocks behavior; TASK-14 verifies policy.)
        let mut headers = HeaderMap::new();
        headers.insert("x-datadome-response", "blocked".parse().unwrap());

        let result = WafInspector::verify_integrity(&headers, "normal content");
        assert!(result.is_ok(), "T2 header alone must not block (degraded)");

        // cf-ray alone also doesn't trigger (common in normal requests).
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
