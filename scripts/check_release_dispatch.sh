#!/usr/bin/env bash
# Release hand-off guard — a release tag must reach release.yml exactly once.
#
# Invariant: the ONLY path from a release-plz tag to release.yml is the native
# `push: tags` trigger, fired because the tag was pushed with a GitHub App
# token; release.yml builds the tag it is handed.
#
# Why a tag push IS enough now (and was not before): tags pushed with
# GITHUB_TOKEN never deliver `push: tags` events — GitHub suppresses workflow
# runs for GITHUB_TOKEN-generated events — so the hand-off used to be an
# explicit workflow_dispatch call. Both release-plz jobs now mint an App
# token, and App-pushed tags fire `push: tags` like any human push. Keeping
# the explicit dispatch alongside would run release.yml TWICE per release
# (the sweep resolves in seconds, the builds take minutes, no concurrency
# group). Measured consequence of the missing call under the old wiring:
# v2.1.0 shipped its binaries only via a manual dispatch, and v2.1.1 ended
# with a tag and no GitHub Release at all.
#
# Why this is a guard and not just a comment: the failure mode is not a typo,
# it is a WHOLESALE REWRITE of release-plz.yml that quietly drops the App-token
# step (tags go silent again — the v2.1.1 shape) or reintroduces a dispatch job
# next to App-token pushes (duplicate concurrent release.yml runs). That is not
# hypothetical — PR #1455 (fix/release-plz-green-lie, closed unmerged) replaced
# the file's tail, and merging it as-is would have deleted the entire
# `dispatch-release` job while restoring a comment claiming the tag push
# triggers release.yml. A comment cannot fail a merge; this can.
#
# Scope: the structural invariants that caused the incident. Prose in these
# workflows is deliberately NOT asserted — comments change for good reasons,
# and a guard that fails on re-wording gets disabled instead of fixed. The one
# exception is the push-only gate on the version check (see check 14), because
# it is an expression, not prose, and it silently disables a validation.
#
# RELEASE_DISPATCH_ROOT overrides the scanned repository root, so fixtures can
# exercise this guard without touching the real workflows.
set -euo pipefail

REPO_ROOT="${RELEASE_DISPATCH_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

RELEASE_PLZ_YML="$REPO_ROOT/.github/workflows/release-plz.yml"
RELEASE_YML="$REPO_ROOT/.github/workflows/release.yml"
CUT_PATCH_YML="$REPO_ROOT/.github/workflows/cut-patch-tag.yml"
RECONCILE_YML="$REPO_ROOT/.github/workflows/release-reconcile.yml"
ENSURE_SH="$REPO_ROOT/scripts/ensure-release.sh"
RECONCILE_SH="$REPO_ROOT/scripts/reconcile-releases.sh"
SWEEP_SH="$REPO_ROOT/scripts/sweep-releases.sh"

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

echo "check_release_dispatch: verifying the release hand-off invariant (L1 provenance)"

# ─────────────────────────────────────────────────────────────────────────────
# 1. release-plz.yml: NO dispatch job — exactly one hand-off path.
# With App-token tag pushes firing `push: tags` natively, an explicit dispatch
# job would run release.yml a second time per release. A reintroduced
# `dispatch-release` job (or any second caller of the sweep core from this
# file) is the dual hand-off this guard exists to refuse.
# ─────────────────────────────────────────────────────────────────────────────
BLOCK="$(job_block "$RELEASE_PLZ_YML" dispatch-release || true)"
if [[ -n "$BLOCK" ]]; then
  echo "::error::check_release_dispatch: release-plz.yml has a 'dispatch-release' job alongside App-token tag pushes. The tag push already fires release.yml's push: tags trigger natively, so this job would run release.yml TWICE per release (sweep resolves in seconds, builds take minutes, no concurrency group). Delete the job; the native push is the only hand-off."
  step "release-plz.yml: dispatch-release job" "PRESENT (dual hand-off)"
  FAIL=1
else
  step "release-plz.yml: dispatch-release job" "absent (single hand-off)"
fi

