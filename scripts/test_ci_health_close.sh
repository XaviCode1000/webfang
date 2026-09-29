#!/usr/bin/env bash
#
# Semantics harness for the CI Health Observer key + close path.
#
# Three properties here are behavioral and cannot be proven by grepping:
#   1. close_mine closes exactly the open tracking issues that carry the
#      ci-health label — an unlabeled issue with a matching title must survive,
#      and an empty issue set must be a silent no-op;
#   2. the upsert and reconcile steps share ONE close implementation — the
#      drifted inline close branch in reconcile_one (missing the unlabeled
#      Skip notice) must stay deleted;
#   3. the tracking key carries the RUN ORIGIN (#1712): a red `push` and a red
#      `schedule` are two issues, and a green run closes only its own origin.
#      The same key still yields ONE issue for repeated failures of that
#      origin, and the reconcile sweep still closes what it opened.
#
# This harness does NOT hardcode the title any more. It reads TITLE_CI and
# RETIRED_TITLE_CI out of the workflow, and it EXTRACTS the real code out of
# .github/workflows/ci-health.yml (the origin classifier, open_or_comment,
# reconcile_one) and executes it against a fake `gh`. So a change to the
# workflow's key logic fails here instead of turning CI red in ci.yml's
# repo-guards job.
#
# Everything runs against mktemp fixtures and a fake `gh` on PATH.
# No network, no real issues, no real repository state.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKFLOW="$REPO_ROOT/.github/workflows/ci-health.yml"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# State file $STATE/issues holds TSV rows: number<TAB>title<TAB>comma-labels.
# $STATE/body-<n> holds the body of a created issue, $STATE/comment-<n> the
# appended comments of one. `gh issue close` appends the number to
# $STATE/closed; later `issue list` calls hide closed issues.
#
# The fake matches titles byte-for-byte, which is STRICTER than GitHub's real
# `in:title` search (that one strips punctuation and ANDs the words — verified
# 2026-09-29 against this repo: `--search 'windows-latest in:title'` finds
# #1697 "fix(ci): Tests (windows-latest) …", and adding one term that is absent
# from a title drops the hit to zero). So the harness proves the weaker half —
# different keys never collide even compared byte-for-byte — and the stronger
# half is pinned by the "origin lands as its own search term" check below.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"
sub="$1 $2"
shift 2
case "$sub" in
  "issue list")
    search="" want_count=0
    while [[ $# -gt 0 ]]; do
      case "${1:-}" in
        --search) search="${2:-}"; shift 2 ;;
        --jq) [[ "${2:-}" == "length" ]] && want_count=1; shift 2 ;;
        *) shift ;;
      esac
    done
    title="${search% in:title}"
    matches=()
    if [[ -f "$state/issues" ]]; then
      while IFS=$'\t' read -r number row_title _labels; do
        [[ -n "$number" ]] || continue
        [[ "$row_title" == "$title" ]] || continue
        if [[ -f "$state/closed" ]] && grep -qxF "$number" "$state/closed"; then
          continue
        fi
        matches+=("$number")
      done < "$state/issues"
    fi
    if (( want_count )); then
      printf '%s\n' "${#matches[@]}"
    else
      printf '%s\n' "${matches[@]}"
    fi
    ;;
  "issue view")
    number="${1:-}"
    if [[ -f "$state/issues" ]]; then
      while IFS=$'\t' read -r row_number _title labels; do
        if [[ "$row_number" == "$number" ]]; then
          IFS=',' read -ra names <<< "$labels"
          printf '%s\n' "${names[@]}"
          exit 0
        fi
      done < "$state/issues"
    fi
    exit 1
    ;;
  "issue create")
    title=""; body=""; labels=()
    while [[ $# -gt 0 ]]; do
      case "${1:-}" in
        --title) title="${2:-}"; shift 2 ;;
        --body) body="${2:-}"; shift 2 ;;
        --label) labels+=("${2:-}"); shift 2 ;;
        *) shift ;;
      esac
    done
    if [[ -z "$title" ]]; then echo "fake gh: issue create without --title" >&2; exit 98; fi
    rows=0
    [[ -f "$state/issues" ]] && rows="$(wc -l <"$state/issues" | tr -d ' ')"
    number=$((100 + rows))
    joined=""
    if (( ${#labels[@]} > 0 )); then joined="$(IFS=,; echo "${labels[*]}")"; fi
    printf '%s\t%s\t%s\n' "$number" "$title" "$joined" >>"$state/issues"
    printf '%s' "$body" >"$state/body-$number"
    printf 'https://github.com/owner/repo/issues/%s\n' "$number"
    ;;
  "issue comment")
    number="${1:-}"; shift; comment=""
    while [[ $# -gt 0 ]]; do
      case "${1:-}" in
        --body) comment="${2:-}"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s' "$comment" >>"$state/comment-$number"
    ;;
  "issue edit")
    exit 0
    ;;
  "issue close")
    number="${1:-}"
    shift
    comment=""
    while [[ $# -gt 0 ]]; do
      case "${1:-}" in
        --comment) comment="${2:-}"; shift 2 ;;
        *) shift ;;
      esac
    done
    printf '%s\n' "$number" >>"$state/closed"
    printf '%s' "$comment" >"$state/closedcomment-$number"
    ;;
  *)
    echo "fake gh: unhandled invocation: $sub $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

