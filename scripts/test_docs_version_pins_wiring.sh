#!/usr/bin/env bash
# test_docs_version_pins_wiring.sh
#
# Semantics harness for the WIRING of scripts/check_docs_version_pins.sh, the
# docs version-pin drift guard. Companion to test_docs_version_pins.sh, which
# pins the guard's SEMANTICS; this file pins that the guard is connected to
# anything at all.
#
# Why this file exists: the guard shipped in #2012 and stayed inert for a whole
# release. Its semantics were right; no workflow in .github/ invoked it, so a
# tree carrying nine stale install recipes stayed green. The defect was not in
# the guard — it was in the absence of an edge. #2026 added the edge and nothing
# pinned it: a deleted step is valid YAML, and a green run with no guard looks
# exactly like a green run with one. That is the regression class here.
#
# What it proves, offline and non-vacuously (a check that cannot see its input
# fails instead of passing):
#
#   - .github/workflows/ci.yml runs the gate in `repo-guards`, a job with no
#     `paths:` filter and no `if:` condition, so drift reds a required check on
#     EVERY PR — including a markdown-only one, which is the side the drift
#     actually starts from (the Cargo.toml side).
#   - ...and `gate` names `repo-guards` in both its `needs:` and its narrow-scope
#     always-applicable list. Without the first, the failure never reaches the
#     aggregator; without the second, a docs-only PR would not gate on it at all.
#   - scripts/ci_fast_gate.sh mirrors the gate in the two lanes that see its real
#     trigger surfaces (`lane_docs` for a markdown edit, `lane_fmt_and_guards`
#     for a version bump) and runs this repo's version-pins harnesses in the
#     CI-ONLY lane — the only lane a `scripts/**` change ever lands in.
#   - "Mirrors" means EXECUTES, not mentions, in all three lanes: a lane that only
#     names a path in a comment is not wired, and the negative controls below
#     (cases 11 and 12) keep that true for the gate and for the lane_ci harnesses.
#
# Usage: bash scripts/test_docs_version_pins_wiring.sh [ci-yml] [fast-gate]
# The optional paths exist so each check can be pointed at a mutated revision for
# its negative controls; they default to the repo's copies.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CI_YML="${1:-$ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$ROOT/scripts/ci_fast_gate.sh}"

GATE_SCRIPT="scripts/check_docs_version_pins.sh"
SEMANTICS_SCRIPT="scripts/test_docs_version_pins.sh"
WIRING_SCRIPT="scripts/test_docs_version_pins_wiring.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

declare -i cases_run=0 cases_failed=0 case_fail=0

# job_block <workflow> <job> — the YAML block of one job: its 2-space key down to
# (not including) the next 2-space key. Prints nothing when the job is absent,
# which callers treat as a blind harness rather than a pass.
job_block() {
  awk -v job="$2" '
    $0 == "  " job ":" { in_job = 1; print; next }
    in_job && /^  [a-z0-9_-]+:/ { exit }
    in_job { print }
  ' "$1"
}

# job_header <block-file> — the job block up to its `steps:` key. Job-level keys
# live at 4-space indentation there, so a `paths:` or `if:` appearing inside a
# step's script body cannot be mistaken for a job-level one.
job_header() {
  awk '/^    steps:/ { exit } { print }'
}

# lane_block <fast-gate> <function> — the body of one bash function: its
# `name() {` line down to the first column-0 `}`.
lane_block() {
  awk -v fn="$2" '
    $0 == fn "() {" { in_fn = 1; print; next }
    in_fn && /^}/ { print; exit }
    in_fn { print }
  ' "$1"
}

