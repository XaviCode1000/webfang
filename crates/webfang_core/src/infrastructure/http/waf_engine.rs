//! WAF Detection Engine - Layer 7 Protection
//!
//! This module provides advanced WAF detection beyond the basic signature matching
//! in http_client.rs. It includes:
//! - Detection by Control Headers (x-datadome-response, cf-mitigated, etc.)
//! - Entropy analysis for "Silent Challenge" detection
//! - Efficient O(N) matching using Aho-Corasick for 60+ signatures
//! - Context-aware, tiered, evidence-based inspection via [`WafInspector::inspect`]
//!
//! # Usage
//!
//! ```ignore
//! use webfang_core::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
//!
//! # let headers = wreq::header::HeaderMap::new();
//! # let body = String::new();
//! // Build the inspection context from the HTTP response (status + content-type
//! // + headers); callers with only the body use the degraded default.
//! let ctx = InspectionContext {
//!     status: Some(200),
//!     content_type: Some("text/html".into()),
//!     headers,
//!     ignore_waf: false,
//! };
//! let verdict = WafInspector::inspect(&body, &ctx);
//! if verdict.is_blocked {
//!     return Err(verdict.evidence_chain());
//! }
//! ```

use aho_corasick::AhoCorasick;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::collections::HashSet;

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

/// Where a piece of WAF evidence was observed (FIX B).
///
/// Drives the 5xx body-vs-header carve-out in the verdict policy: on a 5xx
/// response a bare vendor mention sourced from the BODY ([`EvidenceSource::Body`])
/// is ubiquitous diagnostic noise and does not block, whereas the same Fingerprint
/// tier sourced from a control HEADER ([`EvidenceSource::Header`], e.g.
/// `cf-mitigated`) signals active mitigation and still blocks. Internal to the
/// verdict — it is deliberately NOT part of the Spanish evidence chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    /// Evidence matched in the response body (signatures or entropy).
    Body,
    /// Evidence matched in a response control header.
    Header,
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
    /// Where the evidence was observed (body or header) — drives the 5xx
    /// body-vs-header carve-out in the verdict policy; not surfaced in the
    /// Spanish evidence chain.
    pub source: EvidenceSource,
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

    /// Spanish user-facing evidence chain for the block error (REQ-WAF-08).
    ///
    /// Each evidence renders as `provider (patrón: <pattern>, tier: <label_es>)`,
    /// joined by `; `. Falls back to a generic label when the verdict carries no
    /// evidence. Callers that raise a block error (HTTP client, scraper service,
    /// crawler discovery, MCP) pass this string as the `provider` payload so the
    /// full chain reaches the user instead of a bare first-hit provider name.
    #[must_use]
    pub fn evidence_chain(&self) -> String {
        format_evidence_chain(&self.evidences)
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
    pub headers: HashMap<String, String>,
    /// Bypass WAF detection entirely (yields a clean verdict).
    pub ignore_waf: bool,
}

impl InspectionContext {
    /// Build a full context from an HTTP status and a lowercased-key header map,
    /// as captured by the domain `HttpResponse` and infrastructure `FetchedPage`
    /// types (REQ-WAF-01).
    ///
    /// The `content-type` entry is lifted into [`Self::content_type`] for the
    /// REQ-WAF-02 gate; the whole map is retained as plain lowercased-key
    /// `String` pairs so control-header evidence (REQ-WAF-03) needs no `wreq`
    /// types at the inspection boundary. Headers are single-valued at capture
    /// (duplicate names collapse, last value wins — WAF control headers are
    /// single-valued, so no evidence is lost). `ignore_waf` short-circuits
    /// inspection to a clean verdict (REQ-WAF-07).
    #[must_use]
    pub fn from_lowercase_headers(
        status: u16,
        headers: &std::collections::HashMap<String, String>,
        ignore_waf: bool,
    ) -> Self {
        let content_type = headers.get("content-type").cloned();
        Self {
            status: Some(status),
            content_type,
            headers: headers.clone(),
            ignore_waf,
        }
    }
}