# ─── Extract the real workflow code (no copies, no reimplementation) ───────────
# extract_run_block <step-name-needle> <file>  -> that step's `run: |` payload,
# dedented. Walks past the step's own id/env keys to reach the run block.
extract_run_block() {
  awk -v needle="$1" '
    stage == 0 && index($0, needle) > 0 { stage = 1; next }
    stage == 1 && $0 ~ /^        run: \|/ { stage = 2; next }
    stage == 2 {
      if ($0 ~ /^[[:space:]]*$/) { print ""; next }
      if ($0 !~ /^          /) { exit }
      sub(/^          /, "")
      print
    }
  ' "$2"
}

# extract_fn <fn-name> <file>  -> function source, dedented
extract_fn() {
  awk -v fn="$1" '
    $0 ~ "^ +" fn "\\(\\) \\{" { grab = 1 }
    grab == 1 {
      sub(/^ +/, "")
      print
      if ($0 ~ /^}$/) { exit }
    }
  ' "$2"
}

# The origin classifier is a whole step, not a function.
extract_run_block "- name: Classify run origin" "$WORKFLOW" >"$WORK/classify.sh"
extract_fn "open_or_comment" "$WORKFLOW" >"$WORK/fn_open.sh"
extract_fn "reconcile_one" "$WORKFLOW" >"$WORK/fn_reconcile.sh"

# Titles come from the workflow, never from a literal in this file: that was
# the #1712 coupling (a title change here turned ci.yml's repo-guards job red).
TITLE_TEMPLATE="$(sed -n 's/^  TITLE_CI: "\(.*\)"$/\1/p' "$WORKFLOW")"
RETIRED_TITLE="$(sed -n 's/^  RETIRED_TITLE_CI: "\(.*\)"$/\1/p' "$WORKFLOW")"

# One runner for the extracted workflow code + the shared close helper, with
# the same environment the real step has.
URL="https://github.com/owner/repo/actions/runs/123"
CTX="Run origin: probe (test context)."
cat >"$WORK/run_ci_health.sh" <<RUNNER
set -euo pipefail
# shellcheck disable=SC1090
source "$REPO_ROOT/scripts/ci-health-close.sh"
source "$WORK/fn_open.sh"
source "$WORK/fn_reconcile.sh"
SERVER="https://github.com"
GH_REPO="owner/repo"
RUN_URL="$URL"
BRANCH="main"
LABEL="ci-health"
LABEL_AUTO="automated"
case "\$1" in
  open) shift; open_or_comment "\$@" ;;
  close) shift; close_mine "\$@" ;;
  reconcile) shift; reconcile_one "\$@" ;;
  *) echo "runner: unknown op \$1" >&2; exit 97 ;;
esac
RUNNER