# check_ci_wiring <workflow-path> — nonzero when the gate is present in the tree
# but not connected to a required check, one line per reason.
check_ci_wiring() {
  local wf="$1" bad=0 script
  local block="$tmp/ci-repo-guards.yml"
  local header="$tmp/ci-repo-guards-header.yml"
  local gate_block="$tmp/ci-gate.yml"
  job_block "$wf" repo-guards > "$block"
  if [[ ! -s "$block" ]]; then
    echo "job 'repo-guards' not found: a renamed job would take the gate with it, unnoticed"
    return 1
  fi
  job_header < "$block" > "$header"
  if grep -qE '^    paths(-ignore)?:' "$header"; then
    echo "job 'repo-guards' is path-filtered: a PR outside the filter would not run the gate at all"
    bad=1
  fi
  if grep -qE '^    if:' "$header"; then
    echo "job 'repo-guards' is conditional: the gate is skipped whenever that condition is false"
    bad=1
  fi
  for script in "$GATE_SCRIPT" "$SEMANTICS_SCRIPT" "$WIRING_SCRIPT"; do
    if ! grep -qF -e "run:" "$block" || ! grep -qE "run:[[:space:]]*.*${script//./\\.}" "$block"; then
      echo "no step in 'repo-guards' runs $script: the script is in the tree and connected to nothing"
      bad=1
    fi
  done
  job_block "$wf" gate > "$gate_block"
  if [[ ! -s "$gate_block" ]]; then
    echo "job 'gate' not found: the required aggregator cannot be checked"
    return 1
  fi
  if ! grep -qE '^    needs:.*(^|[^a-z-])repo-guards([^a-z-]|$)' "$gate_block"; then
    echo "'gate' does not list 'repo-guards' in needs: a failure there never reaches the required CI Gate"
    bad=1
  fi
  # The narrow-scope branch runs the always-applicable producers first and the
  # skipped code producers second; the first `for job in` line is the one that
  # has to carry the gate. `clippy` is the anchor that tells the two apart: it is
  # never applicable in narrow scope, so finding it in the first list means the
  # lists moved and this check is reading the wrong one.
  local applicable
  applicable="$(grep -E '^[[:space:]]*for job in ' "$gate_block" | head -n1)"
  if [[ -z "$applicable" ]]; then
    echo "'gate' has no narrow-scope producer list: the applicable set cannot be checked and the gate may be scope-blind"
    bad=1
  elif grep -qw clippy <<< "$applicable"; then
    echo "'gate' narrow-scope lists were reordered: the always-applicable list is no longer first, so this check is blind — re-derive it"
    bad=1
  elif ! grep -qw repo-guards <<< "$applicable"; then
    echo "'gate' narrow-scope always-applicable list omits 'repo-guards': a docs-only PR would not gate on the drift at all"
    bad=1
  fi
  [[ $bad -eq 0 ]]
}

# check_fast_gate_wiring <fast-gate-path> — nonzero when the local mirror has
# lost one of its edges, one line per reason.
check_fast_gate_wiring() {
  local fg="$1" bad=0 lane script trigger escaped
  for lane in lane_docs lane_ci lane_fmt_and_guards; do
    lane_block "$fg" "$lane" > "$tmp/fg-$lane.sh"
    if [[ ! -s "$tmp/fg-$lane.sh" ]]; then
      echo "function '$lane' not found in the fast gate: the local mirror cannot be checked"
      return 1
    fi
  done
  for lane in lane_docs lane_fmt_and_guards; do
    # A path named in a comment is not a wiring. The check has to see an
    # EXECUTION line: the house form in those lanes is `run_guard "<name>"
    # <script>` followed by `bash <script>`, and either shape counts — a comment
    # never does (verified gap: a lane whose only mention was a comment used to
    # satisfy the old path-only check).
    escaped="${GATE_SCRIPT//./\\.}"
    if ! grep -vE '^[[:space:]]*#' "$tmp/fg-$lane.sh" \
      | grep -qE "run_guard .*docs version pins gate|bash ${escaped}"; then
      echo "lane '$lane' does not run $GATE_SCRIPT: a markdown edit or a version bump would pass locally with the drift unnamed"
      bad=1
    fi
  done
  # Same rule as the two lanes above, one lane over: a path named in a comment is
  # not a wiring, and neither is a path on a line that does not execute it. The
  # house form in this lane is `run_step "<name>" bash <script>` (or a `run_guard
  # "<name>" <script>` whose wrapped command is that same `bash <script>`), so the
  # check requires an EXECUTION line. Verified gap (#2027 advisories
  # R2-lane-ci-comment-gap / R3-002): comparing the path alone let a comment
  # satisfy this check while the harness never ran. Case 12 pins it.
  for script in "$SEMANTICS_SCRIPT" "$WIRING_SCRIPT"; do
    escaped="${script//./\\.}"
    if ! grep -vE '^[[:space:]]*#' "$tmp/fg-lane_ci.sh" \
      | grep -qE "run_guard .*${escaped}|bash ${escaped}"; then
      echo "lane_ci does not run $script: a scripts/** change — the only kind that can break it — would not exercise it locally"
      bad=1
    fi
  done
  trigger="$(grep -E 'grep -E' "$tmp/fg-lane_ci.sh" | grep -F -e check_docs_version_pins | head -n1)"
  if [[ -z "$trigger" ]]; then
    echo "lane_ci has no trigger naming the docs version pins files: the harnesses are either unconditional or unreachable"
  fi
  # An absent trigger and a trigger that lost one alternative are the same defect
  # for every script it no longer matches, so both shapes report per script.
  for script in "$GATE_SCRIPT" "$SEMANTICS_SCRIPT" "$WIRING_SCRIPT"; do
    escaped="${script//./\\.}"
    if [[ -z "$trigger" ]] || ! grep -qF -e "$escaped" <<< "$trigger"; then
      echo "lane_ci's trigger does not match $script: a PR touching it would skip the guard's harnesses"
      bad=1
    fi
  done
  [[ $bad -eq 0 ]]
}

