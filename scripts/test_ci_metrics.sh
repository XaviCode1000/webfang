#!/usr/bin/env bash
#
# Semantics harness for the read-only snapshot in ci_metrics.sh (B54, B55).
#
# Two properties, both about what the snapshot SAYS rather than about a line
# of code:
#
#   1. B54 — the required contexts are read from BOTH protection shapes.
#      Modern branch protections expose them as `checks[].context`; the
#      deprecated `contexts[]` still exists on older ones. Reading only the
#      deprecated shape reported "0 required contexts" for a checks[]-only
#      protection and exited 0 — the snapshot claimed there was nothing to
#      require while the repository required two checks.
#   2. B55 — every listed run is accounted for. A run whose payload cannot
#      be parsed used to vanish from the sample while the snapshot still
#      printed as a complete picture of the window. The snapshot must either
#      analyze the run or say that it could not.
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No
# network, no real runs, no repository state, no cargo.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the four reads ci_metrics.sh performs: the repo identity, the branch
# protection, the ruleset rules and the run list.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

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
      nameWithOwner) printf '%s' '{"nameWithOwner":"owner/repo"}' | jq -r "${jqexpr:-.}"; exit 0 ;;
      *) echo "fake gh: unhandled repo view --json $fields" >&2; exit 99 ;;
    esac
    ;;
  api)
    case "${2:-}" in
      repos/*/branches/main/protection)
        if [[ -f "$state/protection-fails" ]]; then
          # Real gh prints the API error body to STDOUT while exiting nonzero:
          # modeling only "no output" would miss the misclassification where a
          # 404 envelope on stdout masquerades as a readable protection.
          printf '%s\n' '{"message":"Branch not protected","status":"404"}'
          exit 9
        fi
        cat "$state/protection.json"
        ;;
      repos/*/rules/branches/main)
        if [[ -f "$state/rules-fails" ]]; then
          printf '%s\n' '{"message":"Ruleset not found","status":"404"}'
          exit 9
        fi
        cat "$state/rules.json"
        ;;
      *) echo "fake gh: unhandled api ${2:-}" >&2; exit 99 ;;
    esac
    ;;
  run)
    if [[ -f "$state/runlist-fails" ]]; then
      echo "gh: simulated API failure (500)" >&2
      exit 9
    fi
    cat "$state/runs.json"
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

RUN_RC=0
RUN_OUT=""
run_metrics() {
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    bash scripts/ci_metrics.sh --days 14 --workflow ci.yml --branch main 2>&1 )" || RUN_RC=$?
}
# The line the snapshot prints for the sample size.
window_line() { sed -n 's/^- runs in window: //p' <<<"$RUN_OUT"; }
required_count() { sed -n 's/^## Required checks (\(.*\))$/\1/p' <<<"$RUN_OUT"; }

# Timestamps inside the 14-day window, so the sample is not filtered out.
now_iso() { date -u -d "$1" +%Y-%m-%dT%H:%M:%SZ; }

echo "test_ci_metrics: the snapshot reports the required checks and the whole sample"

# ─── 1. B54: a checks[]-only protection reports its required contexts ─────
printf '%s\n' '{"required_status_checks":{"checks":[{"context":"CI Gate"},{"context":"repo-guards"}]}}' \
  > "$STATE/protection.json"
printf '%s\n' '[]' > "$STATE/runs.json"
run_metrics
check "B54: checks[]-only protection -> exit 0" "0" "$RUN_RC"
check "B54: checks[]-only protection -> 2 required contexts" "2" "$(required_count)"
check_contains "B54: the contexts are listed" "- CI Gate" "$RUN_OUT"
check_contains "B54: the contexts are listed" "- repo-guards" "$RUN_OUT"

# ─── 2. B54: the deprecated contexts[] shape still works ───────────────────
printf '%s\n' '{"required_status_checks":{"contexts":["CI Gate","repo-guards"]}}' \
  > "$STATE/protection.json"
run_metrics
check "B54: contexts[]-only protection -> 2 required contexts" "2" "$(required_count)"