# No second candidate loop may hide in this file either: the immediate path is
# the push event itself, so release-plz.yml must not resolve candidates at all —
# neither through the shared sweep core nor through a HEAD-scoped lookup (the
# latter finds nothing after a squash merge and was the webfang#1674 false
# green: job success in 6 s, zero release.yml runs).
if grep -qF -- "sweep-releases.sh" "$RELEASE_PLZ_YML"; then
  echo "::error::check_release_dispatch: release-plz.yml runs scripts/sweep-releases.sh. The immediate hand-off is the native tag push, not a sweep — a second candidate loop here drifts from the backstop's and re-opens the webfang#1674 divergence."
  step "release-plz.yml: no own candidate loop" "SWEEP REFERENCED"
  FAIL=1
elif grep -vE '^[[:space:]]*#' "$RELEASE_PLZ_YML" | grep -qF -- '--points-at HEAD'; then
  echo "::error::check_release_dispatch: release-plz.yml performs a 'git tag --points-at HEAD' lookup. That lookup is HEAD-scoped and finds nothing when a Release PR is merged by squash — the tag stays on the pre-squash commit — which is the measured webfang#1674 false green. Candidate resolution belongs in scripts/sweep-releases.sh over the shared trust predicate, on the backstop path only."
  step "release-plz.yml: no own candidate loop" "HEAD-SCOPED"
  FAIL=1
else
  step "release-plz.yml: no own candidate loop" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 2. release-plz.yml: the release job pushes its tag with an App token, scoped
# to contents write ONLY. Without this step the tag goes out under GITHUB_TOKEN
# and `push: tags` never fires — the v2.1.1 shape (a tag with no binaries). The
# pull-requests scope belongs to the release-pr job alone: this job never opens
# PRs, so granting it there is the widest-grant pattern removed in #1607.
# ─────────────────────────────────────────────────────────────────────────────
RELEASE_JOB="$(job_block "$RELEASE_PLZ_YML" release-plz-release)"
if [[ -z "$RELEASE_JOB" ]]; then
  echo "::error::check_release_dispatch: release-plz.yml has no 'release-plz-release' job to inspect — the guard cannot tell whether the tag is pushed with an App token. Update this guard if the job was renamed."
  step "release-plz.yml: release job uses an App token" "UNVERIFIABLE"
  FAIL=1
else
  while IFS='|' read -r needle why; do
    if grep -qF -- "$needle" <<< "$RELEASE_JOB"; then
      step "  release job: $why" "ok"
    else
      echo "::error::check_release_dispatch: the 'release-plz-release' job no longer has '$needle' ($why). Without the App-token step the tag is pushed under GITHUB_TOKEN, push: tags never fires, and the release lands as a tag with no binaries (v2.1.1)."
      step "  release job: $why" "MISSING"
      FAIL=1
    fi
  done <<< "actions/create-github-app-token@|the App-token action is used
id: app-token|the minted token has a stable step id
client-id:|the canonical v3 client-id input (never app-id)
private-key:|the App private key is supplied
permission-contents: write|the token is scoped to contents write
\${{ steps.app-token.outputs.token }}|the release-plz step is fed the App token"
  if grep -qF -- "permission-pull-requests" <<< "$RELEASE_JOB"; then
    echo "::error::check_release_dispatch: the 'release-plz-release' job grants permission-pull-requests. This job never opens PRs — the pull-requests scope belongs to the release-pr job alone. Least privilege: contents write only."
    step "  release job: no pull-requests scope" "OVER-SCOPED"
    FAIL=1
  else
    step "  release job: no pull-requests scope" "ok"
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# 3. Neither release-plz step may be fed secrets.GITHUB_TOKEN. A GITHUB_TOKEN-fed
# `release` step pushes a tag that cannot trigger release.yml (suppression);
# a GITHUB_TOKEN-fed `release-pr` step re-opens #1228 (Release PR checks stuck
# in action_required). Either one silently restores the incident this migration
# removed. (The old dispatch job's GH_TOKEN shape is refused too.)
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF -- 'GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}' "$RELEASE_PLZ_YML"; then
  echo "::error::check_release_dispatch: release-plz.yml feeds secrets.GITHUB_TOKEN to a release-plz step. Tags pushed under GITHUB_TOKEN never fire push: tags (suppression — v2.1.1), and PRs opened under it never deliver pull_request events (#1228). Both release-plz steps must run on the App token."
  step "release-plz.yml: no secrets.GITHUB_TOKEN step" "PRESENT"
  FAIL=1
