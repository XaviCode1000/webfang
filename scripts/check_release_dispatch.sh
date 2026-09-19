#!/usr/bin/env bash
# Release hand-off guard — a release tag must never land without its binaries.
#
# Invariant: every path that creates a release tag hands it to release.yml
# explicitly, and release.yml builds the tag it is handed.
#
# Why a tag push is not enough (the root cause, not a preference): the tags
# release-plz pushes are authored by github-actions[bot], and GitHub suppresses
# workflow runs for events generated with GITHUB_TOKEN. So release.yml's own
# `push: tags` trigger CANNOT fire for them, no matter how the workflow is
# written. workflow_dispatch IS an exception to that suppression, which is why
# the hand-off is an explicit call rather than an event. Measured consequence
# of the missing call: v2.1.0 shipped its binaries only via a manual dispatch,
# and v2.1.1 ended with a tag and no GitHub Release at all.
#
# Why this is a guard and not just a comment: the failure mode is not a typo,
# it is a WHOLESALE REWRITE of release-plz.yml that quietly drops the
# dispatching job. That is not hypothetical — PR #1455 (fix/release-plz-green-lie,
# closed unmerged) replaced the file's tail, and merging it as-is would have
# deleted the entire `dispatch-release` job while restoring a comment claiming
# the tag push triggers release.yml. A comment cannot fail a merge; this can.
#
# Scope: the structural invariants that caused the incident. Prose in these
# workflows is deliberately NOT asserted — comments change for good reasons,
# and a guard that fails on re-wording gets disabled instead of fixed. The one
# exception is the push-only gate on the version check (see check 4), because
# it is an expression, not prose, and it silently disables a validation.
#
# RELEASE_DISPATCH_ROOT overrides the scanned repository root, so fixtures can
# exercise this guard without touching the real workflows.
set -euo pipefail

REPO_ROOT="${RELEASE_DISPATCH_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

RELEASE_PLZ_YML="$REPO_ROOT/.github/workflows/release-plz.yml"
RELEASE_YML="$REPO_ROOT/.github/workflows/release.yml"
CUT_PATCH_YML="$REPO_ROOT/.github/workflows/cut-patch-tag.yml"

for f in "$RELEASE_PLZ_YML" "$RELEASE_YML" "$CUT_PATCH_YML"; do
  [[ -f "$f" ]] || {
    echo "::error::check_release_dispatch: missing workflow ${f#"$REPO_ROOT"/} — the guard cannot verify the hand-off. If a workflow was renamed, update this guard."
    exit 1
  }
done

FAIL=0
step() { printf '  %-52s %s\n' "$1" "$2"; }

# ─────────────────────────────────────────────────────────────────────────────
# Extract one top-level job block (2-space indent under `jobs:`) up to the next
# job key or a dedent to column 0. Block-scoped checks matter: a bare grep for
# `actions: write` would pass on a permission declared by a DIFFERENT job.
# ─────────────────────────────────────────────────────────────────────────────
job_block() {
  awk -v job="$2" '
    !in_job && $0 ~ ("^  " job ":[[:space:]]*$") { in_job = 1 }
    in_job {
      if ($0 ~ ("^  " job ":[[:space:]]*$")) { print; next }
      if ($0 ~ "^  [A-Za-z0-9_.-]+:[[:space:]]*$") exit
      if ($0 ~ "^[^[:space:]]") exit
      print
    }
  ' "$1"
}

echo "check_release_dispatch: verifying the release hand-off invariant"

# ─────────────────────────────────────────────────────────────────────────────
# 1-2. release-plz.yml: the dispatching job exists and can actually dispatch.
# `actions: write` is required to create the workflow_dispatch run; without it
# the job fails at the final call, which is the worst place to find out.
# ─────────────────────────────────────────────────────────────────────────────
BLOCK="$(job_block "$RELEASE_PLZ_YML" dispatch-release)"
if [[ -z "$BLOCK" ]]; then
  echo "::error::check_release_dispatch: release-plz.yml has no 'dispatch-release' job. The tags release-plz pushes cannot trigger release.yml (GITHUB_TOKEN suppression), so without this job a tag lands with no binaries — this is how v2.1.1 was lost."
  step "release-plz.yml: dispatch-release job" "MISSING"
  FAIL=1
