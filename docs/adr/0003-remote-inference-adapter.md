# ADR 0003: Remote OpenAI-Compatible Inference Adapter as the Default AI Path

- **Status:** Accepted
- **Date:** 2026-08-25
- **Deciders:** Project owner
- **Related issues:** #789 (LLM-first extraction core), #917 (SSRF validating DNS resolver), #302 (stabilization gate)
- **Supersedes:** —

## Context

WebFang's AI story today is split between two halves that do not meet:

**The remote-inference foundation exists but is dead code from the binaries.**

| Component | Location | Role | Wired into production? |
|-----------|----------|------|------------------------|
| `LlmPort` trait | `crates/webfang_core/src/domain/llm_port.rs:53` | Domain port (`send_completion`) | Port defined, never injected |
| `OpenAiLlmClient` | `crates/webfang_core/src/infrastructure/llm/client.rs:63` (`impl LlmPort` at l.95) | OpenAI-compatible chat/completions over `wreq` (Chrome145, `response_format=json_object`, `temperature=0.0`) | **No** — constructed only in tests (`client.rs:185`) |
| `LlmExtractionService` | `crates/webfang_core/src/application/llm_extraction.rs:44` | Schema-gate → SSRF-gate → fetch → clean → chunk → LLM loop → validate pipeline; holds `Option<Arc<dyn LlmPort>>` (l.49) | **No** — unreachable without an injected port |
| Container LLM slot | `crates/webfang_core/src/application/container.rs:117` (`OnceCell<Arc<dyn LlmPort>>`), accessor l.282, builder `with_llm_port` l.364 | Lazy injection point | **No production caller** |
| `VaultAiPorts` bundle | `crates/webfang_core/src/application/container.rs:129-140` | One-shot binary-layer injection of AI ports | Carries `embedding_port`, `note_repository`, `text_chunker`, `cleaner` — **no `llm_port` field**, so binaries cannot wire the LLM port through the bundle |

The consequence: every binary entry point (CLI, MCP) runs with no LLM provider; `extract` surfaces the honest Spanish `Config("no hay proveedor LLM configurado")`. The capability is compiled but unreachable.

**The local ONNX path is heavy and mis-gated.** The Granite models (~390MB / ~1.25GB) load via `ort` + `hf-hub` inside `webfang_ai`, behind the `ai` feature (`ort`/`tokenizers` are optional deps). However `hf-hub` itself is **not** feature-gated (`crates/webfang_ai/Cargo.toml`: `hf-hub = { workspace = true }` sits outside `[features] ai = [...]`) — a wart that keeps the dependency tree heavier than the feature flag suggests.

**The deployment target changed the calculus.** The owner's target host is a small professional VPS where resident-model RAM and the BoringSSL+ONNX build cost are inviable as the *default* experience. This is an explicit product decision, not a performance micro-tuning one.

**SSRF posture is ready.** The hostname-DNS resolution gap formerly tracked as pending work ("slice B") was closed by #917 (commit `5f445fcf`): a connect-time `ValidatingResolver` re-validates every DNS answer against `is_forbidden_ip` and is installed both on scrape clients and on `OpenAiLlmClient` itself (`client.rs:84`), with the entry-level `ssrf_gate` retained as fast-fail UX.

## Decision

Adopt a **remote inference adapter speaking the OpenAI-compatible protocol as the default AI path**, backed by the existing `LlmPort` abstraction:

1. **Default = remote endpoint, self-hosted.** Configurable via environment: `WEBFANG_AI_ENDPOINT` (base URL), `WEBFANG_AI_API_KEY` (secret), `WEBFANG_AI_MODEL`. The protocol choice deliberately targets the de-facto self-hosted standard served by vLLM, llama.cpp server, and Ollama.
2. **Adapter placement: `webfang_core/src/infrastructure/`.** The concrete adapter implements `LlmPort` next to `OpenAiLlmClient`, respecting the enforced dependency direction (`webfang_ai → webfang_core` only; core must never depend on `ai`). `OpenAiLlmClient` already satisfies this placement and becomes the reference implementation.
3. **HTTP client is `wreq`, never `reqwest`.** Non-negotiable repository policy: TLS fingerprint impersonation and the existing `ValidatingResolver` integration come with `wreq`.
4. **Wiring closes the actual gap.** Extend `VaultAiPorts` (or its builder path) so the binary layer can inject the LLM port alongside the other AI ports. Until wired, the service keeps returning the honest Spanish config error — no silent fallbacks, no panics.
5. **Local ONNX goes dormant, not deleted.** Granite via `ort` + `hf-hub` remains intact behind `--features ai` as the offline fallback. No rewrite of `webfang_ai`; the remote path does not touch it.
6. **Failure semantics reuse the existing taxonomy.** Transport failures classify as `ErrorClass::TransientRetriable` (`crates/webfang_core/src/domain/error/error_class.rs:16-18`; LLM client classification at `client.rs:6`), so retry/backoff policy composes with the rest of the system instead of inventing a parallel one.