# ─── 3. B54: an unreadable protection still fails closed ───────────────────
# The 404 envelope arrives on STDOUT (raw gh behavior): the rc-aware read must
# treat it as not-protection and fail closed, never as "0 required contexts".
: > "$STATE/protection-fails"
run_metrics
check "B54: an unreadable protection -> exit 2" "2" "$RUN_RC"
check_contains "B54: the reason is reported" "could not read branch protection" "$RUN_OUT"
rm -f "$STATE/protection-fails"

# ─── 4. control: a complete sample is printed as complete ──────────────────
t1="$(now_iso '-1 hour')"
t2="$(now_iso '-2 hours')"
jq -n --arg c "$t1" --arg u "$t1" \
  '[{createdAt:$c,updatedAt:$u,conclusion:"success",event:"push"},
    {createdAt:$c,updatedAt:$u,conclusion:"failure",event:"push"}]' > "$STATE/runs.json"
run_metrics
check "control: both runs analyzed -> the sample line is the plain count" "2" "$(window_line)"
check_absent "control: a complete sample is not marked incomplete" "INCOMPLETE" "$RUN_OUT"
check_contains "control: the pass rate is reported" "pass rate: 50.0%" "$RUN_OUT"

# ─── 5. B55: an unparsable run is COUNTED and the sample says so ───────────
jq -n --arg c "$t1" --arg u "$t1" \
  '[{createdAt:$c,updatedAt:$u,conclusion:"success",event:"push"},
    {createdAt:"not-a-date",updatedAt:"not-a-date",conclusion:"failure",event:"push"}]' \
  > "$STATE/runs.json"
run_metrics
check "B55: an unparsable run -> the snapshot is still printed" "0" "$RUN_RC"
check_contains "B55: the count line says the sample is INCOMPLETE" "INCOMPLETE" "$(window_line)"
check_contains "B55: the count line names how many runs could not be parsed" \
  "1 of 2 listed runs could not be parsed" "$(window_line)"
check_contains "B55: the snapshot warns about the partial sample" \
  "cover a PARTIAL sample" "$RUN_OUT"

# ─── 6. B55: a run list that is not a list is unknown, not empty ───────────
printf '%s\n' '{"message":"Not Found"}' > "$STATE/runs.json"
run_metrics
check "B55: a non-array run list -> the sample is unknown" \
  "unknown (gh run list failed, returned no data, or could not be parsed)" "$(window_line)"

# ─── 7. control: an empty run list is an empty sample, not unknown ─────────
printf '%s\n' '[]' > "$STATE/runs.json"
run_metrics
check "control: an empty run list -> 0 runs in window" "0" "$(window_line)"

# ─── 8. B2284: a ruleset-migrated protection still reports its contexts ────
# When classic branch protection is migrated to repository rulesets, the
# classic endpoint answers 404 and the required contexts must be read from
# the active required_status_checks rules instead — no fabricating, no
# "no required contexts" snapshot on a protected branch.
: > "$STATE/protection-fails"
printf '%s\n' '[]' > "$STATE/runs.json"
printf '%s\n' '[{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"CI Gate"},{"context":"repo-guards"}]}},{"type":"merge_queue","parameters":{}}]' \
  > "$STATE/rules.json"
run_metrics
check "B2284: ruleset-migrated protection -> exit 0" "0" "$RUN_RC"
check "B2284: ruleset-migrated protection -> 2 required contexts" "2" "$(required_count)"
check_contains "B2284: the contexts are listed" "- CI Gate" "$RUN_OUT"
rm -f "$STATE/protection-fails"

# ─── 9. B2284: classic AND rules both unreadable still fail closed ─────────
: > "$STATE/protection-fails"
: > "$STATE/rules-fails"
run_metrics
check "B2284: both sources unreadable -> exit 2" "2" "$RUN_RC"
rm -f "$STATE/protection-fails" "$STATE/rules-fails"

echo "test_ci_metrics: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: the snapshot reports every required context and accounts for every listed run."
