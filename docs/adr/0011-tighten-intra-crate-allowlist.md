# ADR 0011: Tighten Intra-Crate Allowlist — Remove Broad `infrastructure::`

- **Status:** Accepted
- **Date:** 2026-08-28
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0010, issue 990 follow-up, `scripts/check_intra_crate_direction_allowlist.txt` (5 entries)
- **Supersedes:** ADR-0010 §2 `infrastructure::` broad

## Context

ADR-0010 hybrid left `infrastructure::` broad (20 violations) as deferred keep ≤5. Strict without broad reveals 20 violations across 10 subdomains (waf 4, export 5, scraper 3, user_agent 2, network 1, bridge 1, axtree 1, converter 1, obsidian 1, llm 1). 53 total violations before allowlist = 33 (crawler 13 + downloader 9 + container 7 + observability 4) + 20 strict. Broad is permanent exception, violates `infrastructure→adapters→application→domain`.

## Decision

Adopt hybrid narrow slice: port 9 subdomains (15 violations, ~560L) to `domain` (waf VO+port, user_agent provider, scraper_port, session_port extend, html_cleaner pure, llm validation pure, cpu_executor, axtree/vault ports); keep single narrow `infrastructure::export` (5 violations) as deferred. Allowlist becomes 5: `application/container.rs`, `infrastructure::observability`, `infrastructure::crawler`, `infrastructure::downloader`, `infrastructure::export` (replaces broad). Next slice ports export (300L, D3) and frees `infrastructure::crawler` (13) to allow split into `waf`+`export` specifics while staying ≤5.

## Consequences

- Strict gate: 20→0 (port 15) +5 deferred via `export` (allowlisted 38→33 after slice, printed)
- Domain gains `domain::waf`, `domain::user_agent`, `domain::scraper_port`, `domain::html_cleaner`, `domain::llm::validation` (pure) + trait ports
- Container wires new `Arc<dyn Port>` (clone before await)
- Allowlist stays ≤5, each with ADR reason, CI fails if >5
- No public API break (shims)

## Alternatives Rejected

| Option | Verdict |
|--------|---------|
| Port all 20 now (A, ~1100L) | Too large for 800 budget, Engine lock risk |
| Keep broad (B) | Permanent debt, skill rejects |
| 2-new with crawler free now (680L) | Viable but extra 120L for crawler; deferred to next slice |

## References

`AGENTS.md` allow-matrix, `check_intra_crate_direction.sh`, `check_intra_crate_direction_allowlist.txt`, `domain/{waf,html_cleaner,llm}`, `infrastructure/export/{state_store,record_store}`, `application/export_factory.rs` D3 protocol
