# P6-2 / RC-2 — CrawlSession design

Design-only artifact set for **P6-2 (RC-2): the `CrawlSession` run abstraction**.
No production code. Baseline `main` @ `95a368b7`.
Branch: `docs/p62-crawlsession-rc2` · worktree: `~/Projects/Rust/webfang-worktrees/docs-p62-crawlsession-rc2`.

| File | SDD phase | Content |
|---|---|---|
| `exploration.md` | sdd-explore | current state, 7 assembly sites, approaches A/B/C, recommendation |
| `proposal.md` | sdd-propose | intent, scope/non-goals, capabilities, risks, rollback, success criteria |
| `spec.md` | sdd-spec | 9 requirements + Given/When/Then scenarios (WHAT, no HOW) |
| `design.md` | sdd-design | 10 decisions, signature sketches, async rules, data flow, observability, error stratification, verification plan |
| `../../adr/0017-crawlsession-run-owner.md` | ADR log | context / decision / consequences / alternatives rejected |

## Canonical location (resolved P5)

**This directory is the single canonical home** for the RC-2 design artifacts, plus
`docs/adr/0017-*.md`. `openspec/changes/crawl-session-abstraction/` is scratch only:
`.gitignore:39` ignores `openspec/` ("AI AGENTS — SDD / Agent working directories") and
`.gitignore:41` ignores `specs/` **at any depth**, so neither the SDD convention path nor
its spec-delta subpath can be committed here. Do not edit the scratch copy as if it were
source; edit these files and re-sync the scratch if a tool needs it.

Two consequences worth carrying forward, because they cost a silent half-commit before
they were caught:

1. Writing artifacts only under `openspec/` yields an empty `git diff` — the reviewable
   unit is not created at all.
2. `specs/<capability>/spec.md` is uncommittable anywhere in this repo, so the spec delta
   lives flat at `spec.md` here. Verify with `git check-ignore -v <path>` and cross-check
   `git diff --stat` against the files on disk: a file count mismatch is the only signal
   these traps give.

## Read order

`exploration.md` → `proposal.md` → `spec.md` → `design.md` → ADR-0017.

## Status

**Design closed and signed by the orchestrator (P1–P4 FIRMADO, P5 resolved).** No code in
this mission. Slice 1 is unblocked for the code mission; slices 2–4 execute under the
signed decisions recorded in ADR-0017 and `design.md` §Decisions. Delivery (push/PR) is
the orchestrator's, not this branch's.
