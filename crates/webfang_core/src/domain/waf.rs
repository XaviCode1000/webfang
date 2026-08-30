//! WAF domain port — VOs + sealed inspection trait.
//!
//! Domain-owned value objects for WAF detection. The infrastructure layer
//! (`infrastructure::http::waf_engine`) keeps the Aho-Corasick automaton
//! and implements [`WafInspectorPort`]. Application imports only this
//! module.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// WAF signature tier — drives blocking policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WafTier {
    /// Unambiguous challenge/captcha marker — blocks at any status.
    Challenge,
    /// Fingerprint marker — blocks only with WAF-correlated status.
    Fingerprint,
}

impl WafTier {
    /// Spanish user-facing label for the evidence chain.
    #[must_use]
    pub const fn label_es(self) -> &'static str {
        match self {
            Self::Challenge => "desafío",
            Self::Fingerprint => "huella",
        }
    }
}

/// Where a piece of WAF evidence was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    /// Evidence matched in the response body.
    Body,
    /// Evidence matched in a response control header.
    Header,
}

/// A single piece of WAF detection evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WafEvidence {
    /// Detected WAF provider (e.g. `"Cloudflare"`).
    pub provider: &'static str,
    /// Signature tier that matched.
    pub tier: WafTier,
    /// The literal pattern that matched.
    pub matched_pattern: &'static str,
    /// Where the evidence was observed.
    pub source: EvidenceSource,
}

/// Verdict of a WAF inspection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WafVerdict {
    /// Whether the response should be treated as WAF-blocked.
    pub is_blocked: bool,
    /// All collected evidence.
    pub evidences: Vec<WafEvidence>,
}

impl WafVerdict {
    /// A clean verdict: not blocked, no evidence.
    #[must_use]
    pub fn clean() -> Self {
        Self::default()
    }

    /// Spanish evidence chain for user-facing errors.
    #[must_use]
    pub fn evidence_chain(&self) -> String {
        if self.evidences.is_empty() {
            return "WAF desconocido".to_string();
        }
        self.evidences
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
}

/// HTTP context for a WAF inspection (domain pure, no `wreq::HeaderMap`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectionContext {
    /// HTTP status code, if known.
    pub status: Option<u16>,
    /// Content-Type header value, if known.
    pub content_type: Option<String>,
    /// Response headers (lowercased keys).
    pub headers: HashMap<String, String>,
    /// Bypass WAF detection entirely.
    pub ignore_waf: bool,
}