/// Control headers that indicate WAF processing (REQ-WAF-03).
///
/// Every control header is [`WafTier::Fingerprint`] evidence — it NEVER
/// auto-blocks on mere presence, only when correlated with a WAF status code
/// (correction B). The retained headers correlate with *active* processing or
/// mitigation: a DataDome response, a Cloudflare mitigation flag, an Akamai
/// edge-auth challenge — so status-correlated blocking is the intended #346
/// semantics.
///
/// RES-01: `cf-ray` and `x-sucuri-id` were purged. They are ubiquitous trace
/// headers — present on 100% of Cloudflare / Sucuri-proxied traffic, including
/// every genuine transient origin failure — so a fingerprint that rides on all
/// of a vendor's traffic carries zero challenge evidence. Treating them as
/// Fingerprint evidence block-correlated every real 503 behind those vendors
/// (instant `WafChallenge`, zero retries). They are noise, not signal.
/// `x-wordpress` (not a real WAF header) and `x-cdn` (generic CDN header) were
/// purged earlier as false-positive risks.
const WAF_CONTROL_HEADERS: &[(&str, &str)] = &[
    ("x-datadome-response", "DataDome"),
    ("cf-mitigated", "Cloudflare"),
    ("x-akamai-edge-auth", "Akamai"),
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
        WafTier::Fingerprint,
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
    (
        "h-captcha-response",
        "hCaptcha",
        WafTier::Challenge,
        BoundaryMode::Phrase,
    ), // hidden response field injected by the hCaptcha widget
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

/// Maximum number of Fingerprint (T2) evidences accumulated per inspection pass
/// (RISK-01 — DoS-hardening bound on the evidence allocation).
///
/// This caps the T2 allocation, NOT detection sensitivity: the whole body is
/// scanned, and a [`WafTier::Challenge`] (T1) marker short-circuits the pass the
/// moment it survives the boundary filter, so a T1 marker past this many earlier
/// T2 matches is still detected (see [`collect_body_evidence`]). A verdict needs
/// at most one T1 marker (blocks any status) or one [`WafTier::Fingerprint`]
/// marker correlated with a WAF status, and 64 is far beyond any real challenge
/// page's signal count. Without the cap the all-evidence collector materialized
/// every Aho-Corasick match into an uncapped `Vec` (~3-4x body size on
/// adversarial text/html under concurrency); the old first-hit detector was
/// O(1). This bound keeps the worst case bounded-linear.
const MAX_EVIDENCE_PER_BODY: usize = 64;

/// Aho-Corasick automaton for O(N) multi-pattern body matching.
///
/// Compiled once from [`WAF_BODY_SIGNATURES`] on first access (thread-safe via
/// `Lazy`). Signatures are compile-time constants and validated in tests — but a
/// misconfigured signature would silently panic on first use. We therefore
/// make the builder fallible and surface clear diagnostics if it ever happens.
// LCOV_EXCL_START defensive: waf-automaton-build — signature patterns are compile-time constants verified in tests
static WAF_AC: Lazy<AhoCorasick> = Lazy::new(|| {
    AhoCorasick::new(WAF_BODY_SIGNATURES.iter().map(|(sig, _, _, _)| sig)).unwrap_or_else(|err| {
        panic!(
            "WAF body signature automaton build failed — verify the {} compile-time \
                 signatures in `WAF_BODY_SIGNATURES` (patterns must be valid UTF-8 and \
                 within byte-depth limits). Aho-Corasick build error: {err}",
            WAF_BODY_SIGNATURES.len()
        )
    })
});
// LCOV_EXCL_STOP

/// WafInspector provides multi-layer WAF detection
pub struct WafInspector;

impl crate::domain::waf::sealed::Sealed for WafInspector {}

impl crate::domain::waf::WafInspectorPort for WafInspector {
    fn inspect(
        &self,
        body: &str,
        ctx: &crate::domain::waf::InspectionContext,
    ) -> crate::domain::waf::WafVerdict {
        let infra_ctx = InspectionContext {
            status: ctx.status,
            content_type: ctx.content_type.clone(),
            headers: ctx.headers.clone(),
            ignore_waf: ctx.ignore_waf,
        };
        let verdict = Self::inspect(body, &infra_ctx);
        crate::domain::waf::WafVerdict {
            is_blocked: verdict.is_blocked,
            evidences: verdict
                .evidences
                .into_iter()
                .map(|e| crate::domain::waf::WafEvidence {
                    provider: e.provider,
                    tier: match e.tier {
                        WafTier::Challenge => crate::domain::waf::WafTier::Challenge,
                        WafTier::Fingerprint => crate::domain::waf::WafTier::Fingerprint,
                    },
                    matched_pattern: e.matched_pattern,
                    source: match e.source {
                        EvidenceSource::Body => crate::domain::waf::EvidenceSource::Body,
                        EvidenceSource::Header => crate::domain::waf::EvidenceSource::Header,
                    },
                })
                .collect(),
        }
    }
}

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
    /// The HTTP-200 entropy escalation (REQ-WAF-06 rule (a)) requires a GENUINE
    /// challenge-context Fingerprint: a config-string occurrence (#781, e.g.
    /// `"wgConfirmEdit...":"hcaptcha"` in a MediaWiki config dump) is recorded
    /// as evidence but never escalates a dense legitimate page into a block.
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

        // Body signatures (boundary-filtered), gated by content-type. The
        // collector also reports whether at least one GENUINE challenge-context
        // Fingerprint survived (config-string occurrences, #781, count as
        // evidence but not as genuine challenge context).
        let mut has_genuine_fingerprint = false;
        if scan_body {
            let (body_evidences, genuine) = collect_body_evidence(body);
            evidences.extend(body_evidences);
            has_genuine_fingerprint = genuine;
        }

        // Control headers are always inspected (independent of the body gate).
        // Every control header is Fingerprint-tier evidence of ACTIVE mitigation,
        // which is genuine challenge context for the entropy rule.
        let header_evidences = collect_header_evidence(&ctx.headers);
        has_genuine_fingerprint |= !header_evidences.is_empty();
        evidences.extend(header_evidences);

        // Entropy signals need full T2 awareness (rule (a) coexistence), so they
        // run after body + header evidence is collected.
        if scan_body {
            evidences.extend(entropy_evidence(body, ctx, has_genuine_fingerprint));
        }

        let is_blocked = decide(&evidences, ctx);

        // Observability (REQ-WAF-08): the engine inspects silently — both the
        // blocking and the informational path log at debug. The decision site
        // (HTTP client / scraper service / crawler discovery) owns the WARN or
        // structured error, so a block is not double-logged here.
        if is_blocked {
            tracing::debug!(
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

/// Collect body-signature evidence in a single Aho-Corasick pass, applying the
/// boundary post-filter (REQ-WAF-04) to Fingerprint `[B]` matches.
///
/// Returns the evidence vector plus a `has_genuine_fingerprint` flag used by the
/// entropy rule (REQ-WAF-06a, #781): a Fingerprint-tier match that is merely a
/// quoted JS/JSON **config value** (e.g. MediaWiki's
/// `"wgConfirmEditHCaptchaSiteKey": "hcaptcha"`) is still recorded — it may
/// serve the evidence chain of a status-correlated block (403/429/503) — but it
/// is NOT genuine challenge context, so it can no longer escalate a large
/// high-entropy legitimate page into an obfuscated-WAF block at HTTP 200.
///
/// The whole body is scanned: [`MAX_EVIDENCE_PER_BODY`] caps only the
/// *Fingerprint* (T2) accumulation (RISK-01 DoS hardening), never detection. A
/// [`WafTier::Challenge`] (T1) marker is a definitive verdict — the moment one
/// survives the filter it clears any accumulated T2 noise and short-circuits the
/// pass, so a T1 marker past 64 earlier T2 matches is still seen (the cap is not
/// a detection limit). T2 matches keep accumulating (bounded by the cap) and the
/// scan continues past the cap so a later T1 marker is still examined. A verdict
/// needs at most one T1 marker or one correlated T2 marker, and the T2 cap is far
/// beyond any real challenge page's signal count.
fn collect_body_evidence(body: &str) -> (Vec<WafEvidence>, bool) {
    let mut evidences = Vec::new();
    let mut has_genuine_fingerprint = false;
    for mat in WAF_AC.find_iter(body) {
        let sig = &WAF_BODY_SIGNATURES[mat.pattern()];
        if !passes_boundary_filter(body, &mat, sig.2, sig.3) {
            continue;
        }
        match sig.2 {
            // T1 is a definitive verdict that noise density cannot hide: drop any
            // accumulated T2 noise and short-circuit on the first T1 marker.
            WafTier::Challenge => {
                evidences.clear();
                evidences.push(WafEvidence {
                    provider: sig.1,
                    tier: sig.2,
                    matched_pattern: sig.0,
                    source: EvidenceSource::Body,
                });
                return (evidences, true);
            },
            // T2 accumulation is capped (RISK-01); keep scanning past the cap so a
            // later T1 marker is still examined.
            WafTier::Fingerprint => {
                // #781: a quoted config-string occurrence is evidence (feeds the
                // block chain under a WAF status) but not genuine challenge
                // context for the entropy rule.
                has_genuine_fingerprint |= !is_config_string_match(body, &mat);
                if evidences.len() < MAX_EVIDENCE_PER_BODY {
                    evidences.push(WafEvidence {
                        provider: sig.1,
                        tier: sig.2,
                        matched_pattern: sig.0,
                        source: EvidenceSource::Body,
                    });
                }
            },
        }
    }
    (evidences, has_genuine_fingerprint)
}

/// Whether a body match is a quoted JS/JSON **config-value** occurrence rather
/// than a served challenge (#781).
///
/// A match is a config string when it fills an ENTIRE double-quoted string that
/// sits in value position — either preceded by `"key":` / `"key" :` (JSON
/// object) or by a bare identifier and `=` or `:` (JS assignment). Real
/// challenge occurrences never take this shape: widget markup
/// (`class="h-captcha"`, `data-sitekey="..."`) embeds the marker inside a
/// quoted attribute, script sources (`hcaptcha.com/1/api.js`, `hcaptcha.js`)
/// carry a path suffix, and challenge prose is unquoted. The filter is
/// conservative: anything ambiguous stays a genuine match.
///
/// Complexity is bounded by a small constant walk around the match start/end —
/// the Aho-Corasick scan stays the only O(N) pass.
/// Whether a body match is a quoted JS/JSON **config-value** occurrence rather
/// than a served challenge (#781).
///
/// A match is a config string when it fills an ENTIRE double-quoted string in
/// the value position of a quoted-key object entry: `"key": "<match>"`. This is
/// exactly the shape of MediaWiki's captcha config dump
/// (`"wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha"`). Real challenge
/// occurrences never take this shape: widget markup (`class="h-captcha"`,
/// `data-sitekey="..."`) embeds the marker in an HTML attribute value (preceded
/// by `=`), script sources (`hcaptcha.com/1/api.js`) carry a URL prefix before
/// the opening quote, and challenge prose is unquoted. The filter is
/// conservative: anything ambiguous stays a genuine match.
///
/// The scan walks a bounded constant number of bytes backwards from the match,
/// so the Aho-Corasick pass remains the only O(N) walk over the body.
#[inline]
fn is_config_string_match(body: &str, mat: &aho_corasick::Match) -> bool {
    let bytes = body.as_bytes();
    // The match must fill an ENTIRE double-quoted string: `"<match>"`.
    if mat.start() == 0 || bytes[mat.start() - 1] != b'"' || bytes.get(mat.end()) != Some(&b'"') {
        return false;
    }
    // Before the opening quote (whitespace-tolerant) must stand a colon.
    let Some(colon) = skip_back(bytes, mat.start() - 1, is_ws) else {
        return false;
    };
    if bytes[colon] != b':' {
        return false;
    }
    // Before the colon (whitespace-tolerant) must stand the key's closing quote.
    let Some(key_close) = skip_back(bytes, colon, is_ws) else {
        return false;
    };
    if bytes[key_close] != b'"' {
        return false;
    }
    // Walk back over the key's identifier chars to its opening quote; a
    // non-empty identifier key is required (`"": "value"` is not a config
    // entry of any known captcha plugin).
    let Some(key_open) = skip_back(bytes, key_close, is_ident_char) else {
        return false;
    };
    bytes[key_open] == b'"' && key_open + 1 < key_close
}

/// Scan backward from (but excluding) `from` for the first byte NOT satisfying
/// `skip`; `None` when the entire prefix satisfies `skip`.
#[inline]
fn skip_back(bytes: &[u8], from: usize, skip: impl Fn(u8) -> bool) -> Option<usize> {
    let mut i = from;
    while i > 0 {
        i -= 1;
        if !skip(bytes[i]) {
            return Some(i);
        }
    }
    None
}

/// Identifier characters accepted in JS/JSON config keys and variable names.
#[inline]
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// ASCII whitespace predicate (value form for `skip_back`).
#[inline]
fn is_ws(b: u8) -> bool {
    b.is_ascii_whitespace()
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
fn collect_header_evidence(headers: &HashMap<String, String>) -> Vec<WafEvidence> {
    let mut evidences = Vec::new();
    for (name, provider) in WAF_CONTROL_HEADERS {
        if headers.get(*name).is_some() {
            evidences.push(WafEvidence {
                provider,
                tier: WafTier::Fingerprint,
                matched_pattern: name,
                source: EvidenceSource::Header,
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
/// FIX B (body-vs-header granularity): on a 5xx response a Fingerprint match
/// sourced from the BODY ([`EvidenceSource::Body`]) is ubiquitous diagnostic
/// noise — the body analog of the purged `cf-ray` header — and does NOT block,
/// so a genuine transient 5xx retries. Fingerprint evidence sourced from a
/// control HEADER ([`EvidenceSource::Header`], e.g. `cf-mitigated`) signals
/// active mitigation and still blocks on 5xx (RES-01). The distinction is by
/// evidence source, not status — [`is_t2_blocking_status`] is unchanged.
///
/// Entropy challenges are emitted as [`WafTier::Challenge`] evidence only when
/// their own policy (REQ-WAF-06) already decided to block, so they short-circuit
/// here like any other Challenge marker.
fn decide(evidences: &[WafEvidence], ctx: &InspectionContext) -> bool {
    for ev in evidences {
        match ev.tier {
            WafTier::Challenge => return true,
            WafTier::Fingerprint => {
                if !is_t2_blocking_status(ctx.status) {
                    continue;
                }
                // RES-01 extension: on 5xx a bare vendor mention in the BODY is
                // ubiquitous diagnostic noise (the body analog of the purged
                // cf-ray header) — not blocking. Header-sourced T2 (cf-mitigated
                // etc.) signals active mitigation and still blocks.
                if is_5xx(ctx.status) && ev.source == EvidenceSource::Body {
                    continue;
                }
                return true;
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

/// Whether an HTTP status is a 5xx server error (FIX B body-vs-header carve-out).
#[inline]
fn is_5xx(status: Option<u16>) -> bool {
    matches!(status, Some(500..=599))
}

/// Entropy-based challenge detection (REQ-WAF-06).
///
/// Returns evidence ONLY when a rule decides to block (so its presence in a
/// verdict always means "blocked"):
/// - Rule (a): body > 100KB AND entropy > 5.5 b/B → block if status != 200
///   (unknown/degraded counts as != 200) OR `has_genuine_fingerprint` — a
///   Fingerprint in genuine challenge context: a non-config-string body match
///   (#781 — a quoted JS/JSON config value like `"wgConfirmEdit...":"hcaptcha"`
///   is NOT genuine) or a control header (active mitigation). This keeps dense
///   legitimate pages (Wikipedia) from escalating to an obfuscated-WAF block on
///   HTTP 200.
/// - Rule (b): body < 1500B AND > 5 `<script>` tags → block if non-2xx OR
///   (200 + HTML content-type). The 200+HTML case is the "H3 fix" silent
///   challenge and MUST be kept (`discovery.rs` depends on it).
fn entropy_evidence(
    body: &str,
    ctx: &InspectionContext,
    has_genuine_fingerprint: bool,
) -> Option<WafEvidence> {
    // Rule (a): large + high entropy → obfuscated WAF.
    if body.len() > SUSPICIOUS_SIZE_THRESHOLD {
        let entropy = calculate_entropy(body);
        if entropy > ENTROPY_THRESHOLD {
            if ctx.status != Some(200) || has_genuine_fingerprint {
                return Some(WafEvidence {
                    provider: "Obfuscated WAF",
                    tier: WafTier::Challenge,
                    matched_pattern: "high-entropy body (>100KB, >5.5 b/B)",
                    source: EvidenceSource::Body,
                });
            }
            // Informational detection (REQ-WAF-08): high entropy at status 200
            // without a genuine challenge-context fingerprint is logged, not
            // blocked.
            tracing::debug!(
                entropy,
                "high-entropy body at status 200 without genuine challenge context; not blocking"
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
                    source: EvidenceSource::Body,
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
    use crate::error::ScraperError;

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
                    source: EvidenceSource::Body,
                },
                WafEvidence {
                    provider: "Akamai",
                    tier: WafTier::Fingerprint,
                    matched_pattern: "akamai",
                    source: EvidenceSource::Body,
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
        // RES-01: cf-ray and x-sucuri-id purged — ubiquitous trace headers present
        // on 100% of a vendor's traffic carry zero challenge evidence, so treating
        // them as Fingerprint evidence block-correlated every genuine transient 503
        // behind Cloudflare/Sucuri. Only status-correlated control headers remain.
        let names: Vec<&str> = WAF_CONTROL_HEADERS.iter().map(|(n, _)| *n).collect();
        assert!(
            !names.contains(&"x-wordpress"),
            "x-wordpress must be deleted"
        );
        assert!(!names.contains(&"x-cdn"), "x-cdn must be deleted");
        assert!(
            !names.contains(&"cf-ray"),
            "cf-ray trace header must be purged (RES-01)"
        );
        assert!(
            !names.contains(&"x-sucuri-id"),
            "x-sucuri-id trace header must be purged (RES-01)"
        );
        assert!(names.contains(&"x-datadome-response"));
        assert!(names.contains(&"cf-mitigated"));
        assert!(names.contains(&"x-akamai-edge-auth"));
        assert_eq!(
            names.len(),
            3,
            "exactly 3 correlated control headers expected"
        );
    }

    // ========================================================================
    // TASK-03 — Boundary post-filter (REQ-WAF-04)
    // ========================================================================

    #[test]
    fn test_boundary_rejects_trailing_underscore() {
        // Fixture 1 core: bare "akamai" [B] rejected when followed by '_'.
        let (evidences, _) = collect_body_evidence(r#"{"key": "akamai_hash"}"#);
        assert!(
            !evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "bare 'akamai' must be rejected when followed by '_' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_rejects_alphanumeric_prefix() {
        // bare "akamai" [B] rejected when preceded by an alphanumeric byte.
        let (evidences, _) = collect_body_evidence("xakamai");
        assert!(
            !evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "bare 'akamai' must be rejected when preceded by 'x' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_stands_in_prose() {
        // Fixture 4 core: "cloudflare" surrounded by spaces stands (T2 evidence).
        let (evidences, _) = collect_body_evidence("an article mentioning cloudflare in prose");
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
        let (evidences, _) = collect_body_evidence("cookie incap_ses_123 = abc");
        assert!(
            evidences.iter().any(|e| e.matched_pattern == "incap_ses"),
            "[E] 'incap_ses' must match inside 'incap_ses_123' (got {evidences:?})"
        );
    }

    #[test]
    fn test_boundary_t1_exempt() {
        // Challenge-tier patterns are exempt from the boundary filter.
        let (evidences, _) = collect_body_evidence("xxcf-turnstileyy");
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
        let (evidences, _) = collect_body_evidence("ñakamaiñ");
        assert!(
            evidences.iter().any(|e| e.matched_pattern == "akamai"),
            "non-ASCII adjacent bytes are boundaries, 'akamai' stands (got {evidences:?})"
        );
    }

    // ========================================================================
    // RISK-01 — Evidence allocation cap (DoS hardening)
    // ========================================================================

    #[test]
    fn test_body_evidence_capped_at_max_per_body() {
        // RISK-01: adversarial text/html cannot materialize unbounded evidence.
        // Twice-the-cap boundary-clean bare "cloudflare" matches collapse to the
        // cap (the old code returned every match — an uncapped Vec).
        let body = " cloudflare ".repeat(MAX_EVIDENCE_PER_BODY * 2);
        let (evidences, _) = collect_body_evidence(&body);
        assert_eq!(
            evidences.len(),
            MAX_EVIDENCE_PER_BODY,
            "evidence must be capped at MAX_EVIDENCE_PER_BODY"
        );
    }

    #[test]
    fn test_body_evidence_below_cap_collects_all() {
        // Triangulation: the cap is an upper bound, not a target — a body with
        // fewer matches than the cap still collects every one of them.
        let body = " cloudflare  datadome  sucuri ";
        let (evidences, _) = collect_body_evidence(body);
        assert_eq!(
            evidences.len(),
            3,
            "all sub-cap matches collected (got {evidences:?})"
        );
    }

    #[test]
    fn test_t1_marker_visible_past_evidence_cap() {
        // Regression (cap-vs-tier false negative): MAX_EVIDENCE_PER_BODY bounds
        // T2 accumulation, NOT detection. A body carrying a full cap of T2
        // matches FOLLOWED BY a T1 marker must still block at status 200 — the
        // T1 is a definitive verdict that noise density cannot hide. The old
        // code applied `.take(MAX)` to the raw Aho-Corasick iterator, so a T1
        // marker past 64 earlier matches was never examined → false negative.
        let body = format!(
            "{} cf-turnstile ",
            " cloudflare ".repeat(MAX_EVIDENCE_PER_BODY)
        );
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&body, &ctx);
        assert!(
            verdict.is_blocked,
            "T1 marker past the T2 cap must still block at 200 (got {verdict:?})"
        );
        assert!(
            verdict
                .evidences
                .iter()
                .any(|e| e.tier == WafTier::Challenge),
            "the surviving evidence must be the T1 marker (got {:?})",
            verdict.evidences
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
    fn test_inspect_t2_body_at_503_does_not_block() {
        // FIX B: on 5xx a bare vendor mention in the BODY is ubiquitous diagnostic
        // noise (the body analog of the purged cf-ray header) — NOT blocking, so a
        // genuine transient 503 retries instead of dying as an instant WafChallenge.
        let ctx = ctx_with(Some(503), Some("text/html"));
        let verdict = WafInspector::inspect("served by cloudflare", &ctx);
        assert!(
            !verdict.is_blocked,
            "bare body vendor mention (T2 Body) at 503 must NOT block (got {verdict:?})"
        );
        assert!(
            !verdict.evidences.is_empty(),
            "T2 evidence is still collected (informational)"
        );
    }

    #[test]
    fn test_inspect_t2_body_at_403_blocks() {
        // FIX B: 403 is NOT 5xx, so a bare body vendor mention (T2 Body) still
        // blocks — the body-vs-header carve-out applies only to 5xx.
        let ctx = ctx_with(Some(403), Some("text/html"));
        let verdict = WafInspector::inspect("served by cloudflare", &ctx);
        assert!(
            verdict.is_blocked,
            "bare body vendor mention (T2 Body) at 403 must block (got {verdict:?})"
        );
    }

    #[test]
    fn test_inspect_t2_blocking_status_set() {
        // Only 403/429/503/520-529 correlate with T2 evidence. Exercised via a
        // control HEADER (cf-mitigated): header-sourced T2 blocks on every
        // correlated status, including the 5xx ones (RES-01), so this validates
        // is_t2_blocking_status directly. Body-sourced T2 at 5xx is carved out
        // by FIX B — see test_inspect_t2_body_at_503_does_not_block.
        for status in [403u16, 429, 503, 520, 525, 529] {
            let mut headers = HashMap::new();
            headers.insert("cf-mitigated".to_string(), "challenge".to_string());
            let ctx = InspectionContext {
                status: Some(status),
                content_type: Some("text/html".into()),
                headers,
                ..Default::default()
            };
            let verdict = WafInspector::inspect("ordinary body", &ctx);
            assert!(verdict.is_blocked, "T2 header at {status} must block");
        }
        for status in [200u16, 201, 301, 404, 500, 519, 530] {
            let mut headers = HashMap::new();
            headers.insert("cf-mitigated".to_string(), "challenge".to_string());
            let ctx = InspectionContext {
                status: Some(status),
                content_type: Some("text/html".into()),
                headers,
                ..Default::default()
            };
            let verdict = WafInspector::inspect("ordinary body", &ctx);
            assert!(!verdict.is_blocked, "T2 header at {status} must NOT block");
        }
    }

    #[test]
    fn test_collect_body_evidence_source_is_body() {
        // FIX B: body-signature evidence is tagged EvidenceSource::Body so the
        // verdict can carve out 5xx body noise from header-sourced mitigation.
        let (evidences, genuine) =
            collect_body_evidence("an article mentioning cloudflare in prose");
        assert!(
            !evidences.is_empty(),
            "expected body evidence (got {evidences:?})"
        );
        assert!(
            evidences.iter().all(|e| e.source == EvidenceSource::Body),
            "body evidence must be tagged Body (got {evidences:?})"
        );
        assert!(genuine, "prose vendor mention is genuine challenge context");
    }

    #[test]
    fn test_collect_header_evidence_source_is_header() {
        // FIX B: control-header evidence is tagged EvidenceSource::Header so it
        // still blocks on 5xx (active mitigation) unlike body-sourced T2.
        let mut headers = HashMap::new();
        headers.insert("cf-mitigated".to_string(), "challenge".to_string());
        let evidences = collect_header_evidence(&headers);
        assert_eq!(evidences.len(), 1, "expected one header evidence");
        assert!(
            evidences.iter().all(|e| e.source == EvidenceSource::Header),
            "header evidence must be tagged Header (got {evidences:?})"
        );
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
        let mut headers = HashMap::new();
        headers.insert("x-datadome-response".to_string(), "blocked".to_string());
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
        let mut headers = HashMap::new();
        headers.insert("cf-mitigated".to_string(), "challenge".to_string());
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
        // REQ-WAF-01: the verdict carries ALL collected evidences, not first-hit.
        // T2-only body at 403 (a T1 marker would short-circuit the pass; a 5xx
        // status would carve out body-sourced T2 per FIX B): every boundary-clean
        // fingerprint is collected and correlates with the non-5xx WAF status.
        let ctx = ctx_with(Some(403), Some("text/html"));
        let verdict =
            WafInspector::inspect("protected by cloudflare and datadome and perimeterx", &ctx);
        assert!(verdict.is_blocked);
        assert!(
            verdict.evidences.len() >= 3,
            "multiple T2 evidences collected (got {:?})",
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
    // #781 — Config-string false positives (Wikipedia / MediaWiki)
    //
    // A quoted JS/JSON config value — MediaWiki's edit-captcha config dump,
    // e.g. `"wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha"` — is NOT a
    // served challenge. It stays evidence (it feeds the chain under a WAF
    // status), but it is not genuine challenge context: it must not escalate a
    // dense legitimate page into an obfuscated-WAF block at HTTP 200.
    // ========================================================================

    /// Deterministic >100KB high-entropy body (byte cycle ≈ 8.0 b/B), with an
    /// optional fragment appended. Same construction as the existing
    /// entropy-policy tests so no randomness leaks into the suite.
    fn high_entropy_body(fragment: &str) -> String {
        let mut body: String = (0u8..=255)
            .map(|b| b as char)
            .cycle()
            .take(104_000)
            .collect();
        body.push_str(fragment);
        body
    }

    /// MediaWiki-style captcha config excerpt (issue #781 shape, no spaces).
    const MEDIAWIKI_HCAPTCHA_CONFIG: &str = r#"{"wgConfirmEditCaptchaNeededForGenericEdit":"hcaptcha","wgConfirmEditHCaptchaSiteKey":"5d0c670e-4dc5-4e69-a45e-9f5b7c2d11ab"}"#;

    /// Same config with spaces around the colon (pretty-printed variant).
    const MEDIAWIKI_HCAPTCHA_CONFIG_SPACED: &str = r#"{ "wgConfirmEditCaptchaNeededForGenericEdit": "hcaptcha", "wgConfirmEditHCaptchaSiteKey": "5d0c670e-4dc5-4e69-a45e-9f5b7c2d11ab" }"#;

    #[test]
    fn test_config_string_match_json_value_is_config() {
        // #781 core: a full-quoted-string hcaptcha value preceded by a quoted
        // key is a config occurrence — evidence yes, genuine context no.
        for config in [MEDIAWIKI_HCAPTCHA_CONFIG, MEDIAWIKI_HCAPTCHA_CONFIG_SPACED] {
            let (evidences, genuine) = collect_body_evidence(config);
            assert!(
                evidences.iter().any(|e| e.matched_pattern == "hcaptcha"),
                "config occurrences still produce evidence (got {evidences:?})"
            );
            assert!(
                !genuine,
                "quoted-key JSON values are not genuine challenge context ({config})"
            );
        }
    }

    #[test]
    fn test_config_string_match_html_attribute_is_genuine() {
        // `class="hcaptcha"` embeds the marker in an HTML attribute value
        // (preceded by '='), never in JSON value position → genuine.
        let (_, genuine) = collect_body_evidence(r#"<div class="hcaptcha"></div>"#);
        assert!(genuine, "attribute-embedded marker stays genuine");
    }

    #[test]
    fn test_config_string_match_script_src_is_genuine() {
        // `src="https://hcaptcha.com/..."` has a URL prefix before the marker
        // inside the quoted string → genuine challenge signal.
        let (_, genuine) =
            collect_body_evidence(r#"<script src="https://hcaptcha.com/1/api.js"></script>"#);
        assert!(genuine, "script-source marker stays genuine");
    }

    #[test]
    fn test_config_string_match_prose_is_genuine() {
        let (_, genuine) = collect_body_evidence("the site uses hcaptcha for protection");
        assert!(genuine, "unquoted prose mentions stay genuine");
    }

    #[test]
    fn test_config_string_match_embedded_in_string_is_genuine() {
        // The marker must fill the ENTIRE quoted string; embedded mentions are
        // genuine (conservative default of the #781 filter).
        let (_, genuine) = collect_body_evidence(r#"{"msg":"say hcaptcha today"}"#);
        assert!(genuine, "marker embedded inside a string stays genuine");
    }

    #[test]
    fn test_mediawiki_hcaptcha_config_dense_200_not_blocked() {
        // Issue #781 fixture (a): MediaWiki config with `hcaptcha` config
        // strings + >100KB dense body + HTTP 200 → NOT blocked. The config
        // occurrence is still reported as (non-genuine) fingerprint evidence.
        let body = high_entropy_body(MEDIAWIKI_HCAPTCHA_CONFIG);
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&body, &ctx);
        assert!(
            !verdict.is_blocked,
            "config-string fingerprint must not escalate a 200 page (got {verdict:?})"
        );
        assert!(
            verdict
                .evidences
                .iter()
                .any(|e| e.matched_pattern == "hcaptcha" && e.tier == WafTier::Fingerprint),
            "the config occurrence is still collected as evidence (got {:?})",
            verdict.evidences
        );
    }

    #[test]
    fn test_real_hcaptcha_widget_dense_200_blocks() {
        // Issue #781 fixture (b): a REAL served hCaptcha widget on a large
        // body at HTTP 200 still blocks (T1 challenge marker, any status).
        let body = high_entropy_body(
            r#"<div class="h-captcha" data-sitekey="5d0c670e"></div>
              <script src="https://hcaptcha.com/1/api.js"></script>"#,
        );
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&body, &ctx);
        assert!(
            verdict.is_blocked,
            "real hCaptcha widget markup must still block at 200"
        );
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("hCaptcha"),
            "the challenge marker drives the block (got {:?})",
            verdict.evidences
        );
    }

    #[test]
    fn test_config_string_hcaptcha_403_blocks() {
        // Issue #781 fixture (c): the SAME config-only body at a WAF status
        // (403) still blocks — config evidence correlates with the challenge
        // status, so nothing about detection weakens.
        let ctx = ctx_with(Some(403), Some("text/html"));
        let verdict = WafInspector::inspect(MEDIAWIKI_HCAPTCHA_CONFIG, &ctx);
        assert!(
            verdict.is_blocked,
            "config fingerprint at 403 must still block"
        );
        assert!(
            verdict
                .evidences
                .iter()
                .any(|e| e.matched_pattern == "hcaptcha"),
            "the evidence chain carries the hcaptcha fingerprint (got {:?})",
            verdict.evidences
        );
    }

    #[test]
    fn test_genuine_fingerprint_still_escalates_entropy_200() {
        // Triangulation for the #781 gate: the entropy rule keeps escalating at
        // 200 when the coexisting fingerprint is GENUINE (prose marker here),
        // distinguishing it from the config-string case above.
        let body = high_entropy_body(" protected by cloudflare edge ");
        let ctx = ctx_with(Some(200), Some("text/html"));
        let verdict = WafInspector::inspect(&body, &ctx);
        assert!(
            verdict.is_blocked,
            "genuine T2 coexistence with high entropy must still block at 200"
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
                source: EvidenceSource::Body,
            },
            WafEvidence {
                provider: "Akamai",
                tier: WafTier::Fingerprint,
                matched_pattern: "akamai",
                source: EvidenceSource::Body,
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

    // ========================================================================
    // TASK-10 — Public verdict evidence-chain accessor (REQ-WAF-08)
    // ========================================================================

    #[test]
    fn test_verdict_evidence_chain_lists_all_evidences() {
        // REQ-WAF-08: callers (client/scraper_service/discovery/MCP) format the
        // Spanish evidence chain straight from the verdict they received.
        let verdict = WafVerdict {
            is_blocked: true,
            evidences: vec![
                WafEvidence {
                    provider: "Cloudflare",
                    tier: WafTier::Challenge,
                    matched_pattern: "cf-turnstile",
                    source: EvidenceSource::Body,
                },
                WafEvidence {
                    provider: "Akamai",
                    tier: WafTier::Fingerprint,
                    matched_pattern: "akamai",
                    source: EvidenceSource::Body,
                },
            ],
        };
        let chain = verdict.evidence_chain();
        assert!(chain.contains("Cloudflare"), "chain: {chain}");
        assert!(chain.contains("cf-turnstile"), "chain: {chain}");
        assert!(
            chain.contains("desafío"),
            "Challenge Spanish label: {chain}"
        );
        assert!(chain.contains("Akamai"), "chain: {chain}");
        assert!(
            chain.contains("huella"),
            "Fingerprint Spanish label: {chain}"
        );
    }

    #[test]
    fn test_verdict_evidence_chain_clean_is_fallback() {
        // A clean verdict (no evidence) renders the generic fallback label.
        let verdict = WafVerdict::clean();
        assert_eq!(verdict.evidence_chain(), "WAF desconocido");
    }

    // ========================================================================
    // TASK-11 — InspectionContext from lowercased-key header maps (REQ-WAF-01)
    // ========================================================================

    #[test]
    fn test_from_lowercase_headers_builds_full_context() {
        // scraper_service / discovery capture headers as lowercased-key
        // HashMaps; the constructor lifts status + content-type and converts
        // the map into a wreq HeaderMap for control-header evidence.
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "text/html; charset=utf-8".to_string(),
        );
        headers.insert("cf-mitigated".to_string(), "challenge".to_string());

        let ctx = InspectionContext::from_lowercase_headers(200, &headers, false);

        assert_eq!(ctx.status, Some(200));
        assert_eq!(
            ctx.content_type.as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert!(!ctx.ignore_waf);
        // Both headers survive the conversion (control header is T2 evidence).
        assert!(ctx.headers.contains_key("cf-mitigated"));
        assert!(ctx.headers.contains_key("content-type"));
    }

    #[test]
    fn test_from_lowercase_headers_missing_content_type_is_none() {
        // No content-type entry → None (the REQ-WAF-02 gate then scans the body).
        let headers = std::collections::HashMap::new();
        let ctx = InspectionContext::from_lowercase_headers(404, &headers, true);
        assert_eq!(ctx.status, Some(404));
        assert!(ctx.content_type.is_none());
        assert!(ctx.ignore_waf, "ignore_waf flag must propagate");
        assert!(ctx.headers.is_empty());
    }

    #[test]
    fn test_from_lowercase_headers_drives_control_header_evidence() {
        // End-to-end: a control header supplied via the lowercased map is
        // collected as Fingerprint evidence (never auto-blocking at 200).
        let mut headers = std::collections::HashMap::new();
        headers.insert("x-datadome-response".to_string(), "1".to_string());
        let ctx = InspectionContext::from_lowercase_headers(200, &headers, false);

        let verdict = WafInspector::inspect("<html>clean body</html>", &ctx);
        assert!(!verdict.is_blocked, "T2 header at 200 must not block");
        assert!(
            verdict
                .evidences
                .iter()
                .any(|e| e.matched_pattern == "x-datadome-response"),
            "control header must be collected as evidence"
        );
    }

    #[test]
    fn test_verify_integrity_error_carries_evidence_chain() {
        // REQ-WAF-08: a blocked verdict exposes the Spanish evidence chain.
        let verdict = WafInspector::inspect("Just a moment...", &InspectionContext::default());
        assert!(verdict.is_blocked, "T1 prose must block");
        let chain = verdict.evidence_chain();
        assert!(chain.contains("Cloudflare"), "chain provider: {chain}");
        assert!(chain.contains("patrón:"), "chain pattern label: {chain}");
        assert!(chain.contains("tier:"), "chain tier label: {chain}");
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
                source: EvidenceSource::Body,
            }]),
        );
        assert_eq!(err.classify(), crate::error::ErrorClass::PermanentFatal);
    }

    // ========================================================================
    // Body-only degraded inspection tests (via inspect, ported from waf.rs)
    // ========================================================================

    #[test]
    fn test_detect_body_cloudflare_turnstile() {
        let html = r#"<div id="cf-turnstile" data-sitekey="abc123"></div>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Cloudflare Turnstile")
        );
    }

    #[test]
    fn test_detect_body_cloudflare_just_a_moment() {
        let html = "<html><body><h1>Just a moment...</h1></body></html>";
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Cloudflare")
        );
    }

    #[test]
    fn test_detect_body_cloudflare_checking_browser_degraded_does_not_block() {
        // "Checking your browser" is Fingerprint-tier: in degraded mode (no status)
        // it must NOT block (REQ-WAF-09). It is collected as evidence but the
        // verdict is not blocked without a correlated WAF status.
        let html = "<html><body>Checking your browser before accessing...</body></html>";
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(!verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Cloudflare")
        );
        assert_eq!(
            verdict.evidences.first().map(|e| e.tier),
            Some(WafTier::Fingerprint)
        );
    }

    #[test]
    fn test_detect_body_cloudflare_checking_browser_blocks_with_waf_status() {
        // Fingerprint-tier blocks when correlated with a WAF status (403/429/503/520-529).
        let html = "<html><body>Checking your browser before accessing...</body></html>";
        let ctx = InspectionContext {
            status: Some(403),
            ..Default::default()
        };
        let verdict = WafInspector::inspect(html, &ctx);
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Cloudflare")
        );
    }

    #[test]
    fn test_detect_body_recaptcha() {
        let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=abc"></script>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("reCAPTCHA")
        );
    }

    #[test]
    fn test_detect_body_g_recaptcha() {
        let html = r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("reCAPTCHA")
        );
    }

    #[test]
    fn test_detect_body_hcaptcha() {
        let html = r#"<div class="h-captcha" data-sitekey="abc"></div>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("hCaptcha")
        );
    }

    #[test]
    fn test_detect_body_datadome() {
        // DataDome's own T1 marker (dd-captcha) is the decisive evidence. The old
        // fixture mixed `captcha.js` (a Generic Challenge T1) with DataDome T2
        // names; the T1 short-circuit reports the decisive challenge marker, so
        // the body uses a DataDome-specific T1 to assert DataDome attribution.
        let html = r#"<div id="dd-captcha">challenge</div>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("DataDome")
        );
    }

    #[test]
    fn test_detect_body_perimeterx() {
        let html = r#"<script>var _pxCaptcha = {};</script>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("PerimeterX")
        );
    }

    #[test]
    fn test_detect_body_akamai() {
        // `_abck` is a Fingerprint-tier ([E]) marker. In degraded mode T2
        // evidence never blocks, so this is not reported as a block — the
        // false positive that motivated issue #346.
        let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(
            !verdict.is_blocked,
            "T2 evidence must not block in degraded mode"
        );
    }

    #[test]
    fn test_detect_body_generic_challenge() {
        let html = "<p>Please verify you are a human to continue.</p>";
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Generic Challenge")
        );
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
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_detect_body_empty() {
        let verdict = WafInspector::inspect("", &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_detect_body_aws_waf_cookie_domain_list() {
        let html = r#"<script>window.awsWafCookieDomainList = [];</script>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("AWS WAF")
        );
    }

    #[test]
    fn test_detect_body_aws_waf_integration() {
        let html = r#"<script>AwsWafIntegration.saveReferrer();</script>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("AWS WAF")
        );
    }

    #[test]
    fn test_detect_body_aws_waf_goku_props() {
        let html = r#"<script>window.gokuProps = {"key":"AQIDAH..."};</script>"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("AWS WAF")
        );
    }

    #[test]
    fn test_detect_body_aws_waf_token() {
        let html = r#"<meta name="aws-waf-token" content="abc123">"#;
        let verdict = WafInspector::inspect(html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("AWS WAF")
        );
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
        let verdict = WafInspector::inspect(&high_entropy_content, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert_eq!(
            verdict.evidences.first().map(|e| e.provider),
            Some("Obfuscated WAF")
        );
    }

    #[test]
    fn test_detect_body_small_low_entropy() {
        let small_content = "<html><body>Redirecting...</body></html>";
        let verdict = WafInspector::inspect(small_content, &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    // ========================================================================
    // Headers + body degraded inspection tests (via inspect)
    // ========================================================================

    #[test]
    fn test_waf_control_header_detection() {
        // Control headers are Fingerprint-tier evidence — mere presence never
        // auto-blocks (correction B). Degraded mode (no status), so a T2 header
        // alone is clean.
        let mut headers = HashMap::new();
        headers.insert("x-datadome-response".to_string(), "blocked".to_string());
        let ctx = InspectionContext {
            headers,
            ..Default::default()
        };
        let verdict = WafInspector::inspect("normal content", &ctx);
        assert!(
            !verdict.is_blocked,
            "T2 header alone must not block (degraded)"
        );

        // cf-ray alone also doesn't trigger (common in normal requests).
        let mut headers = HashMap::new();
        headers.insert("cf-ray".to_string(), "abc123".to_string());
        let ctx = InspectionContext {
            headers,
            ..Default::default()
        };
        let verdict = WafInspector::inspect("normal content", &ctx);
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_waf_body_signature_detection() {
        // Test Cloudflare detection
        let verdict = WafInspector::inspect("Just a moment...", &InspectionContext::default());
        assert!(verdict.is_blocked);

        // Test reCAPTCHA detection
        let verdict =
            WafInspector::inspect("<div class='g-recaptcha'>", &InspectionContext::default());
        assert!(verdict.is_blocked);

        // Test normal content passes
        let verdict = WafInspector::inspect(
            "<html><body><p>Hello World</p></body></html>",
            &InspectionContext::default(),
        );
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_silent_challenge_detection() {
        let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
        let verdict = WafInspector::inspect(body, &InspectionContext::default());
        assert!(verdict.is_blocked);

        let body = "<html><body><p>Hello</p></body></html>";
        let verdict = WafInspector::inspect(body, &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_aho_corasick_performance() {
        let body = "This is a page with Just a moment... and recaptcha/api.js content";
        let verdict = WafInspector::inspect(body, &InspectionContext::default());
        assert!(verdict.is_blocked);
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