else
  step "release-plz.yml: dispatch-release job" "present"
  for needle in "always()" "!= 'cancelled'" "actions: write"; do
    if grep -qF -- "$needle" <<< "$BLOCK"; then
      step "  job declares '$needle'" "ok"
    else
      echo "::error::check_release_dispatch: dispatch-release is missing '$needle'. 'always()' is what makes the hand-off survive the known duplicate-tag 422 (webfang#1341) that leaves a correct tag behind while release-plz exits 1; '!= cancelled' keeps a human cancel from silently becoming a release; 'actions: write' is what permits the workflow_dispatch run."
      step "  job declares '$needle'" "MISSING"
      FAIL=1
    fi
  done
  # --ref pins the dispatched run to the tag. Without it the run resolves the
  # version and Cargo.toml check against whatever ref it was started from.
  if grep -qE 'gh workflow run release\.yml' <<< "$BLOCK" && grep -qE '^\s*--ref\b' <<< "$BLOCK"; then
    step "  job dispatches release.yml with --ref" "ok"
  else
    echo "::error::check_release_dispatch: dispatch-release does not call 'gh workflow run release.yml' with '--ref <tag>'. Dispatching without --ref runs release.yml against the ref the run started from, not the tag being released."
    step "  job dispatches release.yml with --ref" "MISSING"
    FAIL=1
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# 3. release-plz.yml: the duplicate-tag 422 (webfang#1341) is tolerated ONLY
# behind a verification that can fail the job. Asserted as a triple: the
# verification is invoked, the failing step is allowed to continue so the
# verification is reachable at all, and the raw outcome is consumed.
# Losing the first turns the job red again; losing the second or third turns the
# tolerance into the green lie removed in webfang#1476.
# ─────────────────────────────────────────────────────────────────────────────
RELEASE_JOB="$(job_block "$RELEASE_PLZ_YML" release-plz-release)"
if [[ -z "$RELEASE_JOB" ]]; then
  echo "::error::check_release_dispatch: release-plz.yml has no 'release-plz-release' job to inspect — the guard cannot tell whether the duplicate-tag 422 is still tolerated safely. Update this guard if the job was renamed."
  step "release-plz.yml: release job tolerates the 422 safely" "UNVERIFIABLE"
  FAIL=1
else
  while IFS='|' read -r needle why; do
    if grep -qF -- "$needle" <<< "$RELEASE_JOB"; then
      step "  release job: $why" "ok"
    else
      echo "::error::check_release_dispatch: the 'release-plz-release' job no longer has '$needle' ($why). Without it the duplicate-tag 422 either turns the job red again (webfang#1341), or the tolerance becomes a green lie with nothing re-evaluating the failure (webfang#1476)."
      step "  release job: $why" "MISSING"
      FAIL=1
    fi
  done <<< "bash scripts/verify-release-tag.sh|the verification step is invoked
continue-on-error: true|the failing step is allowed to continue
.outcome|the raw outcome is consumed by the verification"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 4. One trust predicate, consumed by BOTH decision points. The fingerprint that
# decides which tags may be trusted is security-relevant, and two copies of it
# drift. It lives in scripts/release-plz-tags-at-head.sh, consumed by the
# dispatcher (which hands a tag to release.yml) and by the verifier (which
# decides the release job's outcome).
# ─────────────────────────────────────────────────────────────────────────────
PREDICATE="scripts/release-plz-tags-at-head.sh"
VERIFIER_SH="$REPO_ROOT/scripts/verify-release-tag.sh"
if grep -qF -- "$PREDICATE" "$RELEASE_PLZ_YML" && [[ -f "$VERIFIER_SH" ]] && grep -qF -- "$PREDICATE" "$VERIFIER_SH"; then
  step "one trust predicate used by dispatcher + verifier" "ok"
else
  echo "::error::check_release_dispatch: $PREDICATE is not consumed by BOTH the dispatcher (release-plz.yml) and the verifier (scripts/verify-release-tag.sh). A duplicated trust predicate drifts, and the two would then disagree about which tags are safe to release."
  step "one trust predicate used by dispatcher + verifier" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 5. release.yml: EVERY checkout is pinned to the tag being released.
