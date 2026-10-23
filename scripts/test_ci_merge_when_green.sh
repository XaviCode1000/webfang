#!/usr/bin/env bash
#
# Semantics harness for the WAIT BOUNDS in merge-when-green.sh (B46, B47).
#
# Two properties here are behavioral and cannot be proven by grepping the
# script, because both are about what the script DOES with time and with a
# bucket value:
#
#   1. B46 (HIGH) — the effective deadline must bound EVERY branch of the
#      wait. When the required-context list cannot be read, the script falls
#      back to `gh pr checks --watch --required --fail-fast`. That fallback
#      used to run with NO wrapper, so `--timeout` was silently ignored and
#      the watch blocked forever. The harness drives the real script with an
#      unreadable branch protection and a watch that never returns, and
#      demands that the script comes back on its own, on time, with the
#      "could not settle" exit code.
#   2. B47 — a cancelled required check is TERMINAL. The API spells the
#      bucket `cancel`; the arm that recognised the wait as over only
#      matched `cancelled`, so a real cancelled check fell through to the
#      "not reported yet" arm and the wait ran to the deadline. Both
#      spellings must fail the wait immediately.
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No
# network, no real repository state, no cargo, no merge (the fake records
# the merge instead of performing one).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the reads merge-when-green.sh performs and records the merge it was
# asked to perform instead of performing one. The head owner is deliberately
# a fork owner so the post-merge remote cleanup (which shells out to
# 'git ls-remote') is skipped: this harness must stay offline.
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
        # An unreadable branch protection is the B46 precondition: both
        # protection reads fail, so the script takes the watch fallback.
        if [[ -f "$state/protection-fails" ]]; then
          echo "gh: simulated API failure (500)" >&2
          exit 9
        fi
        emit '{"required_status_checks":{"checks":[{"context":"required-a"}]}}' \
          '.required_status_checks.checks[]?.context // empty'
        ;;
      repos/*/rules/branches/main)
        # A ruleset-migrated branch protection: the classic endpoint fails and
        # the required contexts live under the active required_status_checks
        # rules (the merge_queue knobs are ruleset-only, B2284).
        if [[ -f "$state/rules-fails" ]]; then
          echo "gh: simulated API failure (500)" >&2
          exit 9
        fi
        jq -r '.[] | select(.type == "required_status_checks") | .parameters.required_status_checks[]?.context // empty' "$state/rules.json"
        ;;
      *) echo "fake gh: unhandled api ${2:-}" >&2; exit 99 ;;
    esac
    ;;
  pr)
    sub="${2:-}"
    shift 2
    if [[ $# -gt 0 && "${1:-}" != --* ]]; then shift; fi   # drop the PR number
    fields=""; jqexpr=""; strategy=""; watch=0
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
        --squash|--merge) strategy="$1" ;;
        --watch) watch=1 ;;
      esac
      shift
    done
    case "$sub" in
      checks)
        if (( watch )); then
          # The watch never returns on its own: only the deadline may end it.
          sleep 60
          exit 0
        fi
        cat "$state/checks.json"
        ;;
      view)
        case "$fields" in
          # The script reads `state` and `mergeStateStatus` in ONE call (the
          # already-merged detection, #1970). Serving only that combined
          # projection keeps ONE reader: a regression to a single-field read
          # fails loudly here instead of silently working.
          state,mergeStateStatus) emit '{"state":"OPEN","mergeStateStatus":"CLEAN"}' "$jqexpr" ;;
          headRefName) emit '{"headRefName":"fixture-branch"}' "$jqexpr" ;;
          headRepositoryOwner) emit '{"headRepositoryOwner":{"login":"fork-owner"}}' "$jqexpr" ;;
          title,commits) emit '{"title":"chore: harness","commits":[]}' "$jqexpr" ;;
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
merged_strategy() { if [[ -f "$STATE/merged" ]]; then tr -d '\n' < "$STATE/merged"; fi; }

# run_merge <script-args...> — runs the REAL script and reports the exit code,
# the wall clock it took and everything it printed.
RUN_RC=0
RUN_OUT=""
RUN_ELAPSED=0
run_merge() {
  rm -f "$STATE/merged"
  RUN_RC=0
  local start=$SECONDS
  # The outer timeout is a safety net for the harness itself: if the script
  # blocks forever the harness must report that, not hang.
  RUN_OUT="$( cd "$REPO_ROOT" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    timeout --kill-after=5 25 bash scripts/merge-when-green.sh "$@" 2>&1 )" || RUN_RC=$?
  RUN_ELAPSED=$((SECONDS - start))
}

echo "test_ci_merge_when_green: the wait is always bounded and a cancel is terminal"

# ─── 1. B46: the fallback watch is bounded by the effective deadline ────────
# Pre-fix this ran `gh pr checks --watch` with no wrapper: the script never
# came back. The fake's watch sleeps 60s; only the deadline may end it.
: > "$STATE/protection-fails"
printf '%s\n' '[{"name":"required-a","bucket":"pending"}]' > "$STATE/checks.json"
run_merge 99 --timeout 2 --squash
check "B46: unreadable protection + blocking watch -> exit 4 (not a hang)" "4" "$RUN_RC"
if (( RUN_ELAPSED <= 10 )); then
  check "B46: the script returned on its own (${RUN_ELAPSED}s <= 10s)" "yes" "yes"
else
  check "B46: the script returned on its own (${RUN_ELAPSED}s <= 10s)" "yes" "no"
fi
check_contains "B46: it says the deadline applied to the fallback" "watcher timeout: 2s" "$RUN_OUT"
check_contains "B46: it names the timeout as the reason" "did not settle within 2s" "$RUN_OUT"
check_absent "B46: it never claims the checks are GREEN" "all required checks are GREEN" "$RUN_OUT"
check "B46: nothing was merged" "" "$(merged_strategy)"

# ─── 2. B47: the `cancel` bucket is terminal ────────────────────────────────
# The API spells it `cancel`. Pre-fix only `cancelled` matched, so this case
# fell through to "not reported yet" and waited for the deadline.
rm -f "$STATE/protection-fails"
printf '%s\n' '[{"name":"required-a","bucket":"cancel"}]' > "$STATE/checks.json"
run_merge 99 --timeout 30 --squash
check "B47: a cancelled required check -> exit 2" "2" "$RUN_RC"
if (( RUN_ELAPSED <= 5 )); then
  check "B47: it failed the wait immediately (${RUN_ELAPSED}s <= 5s)" "yes" "yes"
else
  check "B47: it failed the wait immediately (${RUN_ELAPSED}s <= 5s)" "yes" "no"
fi
check_contains "B47: the message names the cancelled check" "required-a=cancelled" "$RUN_OUT"
check "B47: nothing was merged" "" "$(merged_strategy)"

# ─── 3. B47: the `cancelled` spelling stays terminal ───────────────────────
printf '%s\n' '[{"name":"required-a","bucket":"cancelled"}]' > "$STATE/checks.json"
run_merge 99 --timeout 30 --squash
check "B47: the 'cancelled' spelling -> exit 2" "2" "$RUN_RC"
check_contains "B47: the message names the cancelled check" "required-a=cancelled" "$RUN_OUT"

# ─── 4. control: a green required check still merges ───────────────────────
printf '%s\n' '[{"name":"required-a","bucket":"pass"}]' > "$STATE/checks.json"
run_merge 99 --timeout 30 --squash
check "control: a green required check -> exit 0" "0" "$RUN_RC"
check_contains "control: the GREEN banner is printed" "reported and GREEN" "$RUN_OUT"
check "control: it merged with --squash" "--squash" "$(merged_strategy)"

# ─── 5. control: a failed required check still fails the wait ──────────────
printf '%s\n' '[{"name":"required-a","bucket":"fail"}]' > "$STATE/checks.json"
run_merge 99 --timeout 30 --squash
check "control: a failed required check -> exit 2" "2" "$RUN_RC"
check "control: nothing was merged" "" "$(merged_strategy)"

# ─── 6. B2284: a ruleset-migrated protection feeds the polling wait ────────
# The classic endpoint fails, the required contexts come from the active
# required_status_checks rules, and the script takes the REAL polling path —
# not the degraded `gh pr checks --watch --required` fallback that produced
# the false GREEN in #1011.
: > "$STATE/protection-fails"
printf '%s\n' '[{"type":"merge_queue","parameters":{}},{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"CI Gate"}]}}]' \
  > "$STATE/rules.json"
printf '%s\n' '[{"name":"CI Gate","bucket":"pass"}]' > "$STATE/checks.json"
run_merge 99 --timeout 2 --squash
check "B2284: ruleset-migrated protection -> exit 0" "0" "$RUN_RC"
check_contains "B2284: the required context is read from the rules" \
  "required contexts from branch protection: CI Gate" "$RUN_OUT"
check_absent "B2284: the degraded watcher fallback is never taken" "Falling back" "$RUN_OUT"
check "B2284: it merged with --squash" "--squash" "$(merged_strategy)"
rm -f "$STATE/protection-fails"

echo "test_ci_merge_when_green: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: every wait is bounded by the effective deadline, and a cancel is terminal."
