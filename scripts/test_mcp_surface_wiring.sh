#!/usr/bin/env bash
# test_mcp_surface_wiring.sh
#
# Semantics harness for the WIRING of scripts/check_mcp_public_surface.sh, the
# version-marker half of the MCP public-surface gate (#1614, wired into CI in
# #2038). The guard itself pins the DECISION; this file pins that the decision
# is reached on every PR at all.
#
# Why this file exists: the guard shipped as a local-only step. #2038 moved it
# into `repo-guards` and nothing pinned the edges of that move, while every edge
# here fails silently when it breaks. A `paths:` filter on the job leaves the
# step green and unrun on the PR that needed it; dropping `repo-guards` from the
# narrow-scope list leaves a docs-only PR ungated; a base that stops coming from
# the resolve step's output turns the comparison into "current vs current",
# which is green by construction. All three are valid YAML and all three look
# like a passing run.
#
# What it proves, offline (pure text over the checkout, plus one local git call):
#
#   - the gate step runs the guard with an explicit `--base "$BASE"`, and that
#     BASE comes from the resolve step's output rather than a literal;
#   - the resolve step validates the sha shape, fetches the one commit the
#     depth-1 checkout is missing, and re-checks it with `cat-file` after the
#     fetch — the check that keeps a failed fetch from reading as bootstrap;
#   - `repo-guards` stays unfiltered and unconditional, and `gate` waits on it
#     in both scopes;
#   - the local fast gate still runs the guard in `lane_fmt_and_guards` and this
#     harness in `lane_ci`;
#   - the guard refuses a base that is not a commit instead of calling it a
#     bootstrap commit, which is the fail-open this harness was written next to.
#
# Usage: bash scripts/test_mcp_surface_wiring.sh [ci-yml] [fast-gate]
# The optional paths let each negative control run against a mutated copy.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CI_YML="${1:-$ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$ROOT/scripts/ci_fast_gate.sh}"

GUARD="scripts/check_mcp_public_surface.sh"
WIRING="scripts/test_mcp_surface_wiring.sh"

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
# live at 4-space indentation there, so a `paths:` or `if:` inside a step's
# script body cannot be mistaken for a job-level one.
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

# check_wiring <workflow-path> — nonzero when the gate is present in the tree
# but no longer connected to a required check that compares against the event
# base, one line per reason.
check_wiring() {
  local wf="$1" bad=0 applicable
  local block="$tmp/rg.yml"
  local header="$tmp/rg-header.yml"
  local gate_block="$tmp/gate.yml"
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
  if ! grep -qF "bash $GUARD --base \"\$BASE\"" "$block"; then
    echo "no step in 'repo-guards' runs $GUARD with --base: without it the guard falls back to the last commit that touched the fixture, which is the change itself"
    bad=1
  fi
  if ! grep -qF 'steps.mcp-surface-base.outputs.base' "$block"; then
    echo "the gate step does not read the resolve step's output: a literal or empty base makes the comparison vacuous"
    bad=1
  fi
  if ! grep -qF "git fetch --no-tags --depth=1 origin \"\$BASE\"" "$block"; then
    echo "the resolve step no longer fetches the base commit: the depth-1 checkout does not contain it, so the guard would read bootstrap and pass"
    bad=1
  fi
  # The pre-fetch existence probe carries `2>/dev/null`; the post-fetch one does
  # not, and that second one is the edge: it is what stops a failed fetch from
  # being read as "no fixture at base" and exiting 0.
  if ! grep -F 'git cat-file -e' "$block" | grep -qvF '2>/dev/null'; then
    echo "the resolve step lost its post-fetch cat-file check: a silently failed fetch would read as the bootstrap commit and turn the gate green"
    bad=1
  fi
  if ! grep -qF '[0-9a-f]{40}' "$block"; then
    echo "the resolve step no longer validates the sha shape: a ref name or a truncated sha would reach the guard as a base"
    bad=1
  fi
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
  applicable="$(grep -E '^[[:space:]]*for job in ' "$gate_block" | head -n1)"
  if [[ -z "$applicable" ]]; then
    echo "'gate' has no narrow-scope producer list: the applicable set cannot be checked and the gate may be scope-blind"
    bad=1
  elif grep -qw clippy <<< "$applicable"; then
    echo "'gate' narrow-scope lists were reordered: the always-applicable list is no longer first, so this check is blind — re-derive it"
    bad=1
  elif ! grep -qw repo-guards <<< "$applicable"; then
    echo "'gate' narrow-scope always-applicable list omits 'repo-guards': a docs-only PR would not gate on the marker at all"
    bad=1
  fi
  [[ $bad -eq 0 ]]
}