# finish_case <label> — account and report one completed case.
finish_case() {
  local label="$1"
  cases_run+=1
  if [[ $case_fail -eq 0 ]]; then
    echo "ok: case $cases_run — $label"
  else
    cases_failed+=1
    echo "FAIL: case $cases_run — $label"
  fi
  case_fail=0
}

# --- cases ------------------------------------------------------------------

# Case 1: the committed workflow connects the gate to a required check.
if check_ci_wiring "$CI_YML" > "$tmp/out.txt" 2>&1; then
  echo "  ok: gate runs in an unfiltered, unconditional job that 'gate' waits on in both scopes"
else
  echo "  FAIL: committed wiring does not reach a required check"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "committed ci.yml wiring reaches the required CI Gate"

# Case 2: negative control — the shipped defect, reproduced as a test input. The
# step is deleted; the workflow stays valid YAML and only this harness notices.
sed '/^      - name: Docs version pins gate$/,+1d' "$CI_YML" > "$tmp/ci-no-step.yml"
if cmp -s "$CI_YML" "$tmp/ci-no-step.yml"; then
  echo "  FAIL: fixture injection did not apply — the sed anchor missed and this case proved nothing"
  case_fail=1
elif check_ci_wiring "$tmp/ci-no-step.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a deleted gate step passed — the guard can go inert again unnoticed"
  case_fail=1
elif grep -qF -e "$GATE_SCRIPT" "$tmp/out.txt"; then
  echo "  ok: deleted step rejected and named"
else
  echo "  FAIL: rejected, but the missing script was not named"
  case_fail=1
fi
finish_case "negative control: gate step deleted from repo-guards"

# Case 3: negative control — a `paths:` filter on the job. Drift starts on the
# Cargo.toml side, so a filter that admits only markdown is exactly the shape
# that hides it.
sed 's/^  repo-guards:$/  repo-guards:\n    paths:\n      - "docs\/**"/' "$CI_YML" > "$tmp/ci-filtered.yml"
if cmp -s "$CI_YML" "$tmp/ci-filtered.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_ci_wiring "$tmp/ci-filtered.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a path-filtered gate job passed"
  case_fail=1
elif grep -q -e 'path-filtered' "$tmp/out.txt"; then
  echo "  ok: path filter rejected and named"
else
  echo "  FAIL: rejected, but the filter was not named"
  case_fail=1
fi
finish_case "negative control: repo-guards path-filtered"

