# CI SLOs (Phase 6: observability only)

This document is **observability, not gating**. Nothing here changes CI
behaviour, required checks, or branch protection. It proves the workflow is
faster and safer — the gates themselves are untouched.

Design source: `docs/research/webfang-workflow-transformation-blueprint.md`
Phase 6 ("Observability and SLOs").

## SLO targets

| SLO | Target p90 | Scope |
| --- | --- | --- |
| Tier 0 (static metadata) | 2 min | PR: issue/label/title validation, actionlint/shellcheck, repo guards, classifier |
| Tier 1 (fast correctness gate) | 10 min | PR: fmt + targeted check/clippy/tests for the changed area (see `scripts/ci_fast_gate.sh`) |
| Tier 2 (main integration) | 30 min | Push to `main`: full matrix, coverage, AI integration, sanitizers |
| Docs-only | 3 min | Docs-only changes: cheap validation, no cargo |
| AI path | accepted separately | semantic cleaning runs on remote embedding endpoints outside the PR gate; tracked on its own, never compared here. The local ONNX path was removed (ADR-0004 paso 6) |

## How to regenerate

```bash
bash scripts/ci_metrics.sh --days 14 --workflow ci.yml --branch main
```

Defaults are `--days 14`, `--workflow ci.yml`, `--branch main`. The script is
read-only (`gh run list` + `gh run view` + branch protection read, capped at
200 runs) and prints a markdown SLO snapshot to stdout.

Local fast-gate timings accumulate in `docs/ci-metrics/fast-gate.log`
(`date,branch,lane,result,seconds` per line, appended best-effort by
`scripts/ci_fast_gate.sh` — logging never fails the gate).

## What each metric means

- **Required checks (list + count):** the branch-protection contexts for the
  branch. Read live via the protection API; the metrics script exits 2 rather
  than fabricating this list if the API call fails.
- **Runs in window:** workflow runs created in the last N days (cap: 200).
- **Pass rate:** `success / decided runs`. In-flight runs (empty conclusion)
  are excluded from numerator and denominator but still count in the event
  breakdown — a snapshot taken mid-CI never punishes running jobs.
- **Wall-clock median/p90 (minutes):** `updatedAt − createdAt` per run, i.e.
  queue + execution. Nearest-rank p90 (`ceil(0.9·n)`); median averages the two
  middle values on even samples.
- **By event:** run counts split by `push` / `pull_request` / `schedule`.
- **Status (`PASS`/`WARN`/`unknown`):** observed workflow p90 vs. each target.
  The workflow p90 is an upper-bound proxy — tier lanes are not isolated in
  `gh run` data — so a `WARN` means "investigate", never "block". `unknown`
  means the data was unavailable; numbers are never invented.

## Breach actions

| Breach | Action |
| --- | --- |
| Tier 0 p90 > 2 min | Inspect Tier 0 jobs (metadata validation, actionlint/shellcheck, repo guards, classifier runtime). |
| Tier 1 p90 > 10 min | Inspect classifier lanes: a lane running too much means the classifier scope or lane targeting drifted — check `scripts/ci_path_classifier.sh` outputs and the targeted-cargo package set. |
| Tier 2 p90 > 30 min | Inspect the main integration matrix (feature matrix, coverage, AI lane, sanitizers); consider splitting or caching before touching any gate. |
| Docs-only p90 > 3 min | Check the docs lane stays cargo-free and docs path-ignore still holds. |
| Required-check count growth | Review: every added required context is merge friction — justify it or keep it advisory. |
| Pass-rate drop | Triage recent failures by event (`push` vs `pull_request` vs `schedule`) before changing anything. |
| AI path slow | Accepted separately — track on its own; never let it re-enter the PR gate. |
