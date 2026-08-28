# ADR 0009: PersistenceMode input struct — domain owns the rule, application owns the wiring

- **Status:** Accepted
- **Date:** 2026-08-28
- **Deciders:** Project Architect, `webfang` maintainers
- **Related issues:** #980 (umbrella, slice 5c), #984 (PR)
- **Supersedes:** —

## Context

PR #984 (`refactor/core: unify resume and checkpoint via PersistenceMode`, slice 5c of the
#980 ADR-002 umbrella) introduced a `domain::persistence::PersistenceMode` enum to replace the
implicit 2×2 truth table encoded across four independent CLI flags (`--resume`, `--state-dir`,
`--checkpoint-interval`, `--no-checkpoint`). The pure resolver lives at
`crates/webfang_core/src/domain/persistence.rs:84` as `PersistenceMode::from_limits(...)`.

The refactor's intent is sound: collapse the combinatorics into one exhaustive `match` so the
truth table exists in exactly one place, with the 8-combo matrix testable as a unit.

**The architecture violation:**

`domain/persistence.rs:11` imports from the application layer:

```rust
use crate::application::crawl_options::CrawlLimits;
```

AGENTS.md establishes the Clean Architecture layering as **inward only**:

> `infrastructure → adapters → application → domain` (inward only)

Domain depending on application inverts the dependency direction. The crate-level
`scripts/check_dependency_direction.sh` does not catch this because it only checks
**inter-crate** dependencies in `Cargo.toml`; intra-crate module-level violations are not gated
by CI.

**The redundancy it created:**

`application/crawl_options.rs:258` adds `CrawlLimits::persistence_mode()` as a thin delegator
to `domain::persistence::PersistenceMode::from_limits`. This is a façade that exists only to
hide the direction inversion — a Code smell indicating the call site would rather import
domain directly but cannot because domain already imports application (chicken-and-egg).

**The orchestrator duplication it encouraged:**

`crates/webfang_core/src/cli/orchestrator.rs:621` introduces
`discover_recursive_with_persistence` which **re-implements** the entire body of
`discover_urls_recursive` solely to inject `with_persistence` into the Engine. The original
slice introduced a top-level `if persistence_mode.checkpoint_cfg().is_some()` branch right at
the call site — the precise pattern the refactor was supposed to eliminate.

The driver of slice 5c is "eliminate the chaos of four boolean flags interacting implicitly."
Two of its three concrete changes (the orchestrator duplication, the application dependency)
re-introduce the very chaos they were meant to remove.

## Decision

1. **`domain` owns the resolver's input.** Introduce
   `domain::persistence::ResumeConfig { resume: bool, state_dir: Option<PathBuf>,
   checkpoint_interval: u64, no_checkpoint: bool }` and rename `from_limits` →
   `from_config(&ResumeConfig, &Path) -> PersistenceMode`. The four fields are exactly the
   four CLI flags slice 5c unified; the type is a pure value object with no behavior beyond
   `Default`, `Debug`, `Clone`, `PartialEq`.

2. **`application` owns the wiring.** `CrawlLimits::resume_config(&self) -> ResumeConfig`
   becomes a 4-line delegator. `domain::persistence` no longer imports
   `crate::application::crawl_options`.

3. **The orchestrator stops branching on the mode for the recursive BFS path.**
   `discover_urls_recursive` accepts an `Option<&PersistenceMode>` (or, equivalently, the
   `&EngineOptions` it already needs) and applies `with_persistence` itself when the mode
   enables checkpointing. `discover_recursive_with_persistence` is deleted.

4. **`from_config` replaces `from_limits` everywhere**, including the 8-combo test matrix.
   Tests now exercise the public `ResumeConfig` constructor.

