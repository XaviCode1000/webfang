# Migrate duplication gate to kucherenko/jscpd v5

## Objective
Move the `code-quality` duplication ratchet from jscpd-rs 0.1.12 to kucherenko/jscpd v5 (pinned 5.4.0), and adopt the valuable official features (blame, SARIF, ai reporter, pinned mode, `--format rust`), to improve product and code quality.

## Problem
jscpd-rs is a 5-star one-maintainer clone (last push 2026-09-12). kucherenko/jscpd is the 6335-star upstream: actively maintained (push 2026-10-05), faster on the same corpus (84ms vs 111ms per its published benchmark), reports fewer lines (9133 vs 10317), fail-closed exit codes by design, native `--baseline`, GitHub Action with SARIF upload, prebuilt binaries for 8 platforms.

## Why this order
Detectors differ, so counts differ: the committed baseline (7683, measured with jscpd-rs) becomes incomparable after the switch. Exactly ONE exceptional recalibration is required (measurement-correction policy clause). PR1 must contain ZERO code changes so "tool changed" is never confused with "debt paid".

## Scope
- `code-quality` job install step, `scripts/check_duplication.sh`, `scripts/quality-baselines.json`, jscpd-rs mentions in script headers/comments.
- Follow-up routine lowerings (dead 294-line `infrastructure/llm/validation.rs` duplicate, `--blame`, SARIF, ai reporter) AFTER the migration lands.

## Constraints
- Binary-only install forever: kucherenko MSRV is 1.96 > workspace 1.88. Never a workspace dependency.
- BOTH packages install a binary named `jscpd`. CI and local dev must `cargo uninstall jscpd-rs` (or equivalent) BEFORE installing v5, or PATH collides silently.
- Repo delivery rules: linked issue with `status:approved` (maintainer adds it, agents never self-approve), exactly one `type:*` label, conventional branch, `CHANGELOG.md` untouched, no stacked PRs.
- Recommended install: `cargo install jscpd --version 5.4.0 --locked` (minimal diff to current step, reproducible). Alternative (needs node in job): `npm install -g jscpd` prebuilt. Measure install time in PR1.

## Checklist
- [x] T1: Open migration issue with `gh issue create` → #1867 (status:approved). Never self-approve.
- [x] T2: Branch `chore/jscpd-v5-migration` from fresh `main`. Swap install step (uninstall jscpd-rs first), pin 5.4.0.
- [x] T3: Update `scripts/check_duplication.sh`: header docs, capture jscpd exit code (drop `|| true` fail-open), add `--format rust`, pin `--mode mild`; keep the absolute-lines ratchet (official `--threshold` is percentage-based and cannot express it).
- [x] T4: Run BOTH binaries over `crates/` with identical options, record both counts as evidence, recalibrate `quality-baselines.json` to the v5 number with a dated note (single exceptional recalibration, zero code changes in this PR).
- [x] T5: Verify JSON shape `statistics.formats.rust(.total).duplicatedLines` parses; update stale jscpd-rs references in comments/notes.
- [x] T6: `bash -n` the script, run it locally, push, `scripts/ci_fast_gate.sh` GREEN, merge via `scripts/merge-when-green.sh` (squash). PR #1868 MERGED as 1e707229.
- [ ] T7: Follow-up routine lowerings as separate PRs (dead validation.rs file first: ~-293 lines).
- [ ] T8 (separate issue, evolution): evaluate `--baseline-from-ref origin/main` to replace the hand-maintained JSON + split `src/` vs `tests/` gates.

## Authorized scope (this plan)
ci.yml code-quality job, scripts/check_duplication.sh, scripts/quality-baselines.json, related comments. Nothing else.

## Acceptance criteria
- `code-quality` GREEN with v5 binary; baseline equals the measured v5 number with dated evidence note.
- PR1 diff contains no `crates/` source changes (`git diff --stat main...HEAD` clean except gate files).
- Local repro `jscpd crates/ --min-tokens 50 --format rust` matches CI within documented drift.

## Applicable checks
`bash -n scripts/check_duplication.sh`, `bash scripts/check_duplication.sh` locally, `bash scripts/ci_fast_gate.sh`, non-blocking `gh run list` for the PR branch.

## Skills adopted
- `.agents/skills/jscpd/` — official tool reference (CLI, ai reporter format, config syntax).
- `.agents/skills/dry-refactoring/` — guided clone-removal workflow for agents paying down the ratchet.
- `.agents/skills/codebase-refactoring/` — broader health pass (duplication, dead code via `--dead-code`, complexity via `--complexity`, `--health`).
- Deliberately skipped: `compare-codebases`, `code-migration` (codebase-porting focused; no porting use case in webfang).

## Ready-to-paste issue command (maintainer runs or approves)
`gh issue create --title "chore(quality): migrate duplication gate to kucherenko/jscpd v5" --body "See odd/tasks/jscpd-kucherenko-migration.md. Single exceptional baseline recalibration required (measurement change, zero code changes in PR1)."` + label `type:chore`.

## Progress
- T1–T6 done: issue #1867 approved, PR #1868 merged (1e707229), baseline 13310 live. Follow-up: skills+lock+plan+whitelist PR (part of #1869, branch chore/jscpd-skills-commit). T7/T8 pending.
