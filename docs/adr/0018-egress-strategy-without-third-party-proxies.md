# ADR 0018: Egress Strategy Without Third-Party Proxies — Security Invariants Fixed Now, Physical Packaging Deferred Behind an Observable Trigger

- **Status:** Proposed — draft for owner review. Authorizes no code and no delivery.
- **Date:** 2026-09-21
- **Deciders:** Xavi (owner — decisions 1–4 and the four risk resolutions), orchestrator
  review (surfaced and adjudicated risks 1–4 in-thread)
- **Related:** ADR-0010 / 0011 / 0012-B (intra-crate direction), ADR-0013 (`StateStorePort`,
  `ScraperError` reuse precedent), ADR-0015 (WAF stays in infra — the `WafVerdict` this
  strategy consumes), ADR-0017 (`CrawlSession` run owner)
- **Docs:** `docs/ssrf-layers.md` (the posture preserved by decision 1),
  `docs/error-classification-matrix.md`, `AGENTS.md` §Fetch guard-chain
- **Issues:** gate-fix issue (decision 4), layer-hygiene issue (context §3), Fase 1–3 issues
  (downstream, not opened by this ADR)
- **Supersedes:** —

## Scope note (read before the rest)

This ADR records **four decisions and four consequences**. Implementation detail is not a
decision and does not live here:

| Lives elsewhere | Where |
|---|---|
| Physical packaging code (feature vs. crate vs. copy) | Fase 3 issue, when opened |
| `DomainError`/`ErrorClass` swap mechanics | Layer-hygiene issue |
| Full per-tier rate limiter design | Fase 1 issue |
| `ReputationStore` trait, error type, DI wiring | Fase 1 issue |
| Inter-crate gate fix design | Gate-fix issue (appendix A is input, **not** a decision) |

## 1. Context

### 1.1 The problem

WebFang competes against integrated scraping services whose price includes managed
residential proxy pools. WebFang plus an external proxy provider is cost-prohibitive
against that bundled price. This strategy removes the third-party proxy dependency for the
bulk of targets by stratifying egress across controls the operator owns.

The inherited constraint is not negotiable: WebFang treats every outbound socket as attack
surface (`docs/ssrf-layers.md`, four layers). Any egress feature must pass the same
scrutiny as the existing fetch path.

### 1.2 The four risks that made this an architectural decision

Adjudicated in review before this ADR. Each is a consequence section below, not a decision:

1. **Guard-chain position.** The egress tier decision changes which socket is dialled, so
   it cannot sit after rate limiting (stage 2) — pacing budget would be burned on requests
   destined for a different route. It enters between stage 1 (`ValidUrl`) and stage 2.
2. **IPv6 null-route.** Hosting providers that null-route a `/64` fail slowly, not loudly.
   Happy-eyeballs delay is ~200-300 ms per attempt, so a 1,000-URL crawl against a dead
   prefix accumulates ~300 s before a global circuit breaker acts.
3. **CAPTCHA solver contract.** "Async queue" is ambiguous: it can mean the caller polls,
   or the solver has internal concurrency control. These are different designs.
4. **Multi-VPS trust boundary.** "Worker does only fetch" plus "worker is auditable" is a
   contradiction, because the fetch guard-chain *is* WebFang's business logic.

### 1.3 Verified state at `main` @ `4f852094`

Measured, not assumed. Two findings changed the shape of this ADR.

**Finding 1 — the minimal extraction set is not minimal.** The candidate set for a shared
network-types crate, and what it drags:

| File | Lines | Drags |
|---|---|---|
| `domain/ssrf_guard.rs` | 966 | `domain::downloader_factory` (178) |
| `domain/value_objects.rs` | 740 | `crate::ScraperError` → `src/error.rs` (**2,003**) |
| `domain/downloader_port.rs` | 253 | `domain::CrawlError` (**1,005**), `error::ErrorClass` (47) |
| `domain/http_config.rs` | 241 | `downloader_factory` const, `domain::profile` (89) |

2,200 direct + ~4,950 transitive ≈ **7,150 lines**, and `src/error.rs` depends on
`wreq::Error` — so the "lightweight worker crate" would compile wreq anyway, defeating the
point. Two names in the originally proposed set do not exist: `http_config.rs` exports
`HttpClientConfig` and `UnknownProfileError`; the TLS profile lives in `domain/profile.rs`.
(`HttpConfig` and `TlsProfile` have zero `use` sites repo-wide.)