# Asserted as a count so ADDING an unpinned checkout fails too, not only
# reverting one of the existing two. The preflight checkout alone is not
# enough: pinning only it would validate the tag while the build job compiles
# main and publishes those binaries under the tag.
# ─────────────────────────────────────────────────────────────────────────────
CHECKOUTS="$(grep -cE '^[[:space:]]*(-[[:space:]]+)?uses: actions/checkout@' "$RELEASE_YML" || true)"
PINNED="$(grep -cE '^[[:space:]]*ref:[[:space:]]*.*github\.event\.inputs\.tag' "$RELEASE_YML" || true)"
if [[ "$CHECKOUTS" -eq 0 ]]; then
  echo "::error::check_release_dispatch: release.yml has no 'uses: actions/checkout@' step — the guard can no longer verify the checkout pin. Update this guard if checkout moved to a composite action."
  step "release.yml: checkout pin" "UNVERIFIABLE"
  FAIL=1
elif [[ "$PINNED" -lt "$CHECKOUTS" ]]; then
  echo "::error::check_release_dispatch: release.yml has $CHECKOUTS checkout step(s) but only $PINNED tag-pinned 'ref:'. Every checkout must use 'ref: \${{ github.event.inputs.tag || github.ref_name }}'. An unpinned checkout compiles the ref the run started from (a branch) and publishes those binaries under the tag."
  step "release.yml: checkout pin $PINNED/$CHECKOUTS" "UNPINNED"
  FAIL=1
else
  step "release.yml: all $CHECKOUTS checkouts pinned to the tag" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 6. release.yml: the version check is not gated back to push-only.
# On workflow_dispatch the checkout is pinned but the tag INPUT is what names
# the release, so a tag whose version disagrees with Cargo.toml must still be
# rejected. The original defect was exactly `if: github.event_name == 'push'`
# on that step, which skipped the assertion on the dispatched path.
# ─────────────────────────────────────────────────────────────────────────────
PREFLIGHT="$(job_block "$RELEASE_YML" preflight)"
if [[ -z "$PREFLIGHT" ]]; then
  # Fail closed: an empty extraction makes the grep below vacuous, and a check
  # that cannot fail is worse than no check (it reports green over a missing
  # invariant). Renaming or restructuring the job must update this guard.
  echo "::error::check_release_dispatch: release.yml has no 'preflight' job to inspect — the guard cannot tell whether the tag/version validation still runs on the workflow_dispatch path. Update this guard if the job was renamed."
  step "release.yml: preflight has no push-only gate" "UNVERIFIABLE"
  FAIL=1
elif grep -qE "github\.event_name == ['\"]push['\"]" <<< "$PREFLIGHT"; then
  echo "::error::check_release_dispatch: a step in release.yml's 'preflight' job is gated with 'if: github.event_name == ''push''' — a push-only gate silently skips that validation on the workflow_dispatch path, which is now the path every release takes."
  step "release.yml: preflight has no push-only gate" "MISSING"
  FAIL=1
else
  step "release.yml: preflight has no push-only gate" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 7. cut-patch-tag.yml: the second tag-creating path has the same obligation.
# It pushes vX.Y.Z with GITHUB_TOKEN, so it is affected by the same suppression
# and must hand the tag over explicitly (webfang#1480).
# ─────────────────────────────────────────────────────────────────────────────
if grep -qE '^\s*actions: write' "$CUT_PATCH_YML" &&
   grep -qE 'gh workflow run release\.yml' "$CUT_PATCH_YML" &&
   grep -qE '^\s*--ref\b' "$CUT_PATCH_YML"; then
  step "cut-patch-tag.yml: dispatches release.yml with --ref" "ok"
else
  echo "::error::check_release_dispatch: cut-patch-tag.yml creates a tag with GITHUB_TOKEN but does not declare 'actions: write' and dispatch release.yml with '--ref'. Its tag push is suppressed exactly like release-plz's, so a PATCH release on a support branch would ship without binaries."
  step "cut-patch-tag.yml: dispatches release.yml with --ref" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# Verdict
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$FAIL" -ne 0 ]]; then
  echo "FAILED: the release hand-off invariant is broken (see ::error:: lines above)."
  echo "Each one re-creates the v2.1.1 outcome: a tag that exists with no GitHub Release behind it."
  exit 1
fi

echo "OK: release hand-off intact — tags are handed to release.yml explicitly and built from the tag."
exit 0