# check_fast_gate <fast-gate-path> — nonzero when the local mirror has lost one
# of its edges. A path named in a comment is not an edge, so both checks require
# an execution line.
check_fast_gate() {
  local fg="$1" bad=0
  local guards_lane="$tmp/fg-lane_fmt_and_guards.sh"
  local ci_lane="$tmp/fg-lane_ci.sh"
  lane_block "$fg" lane_fmt_and_guards > "$guards_lane"
  if [[ ! -s "$guards_lane" ]]; then
    echo "function 'lane_fmt_and_guards' not found in the fast gate: the local mirror cannot be checked"
    return 1
  fi
  if ! grep -vE '^[[:space:]]*#' "$guards_lane" | grep -qF "bash $GUARD"; then
    echo "lane_fmt_and_guards does not run $GUARD: a pre-push would pass with the marker unmoved"
    bad=1
  fi
  lane_block "$fg" lane_ci > "$ci_lane"
  if [[ ! -s "$ci_lane" ]]; then
    echo "function 'lane_ci' not found in the fast gate: the wiring harness cannot be checked"
    return 1
  fi
  if ! grep -vE '^[[:space:]]*#' "$ci_lane" | grep -qF "bash $WIRING"; then
    echo "lane_ci does not run $WIRING: a scripts/** change — the only kind that can break these edges — would not exercise them"
    bad=1
  fi
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

# Case 1: the committed workflow reaches the required check with the event base.
if check_wiring "$CI_YML" > "$tmp/out.txt" 2>&1; then
  echo "  ok: gate runs in an unfiltered, unconditional job, fed by the resolve step, and 'gate' waits on it in both scopes"
else
  echo "  FAIL: committed wiring does not reach a required check with the event base"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "committed ci.yml wiring compares against the event base"

# Case 2: the committed fast gate still mirrors both edges locally.
if check_fast_gate "$FAST_GATE" > "$tmp/out.txt" 2>&1; then
  echo "  ok: guard in lane_fmt_and_guards, harness in lane_ci"
else
  echo "  FAIL: committed fast-gate wiring is incomplete"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "committed fast-gate wiring mirrors the guard and this harness"

# Case 3: negative control — the gate step stops passing --base. The guard then
# falls back to the last commit that touched the fixture, which on a PR that
# moved it IS the change, so every row reads as unchanged and the gate is green.
awk -v needle='bash scripts/check_mcp_public_surface.sh --base "$BASE"' '
  index($0, needle) > 0 { print "          # gate step removed"; next }
  { print }
' "$CI_YML" > "$tmp/ci-no-step.yml"
if cmp -s "$CI_YML" "$tmp/ci-no-step.yml"; then
  echo "  FAIL: fixture injection did not apply — the anchor missed and this case proved nothing"
  case_fail=1
elif check_wiring "$tmp/ci-no-step.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a gate step without --base passed — the guard can go vacuous again unnoticed"
  case_fail=1
elif grep -qF -e "$GUARD" "$tmp/out.txt"; then
  echo "  ok: missing --base rejected and named"
else
  echo "  FAIL: rejected, but the guard was not named"
  case_fail=1
fi
finish_case "negative control: gate step no longer passes --base"

# Case 4: negative control — the gate step stops reading the resolve step's
# output and compares against a literal.
sed 's/steps\.mcp-surface-base\.outputs\.base/0123456789abcdef0123456789abcdef01234567/' \
  "$CI_YML" > "$tmp/ci-literal-base.yml"
if cmp -s "$CI_YML" "$tmp/ci-literal-base.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_wiring "$tmp/ci-literal-base.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a hardcoded base passed — the comparison is green by construction"
  case_fail=1
elif grep -q -e "resolve step's output" "$tmp/out.txt"; then
  echo "  ok: literal base rejected and named"
else
  echo "  FAIL: rejected, but the missing edge was not named"
  case_fail=1
fi
finish_case "negative control: gate step fed a literal base"

# Case 5: negative control — the post-fetch cat-file check is dropped. This is
# the shape that reads as a passing gate on a failed fetch: the guard sees no
# fixture at base and calls it the bootstrap commit.
awk '
  index($0, "git cat-file -e") > 0 && index($0, "2>/dev/null") == 0 {
    print "          if false; then"; next
  }
  { print }
' "$CI_YML" > "$tmp/ci-no-catfile.yml"
if cmp -s "$CI_YML" "$tmp/ci-no-catfile.yml"; then
  echo "  FAIL: fixture injection did not apply — the anchor missed and this case proved nothing"
  case_fail=1
elif check_wiring "$tmp/ci-no-catfile.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a resolve step without the post-fetch check passed — a failed fetch reads as bootstrap"
  case_fail=1
elif grep -q -e 'post-fetch' "$tmp/out.txt"; then
  echo "  ok: missing post-fetch check rejected and named"
else
  echo "  FAIL: rejected, but the missing check was not named"
  case_fail=1
fi
finish_case "negative control: post-fetch cat-file check removed"

# Case 6: negative control — a `paths:` filter on the job.
sed 's/^  repo-guards:$/  repo-guards:\n    paths:\n      - "scripts\/**"/' "$CI_YML" > "$tmp/ci-filtered.yml"
if cmp -s "$CI_YML" "$tmp/ci-filtered.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_wiring "$tmp/ci-filtered.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a path-filtered gate job passed"
  case_fail=1
elif grep -q -e 'path-filtered' "$tmp/out.txt"; then
  echo "  ok: path filter rejected and named"
else
  echo "  FAIL: rejected, but the filter was not named"
  case_fail=1
fi
finish_case "negative control: repo-guards path-filtered"

# Case 7: negative control — the job drops out of the narrow-scope applicable
# set, which is the docs-only PR path.
sed 's/for job in change-scope toolchain repo-guards security doc-quality; do/for job in change-scope toolchain security doc-quality; do/' \
  "$CI_YML" > "$tmp/ci-narrow-blind.yml"
if cmp -s "$CI_YML" "$tmp/ci-narrow-blind.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_wiring "$tmp/ci-narrow-blind.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a narrow-scope list without repo-guards passed"
  case_fail=1
elif grep -q -e 'narrow-scope' "$tmp/out.txt"; then
  echo "  ok: narrow-scope omission rejected and named"
else
  echo "  FAIL: rejected, but the narrow scope was not named"
  case_fail=1
fi
finish_case "negative control: repo-guards out of the narrow-scope set"

# Case 8: the guard's own read of the base. A base that is not a commit used to
# take the bootstrap branch and exit 0; it has to refuse instead, or every case
# above can be defeated by a base that was never fetched.
if ( cd "$ROOT" && bash "$GUARD" --base deadbeefdeadbeefdeadbeefdeadbeefdeadbeef ) > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a base that is not a commit exited 0 — that is the bootstrap path, and it is how a bad base goes green"
  case_fail=1
elif grep -qF 'is not a commit' "$tmp/out.txt"; then
  echo "  ok: unreadable base rejected as a non-commit, not as bootstrap"
else
  echo "  FAIL: rejected, but not as a non-commit: $(head -n1 "$tmp/out.txt")"
  case_fail=1
fi
finish_case "guard refuses a base that is not a commit"

if [[ $cases_failed -eq 0 ]]; then
  echo "OK: $cases_run/$cases_run MCP surface wiring cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run MCP surface wiring cases failed"
exit 1