impl InspectionContext {
    /// Build a full context from status and lowercased header map.
    #[must_use]
    pub fn from_lowercase_headers(
        status: u16,
        headers: &HashMap<String, String>,
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

#[allow(missing_docs)]
pub mod sealed {
    #[allow(missing_docs)]
    pub trait Sealed {}
}

/// Domain port for WAF inspection — sealed.
/// Only the infrastructure `WafInspector` may implement it.
pub trait WafInspectorPort: Send + Sync + sealed::Sealed {
    /// Inspect a response body with full HTTP context and return a verdict.
    fn inspect(&self, body: &str, ctx: &InspectionContext) -> WafVerdict;
}

/// Process-wide WAF inspector instance, populated by the composition root
/// (CLI / MCP / test harness) at startup and read by application-layer call
/// sites that need WAF detection but cannot reach into the infrastructure
/// layer (issue #996).
///
/// The static is typed as the **domain** trait [`WafInspectorPort`] so the
/// domain layer never names the infrastructure concrete — the only
/// `crate::infrastructure` reference lives in the composition root that
/// constructs the `Arc<dyn WafInspectorPort>` and hands it to
/// [`set_waf_inspector`].
static WAF_INSPECTOR: OnceLock<Arc<dyn WafInspectorPort>> = OnceLock::new();

/// Install the process-wide WAF inspector. Idempotent: a second call is a
/// no-op when the first value is already set. Called by the composition
/// root (CLI / MCP / test harness) at startup.
pub fn set_waf_inspector(inspector: Arc<dyn WafInspectorPort>) {
    let _ = WAF_INSPECTOR.set(inspector);
}

/// Read the process-wide WAF inspector. When the composition root has not
/// installed one, falls back to a clean-verdict no-op stub so application
/// code paths that incidentally reach WAF inspection (e.g. an HTTP client
/// fetching a 500 with no challenge markers) keep working in tests that
/// did not go through [`crate::application::container::Container::new`].
///
/// The fallback yields a clean verdict for any input, which is semantically
/// a no-op: real WAF detection is opt-in via [`set_waf_inspector`], and
/// any test or production call site that needs real detection initializes
/// the static at startup. See `application::container::Container::new`,
/// which installs the real infrastructure impl.
#[must_use]
pub fn waf_inspector() -> Arc<dyn WafInspectorPort> {
    if let Some(inspector) = WAF_INSPECTOR.get() {
        return inspector.clone();
    }
    static FALLBACK: OnceLock<Arc<dyn WafInspectorPort>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| Arc::new(CleanWafInspector) as Arc<dyn WafInspectorPort>)
        .clone()
}

/// Clean-verdict fallback used when no composition root has installed the
/// real inspector. Lives in `domain` because it is a domain-layer fallback
/// (no infrastructure dependency) — the real impl lives behind the
/// composition root, not here.
struct CleanWafInspector;

impl sealed::Sealed for CleanWafInspector {}

impl WafInspectorPort for CleanWafInspector {
    fn inspect(&self, _body: &str, _ctx: &InspectionContext) -> WafVerdict {
        WafVerdict::clean()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn inspection_context_default_is_degraded() {
        let ctx = InspectionContext::default();
        assert!(ctx.status.is_none());
        assert!(ctx.content_type.is_none());
        assert!(ctx.headers.is_empty());
        assert!(!ctx.ignore_waf);
    }

    #[test]
    fn inspection_context_from_lowercase_headers_extracts_content_type() {
        let mut h = HashMap::new();
        h.insert("content-type".to_string(), "text/html".to_string());
        h.insert("x-datadome-response".to_string(), "1".to_string());
        let ctx = InspectionContext::from_lowercase_headers(200, &h, false);
        assert_eq!(ctx.status, Some(200));
        assert_eq!(ctx.content_type, Some("text/html".to_string()));
        assert_eq!(
            ctx.headers.get("x-datadome-response"),
            Some(&"1".to_string())
        );
        assert!(!ctx.ignore_waf);

        // Second case: ignore flag true, different status.
        let mut h2 = HashMap::new();
        h2.insert("content-type".to_string(), "application/json".to_string());
        let ctx2 = InspectionContext::from_lowercase_headers(403, &h2, true);
        assert_eq!(ctx2.status, Some(403));
        assert!(ctx2.ignore_waf);
        assert_eq!(ctx2.content_type, Some("application/json".to_string()));
    }

    #[test]
    fn waf_verdict_clean_is_not_blocked() {
        let v = WafVerdict::clean();
        assert!(!v.is_blocked);
        assert!(v.evidences.is_empty());
        assert_eq!(v.evidence_chain(), "WAF desconocido");
    }

    #[test]
    fn waf_verdict_carries_all_evidences_and_formats_spanish_chain() {
        let v = WafVerdict {
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
                    source: EvidenceSource::Header,
                },
            ],
        };
        assert_eq!(v.evidences.len(), 2);
        let chain = v.evidence_chain();
        assert!(chain.contains("Cloudflare"));
        assert!(chain.contains("cf-turnstile"));
        assert!(chain.contains("desafío"));
        assert!(chain.contains("Akamai"));
        assert!(chain.contains("huella"));
        assert!(chain.contains("; "));
    }

    #[test]
    fn waf_tier_label_es_spanish() {
        assert_eq!(WafTier::Challenge.label_es(), "desafío");
        assert_eq!(WafTier::Fingerprint.label_es(), "huella");
    }

    #[test]
    fn evidence_source_distinguishes_body_header() {
        let body = WafEvidence {
            provider: "DataDome",
            tier: WafTier::Fingerprint,
            matched_pattern: "datadome",
            source: EvidenceSource::Body,
        };
        let header = WafEvidence {
            provider: "DataDome",
            tier: WafTier::Fingerprint,
            matched_pattern: "x-datadome-response",
            source: EvidenceSource::Header,
        };
        assert_ne!(body.source, header.source);
        assert_eq!(body.tier, WafTier::Fingerprint);
    }

    #[test]
    fn waf_inspector_port_is_object_safe_via_sealed() {
        struct FakeInspector;
        impl sealed::Sealed for FakeInspector {}
        impl WafInspectorPort for FakeInspector {
            fn inspect(&self, _body: &str, _ctx: &InspectionContext) -> WafVerdict {
                WafVerdict::clean()
            }
        }
        fn assert_dyn(_: &dyn WafInspectorPort) {}
        let fake = FakeInspector;
        assert_dyn(&fake);
        // Second call with different body produces clean as well.
        let ctx = InspectionContext::default();
        let v = fake.inspect("hello world", &ctx);
        assert!(!v.is_blocked);
    }
}