# Case 4: negative control — an `if:` condition on the job.
sed 's/^  repo-guards:$/  repo-guards:\n    if: ${{ github.event_name == "pull_request" }}/' "$CI_YML" > "$tmp/ci-conditional.yml"
if cmp -s "$CI_YML" "$tmp/ci-conditional.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_ci_wiring "$tmp/ci-conditional.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a conditional gate job passed"
  case_fail=1
elif grep -q -e 'conditional' "$tmp/out.txt"; then
  echo "  ok: job-level condition rejected and named"
else
  echo "  FAIL: rejected, but the condition was not named"
  case_fail=1
fi
finish_case "negative control: repo-guards made conditional"

# Case 5: negative control — `gate` stops waiting on the job. This is the
# silently-green shape: the step runs, fails, and nothing required ever looks.
sed 's/^    needs: \[change-scope, toolchain, repo-guards,/    needs: [change-scope, toolchain,/' "$CI_YML" > "$tmp/ci-unneeded.yml"
if cmp -s "$CI_YML" "$tmp/ci-unneeded.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_ci_wiring "$tmp/ci-unneeded.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a gate that does not wait on repo-guards passed"
  case_fail=1
elif grep -q -e 'needs' "$tmp/out.txt"; then
  echo "  ok: missing needs: edge rejected and named"
else
  echo "  FAIL: rejected, but the missing edge was not named"
  case_fail=1
fi
finish_case "negative control: gate does not depend on repo-guards"

# Case 6: negative control — the job drops out of the narrow-scope applicable
# set, which is the docs-only PR path.
sed 's/for job in change-scope toolchain repo-guards security doc-quality; do/for job in change-scope toolchain security doc-quality; do/' "$CI_YML" > "$tmp/ci-narrow-blind.yml"
if cmp -s "$CI_YML" "$tmp/ci-narrow-blind.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_ci_wiring "$tmp/ci-narrow-blind.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a narrow-scope list without repo-guards passed"
  case_fail=1
elif grep -q -e 'narrow-scope' "$tmp/out.txt"; then
  echo "  ok: narrow-scope omission rejected and named"
else
  echo "  FAIL: rejected, but the narrow scope was not named"
  case_fail=1
fi
finish_case "negative control: repo-guards out of the narrow-scope set"

# Case 7: the committed fast gate mirrors the gate in both lanes that see it.
if check_fast_gate_wiring "$FAST_GATE" > "$tmp/out.txt" 2>&1; then
  echo "  ok: gate mirrored in lane_docs + lane_fmt_and_guards, harnesses in lane_ci"
else
  echo "  FAIL: committed fast-gate wiring is incomplete"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "committed fast-gate wiring mirrors the gate and its harnesses"

# Case 8: negative control — the gate is dropped from lane_docs only, so the
# markdown side loses its local mirror while the version-bump side keeps one.
awk '
  /^lane_docs\(\) \{/ { in_fn = 1 }
  in_fn && /run_guard "docs version pins gate/ { skip = 2 }
  skip > 0 { skip--; next }
  { print }
  in_fn && /^\}/ { in_fn = 0 }
' "$FAST_GATE" > "$tmp/fg-no-lane-docs.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-no-lane-docs.sh"; then
  echo "  FAIL: fixture injection did not apply — the awk anchor missed and this case proved nothing"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-no-lane-docs.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: lane_docs without the gate passed"
  case_fail=1
elif grep -q -e 'lane_docs' "$tmp/out.txt"; then
  echo "  ok: gate dropped from lane_docs is rejected and named"
else
  echo "  FAIL: rejected, but the lane was not named"
  case_fail=1
fi
finish_case "negative control: gate dropped from lane_docs"

# Case 9: negative control — the lane_ci trigger stops matching the guard, so a
# `scripts/**` PR changes the guard and runs nothing.
sed 's#scripts/check_docs_version_pins\\\.sh|##' "$FAST_GATE" > "$tmp/fg-narrow-trigger.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-narrow-trigger.sh"; then
  echo "  FAIL: fixture injection did not apply — the trigger pattern moved and this case proved nothing"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-narrow-trigger.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a trigger that no longer matches the guard passed"
  case_fail=1