5. **Crate-level CI gate extends.** Add a small Rust-side lint
   `scripts/check_intra_crate_direction.sh` that scans `use crate::`
   declarations and reports any `domain::*` module that imports
   from `application::`, `adapters::`, or `infrastructure::` (and the
   same inwards at each level). The script runs in **WARN mode** by
   default (exit 0, `::warning::` annotations in CI logs) so it can
   ship without breaking the build on the 70+ pre-existing
   application→infrastructure violations the audit surfaced; a
   follow-up slice (#XXX — see "Out of scope" below) flips it to
   strict mode after the violations are fixed.

## Out of scope (recorded for the next slice)

The audit surfaced **70+ pre-existing violations** of the
intra-crate direction rule, all of the shape
`application::*` → `infrastructure::*`. None of these were
introduced by slice 5c; they are pre-existing technical debt from
the original layering where `CrawlerConfig`, `HttpClientConfig`, and
the `Engine` itself were moved freely between layers. The lint
catches them but does not fail the build in WARN mode. A separate
issue (#XXX, to be filed by the maintainer) should:

- Move the offending `infrastructure::` types that the application
  layer genuinely needs into a `domain::ports` trait surface, with
  the concrete impl staying in `infrastructure`.
- Or accept a documented exception for each specific call site that
  has a clear business reason to break the direction (e.g. DI
  wiring in `application::container`).
- Then flip `INTRA_CRATE_MODE=strict` in the CI invocation of the
  lint.

**Estimated scope:** 200-400 lines of refactor + ~10 file moves.
Should land in its own slice, not in 5c.

## Consequences

**Positive**

- Domain stays pure: `from_config` operates on a value object owned by domain, with no
  dependency on the application layer.
- The orchestrator's recursive BFS path is a single function, with persistence wiring as an
  input rather than a top-level branch. The driver of the slice ("one truth table, one place")
  is now true for the call site too, not just for the resolver.
- Adding the new CI lint is a one-time cost and catches the same class of violation in future
  slices.
- The `ResumeConfig` struct is reusable: a future MCP-driven crawl that takes resume/checkpoint
  configuration over JSON can deserialize directly into `ResumeConfig` without going through
  `CrawlLimits`.

**Negative / Risks**

- A second public type (`ResumeConfig` alongside `CrawlLimits`) doubles the surface area of
  "things a caller has to know about" for resume/checkpoint. Mitigated by keeping
  `CrawlLimits::resume_config` as the canonical entry point — the orchestrator never names
  `ResumeConfig` directly.
- The CI lint is new and unproven; it may produce false positives on `#[cfg(test)]` modules
  or doc-tests. Mitigated by gating on release builds (`!#[cfg(test)]`) first, and by
  allowing `pub use` re-exports.
- The follow-up commit on top of #984 (`eaf646c6`, "BLOCKER") is on top of the
  direction-violating base. It will need to be rebased or amended. Net work is small (one
  rename + one new struct + one orchestrator refactor + one new lint script), but it
  changes the shape of the existing PR.

## Alternatives rejected

- **Keep `from_limits(&CrawlLimits, ...)`.** Rejected: perpetuates the direction violation.
  AGENTS.md's "inward only" rule is structural, not aspirational. A single
  application-layer import is enough to invert it; a single thin delegator does not undo
  the inversion.
- **Move the entire `CrawlLimits` to domain.** Rejected: `CrawlLimits` carries ~12 unrelated
  fields (concurrency, rate-limit, headers, cookies, patterns, etc.) that are application
  composition concerns, not domain rules. A wholesale move would inflate the domain crate
  with no benefit and would block the slice for an unrelated cleanup.
- **Generics on `from_limits<T: HasResumeFields>(...)`.** Rejected: pushes the cost to
  every call site, adds trait-bound noise to the API, and provides no value over a typed
  value object.
- **Add a `pub use` re-export of `CrawlLimits` from domain.** Rejected: this is the same
  violation with extra steps. The lint would still catch it (re-exports are not the same as
  definitions), and it papers over the real problem.
- **Defer the lint, document the violation in code only.** Rejected: the violation was
  introduced by a slice whose own stated goal was to remove architectural debt. A
  documented exception is a permanent exception; the next slice will copy the pattern.