Implementation lands in a follow-up change (Sprint 12+, after Gate 5 per the stabilization roadmap).

## Trade-offs

| Quality attribute | Local ONNX default (rejected) | Remote adapter default (chosen) |
|-------------------|-------------------------------|--------------------------------|
| Target-host RAM | Model-resident (~390MB–1.25GB) | Zero model residency |
| Build cost | BoringSSL + `ort` compile burden | None by default |
| Binary/deps footprint | `webfang_ai` + non-optional `hf-hub` | Core-only adapter, `wreq` reused |
| Latency p50 | CPU-bound inference (1200–3000ms class) | Network round-trip (800–1500ms class) |
| Availability | Offline-capable | Requires reachable endpoint |
| Recurring cost | USD 0 | Pay-per-use tokens |

## Consequences

**Positive**
- Builds get faster and lighter by default: `webfang_ai` stops compiling in the default path once binaries wire the remote adapter, dropping the BoringSSL+ONNX burden from routine builds.
- Small-VPS deployments become viable: no resident model RAM, no model downloads.
- Provider-agnostic: any OpenAI-compatible endpoint works without touching domain code — the port boundary absorbs vendor churn.
- Operational failures compose with the existing error strategy: `TransientRetriable` classification gives retry/backoff and exit-code behavior for free.

**Negative / costs**
- Network latency enters the hot path. Mitigation: batch chunks per request where schemas allow, and respect the concurrency budget established by the Sprint 7–8 stabilization work rather than issuing unbounded parallel completions.
- Variable per-use token cost replaces fixed compute cost; budget-conscious usage needs chunk-budget discipline (already enforced by `CHARS_PER_TOKEN` sizing in the extraction pipeline).
- New operational dependency: endpoint availability and credential rotation (`WEBFANG_AI_API_KEY`) become part of running WebFang.
- The `hf-hub` non-optional wart remains until a separate cleanup gates it under `ai`.

**Neutral**
- `webfang_ai` and its ONNX adapters stay byte-for-byte intact in dormancy; re-enabling them is a feature-flag decision, not a migration.
- Tests keep using `wiremock` against the OpenAI-compatible contract (ephemeral adapters, zero real network), matching the existing behavioral-test harness conventions.

## Alternatives rejected

1. **Vendor SDK (e.g., an `openai` crate).** Couples WebFang to one provider's client, duplicating what `OpenAiLlmClient` already does over plain `wreq`, and contradicts the port-agnostic design. Rejected.
2. **Keep local ONNX as the default.** Inviable on the target VPS (model RAM plus BoringSSL+ONNX build time) and contradicts the owner's explicit product decision. It survives as the offline fallback instead. Rejected.
3. **Rewrite `webfang_ai`.** Violates the stated non-goal of leaving the AI crate untouched; the crate is correct, just wrongly positioned as default. Rejected.
4. **Remote adapter inside `webfang_ai`.** Would invert the dependency direction: `webfang_ai → webfang_core` is allowed, but making core's *default* path depend on `webfang_ai` breaks the enforced allow-matrix and couples the base build to the AI stack. Rejected.

## References

- `crates/webfang_core/src/domain/llm_port.rs:53` — `LlmPort` trait (domain port)
- `crates/webfang_core/src/infrastructure/llm/client.rs:63,84,95` — `OpenAiLlmClient` (`wreq`, Chrome145, `ValidatingResolver`, `impl LlmPort`)
- `crates/webfang_core/src/application/llm_extraction.rs:44-49` — `LlmExtractionService` and its optional port
- `crates/webfang_core/src/application/container.rs:117,129-140,364` — Container LLM slot, `VaultAiPorts` (missing `llm_port`), `with_llm_port`
- `crates/webfang_core/src/domain/error/error_class.rs:16-18` — `ErrorClass::TransientRetriable`
- `crates/webfang_ai/Cargo.toml` — `hf-hub` non-optional wart; `ai` feature gates only `ort`/`tokenizers`
- `docs/llm-backend-decision.md` — prior comparative analysis (remote vs SLM vs hybrid)
- Issues: #789, #917, #302
