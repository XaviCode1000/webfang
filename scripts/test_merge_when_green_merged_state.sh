#!/usr/bin/env bash
#
# Negation harness for the ALREADY-MERGED detection in merge-when-green.sh
# (#1970).
#
# The defect is a property of the MERGE PATH, not of any line of text: for a PR
# that GitHub already merged, the API answers `mergeable: UNKNOWN` and
# `mergeStateStatus: UNKNOWN` FOREVER (the mergeability of a closed PR is never
# computed). The script read only `mergeStateStatus`, treated UNKNOWN as "GitHub
# is still computing", burned its 90s retry window and then exited 4 — on a PR
# whose merge had already happened. The state desired ("the PR is merged") was
# already reached, so the correct outcome is exit 0, not a failure.
#
# NEGATION (the mandatory case from the issue): a PR whose `state` is MERGED
# exited non-zero after the 90s wait. Case 1 pins exactly that input and demands
# exit 0 with an "already merged" message and no wait at all.
#
# Properties pinned here:
#   1. state == MERGED  => exit 0, "already merged", and NO wait (the 90s
#      UNKNOWN window must not be entered, and no second merge is attempted);
#   2. state == OPEN + mergeStateStatus == UNKNOWN => the script KEEPS
#      retrying: UNKNOWN on a live PR is still "GitHub is computing", so the
#      bounded wait must survive the fix untouched;
#   3. CLEAN and UNSTABLE => the merge path is intact (exit 0, squash merge);
#   4. BEHIND and BLOCKED => exit 3, unchanged, nothing merged;
#   5. a state that is NOT MERGED (CLOSED) is never reported as "already
#      merged" — the detection is specific to MERGED, not to "not OPEN".
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No network,
# no real repository state, no cargo, no merge (the fake records the merge
# instead of performing one).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the reads merge-when-green.sh performs and records the merge it was
# asked to perform instead of performing one. The head owner is deliberately a
# fork owner so the post-merge remote cleanup (which shells out to
# 'git ls-remote') is skipped: this harness must stay offline.
#
# The PR state lives in ONE fixture file so the script can read `state` and
# `mergeStateStatus` from a single call.
#
# The fake ALSO serves the single-field projections, on purpose: a harness that
# only understands the post-fix shape cannot tell "the script is fixed" from
# "the script asked for a field the stub does not know" — the pre-fix script
# would die on its first read instead of demonstrating the defect (it burns
# the 90s UNKNOWN window and exits non-zero on a PR that is already merged).
# The property "ONE reader" is asserted EXPLICITLY below, from the recorded
# read log, instead of being smuggled into the stub's shape.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

emit() { # emit <json> [<jq-expr>]
  if [[ -n "${2:-}" ]]; then printf '%s' "$1" | jq -r "$2"; else printf '%s\n' "$1"; fi
}