elif grep -qF -e "$GATE_SCRIPT" "$tmp/out.txt"; then
  echo "  ok: trigger that lost the guard is rejected and named"
else
  echo "  FAIL: rejected, but the unmatched script was not named"
  case_fail=1
fi
finish_case "negative control: lane_ci trigger stops matching the guard"

# Case 10: negative control — this file's own step is removed from lane_ci. A
# wiring harness nobody runs is the same defect one level up.
sed '/run_step "docs version pins wiring (offline)"/d' "$FAST_GATE" > "$tmp/fg-no-wiring.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-no-wiring.sh"; then
  echo "  FAIL: fixture injection did not apply — the sed anchor missed and this case proved nothing"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-no-wiring.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: lane_ci without the wiring harness passed"
  case_fail=1
elif grep -qF -e "$WIRING_SCRIPT" "$tmp/out.txt"; then
  echo "  ok: wiring harness dropped from lane_ci is rejected and named"
else
  echo "  FAIL: rejected, but the harness was not named"
  case_fail=1
fi
finish_case "negative control: wiring harness dropped from lane_ci"

# Case 11: negative control — the gate survives in lane_docs only as a COMMENT
# that names its path. A comment is not an edge, and a path-only check cannot
# tell the difference: this case exists because an independent verifier proved
# that the first version of the lane check could be satisfied by a comment.
awk -v script="$GATE_SCRIPT" '
  /^lane_docs\(\) \{/ { in_fn = 1 }
  in_fn && /run_guard "docs version pins gate/ {
    skip = 2
    print "  # see " script " for the docs version pin drift check"
    next
  }
  skip > 0 { skip--; next }
  { print }
  in_fn && /^\}/ { in_fn = 0 }
' "$FAST_GATE" > "$tmp/fg-comment-only.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-comment-only.sh"; then
  echo "  FAIL: fixture injection did not apply — the awk anchor missed and this case proved nothing"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-comment-only.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a lane that only MENTIONS the gate in a comment passed — that is the exact gap this case closes"
  case_fail=1
elif grep -q -e 'lane_docs' "$tmp/out.txt"; then
  echo "  ok: comment-only mention in lane_docs is rejected and named"
else
  echo "  FAIL: rejected, but the lane was not named"
  case_fail=1
fi
finish_case "negative control: gate only mentioned in a comment"

# Case 12: negative control — the wiring harness survives in lane_ci only as a
# COMMENT that names its path. Same class as case 11, one lane over: the lane_ci
# check compared the path alone while the two lane checks above required an
# execution line, so a comment naming the script satisfied it (review advisories
# R2-lane-ci-comment-gap and R3-002 on #2027). A future edit that comments the
# run_step out — or a lane rewritten around a mention — would have left the wiring
# guard green while the harness never ran.
awk -v script="$WIRING_SCRIPT" '
  /^lane_ci\(\) \{/ { in_fn = 1 }
  in_fn && /run_step "docs version pins wiring \(offline\)"/ {
    print "    # see " script " for the wiring harness"
    next
  }
  { print }
  in_fn && /^\}/ { in_fn = 0 }
' "$FAST_GATE" > "$tmp/fg-lane-ci-comment-only.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-lane-ci-comment-only.sh"; then
  echo "  FAIL: fixture injection did not apply — the awk anchor missed and this case proved nothing"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-lane-ci-comment-only.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: lane_ci that only MENTIONS the harness in a comment passed — that is the exact gap this case closes"
  case_fail=1
elif grep -qF -e "$WIRING_SCRIPT" "$tmp/out.txt"; then
  echo "  ok: comment-only mention in lane_ci is rejected and named"
else
  echo "  FAIL: rejected, but the harness was not named"
  case_fail=1
fi
finish_case "negative control: wiring harness only mentioned in a comment"

if [[ $cases_failed -eq 0 ]]; then
  # Both sides are the count actually run: a hardcoded total is one more number
  # that can drift out of step with the cases.
  echo "OK: $cases_run/$cases_run docs version pins wiring cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run docs version pins wiring cases failed"
exit 1
