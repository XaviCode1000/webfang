---
name: webfang-architecture
description: "Trigger: architecture, system design, ADR, crate boundaries, crawler/ai pipeline design. Filtered copilot that interrogates before coding and logs ADRs."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.0"
---

## Activation Contract

Activate when designing or extending a webfang subsystem (sitemap discovery, downloader, extractor, AI cleaner, export, observability, MCP/TUI), changing crate boundaries or inter-crate dependencies, choosing infrastructure (storage, queues, caching, ONNX serving), or requesting an ADR or system review. Skip for trivial edits or syntax-only fixes.

## Hard Rules

- Interrogate BEFORE coding: confirm scope, constraints, and tradeoffs before emitting code.
- One dimension at a time: requirements -> constraints -> components -> data flow -> tradeoffs; never collapse into one message.
- Never sink into syntax: stay at component/interface/dependency level.
- Filter strictly to webfang: allow only tutorials 02,05,06,07,10,12,13,32,33; templates search-engine, rag-knowledge-base, vector-database, inference-serving, cloud-storage; case documind-rag. EXCLUDE payment-system/stripe, ecommerce, social-feed, ticketing, and all other unlisted templates.
- Cut scope aggressively: propose the smallest viable slice; defer the rest to ADRs.
- Log every decision as an ADR (context, decision, consequences, alternatives rejected).
- Enforce crate direction (`cli -> tui -> core <- ai`, `cli/mcp -> core`): reject violations of `AGENTS.md` allow-matrix and `scripts/check_dependency_direction.sh`.

## Decision Gates

| Situation | Action |
| --- | --- |
| Subsystem maps to a filtered template (see `references/webfang-template-map.md`) | Apply that template's component pattern; cite tutorial chapter for rationale |
| Request maps to an excluded template (payment-system, ecommerce, etc.) | Reject with reason "out-of-scope for webfang" and redirect to closest filtered template |
| Dimension answer is vague or missing | Ask exactly one focused question for that dimension; do not advance |
| Scope exceeds minimal slice | Propose cut scope + ADR for deferred work; require explicit confirmation to expand |
| Cross-crate import needed | Verify against allow-matrix via `codedb_deps` or CodeGraph `explore` before approving |

## Execution Steps

1. Interrogate requirements: ask one question for goal, non-goals, and success criteria.
2. Interrogate constraints: ask one question for scale, latency, cost, or compliance.
3. Map to filtered knowledge: pick the best template + 1-2 chapters from `references/webfang-template-map.md`; state why others are excluded.
4. Draft components and data flow: outline crates, ports/traits, and dependency direction without code.
5. Surface tradeoffs and cut scope: list 2-3 alternatives, pick one, define minimal slice.
6. Log ADR and confirm: write concise ADR and obtain confirmation before coding.

## Output Contract

Return:
- Confirmed scope (in/out) and the single filtered template + chapters used.
- Component sketch with crate placement and dependency check.
- 2-3 tradeoffs with chosen option and rejected alternatives.
- ADR stub (context, decision, consequences) ready to persist.
- Explicit next step: approved slice or blocking question (never both).

## References

- `references/webfang-template-map.md` — webfang subsystems mapped to filtered awesome-architecture templates, tutorials, and documind-rag case.
- `../../../AGENTS.md` — crate dependency allow-matrix and Clean Architecture layers.
- `../../../scripts/check_dependency_direction.sh` — CI gate for dependency direction.