case "${1:-}" in
  repo)
    shift
    fields=""; jqexpr=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
      esac
      shift
    done
    case "$fields" in
      nameWithOwner) emit '{"nameWithOwner":"owner/repo"}' "$jqexpr" ;;
      owner) emit '{"owner":{"login":"owner"}}' "$jqexpr" ;;
      *) echo "fake gh: unhandled repo view --json $fields" >&2; exit 99 ;;
    esac
    ;;
  api)
    case "${2:-}" in
      repos/*/branches/main/protection)
        emit '{"required_status_checks":{"checks":[{"context":"Validate PR metadata"},{"context":"CI Gate"}]}}' \
          '.required_status_checks.checks[]?.context // empty'
        ;;
      *) echo "fake gh: unhandled api ${2:-}" >&2; exit 99 ;;
    esac
    ;;
  pr)
    sub="${2:-}"
    shift 2
    if [[ $# -gt 0 && "${1:-}" != --* ]]; then shift; fi   # drop the PR number
    fields=""; jqexpr=""; strategy=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
        --squash|--merge) strategy="$1" ;;
      esac
      shift
    done
    case "$sub" in
      checks)
        cat "$state/checks.json"
        ;;
      view)
        printf '%s\n' "$fields" >> "$state/reads.log"
        case "$fields" in
          state,mergeStateStatus) emit "$(cat "$state/pr-state.json")" "$jqexpr" ;;
          mergeStateStatus) emit "$(jq -c '{mergeStateStatus: .mergeStateStatus}' "$state/pr-state.json")" "$jqexpr" ;;
          state) emit "$(jq -c '{state: .state}' "$state/pr-state.json")" "$jqexpr" ;;
          headRefName) emit '{"headRefName":"fixture-branch"}' "$jqexpr" ;;
          headRepositoryOwner) emit '{"headRepositoryOwner":{"login":"fork-owner"}}' "$jqexpr" ;;
          title,commits) cat "$state/pr-meta.json" ;;
          *) echo "fake gh: unhandled pr view --json $fields" >&2; exit 99 ;;
        esac
        ;;
      merge)
        printf '%s\n' "$strategy" >> "$state/merged"
        ;;
      *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
    esac
    ;;
  *)
    echo "fake gh: unhandled invocation: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

printf '%s\n' '[{"name":"Validate PR metadata","bucket":"pass"},{"name":"CI Gate","bucket":"pass"}]' \
  > "$STATE/checks.json"
printf '%s\n' '{"title":"fix(cli): harness fixture","commits":[]}' > "$STATE/pr-meta.json"

# ─── Fixtures ───────────────────────────────────────────────────────────────
# set_pr_state <state> <mergeStateStatus>: the exact shape GitHub answers for
# a PR. A MERGED PR is the shape of #1970: state MERGED, mergeStateStatus
# UNKNOWN (the mergeability of a closed PR is never computed).
set_pr_state() {
  jq -n --arg s "$1" --arg m "$2" '{state: $s, mergeStateStatus: $m}' > "$STATE/pr-state.json"
  : > "$STATE/reads.log"
}

RUN_OUT=""
RUN_RC=0
RUN_ELAPSED=0
# Portable bounded run: a watchdog in plain bash, NOT `timeout` (GNU coreutils
# only — a macOS contributor has no `timeout`, and a harness that only runs on
# one platform is a harness half the team never runs). The deadline is the
# harness's own safety net AND the way the "keeps retrying" case is observed:
# rc=124 means the script was still running when the deadline hit.
run_merge() { # run_merge <deadline-secs> <script-args...>
  local limit="$1"; shift
  rm -f "$STATE/merged"
  RUN_RC=0
  local start=$SECONDS
  local out_file="$STATE/run.out"
  : > "$out_file"
  ( cd "$REPO_ROOT" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    bash scripts/merge-when-green.sh "$@" ) >"$out_file" 2>&1 &
  local pid=$!
  local waited=0
  while kill -0 "$pid" 2>/dev/null && (( waited < limit )); do
    sleep 1
    waited=$(( waited + 1 ))
  done
  if kill -0 "$pid" 2>/dev/null; then
    pkill -P "$pid" 2>/dev/null || true
    kill -TERM "$pid" 2>/dev/null || true
    sleep 1
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    RUN_RC=124
  else
    wait "$pid" || RUN_RC=$?
  fi
  RUN_OUT="$(cat "$out_file")"
  RUN_ELAPSED=$((SECONDS - start))
}
merged_strategy() { if [[ -f "$STATE/merged" ]]; then tr -d '\n' < "$STATE/merged"; fi; }

PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"; FAIL=$((FAIL + 1))
  fi
}
check_contains() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] not found in output\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_absent() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] unexpectedly present\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_no_merge() {
  local label="$1" got
  got="$(merged_strategy)"
  if [[ -z "$got" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — gh pr merge was invoked with [%s]\n' "$label" "$got"; FAIL=$((FAIL + 1))
  fi
}

echo "test_merge_when_green_merged_state: an already merged PR exits clean and fast (webfang#1970)"

# ─── 1. THE MANDATORY NEGATION: the PR is already MERGED ────────────────────
# #1970 exactly: state=MERGED, mergeStateStatus=UNKNOWN. Pre-fix the script
# entered the UNKNOWN retry loop, waited its 90s and exited non-zero on a PR
# whose merge had already happened.
set_pr_state MERGED UNKNOWN
run_merge 20 1970
check "MERGED PR -> exit 0 (the desired state is already reached)" "0" "$RUN_RC"
check_contains "MERGED PR -> says 'already merged'" "already merged" "$RUN_OUT"
check_no_merge "MERGED PR -> no second merge was attempted"
check_absent "MERGED PR -> it never entered the UNKNOWN retry loop" \
  "mergeStateStatus is UNKNOWN" "$RUN_OUT"
check_absent "MERGED PR -> it never reported the 90s deadline" \
  "still UNKNOWN after 90s" "$RUN_OUT"
if (( RUN_ELAPSED <= 10 )); then
  check "MERGED PR -> it returned without waiting (${RUN_ELAPSED}s <= 10s)" "yes" "yes"
else
  check "MERGED PR -> it returned without waiting (${RUN_ELAPSED}s <= 10s)" "yes" "no"
fi
# ONE reader, asserted from the recorded read log: the script must ask for
# `state` and `mergeStateStatus` TOGETHER, exactly once. Two reads of the
# same fact from two different points in time is the shape this repo's own
# triage calls out ("one fact, two readers"), and it is what let the
# already-merged verdict and the mergeability verdict disagree.
combined_reads="$(awk '/^state,mergeStateStatus$/{c++} END{print c+0}' "$STATE/reads.log")"
single_reads="$(awk '/^mergeStateStatus$/{c++} END{print c+0}' "$STATE/reads.log")"
check "MERGED PR -> state and mergeStateStatus were read in ONE call" "1" "$combined_reads"
check "MERGED PR -> the state was never read a second time, on its own" "0" "$single_reads"

# ─── 2. NON-REGRESSION: OPEN + UNKNOWN keeps retrying ───────────────────────
# UNKNOWN on a live PR is still "GitHub is computing mergeability" (#1017), so
# the bounded wait must survive the fix untouched. The harness kills the script
# at its own deadline: rc=124 proves it was still retrying instead of exiting.
set_pr_state OPEN UNKNOWN
run_merge 12 1970
check "OPEN+UNKNOWN -> still running when the harness deadline hit (rc=124)" "124" "$RUN_RC"
check_contains "OPEN+UNKNOWN -> it says it is still computing" "still computing" "$RUN_OUT"
check_absent "OPEN+UNKNOWN -> it never claims the PR is merged" "already merged" "$RUN_OUT"
check_no_merge "OPEN+UNKNOWN -> nothing merged"

# ─── 3. NON-REGRESSION: CLEAN keeps the merge path intact ───────────────────
set_pr_state OPEN CLEAN
run_merge 20 1970
check "CLEAN -> exit 0" "0" "$RUN_RC"
check "CLEAN -> merged with --squash" "--squash" "$(merged_strategy)"
check_contains "CLEAN -> the RESULT marker is printed" "RESULT: merged" "$RUN_OUT"
check_absent "CLEAN -> it is not reported as already merged" "already merged" "$RUN_OUT"

# ─── 4. NON-REGRESSION: UNSTABLE keeps the merge path intact ────────────────
# UNSTABLE with required checks green is this repo's normal green state (#823).
set_pr_state OPEN UNSTABLE
run_merge 20 1970
check "UNSTABLE -> exit 0" "0" "$RUN_RC"
check "UNSTABLE -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 5. NON-REGRESSION: BEHIND still exits 3 ─────────────────────────────────
set_pr_state OPEN BEHIND
run_merge 20 1970
check "BEHIND -> exit 3" "3" "$RUN_RC"
check_no_merge "BEHIND -> nothing merged"

# ─── 6. NON-REGRESSION: BLOCKED still exits 3 ───────────────────────────────
set_pr_state OPEN BLOCKED
run_merge 20 1970
check "BLOCKED -> exit 3" "3" "$RUN_RC"
check_no_merge "BLOCKED -> nothing merged"

# ─── 7. the detection is specific to MERGED, not to "not OPEN" ──────────────
# A CLOSED PR is not a merged PR: reporting it as "already merged" would hide
# a real problem behind a success message.
set_pr_state CLOSED BLOCKED
run_merge 20 1970
check "CLOSED (not merged) -> exit 3" "3" "$RUN_RC"
check_absent "CLOSED (not merged) -> it is NOT reported as already merged" \
  "already merged" "$RUN_OUT"
check_no_merge "CLOSED (not merged) -> nothing merged"

echo "test_merge_when_green_merged_state: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: an already merged PR exits clean and fast, and every other verdict is unchanged."