**Finding 2 — both dependency gates are blind to a new crate.** Probe of
`scripts/check_dependency_direction.sh`'s awk, unmodified:

```text
webfang_net_types  -> extracted: [EMPTY]
webfang_egress     -> extracted: [EMPTY]
webfang_test_utils -> extracted: [EMPTY]
webfang_core       -> extracted: [webfang_core]   <- control, positive
```

Details in appendix A. The consequence for this ADR: **a deferral trigger that nothing can
observe is decoration, not discipline.** Decision 4 exists because decision 2 depends on it.

**Finding 3 — no extraction precedent exists; intra-crate moves do.** `eef19afc`
("Phase 4 — decompose into 5-crate workspace", 2026-07-12) shows **zero renames**: the
sibling crates were copied out of a single `src/`, not extracted. Outbound extraction with
a re-export shim is an untried pattern in this repo. Intra-core moves are well-proven
(`6fa0b25e`, `38bd1ddf`, `e428dcdf` at R098–R100 — the ADR-0010/0012 work).

**Usage counts** (symbol-level, whole repo) — relevant because re-export makes churn zero:
`ValidUrl` 41 uses/39 files, `Downloader` 17/16, `DownloadError` 14/14, `ssrf_guard` 12/9,
`FetchedPage` 10/10, `Cookie` 9/8, `SsrfGuard` 1/1 — **58 unique files** for the five core
symbols.

### 1.4 Sequencing fact that makes deferral cheap

The egress worker is needed by **Fase 3 (Multi-VPS) only** — priority P2, explicitly
optional, planned for week 13+ of the PRD. Fase 1 (Reputation DB) and Fase 2 (IPv6 pool)
need no new crate. Deciding physical packaging now, for a phase that may never be funded,
is architecture paid in advance.

### 1.5 Correction to the PRD's references