else
  step "release-plz.yml: no secrets.GITHUB_TOKEN step" "ok"
fi
if grep -qF -- 'GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}' "$RELEASE_PLZ_YML"; then
  echo "::error::check_release_dispatch: release-plz.yml still carries the old dispatch shape (GH_TOKEN from secrets.GITHUB_TOKEN). The dispatch job is deleted; nothing in this file may push or call out under GITHUB_TOKEN."
  step "release-plz.yml: no dispatch-era GH_TOKEN" "PRESENT"
  FAIL=1
else
  step "release-plz.yml: no dispatch-era GH_TOKEN" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 4. release-plz.yml: the duplicate-tag 422 (webfang#1341) is tolerated ONLY
# behind a verification that can fail the job. Asserted as a triple: the
# verification is invoked, the failing step is allowed to continue so the
# verification is reachable at all, and the raw outcome is consumed.
# Losing the first turns the job red again; losing the second or third turns the
# tolerance into the green lie removed in webfang#1476.
# ─────────────────────────────────────────────────────────────────────────────
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
# 5. One trust predicate, consumed by EVERY decision point. The fingerprint that
# decides which tags may be trusted is security-relevant, and copies of it drift.
# It lives in scripts/release-plz-tags.sh, consumed by the verifier (which
# decides the release job's outcome) and by the sweep (which selects historical
# tags on the backstop path), so both agree.
# ─────────────────────────────────────────────────────────────────────────────
PREDICATE="scripts/release-plz-tags.sh"
VERIFIER_SH="$REPO_ROOT/scripts/verify-release-tag.sh"
# The backstop's trust predicate is reached through the shared sweep core
# (webfang#1674): the reconciler counts as a consumer of the predicate when
# it runs the core and the core reads the predicate.
if [[ -f "$VERIFIER_SH" ]] && grep -qF -- "$PREDICATE" "$VERIFIER_SH" \
   && [[ -f "$SWEEP_SH" ]] && grep -qF -- "$PREDICATE" "$SWEEP_SH"; then
  step "one trust predicate used by verifier + sweep" "ok"
else
  echo "::error::check_release_dispatch: the verifier or the sweep does not reach $PREDICATE. Either scripts/verify-release-tag.sh or scripts/sweep-releases.sh stopped reading the predicate. A duplicated trust predicate drifts, and the two would then disagree about which tags are safe to release."
  step "one trust predicate used by verifier + sweep" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 6. The reconciliation sweep (webfang#1484) is now the ONLY dispatching path
# besides the native push — and the daily backstop for it. A push trigger that
# never fires produces no workflow run, so nothing that reads run STATES can
# observe it — the failure is silent by construction, which is how v2.1.1 ended
# as a tag with no binaries until a human noticed. The sweep is the only thing
# that looks at the OUTCOME instead. Asserted: the workflow is scheduled and
# runs the sweep, the sweep uses the shared trust predicate over history, and
# the reconciler delegates to the shared dispatch helper rather than rolling
# its own.
#
# The sweep's BEHAVIOUR (that it retries a transient failure, that it never
# dispatches a human tag) is proven by scripts/test_release_reconcile.sh, not by
# grepping for a loop here.
# ─────────────────────────────────────────────────────────────────────────────
if [[ -f "$RECONCILE_YML" ]] && grep -qE '^[[:space:]]*schedule:' "$RECONCILE_YML" \
   && grep -qF -- "scripts/reconcile-releases.sh" "$RECONCILE_YML"; then
  step "release-reconcile.yml: scheduled sweep wired" "ok"
else
  echo "::error::check_release_dispatch: the reconciliation sweep is missing, unscheduled, or not invoked. Without it a push trigger that never fires stays invisible forever: no run to observe, no retry, and the only recovery is a human spotting a tag with no binaries (webfang#1484)."
  step "release-reconcile.yml: scheduled sweep wired" "MISSING"
  FAIL=1
fi

if [[ -f "$RECONCILE_SH" ]] && grep -qF -- "$PREDICATE" "$SWEEP_SH" \
   && grep -qF -- '--all' "$SWEEP_SH"; then
  step "sweep uses the shared trust predicate (--all)" "ok"
else
  echo "::error::check_release_dispatch: the shared sweep core (scripts/sweep-releases.sh) does not resolve candidates via $PREDICATE --all. A sweep over raw tag history would dispatch human tags too (v1.0.0 has no Release), publishing binaries built from unrelated code."
  step "sweep uses the shared trust predicate (--all)" "MISSING"
  FAIL=1
fi

# The backstop must run the sweep core, not its own copy of the candidate loop.
# That is the whole reason the core exists: two callers resolving candidates
# independently is what let the immediate path green-light a squash-merged
# Release PR (webfang#1674) while the reconciler knew the tag had no binaries.
if [[ -f "$RECONCILE_SH" ]] && grep -qF -- "sweep-releases.sh" "$RECONCILE_SH"; then
  step "backstop runs the shared sweep core" "ok"
else
  echo "::error::check_release_dispatch: the reconciliation backstop (scripts/reconcile-releases.sh) must run scripts/sweep-releases.sh. A second copy of the candidate loop drifts, and that drift is the measured webfang#1674 defect: the immediate path resolved with 'git tag --points-at HEAD', found nothing after a squash merge, and reported a 0-second success with no binaries."
  step "backstop runs the shared sweep core" "MISSING"
  FAIL=1
fi

# L1.1 Shape 2: the sweep must resolve expected_sha and hand it to
# ensure-release.sh — a dispatch without it dies at release.yml preflight
# ("default-branch dispatch requires PROV_EXPECTED_SHA input"), the exact
# fail-closed that would leave every incomplete historical release
# permanently binary-less (webfang#1540).
# shellcheck disable=SC2016  # intentional: match the literal $expected_sha
# token as written in sweep-releases.sh, not an expanded value.
if [[ -f "$SWEEP_SH" ]] \
   && grep -qF -- 'git rev-parse "refs/tags/' "$SWEEP_SH" \
   && grep -qE -- 'ensure-release\.sh".*\$expected_sha|ensure-release\.sh".*\$EXPECTED_SHA' "$SWEEP_SH"; then
  step "sweep resolves and passes expected_sha (L1.1 Shape 2)" "ok"
else
  echo "::error::check_release_dispatch: scripts/sweep-releases.sh must resolve the tag commit via git rev-parse \"refs/tags/<tag>^{commit}\" and pass it as the second argument to ensure-release.sh. Without expected_sha the default-branch dispatch fails closed at L1.1 Identity (webfang#1540)."
  step "sweep resolves and passes expected_sha (L1.1 Shape 2)" "MISSING"
  FAIL=1
fi

# The false green (webfang#1674): a sweep run that found nothing to do must
# never be an unremarkable exit 0, and a run that is genuinely broken must not
# be one either. Asserted as structure: the visible skip plus the hard error
# for a release commit whose tag does not exist.
if [[ -f "$SWEEP_SH" ]] \
  && grep -qF -- '::notice title=Release dispatch' "$SWEEP_SH" \
  && grep -qF -- 'GITHUB_STEP_SUMMARY' "$SWEEP_SH" \
  && grep -qF -- 'head_is_release_commit' "$SWEEP_SH"; then
  step "no-op is a visible skip; untagged release commit is red" "ok"
else
  echo "::error::check_release_dispatch: scripts/sweep-releases.sh must announce a no-op run as a visible skip (a ::notice:: annotation plus a GITHUB_STEP_SUMMARY entry) and must exit 1 when HEAD is a release commit but history holds no trusted tag. A silent exit 0 over a sweep that found nothing is the false green of webfang#1674 — worse than a red, because it fires no alert."
  step "no-op is a visible skip; untagged release commit is red" "MISSING"
  FAIL=1
fi

# shellcheck disable=SC2016  # intentional: the $REPO_ROOT here is the literal
# token as written in sweep-releases.sh, not a value expanded by the guard.
if [[ -f "$ENSURE_SH" ]] \
   && grep -qE 'bash "?\$REPO_ROOT/scripts/ensure-release\.sh|bash scripts/ensure-release\.sh' "$SWEEP_SH"; then
  step "sweep delegates to the shared dispatch helper" "ok"
else
  echo "::error::check_release_dispatch: the sweep no longer reaches scripts/ensure-release.sh. The expected-asset list, the idempotency check and the bounded retry would then exist in two places and drift, so one of the two paths could dispatch without retrying."
  step "sweep delegates to the shared dispatch helper" "MISSING"
  FAIL=1
fi

# The backstop dispatch must hand the tag as an INPUT, and must NOT pin `--ref`.
# The input is what pins the artifact (release.yml points both checkouts at
# `inputs.tag || github.ref_name`); `--ref <tag>` would instead make the run
# execute that tag's HISTORICAL release.yml, which matters because this helper
# is shared with the history-scoped reconciliation sweep: a sweep for an old
# tag would then run the pipeline as it existed back then.
# Scoped to the INVOCATION, never the whole file: the helper also contains the
# string `-f tag=` inside its recovery error message, so a file-wide grep still
# passed after the real input was removed (measured — that is why this extracts
# the call first). An invocation is `gh workflow run ...` plus its continuations
# (either backslash-continued lines ending with ; OR array form ${DISPATCH_CMD[@]}
# with optional expected_sha, executed directly or in subshell).
DISPATCH_INVOCATION="$(perl -0ne 'print $& if /gh workflow run release\.yml(?:[^\n]*\\\n){0,6}[^\n]*;|gh workflow run release\.yml.*?\$\{DISPATCH_CMD\[@\]\}/s' "$ENSURE_SH" 2>/dev/null || true)"
if [[ -z "$DISPATCH_INVOCATION" ]]; then
  echo "::error::check_release_dispatch: could not extract the 'gh workflow run release.yml' invocation from scripts/ensure-release.sh, so the pinning of the build cannot be verified. Fail-closed on purpose: a check that cannot see the invocation must not report success."
  step "  dispatch passes the tag as an input (shared helper)" "UNVERIFIABLE"
  FAIL=1
elif grep -qF -- '-f tag=' <<<"$DISPATCH_INVOCATION" \
  && ! grep -qE -- '(^|[[:space:]])--ref([[:space:]]|$)' <<<"$DISPATCH_INVOCATION"; then
  step "  dispatch passes the tag as an input (shared helper)" "ok"
else
  echo "::error::check_release_dispatch: scripts/ensure-release.sh must call 'gh workflow run release.yml ... -f tag=<tag>' and must NOT pass '--ref <tag>'. The input is what pins the build (release.yml points both checkouts at it); '--ref' would instead run that tag's historical release.yml, so a reconciliation sweep for an old tag would not use the hardened pipeline."
  step "  dispatch passes the tag as an input (shared helper)" "MISSING"
  FAIL=1
fi

# L1 Provenance: ensure-release.sh must pass expected_sha when available
if grep -qF -- 'expected_sha' <<<"$DISPATCH_INVOCATION"; then
  step "  dispatch passes expected_sha (L1 provenance)" "ok"
else
  echo "::error::check_release_dispatch: scripts/ensure-release.sh must accept and pass expected_sha for L1.1 Identity (Shape 2: default-branch dispatch). Update ensure-release.sh to pass -f expected_sha=\$EXPECTED_SHA when provided."
  step "  dispatch passes expected_sha (L1 provenance)" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 7. release.yml: the native half of the hand-off. The App-pushed tag only
# becomes binaries if `push: tags` still triggers this workflow — a rewrite
# that drops the trigger restores the silent v2.1.1 shape with no dispatch
# job left to compensate.
# ─────────────────────────────────────────────────────────────────────────────
if grep -qE '^[[:space:]]*tags:' "$RELEASE_YML" \
   && grep -qF -- '"v*"' "$RELEASE_YML"; then
  step "release.yml: push: tags trigger present" "ok"
else
  echo "::error::check_release_dispatch: release.yml no longer triggers on push: tags v*. The App-token tag push is now the ONLY hand-off, so without this trigger every release lands as a tag with no binaries and nothing dispatches it."
  step "release.yml: push: tags trigger present" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 8. release.yml: EVERY checkout is pinned — either to the input tag (preflight)
# or to the validated commit from preflight (build). Both satisfy L1.1 Identity:
# the preflight resolves the tag, the build compiles the validated commit.
# An unpinned checkout (ref: main, ref: branch) would compile unrelated code.
# ─────────────────────────────────────────────────────────────────────────────
CHECKOUTS="$(grep -cE '^[[:space:]]*(-[[:space:]]+)?uses: actions/checkout@' "$RELEASE_YML" || true)"
# Valid pins: either the tag input OR the validated_commit output from preflight
PINNED="$(grep -cE '^[[:space:]]*ref:[[:space:]]*.*(github\.event\.inputs\.tag|needs\.preflight\.outputs\.validated_commit)' "$RELEASE_YML" || true)"
if [[ "$CHECKOUTS" -eq 0 ]]; then
  echo "::error::check_release_dispatch: release.yml has no 'uses: actions/checkout@' step — the guard can no longer verify the checkout pin. Update this guard if checkout moved to a composite action."
  step "release.yml: checkout pin" "UNVERIFIABLE"
  FAIL=1
elif [[ "$PINNED" -lt "$CHECKOUTS" ]]; then
  echo "::error::check_release_dispatch: release.yml has $CHECKOUTS checkout step(s) but only $PINNED pinned 'ref:'. Every checkout must pin to either the tag input or the validated_commit output. An unpinned checkout compiles the ref the run started from (a branch) and publishes those binaries under the tag."
  step "release.yml: checkout pin $PINNED/$CHECKOUTS" "UNPINNED"
  FAIL=1
else
  step "release.yml: all $CHECKOUTS checkouts pinned (tag or validated_commit)" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 9. release.yml: the version check is not gated back to push-only.
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
  echo "::error::check_release_dispatch: a step in release.yml's 'preflight' job is gated with 'if: github.event_name == ''push''' — a push-only gate silently skips that validation on the workflow_dispatch path, which is now the path every backstop release takes."
  step "release.yml: preflight has no push-only gate" "MISSING"
  FAIL=1
else
  step "release.yml: preflight has no push-only gate" "ok"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 10. L1 Provenance: release.yml preflight job calls check_release_provenance.sh preflight
#    and outputs validated_commit, expected_line, tag, prerelease.
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF 'check_release_provenance.sh preflight' "$RELEASE_YML" \
   && grep -qF 'validated_commit' "$RELEASE_YML" \
   && grep -qF 'expected_line' "$RELEASE_YML" \
   && grep -qF 'prerelease' "$RELEASE_YML"; then
  step "release.yml: L1 preflight calls check_release_provenance.sh" "ok"
else
  echo "::error::check_release_dispatch: release.yml preflight must call 'check_release_provenance.sh preflight' and output validated_commit, expected_line, tag, prerelease for L1.1–L1.3 + L1.5."
  step "release.yml: L1 preflight calls check_release_provenance.sh" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 11. L1 Provenance: release.yml build job checks out validated_commit (not tag ref)
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF 'needs.preflight.outputs.validated_commit' "$RELEASE_YML"; then
  step "release.yml: build job checks out validated_commit" "ok"
else
  echo "::error::check_release_dispatch: release.yml build job must check out 'needs.preflight.outputs.validated_commit' to re-assert L1.1 Identity at build time."
  step "release.yml: build job checks out validated_commit" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 12. L1 Provenance: release.yml publish job runs check_release_provenance.sh publish
#     for L1.4 TOCTOU re-read and includes provenance attestation in release body.
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF 'check_release_provenance.sh publish' "$RELEASE_YML" \
   && grep -qF 'Provenance Attestation' "$RELEASE_YML"; then
  step "release.yml: L1 publish runs TOCTOU check + attestation" "ok"
else
  echo "::error::check_release_dispatch: release.yml publish job must run 'check_release_provenance.sh publish' for L1.4 TOCTOU and include provenance attestation in release body."
  step "release.yml: L1 publish runs TOCTOU check + attestation" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 13. L1 Provenance: release.yml workflow_dispatch accepts expected_sha input
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF 'expected_sha' "$RELEASE_YML"; then
  step "release.yml: workflow_dispatch accepts expected_sha input" "ok"
else
  echo "::error::check_release_dispatch: release.yml workflow_dispatch must declare an 'expected_sha' input for L1.1 Identity Shape 2 (default-branch dispatch)."
  step "release.yml: workflow_dispatch accepts expected_sha input" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 14. L1 Provenance: release.yml checkouts use fetch-depth: 0 + fetch-tags: true
# ─────────────────────────────────────────────────────────────────────────────
FETCH_DEPTH_0_COUNT="$(grep -cE 'fetch-depth:[[:space:]]*0' "$RELEASE_YML" || true)"
FETCH_TAGS_COUNT="$(grep -cE 'fetch-tags:[[:space:]]*true' "$RELEASE_YML" || true)"
if [[ "$FETCH_DEPTH_0_COUNT" -ge "$CHECKOUTS" && "$FETCH_TAGS_COUNT" -ge "$CHECKOUTS" ]]; then
  step "release.yml: all $CHECKOUTS checkouts use fetch-depth: 0 + fetch-tags: true" "ok"
else
  echo "::error::check_release_dispatch: release.yml has $CHECKOUTS checkout(s) but only $FETCH_DEPTH_0_COUNT with fetch-depth: 0 and $FETCH_TAGS_COUNT with fetch-tags: true. L1 provenance requires full history + tags for git merge-base and release-plz-tags.sh --tag."
  step "release.yml: fetch-depth: 0 + fetch-tags: true" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 15. cut-patch-tag.yml: the second tag-creating path has the same obligation.
# It pushes vX.Y.Z with GITHUB_TOKEN, so it is affected by the same suppression
# and must hand the tag over explicitly (webfang#1480).
# NOW WITH L1: must create ANNOTATED tag (git tag -a), dispatch WITHOUT --ref,
# and pass expected_sha input.
# ─────────────────────────────────────────────────────────────────────────────
if grep -qE 'git[[:space:]]+-c[[:space:]]+user\.name' "$CUT_PATCH_YML" \
   && grep -qE 'tag[[:space:]]+-a' "$CUT_PATCH_YML" \
   && grep -qE 'gh workflow run release\.yml' "$CUT_PATCH_YML" \
   && ! grep -qE '^\s*--ref\b' "$CUT_PATCH_YML" \
   && grep -qF 'expected_sha' "$CUT_PATCH_YML" \
   && grep -qF 'git rev-parse' "$CUT_PATCH_YML"; then
  step "cut-patch-tag.yml: annotated tag, dispatches release.yml without --ref, passes expected_sha" "ok"
else
  echo "::error::check_release_dispatch: cut-patch-tag.yml must (1) create annotated tag (git tag -a), (2) dispatch release.yml WITHOUT --ref, (3) pass expected_sha input, (4) compute tag commit via git rev-parse. Its tag push is suppressed exactly like release-plz's, so a PATCH release on a support branch would ship without binaries or L1 provenance."
  step "cut-patch-tag.yml: annotated tag + dispatch without --ref + expected_sha" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# 16. ensure-release.sh: accepts expected_sha and passes it in workflow_dispatch
# ─────────────────────────────────────────────────────────────────────────────
if grep -qF 'EXPECTED_SHA' "$ENSURE_SH" \
   && grep -qF 'expected_sha' "$ENSURE_SH"; then
  step "ensure-release.sh: accepts and passes expected_sha" "ok"
else
  echo "::error::check_release_dispatch: scripts/ensure-release.sh must accept expected_sha as second argument and pass it via -f expected_sha= in the dispatch."
  step "ensure-release.sh: accepts and passes expected_sha" "MISSING"
  FAIL=1
fi

# ─────────────────────────────────────────────────────────────────────────────
# Verdict
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$FAIL" -ne 0 ]]; then
  echo "FAILED: the release hand-off invariant is broken (see ::error:: lines above)."
  echo "Each one re-creates either the v2.1.1 outcome (a tag that exists with no GitHub Release behind it) or its mirror image under the new wiring (two concurrent release.yml runs per release)."
  exit 1
fi

echo "OK: single native-push hand-off intact — App-pushed tags fire push: tags, the backstop sweep still watches the outcome."
exit 0