# ─── Harness ─────────────────────────────────────────────────────────────────
PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"
    FAIL=$((FAIL + 1))
  fi
}

run_ci_health() { PATH="$BIN:$PATH" FAKE_STATE="$STATE" bash "$WORK/run_ci_health.sh" "$@"; }

# The workflow's own origin -> key rule, executed.
# classify_key <trigger event> <payload run event> <latest CI run event> [title template]
classify_key() {
  local out="$WORK/gostep.out"
  : >"$out"
  (
    cd "$REPO_ROOT"
    GITHUB_OUTPUT="$out" EVENT="$1" PAYLOAD_RUN_EVENT="$2" LATEST_CI_EVENT="$3" \
      TITLE_CI="${4:-$TITLE_TEMPLATE}" bash "$WORK/classify.sh" >/dev/null
  )
  sed -n 's/^ci_title=//p' "$out"
}

# classify_note <trigger event> <payload run event> <latest CI run event>
classify_note() {
  local out="$WORK/gostep.out"
  : >"$out"
  (
    cd "$REPO_ROOT"
    GITHUB_OUTPUT="$out" EVENT="$1" PAYLOAD_RUN_EVENT="$2" LATEST_CI_EVENT="$3" \
      TITLE_CI="$TITLE_TEMPLATE" bash "$WORK/classify.sh" >/dev/null
  )
  sed -n 's/^note=//p' "$out"
}

reset_state() { rm -f "$STATE/closed" "$STATE/closedcomment-"* "$STATE/comment-"* "$STATE/body-"*; }
reset_all() { reset_state; : >"$STATE/issues"; }
closed_list() { [[ -f "$STATE/closed" ]] && tr '\n' ' ' <"$STATE/closed" | sed 's/ $//' || true; }
closed_count() { [[ -f "$STATE/closed" ]] && wc -l <"$STATE/closed" | tr -d ' ' || echo 0; }
count_matches() { [[ -f "$2" ]] && { grep -c -- "$1" "$2" || true; } || echo 0; }
closed_comment() { cat "$STATE/closedcomment-$1" 2>/dev/null || true; }
is_closed() { [[ -f "$STATE/closed" ]] && grep -qxF "$1" "$STATE/closed" && echo yes || echo no; }
issue_count() { [[ -f "$STATE/issues" ]] && wc -l <"$STATE/issues" | tr -d ' ' || echo 0; }
issue_titles() { cut -f2 "$STATE/issues" 2>/dev/null | tr '\n' '|' || true; }
number_for() { awk -F'\t' -v t="$1" '$2 == t { print $1; exit }' "$STATE/issues"; }
body_of() { cat "$STATE/body-$1" 2>/dev/null || true; }
comment_of() { cat "$STATE/comment-$1" 2>/dev/null || true; }

echo "test_ci_health_close: behavioral checks for the CI Health key + close helper"

# 0. The extracted code is really the workflow's, and the keys really derive
#    from its own constants. A vacuous extraction would pass everything below.
check "open_or_comment extracted from the workflow" "yes" \
  "$(grep -q '^open_or_comment() {' "$WORK/fn_open.sh" && echo yes || echo no)"
check "reconcile_one extracted from the workflow" "yes" \
  "$(grep -q '^reconcile_one() {' "$WORK/fn_reconcile.sh" && echo yes || echo no)"
check "origin classifier extracted from the workflow" "yes" \
  "$(grep -q 'ci_title=' "$WORK/classify.sh" && echo yes || echo no)"
check "TITLE_CI read from the workflow (not hardcoded here)" "yes" \
  "$([[ -n "$TITLE_TEMPLATE" ]] && echo yes || echo no)"
check "TITLE_CI carries an origin placeholder" "yes" \
  "$([[ "$TITLE_TEMPLATE" == *"{origin}"* ]] && echo yes || echo no)"