The PRD cites "ADR-0016 (SSRF residual)". ADR-0016 is the *interruption/resume lifecycle*;
its "residual" is F-R3-7, unrelated to SSRF. The SSRF posture is `docs/ssrf-layers.md`
(written for #1294 / P6-3) and there is **no SSRF ADR**. This ADR is the first ADR to take a
position on SSRF. Note also the repo already carries two files numbered 0016
(`0016-interruption-resume-lifecycle.md`, `0016-post-load-wait-policy.md`) — numbering is
not collision-free in this repo; this file takes 0018 and that is the end of it here.

## 2. Decision

Four decisions. Two are fixed now; two fix an invariant now and defer the mechanism behind
a trigger that decision 4 makes observable.

### Decision 1 — The egress worker validates SSRF itself. Fixed now, no deferral.

An egress worker **must** run the SSRF guard at its own socket dial. It never relies on the
orchestrator having validated the URL before dispatch.

Rationale: option "orchestrator validates, worker trusts" assumes *private network equals
trust boundary*. It does not. A worker that fetches unvalidated targets is an egress proxy
into the operator's private network — precisely the scenario `docs/ssrf-layers.md` exists to
prevent. The safety property is structural, not procedural.

Consequences for the Fase 3 design (invariant, not mechanism): the worker's fetch path
enforces stage 3c (resolver-level rejection at dial) independently of anything the
orchestrator did. mTLS and WireGuard authenticate *peers*; they do not validate *targets*.

### Decision 2 — Physical packaging of the guard is deferred behind an observable trigger.

How the validated guard is shipped — new crate (`webfang_net_types` / `webfang_egress`),
feature subset in `webfang_core`, or a minimal vendored copy — is **not decided here**.

- **Trigger:** the first commit of Fase 3.
- **Rejected as permanent:** a `ssrf-only` *subset* flag in `webfang_core`. Capability flags
  (`ai`, `chromium`, `persistence`, `mcp`) add capacity to a coherent crate; a subset flag
  carves a fragment out of one. Every future module then owes an answer to "does this
  belong in `ssrf-only`?", which is permanent `#[cfg]` noise, a docs.rs page describing a
  configuration nobody builds, and an allow-matrix that still reads "webfang_core depends on
  nothing" while the compiled artifact is a different crate. A gate that stays green while
  reality changed is the worst failure mode available.
- **Rejected as this-cycle work:** full outbound extraction (~2,500–3,000 lines after the
  §3 decoupling, ~7,150 before it) for an optional phase, using a pattern the repo has never
  exercised.
- **Re-entry condition:** when the trigger fires, the packaging decision is made fresh, with
  §1.3's measurements in hand and the gate fixed by decision 4.

### Decision 3 — The `Downloader` port must guarantee guard-chain stage 1. Invariant now, mechanism deferred.

Verified today:

```rust
fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>>;
```

The port accepts a raw `url::Url`. Stage 1 of the mandatory guard-chain is not expressed in
the type. `ValidUrl` is built at the argv boundary by convention, and every current caller
honours it — but a new consumer can pass a raw `Url` and the compiler is silent. For a
remote worker that receives a URL over a tunnel and hands it to a downloader, this is the
concrete hole.

- **Invariant (fixed now):** every `Downloader` implementation must guarantee stage 1, either
  by type (`&ValidUrl`) or by a verified contract.
- **Mechanism (deferred to decision 2's trigger):** narrowing the signature is breaking —
  17 `use` sites across 16 files, plus test mocks, benchmarks, and the MCP wrapper. It gets
  its own issue, not a footnote.
- **Interim (does not wait for the trigger):** a per-implementor contract test, required as
  part of the Fase 1 issue — Fase 1 is the first real consumer of the `Downloader` port
  post-egress, and the test is defense-in-depth justified by that consumption, not by the
  layer-hygiene work of decision 4. For each `impl Downloader`, assert that a raw `Url`
  targeting private space does not connect. This is the same technique `docs/ssrf-layers.md` uses where types
  do not reach: explicit verification. It generalises the existing
  `crates/webfang_core/tests/ssrf_rfc1918_e2e_test.rs` per implementor, loopback/wiremock
  only.

### Decision 4 — The inter-crate gate must be fixed before the trigger can be relied on.

This ADR decides that **decision 2's trigger must be observable**. Today it is not:
appendix A documents two independent failures in
`scripts/check_dependency_direction.sh`, either of which lets a new crate through unseen.

- **Sequencing, stated precisely:** the fix is a prerequisite **of the trigger, not of this
  ADR**. This ADR does not block on code that has not landed; but it does not pretend the
  deferral is enforced by a gate that cannot see the thing being deferred.
- **Both must be in `main` before Fase 3 starts.** Neither before the other.
- **Scope:** two parts (broaden the crate-name regex; derive the scanned crate list from
  `crates/*/Cargo.toml` with an explicit out-of-policy list) plus a regression test, since
  this gate currently has none. Design lives in the gate-fix issue.

## 3. Consequences

### 3.1 Rate limiting becomes per-tier (from risk 1)

Moving the lookup before stage 2 is not a position change, it is a type change. A single
global bucket cannot serve four tiers whose pacing has different units: `Direct` paces
normally; `Ipv6` shares the bucket with a differentiated socket timeout; `MultiVps` paces
locally per worker under orchestrator coordination; `Captcha` introduces 30–120 s per solve,
which would hold a shared bucket hostage and degrade every other tier.

`SharedRateLimiter` (`application/rate_limiter.rs`) therefore needs a tier→bucket mapping.
This ADR fixes **that it is per-tier and why**; the design is Fase 1 work.

### 3.2 IPv6 pool: TCP health check, per-tier timeout, run-scoped sub-range blacklist (from risk 2)

Three corrections, each load-bearing:

1. **The health check must not use ICMP.** A host that answers ping while null-routing TCP
   makes the check a lying oracle — worse than no check, because it suppresses the fallback.
   The health check opens a real TCP connection with the same timeout the request path uses.
2. **The IPv6 socket timeout is per-tier, not global.** `CrawlerConfig.timeout_secs` stays as
   is; a separate, more aggressive IPv6 socket timeout applies only when the tier is `Ipv6`.
   `Direct` has no null-route exposure and keeps the standard timeout.
3. **Run-scoped sub-range blacklist is the critical piece.** Global circuit breakers and
   5-minute health checks are both too slow for a per-run failure. A
   `DashMap<Ipv6Net /* /80 */, BlacklistEntry>` scoped to the run: three failures inside a
   short window disables that sub-range for the remainder of the crawl. Without it, every
   new request in the same run re-attempts the dead sub-range and latency compounds
   geometrically (~300 s per 1,000 URLs at happy-eyeballs delay). With it, ~1 s.

Pre-announcement of the `/64` at boot (never `ip addr add` per request) is retained from the
PRD. `docs/ipv6-threat-model.md` remains a Fase 2 deliverable.

### 3.3 The CAPTCHA solver contract (from risk 3)

"Async queue" means **the solver owns its concurrency control**, not that the caller polls.

```rust
pub trait CaptchaSolver: Send + Sync {
    async fn solve(
        &self,
        challenge: &WafChallenge,
        timeout: Duration,
    ) -> Result<CaptchaToken, SolverError>;
}
```

The caller awaits `solve(...)`. Internally: a configurable concurrency semaphore (default 3)
so the provider is not saturated; each call allocates a `oneshot::channel` and dispatches to
a task; the caller waits under `tokio::time::timeout`; semaphore exhaustion surfacing as
expiry is `SolverError::Timeout`. Backpressure is real and the runtime is never blocked —
the caller still has an explicit deadline, which is what makes failure honest.

The trait contract is fixed here so the Fase 4 issue implements rather than designs.

### 3.4 Layer decoupling has independent value and its own issue

`domain/value_objects.rs` reaches `crate::ScraperError`, a 2,003-line top-level module that
depends on `wreq`, `serde_json`, and `serde_yaml`. This is the direction ADR-0010/0012 exists
to forbid, and the intra-crate gate cannot see it (appendix A, failure C).

**Framing, settled by evidence:** neither "accepted leak" nor "forgotten bug" — a
**structural blind spot**. The layer regex knows only
`infrastructure|adapters|application`; `crate::ScraperError` and `crate::error::…` generate
no match. The allowlist is at its terminal 19→2 state with both entries PERMANENT
(DI root, transversal tracing) and no mention of `domain → error`. The two most recent
hardening passes that touched these exact lines (`df9d9fa8` #1158, `81103fe6` #1240) neither
removed nor recorded the dependency. ADR-0013 shows a culture of reusing `ScraperError` for
ports ("rejected new dedicated error type per budget"), which is context, not adjudication
of this edge.

The swap is verified cheap: `From<DomainError> for ScraperError` exists
(`error.rs:759`) and maps all seven variants; `Display` is byte-identical for the two
variants in use (`"URL inválida: {0}"`, `"Validación de URL falló: {0}"`); classification
matches (`InvalidUrl` → `PermanentFatal` at `error.rs:442`, `Validation` → `PermanentFatal`
at `:447`). ~8 code sites in `value_objects.rs`.

**One caveat, found during verification:** `ScraperError::invalid_url()` accepts
`impl Into<String>`; `DomainError` has no equivalent helper, so call sites construct
`DomainError::InvalidUrl(msg.to_string())` directly. Cosmetic, but it is why "mechanical"
should not be read as "zero-touch".

**Order inside the hygiene issue matters:** extend the checker first, then swap. Without the
extended checker there is nothing holding the swap green, and the class of edge stays
invisible for the next contributor.

### 3.5 What this ADR does not authorize

No code, no crate, no issue, no delivery. Fase 1–3 issues and the two hygiene issues are
downstream of this document and are opened separately.

The interim contract test in decision 3 is a requirement on the Fase 1 issue, not on this
ADR. This ADR authorizes no work; it fixes invariants that downstream issues must honour.

## 4. Alternatives rejected

- **`ssrf-only` subset feature flag as the permanent shape** — see decision 2. Fragile by
  construction, dishonest docs.rs, and an allow-matrix that reads true while the artifact is
  a different crate.
- **`ssrf-only` as a declared transitional bridge** (with exit criterion and event-anchored
  review) — the honest version of the above, and the fallback if extraction is genuinely
  impossible this cycle. Rejected only because the trigger already carries the same
  discipline with less code to un-wind.
- **Orchestrator validates, worker trusts** — rejected in decision 1. Private network is not
  a trust boundary.
- **Worker depends on full `webfang_core`** — distributes ONNX runtime, bundled SQLite, and
  chromiumoxide to VPSs that fetch and nothing else.
- **Full extraction now, ahead of Fase 3** — ~2,500–3,000 lines of dependency-graph refactor,
  on an untried pattern, for a P2 optional phase. YAGNI.
- **ICMP health check for the IPv6 pool** — rejected in §3.2; a ping-answered, TCP-routed
  host turns the check into a lying oracle.
- **Global circuit breaker as the only IPv6 defence** — too slow per run; retained as
  cross-run memory, with the run-scoped blacklist as the fast path.
- **Fixing the gate as "nice to have"** — rejected in decision 4. A deferral trigger nothing
  can observe is decoration.

## Appendix A — Inter-crate gate: current state (reference material, NOT a decision of this ADR)

> **This appendix is input for the gate-fix issue.** It documents verified current state and
> a sketch. The design lives in the issue. Read it as "what is true today", not as "what was
> decided here" — the only thing this ADR decides about the gate is decision 4 (the fix is a
> prerequisite of decision 2's trigger).

**Subject:** `scripts/check_dependency_direction.sh` (72 lines, bash + awk), the #513 gate,
wired into `.github/workflows/ci.yml:214` and `scripts/ci_fast_gate.sh:381`.

**Failure A — crate-name regex is a hardcoded enumeration.**

```awk
in_deps && $1 ~ /^[[:space:]]*webfang_(core|ai|tui|mcp|cli)[[:space:]]*$/ { … }
```

`webfang_net_types`, `webfang_egress`, `webfang_test_utils` extract to empty (probe in §1.3).
The dependency is never seen, so it can never be vetoed. Note `tui` is still in the regex
after the crate was deleted (`18207ae1`) — the enumeration is already stale in both
directions.

**Failure B — the scanned crate list is a second hardcoded enumeration.**

```bash
CRATES=(webfang_core webfang_ai webfang_mcp webfang_cli webfang_benchmark)
```

Even if failure A were fixed, a new crate is never iterated **as a consumer**, so its own
outbound edges are unchecked. Fixing only the regex leaves half the hole open.

**Existing precedent:** `webfang_test_utils` is already invisible to this gate — the
difference is that the exclusion is *documented* ("outside this policy"). For a future crate
it would be inadvertent.

**Failure C — the sibling gate cannot see root-level modules** (relevant to §3.4).
`scripts/check_intra_crate_direction.sh` uses
`INLINE_LAYER_REGEX='crate::(infrastructure|adapters|application)::…'`. Its own comment:
*"Does NOT match `crate::domain::` (innermost, never outward)"* — which is correct, but the
vocabulary also omits `crate::error::…` and the root re-export `crate::ScraperError`
(`lib.rs:132`). An edge of the form *domain → root module* cannot be named by the checker, so
it cannot fail closed. The allowlist's "any new violation fails closed" rule (#1032) cannot
apply to a class of edge the regex does not know.

**Sketch (design belongs to the issue):** derive `CRATES` from `crates/*/Cargo.toml`; broaden
the extraction to `webfang_[a-z0-9_]+`; keep an explicit out-of-policy list with a reason per
entry; add a root-module term to the intra-crate layer vocabulary. Roughly 15 lines of bash.

**Missing safety net:** the intra-crate gate has `scripts/test_intra_crate_gate.sh`; the
inter-crate gate has **no test at all**. A fix without a regression fixture — a manifest
declaring a misdirected `webfang_net_types` dependency, asserting the gate goes red — proves
nothing, because the current bug is silence.

## Appendix B — Evidence index

| Claim | Where verified |
|---|---|
| Guard-chain stage order | `AGENTS.md` §Fetch guard-chain |
| `Downloader::fetch` takes `&Url` | `domain/downloader_port.rs` |
| Line counts (966/740/253/241; 2,003/1,005/178/89/47/74) | `wc -l`, §1.3 |
| Gate probe output | sandbox reproduction of the script's awk, §1.3 |
| Inter-crate gate wiring | `ci.yml:214`, `ci_fast_gate.sh:381-382` |
| Intra-crate gate green, allowlist at 2 PERMANENT | `bash scripts/check_intra_crate_direction.sh`; `check_intra_crate_direction_allowlist.txt` |
| `From<DomainError> for ScraperError`, 7 variants | `error.rs:759` |
| `Display` byte-identical, classification identical | `domain_error.rs:16,37` vs `error.rs:98`; `error.rs:442,447` |
| `ScraperError::invalid_url` helper asymmetry | `error.rs:339` |
| 58 unique files importing the five core symbols | symbol-level `use` grep |
| `eef19afc` = copy, not extraction (0 renames) | `git show eef19afc -M --name-status` |
| Intra-core moves proven | `6fa0b25e`, `38bd1ddf`, `e428dcdf` (R098–R100) |
| `MockClock`/`Arc<dyn Clock>` exists | `domain/clock.rs:68-75` |
| Per-implementor contract test precedent | `tests/ssrf_rfc1918_e2e_test.rs` |
| `SharedRateLimiter` real name/location | `application/rate_limiter.rs`, used at `application/batch/processor.rs:46` |
| ADR-0016 is interruption/resume, not SSRF | `docs/adr/0016-interruption-resume-lifecycle.md:1` |
