# ADR 0015: WAF Detection Stays Infrastructure-Side — #994 Slice 4 Superseded by the Sealed-Port Design

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** Project Architect (sole orchestrator, delegated authority)
- **Related:** issue #994 (sub-slice 4), issues #1039, #996, ADR-0012-B, PR #1135 (#1100 lint hardening)

## Context

Issue #994 sub-slice 4 specified (80L, 2 files): move the WAF Aho-Corasick
inspect logic "fully into `domain::waf`" as an
`inspect_with_evidence` method on the port, leaving
`infrastructure::http::waf_engine::WafInspector` as a thin trait impl. The
estimate was written when `domain::waf::WafInspector` delegated to infra via a
fully qualified path — the lint-bypass WARNING 4 of the #993 verify report.

Verified state at main @ `ac7f3e90`:

- The delegating shim no longer exists (removed by the #1100-era hardening).
  `domain/` contains **zero** code references to `crate::infrastructure`
  (only doc comments, all of which point inward legally).
- `domain::waf` (issue #996/#1039 design) owns the full VO family
  (`WafVerdict` — which already carries `evidences: Vec<WafEvidence>`, the
  sealed `WafInspectorPort`, `InspectionContext`, tiers) plus the
  process-wide `OnceLock<Arc<dyn WafInspectorPort>>` seam.
- `infrastructure::http::waf_engine` (2402L) implements the port with the AC
  automaton, signature tables, and detection logic — infrastructure → domain
  is the allowed inward direction. The only external constructors of the
  concrete are the DI root (`application/container.rs`, allowlisted,
  permanent) and three application **test** modules that install the real
  inspector for tests.
- The strict intra-crate gate is green at allowlisted 28 (entries: 2).

## Decision

Slice 4 is **closed as superseded — its intent is already met by a better
design**. The automaton stays in `infrastructure::http::waf_engine`.

Rationale, as architecture rather than as an accounting exercise:

1. **The warning the slice targeted is gone.** WARNING 4 was a domain→infra
   qualified-path bypass; the bypass no longer exists in any production code.
2. **Moving the engine would be a downgrade, not the specified "move".** The
   literal AC ("AC logic fully in `domain::waf`") would drag ~2400L of
   signature tables plus the `aho-corasick` dependency into `domain`,
   contradicting the issue's own 80L/2-file budget and coupling the domain
   layer to a detection-mechanics crate. The sealed-port seam (#996) achieves
   the same goal — domain never names infra, application only sees the port —
   while keeping detection mechanics where the I/O-adjacent machinery lives.
3. **The requested API already exists implicitly.** `inspect_with_evidence`
   was proposed because the verdict lacked evidence; `WafVerdict` has carried
   `evidences: Vec<WafEvidence>` since #1039.
4. **Systemic discipline:** a slice whose specification is obsolete and whose
   remaining work would be 2000L of churn with zero caller-facing benefit is
   closed, not implemented (systemic-issue-triage: fixes must shrink the
   system, not shuffle it).

## Consequences

- Issue #994's remaining open scope reduces to sub-slice 3 (persistence +
  loom, delivered via #1140/#1150) and this disposition. Sub-slices 1-2 were
  delivered by #1128/#1135; sub-slice 5 (ADR-0012) landed earlier.
- The `domain::waf` module doc already records the final architecture ("the
  infrastructure engine keeps the Aho-Corasick automaton... Application
  imports only this module") — no doc change needed.
- The three application test-module `use` lines that construct the real
  inspector are the same legitimate exception as the DI root (constructing
  the concrete for wiring); they are not production references and the gate
  is green with them.

## Alternatives rejected

- **Literal implementation of the slice** (move automaton + signature tables
  into `domain::waf`): couples domain to `aho-corasick`, exceeds the budget
  by ~25x, and duplicates the seam that #996 already established.
- **Closing #994 entirely at slice-4 completion**: sub-slice 3 verification
  evidence and the final re-baseline belong on the issue first; the umbrella
  closes when its comment records the full state.