# Real `in:title` search strips punctuation and ANDs words, so the origin only
# discriminates if it lands as its own term — i.e. it is not glued to a
# neighbouring word. "(push)" qualifies: the parens are stripped (evidence in
# the fake-gh header). "mainpush" would not.
check "origin lands as its own search term" "yes" \
  "$(printf '%s' "$TITLE_TEMPLATE" | grep -qE '[^[:alnum:]_]\{origin\}[^[:alnum:]_]' && echo yes || echo no)"
check "retired title declared for the migration watcher" \
  "Scheduled CI failed on main [ci-health]" "$RETIRED_TITLE"

# 1. A labeled open issue with a matching title is closed with a Green-again comment.
reset_all
TITLE="$(classify_key workflow_run push - "$TITLE_TEMPLATE")"
printf '42\t%s\tci-health,automated\n' "$TITLE" >"$STATE/issues"
if run_ci_health close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "labeled issue -> exit 0" "0" "$rc"
check "labeled issue -> exactly one close" "42" "$(closed_list)"
check "labeled issue -> Green-again comment names the run" \
  "Green again: $URL" "$(closed_comment 42)"

# 2. An unlabeled issue with a matching title is skipped, never closed.
reset_state
printf '7\t%s\tneeds-triage\n' "$TITLE" >"$STATE/issues"
out="$(run_ci_health close "$TITLE" "$URL" 2>&1 || true)"
check "unlabeled issue -> no close calls" "" "$(closed_list)"
case "$out" in
  *"Skip #7"*) check "unlabeled issue -> Skip notice printed" "yes" "yes" ;;
  *) check "unlabeled issue -> Skip notice printed" "yes" "no" ;;
esac

# 3. An empty issue set is a silent no-op with exit zero.
reset_state
: >"$STATE/issues"
if run_ci_health close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "empty set -> exit 0" "0" "$rc"
check "empty set -> no close calls" "" "$(closed_list)"

# 4. The origin -> key rule, executed from the workflow's own classifier.
check "event path: push run -> push key" "CI failed on main (push) [ci-health]" \
  "$(classify_key workflow_run push - "$TITLE_TEMPLATE")"
check "event path: schedule run -> schedule key" "CI failed on main (schedule) [ci-health]" \
  "$(classify_key workflow_run schedule - "$TITLE_TEMPLATE")"
check "event path: pull_request run -> its own key" "CI failed on main (pull_request) [ci-health]" \
  "$(classify_key workflow_run pull_request - "$TITLE_TEMPLATE")"
check "event path: manual run -> its own key" "CI failed on main (workflow_dispatch) [ci-health]" \
  "$(classify_key workflow_run workflow_dispatch - "$TITLE_TEMPLATE")"
# THE deliverable: a push red and a schedule red do not share a key.
check "push key != schedule key" "distinct" \
  "$([[ "$(classify_key workflow_run push - "$TITLE_TEMPLATE")" != \
        "$(classify_key workflow_run schedule - "$TITLE_TEMPLATE")" ]] && echo distinct || echo equal)"
# Bounded key space: an event this observer does not model collapses to
# `other` instead of minting a new issue per event name GitHub may add.
check "unmodeled event -> other" "CI failed on main (other) [ci-health]" \
  "$(classify_key workflow_run merge_group - "$TITLE_TEMPLATE")"
check "empty payload event -> other" "CI failed on main (other) [ci-health]" \
  "$(classify_key workflow_run "" - "$TITLE_TEMPLATE")"
# The sweep does not cause the runs it reads, so it must not claim `schedule`:
# it reports the origin of the run it actually found.
check "reconcile on schedule trigger: reads the run's own origin" \
  "CI failed on main (push) [ci-health]" \
  "$(classify_key schedule - push "$TITLE_TEMPLATE")"
check "reconcile on dispatch trigger: reads the run's own origin" \
  "CI failed on main (push) [ci-health]" \
  "$(classify_key workflow_dispatch - push "$TITLE_TEMPLATE")"
check "reconcile with no run event -> other, never a bare 'schedule'" \
  "CI failed on main (other) [ci-health]" \
  "$(classify_key schedule - "" "$TITLE_TEMPLATE")"
check "reconcile says how it observed the run" "yes" \
  "$([[ "$(classify_note schedule - push)" == *"observed indirectly"* ]] && echo yes || echo no)"
check "event path claims direct observation" "yes" \
  "$([[ "$(classify_note workflow_run push -)" == *"observed directly"* ]] && echo yes || echo no)"
# An unsubstituted placeholder must abort, never become a colliding key.
if ( cd "$REPO_ROOT" && GITHUB_OUTPUT=/dev/null EVENT=workflow_run PAYLOAD_RUN_EVENT=push \
      LATEST_CI_EVENT='' TITLE_CI="CI failed on main [ci-health]" bash "$WORK/classify.sh" >/dev/null 2>&1 ); then
  guard_rc=0
else
  guard_rc=$?
fi
check "missing placeholder aborts instead of creating a shared key" "1" "$guard_rc"

# 5. BEFORE (pre-#1712 model): one constant key for every origin. Two failures
#    of different causes collapse into ONE issue, and any green closes it.
reset_all
run_ci_health open "$RETIRED_TITLE" "$CTX" "Failed jobs: gate." ""
run_ci_health open "$RETIRED_TITLE" "$CTX" "Failed jobs: test." ""
check "BEFORE: two origins under one constant key -> 1 issue" "1" "$(issue_count)"
run_ci_health close "$RETIRED_TITLE" "$URL"
check "BEFORE: a green run closes the other origin's red" "1" "$(closed_count)"

# 6. AFTER: origin-qualified keys. A red push and a red schedule are two
#    issues, and a green of one origin closes only that one.
reset_all
TITLE_PUSH="$(classify_key workflow_run push - "$TITLE_TEMPLATE")"
TITLE_SCHEDULE="$(classify_key workflow_run schedule - "$TITLE_TEMPLATE")"
run_ci_health open "$TITLE_PUSH" "Run origin: push (observed directly)." "Failed jobs: gate." ""
PUSH_NUM="$(number_for "$TITLE_PUSH")"
check "AFTER: red push opens one issue" "1" "$(issue_count)"
check "AFTER: that issue is keyed by push" "$TITLE_PUSH" "$(cut -f2 "$STATE/issues")"
check "AFTER: its body names the origin" "yes" \
  "$(grep -q '^Run origin: push' "$STATE/body-$PUSH_NUM" && echo yes || echo no)"
run_ci_health open "$TITLE_SCHEDULE" "Run origin: schedule (observed directly)." "Failed jobs: test." ""
SCHED_NUM="$(number_for "$TITLE_SCHEDULE")"
check "AFTER: red schedule is a SECOND issue, not a comment" "2" "$(issue_count)"
check "AFTER: the two keys are both on file" \
  "$TITLE_PUSH|$TITLE_SCHEDULE|" "$(issue_titles)"
# A green schedule run must not close the push red.
run_ci_health close "$TITLE_SCHEDULE" "$URL"
check "AFTER: green schedule closes ONLY the schedule issue" "$SCHED_NUM" "$(closed_list)"
check "AFTER: green schedule leaves the push red OPEN" "no" "$(is_closed "$PUSH_NUM")"
# A green push run must not close the schedule red either.
reset_state
run_ci_health close "$TITLE_PUSH" "$URL"
check "AFTER: green push closes ONLY the push issue" "$PUSH_NUM" "$(closed_list)"
check "AFTER: the schedule red is still OPEN" "no" "$(is_closed "$SCHED_NUM")"
reset_state

# 7. Idempotency is not sacrificed: a second failure of the SAME origin is a
#    comment on the existing issue, never a second issue.
run_ci_health open "$TITLE_PUSH" "Run origin: push (observed directly)." "Failed jobs: gate, lint." ""
check "same-origin repeat failure -> still 2 issues total" "2" "$(issue_count)"
check "same-origin repeat failure -> commented, not recreated" "yes" \
  "$(grep -q 'Failed jobs: gate, lint' "$STATE/comment-$PUSH_NUM" 2>/dev/null && echo yes || echo no)"
check "same-origin repeat failure -> schedule issue untouched" "0" \
  "$(count_matches 'Still failing' "$STATE/comment-$SCHED_NUM")"
check "same-origin repeat failure -> exactly one extra comment" "1" \
  "$(count_matches 'Still failing' "$STATE/comment-$PUSH_NUM")"

# 8. The reconcile path keeps its own contract (issue criterion 2): the sweep
#    opens on failure, never duplicates, and closes only its own origin's green.
reset_all
run_ci_health reconcile "$TITLE_PUSH" "failure" "900" "CI" "Run origin: push (observed indirectly)."
REC_NUM="$(number_for "$TITLE_PUSH")"
check "reconcile: a red latest run opens the origin-qualified issue" "1" "$(issue_count)"
check "reconcile: that issue is keyed by push" "$TITLE_PUSH" "$(cut -f2 "$STATE/issues")"
run_ci_health reconcile "$TITLE_PUSH" "failure" "901" "CI" "Run origin: push (observed indirectly)."
check "reconcile: repeated red does not duplicate" "1" "$(issue_count)"
run_ci_health reconcile "$TITLE_SCHEDULE" "success" "902" "CI" "Run origin: schedule (observed indirectly)."
check "reconcile: a green of ANOTHER origin closes nothing" "0" "$(closed_count)"
check "reconcile: the red it did not cause stays OPEN" "no" "$(is_closed "$REC_NUM")"
run_ci_health reconcile "$TITLE_PUSH" "success" "903" "CI" "Run origin: push (observed indirectly)."
check "reconcile: same-origin green still closes (criterion preserved)" "$REC_NUM" "$(closed_list)"
check "reconcile: close comment names the run" "Green again: https://github.com/owner/repo/actions/runs/903" \
  "$(closed_comment "$REC_NUM")"
run_ci_health reconcile "$TITLE_PUSH" "cancelled" "904" "CI" "Run origin: push (observed indirectly)."
check "reconcile: a non terminal conclusion opens nothing and closes nothing" "1" "$(issue_count)"
check "reconcile: no extra close from a non terminal conclusion" "1" "$(closed_count)"

# 9. Drift pinning: no workflow step may carry its own close implementation,
#    and the origin must reach BOTH the open and the close path.
check "helper defines close_mine once" \
  "1" "$(grep -c '^close_mine()' "$REPO_ROOT/scripts/ci-health-close.sh" || true)"
check "no inline close left in ci-health.yml" \
  "" "$(grep -n 'gh issue close' "$WORKFLOW" || true)"
check "both steps source the helper" \
  "2" "$(grep -cF ". \"\$GITHUB_WORKSPACE/scripts/ci-health-close.sh\"" "$WORKFLOW" || true)"
# The raw template must never be used as a lookup key: that is the whole bug.
# shellcheck disable=SC2016  # the literal $TITLE_CI is the pattern, not a var
check "raw TITLE_CI is never used as a key" "" \
  "$(grep -nE '(close_mine|open_or_comment|reconcile_one) +"\$TITLE_CI"' "$WORKFLOW" || true)"
check "upsert + reconcile both consume the classified key" "2" \
  "$(grep -cF 'steps.origin.outputs.ci_title' "$WORKFLOW" || true)"
# The origin must be read, never assumed: event path from the payload, sweep
# from the run itself.
check "origin read from the workflow_run payload" "yes" \
  "$(grep -qF 'github.event.workflow_run.event' "$WORKFLOW" && echo yes || echo no)"
check "sweep reads the run's own event field" "yes" \
  "$(grep -qF -- '--json event --jq' "$WORKFLOW" && echo yes || echo no)"
# The watcher is read-only by construction: the only close in this workflow is
# close_mine, and the retired key never reaches it.
# shellcheck disable=SC2016  # the literal ${RETIRED_TITLE_CI} is the pattern
check "retired key watched, never auto-closed" "yes" \
  "$(grep -qF '${RETIRED_TITLE_CI} in:title' "$WORKFLOW" && echo yes || echo no)"

echo "test_ci_health_close: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: shared close helper and origin-scoped key behave as specified."
