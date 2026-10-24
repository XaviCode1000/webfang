#!/usr/bin/env bash
#
# Semantics harness for the CI Health Observer key + close path.
#
# The properties here are behavioral and cannot be proven by grepping:
#   1. close_mine closes exactly the open tracking issues that carry the
#      ci-health label — an unlabeled issue with a matching title must survive,
#      and an empty issue set must be a silent no-op;
#   2. the upsert and reconcile steps share ONE close implementation — the
#      drifted inline close branch in reconcile_one (missing the unlabeled
#      Skip notice) must stay deleted;
#   3. the tracking key carries the RUN ORIGIN (#1712): a red `push` and a red
#      `schedule` are two issues. The same key still yields ONE issue for
#      repeated failures of that origin.
#  4. the SCOPE predicate (#1714): a run that is not main-line CI never
#     reaches an issue. On the event path that is the job-level `if:` (the job
#     is skipped, so no step can open anything at all); on the sweep path it is
#     the guard inside reconcile_one, because the sweep reads the run's own
#     `event` and would otherwise walk in through the back door.
#  5. the W-03 classifier (#2233) matches a FAILURE LINE, not a mention of
#     the known RED test: a log that carries only the nextest FILTER line
#     classifies as known_only=false, a log that carries the FAIL line
#     classifies as known_only=true. The filter line contains the test name
#     in EVERY run, so matching the name muted the whole channel.
#  6. the CLOSE key is a SUBTITLE of the open keys (#2162, #1980): ONE close
#     implementation retires every CI-keyed tracker — the origin-qualified
#     ones AND the ones left under the retired title — and a green run of
#     any origin does it.
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
#
# THE CHECKS live here and only here: the fake `gh`, the issue state and the
# workflow extractors are machinery, and they live in the sibling library
# scripts/ci-health-fake-gh.sh, which this harness sources. Splitting the
# file keeps both halves under the ~500-line ceiling AGENTS.md sets, and it
# cannot weaken a check: every check below is still asserted here, against
# the same extracted code and the same fake `gh`.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKFLOW="$REPO_ROOT/.github/workflows/ci-health.yml"

# The machinery: the fake `gh`, the issue state and the workflow extractors.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=ci-health-fake-gh.sh
source "$SCRIPT_DIR/ci-health-fake-gh.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

ci_health_setup "$REPO_ROOT" "$WORKFLOW" "$WORK" "$BIN" "$STATE"

# The workflow's own constants: the harness never hardcodes a title, a label
# or the known RED test name. That was the #1712 coupling (a title change
# here turned ci.yml's repo-guards job red).
TITLE_TEMPLATE="$(ci_health_wf_env TITLE_CI "$WORKFLOW")"
RETIRED_TITLE="$(ci_health_wf_env RETIRED_TITLE_CI "$WORKFLOW")"
CI_CLOSE_KEY="$(ci_health_wf_env CI_CLOSE_KEY "$WORKFLOW")"
TITLE_DISCOVERY="$(ci_health_wf_env TITLE_DISCOVERY "$WORKFLOW")"
KNOWN_RED_TEST="$(ci_health_wf_env KNOWN_RED_TEST "$WORKFLOW")"
WF_LABEL_KNOWN_FP="$(ci_health_wf_env LABEL_KNOWN_FP "$WORKFLOW")"

# The run URL the observer's steps would carry, and the context line the
# BEFORE fixture opens its issue with.
URL="https://github.com/owner/repo/actions/runs/123"
CTX="Run origin: probe (test context)."

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

echo "test_ci_health_close: behavioral checks for the CI Health key + close helper"

# 0. The extracted code is really the workflow's, and the keys really derive
#    from its own constants. A vacuous extraction would pass everything below.
check "open_or_comment extracted from the workflow" "yes" \
  "$(grep -q '^open_or_comment() {' "$WORK/fn_open.sh" && echo yes || echo no)"
check "reconcile_one extracted from the workflow" "yes" \
  "$(grep -q '^reconcile_one() {' "$WORK/fn_reconcile.sh" && echo yes || echo no)"
check "origin classifier extracted from the workflow" "yes" \
  "$(grep -q 'ci_title=' "$WORK/classify.sh" && echo yes || echo no)"
check "W-03 classifier extracted from the workflow" "yes" \
  "$(grep -q 'known_only=' "$WORK/classify_bug.sh" && echo yes || echo no)"
check "upsert step extracted from the workflow" "yes" \
  "$(grep -q 'open_or_comment' "$WORK/upsert.sh" && echo yes || echo no)"
check "TITLE_CI read from the workflow (not hardcoded here)" "yes" \
  "$([[ -n "$TITLE_TEMPLATE" ]] && echo yes || echo no)"
check "TITLE_CI carries an origin placeholder" "yes" \
  "$([[ "$TITLE_TEMPLATE" == *"{origin}"* ]] && echo yes || echo no)"
check "the close key is read from the workflow" "yes" \
  "$([[ -n "$CI_CLOSE_KEY" ]] && echo yes || echo no)"
check "the known RED test is read from the workflow" "yes" \
  "$([[ -n "$KNOWN_RED_TEST" ]] && echo yes || echo no)"
# Real `in:title` search strips punctuation and ANDs words, so the origin only
# discriminates if it lands as its own term — i.e. it is not glued to a
# neighbouring word. "(push)" qualifies: the parens are stripped (evidence in
# the fake-gh header). "mainpush" would not.
check "origin lands as its own search term" "yes" \
  "$(printf '%s' "$TITLE_TEMPLATE" | grep -qE '[^[:alnum:]_]\{origin\}[^[:alnum:]_]' && echo yes || echo no)"
check "retired title declared (detection + subtitle close)" \
  "Scheduled CI failed on main [ci-health]" "$RETIRED_TITLE"

# 1. A labeled open issue with a matching title is closed with a Green-again comment.
ci_health_reset_all "$STATE"
TITLE="$(ci_health_classify_key workflow_run push - "$TITLE_TEMPLATE")"
printf '42\t%s\tci-health,automated\n' "$TITLE" >"$STATE/issues"
if ci_health_run close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "labeled issue -> exit 0" "0" "$rc"
check "labeled issue -> exactly one close" "42" "$(ci_health_closed_list "$STATE")"
check "labeled issue -> Green-again comment names the run" \
  "Green again: $URL" "$(ci_health_closed_comment "$STATE" 42)"

# 2. An unlabeled issue with a matching title is skipped, never closed.
ci_health_reset_state "$STATE"
printf '7\t%s\tneeds-triage\n' "$TITLE" >"$STATE/issues"
out="$(ci_health_run close "$TITLE" "$URL" 2>&1 || true)"
check "unlabeled issue -> no close calls" "" "$(ci_health_closed_list "$STATE")"
case "$out" in
  *"Skip #7"*) check "unlabeled issue -> Skip notice printed" "yes" "yes" ;;
  *) check "unlabeled issue -> Skip notice printed" "yes" "no" ;;
esac

# 3. An empty issue set is a silent no-op with exit zero.
ci_health_reset_state "$STATE"
: >"$STATE/issues"
if ci_health_run close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "empty set -> exit 0" "0" "$rc"
check "empty set -> no close calls" "" "$(ci_health_closed_list "$STATE")"

# 3b. A FAILED label read is NOT a missing label (B50). Pre-fix the failed
#     `gh issue view` made the label condition false, so the helper printed
#     "Skip #N: missing label" and exited 0: the stale issue stayed open and
#     the reconcile step reported success. The error must propagate instead.
ci_health_reset_state "$STATE"
printf '77\t%s\tci-health,automated\n' "$TITLE" >"$STATE/issues"
: >"$STATE/view-fails"
if ci_health_run close "$TITLE" "$URL" >"$WORK/close-fail-out" 2>&1; then rc=0; else rc=$?; fi
fail_out="$(cat "$WORK/close-fail-out")"
check "unreadable labels -> non-zero exit (fail closed)" "yes" \
  "$([[ $rc -ne 0 ]] && echo yes || echo no)"
check "unreadable labels -> no close calls" "" "$(ci_health_closed_list "$STATE")"
case "$fail_out" in
  *"Skip #77"*) check "unreadable labels -> NOT reported as a missing label" "yes" "no" ;;
  *) check "unreadable labels -> NOT reported as a missing label" "yes" "yes" ;;
esac
case "$fail_out" in
  *"could not read the labels"*) check "unreadable labels -> the read error is reported" "yes" "yes" ;;
  *) check "unreadable labels -> the read error is reported" "yes" "no" ;;
esac
rm -f "$STATE/view-fails"
# The same issue, readable again, is closed normally: the guard is a fail
# closed on the READ, not a blanket mute.
ci_health_reset_state "$STATE"
if ci_health_run close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "labels readable again -> exit 0" "0" "$rc"
check "labels readable again -> the issue is closed" "77" "$(ci_health_closed_list "$STATE")"

# 4. The origin -> key rule, executed from the workflow's own classifier.
check "event path: push run -> push key" "CI failed on main (push) [ci-health]" \
  "$(ci_health_classify_key workflow_run push - "$TITLE_TEMPLATE")"
check "event path: schedule run -> schedule key" "CI failed on main (schedule) [ci-health]" \
  "$(ci_health_classify_key workflow_run schedule - "$TITLE_TEMPLATE")"
check "event path: pull_request run -> its own key" "CI failed on main (pull_request) [ci-health]" \
  "$(ci_health_classify_key workflow_run pull_request - "$TITLE_TEMPLATE")"
check "event path: manual run -> its own key" "CI failed on main (workflow_dispatch) [ci-health]" \
  "$(ci_health_classify_key workflow_run workflow_dispatch - "$TITLE_TEMPLATE")"
# THE deliverable: a push red and a schedule red do not share a key.
check "push key != schedule key" "distinct" \
  "$([[ "$(ci_health_classify_key workflow_run push - "$TITLE_TEMPLATE")" != \
        "$(ci_health_classify_key workflow_run schedule - "$TITLE_TEMPLATE")" ]] && echo distinct || echo equal)"
# Bounded key space: an event this observer does not model collapses to
# `other` instead of minting a new issue per event name GitHub may add.
check "unmodeled event -> other" "CI failed on main (other) [ci-health]" \
  "$(ci_health_classify_key workflow_run merge_group - "$TITLE_TEMPLATE")"
check "empty payload event -> other" "CI failed on main (other) [ci-health]" \
  "$(ci_health_classify_key workflow_run "" - "$TITLE_TEMPLATE")"
# The sweep does not cause the runs it reads, so it must not claim `schedule`:
# it reports the origin of the run it actually found.
check "reconcile on schedule trigger: reads the run's own origin" \
  "CI failed on main (push) [ci-health]" \
  "$(ci_health_classify_key schedule - push "$TITLE_TEMPLATE")"
check "reconcile on dispatch trigger: reads the run's own origin" \
  "CI failed on main (push) [ci-health]" \
  "$(ci_health_classify_key workflow_dispatch - push "$TITLE_TEMPLATE")"
check "reconcile with no run event -> other, never a bare 'schedule'" \
  "CI failed on main (other) [ci-health]" \
  "$(ci_health_classify_key schedule - "" "$TITLE_TEMPLATE")"
check "reconcile says how it observed the run" "yes" \
  "$([[ "$(ci_health_classify_note schedule - push)" == *"observed indirectly"* ]] && echo yes || echo no)"
check "event path claims direct observation" "yes" \
  "$([[ "$(ci_health_classify_note workflow_run push -)" == *"observed directly"* ]] && echo yes || echo no)"
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
ci_health_reset_all "$STATE"
ci_health_run open "$RETIRED_TITLE" "$CTX" "Failed jobs: gate." ""
ci_health_run open "$RETIRED_TITLE" "$CTX" "Failed jobs: test." ""
check "BEFORE: two origins under one constant key -> 1 issue" "1" "$(ci_health_issue_count "$STATE")"
ci_health_run close "$RETIRED_TITLE" "$URL"
check "BEFORE: a green run closes the other origin's red" "1" "$(ci_health_closed_count "$STATE")"

# 6. AFTER: origin-qualified keys for OPENING, ONE close key for CLOSING.
#    #1712 kept the origin inside the title so two causes do not collapse into
#    one issue; #2162/#1980 need a green run to retire EVERY CI-keyed tracker
#    (the origin-qualified ones AND the ones under the retired title), so the
#    CLOSE key is a SUBTITLE of the open keys.
ci_health_reset_all "$STATE"
TITLE_PUSH="$(ci_health_classify_key workflow_run push - "$TITLE_TEMPLATE")"
TITLE_SCHEDULE="$(ci_health_classify_key workflow_run schedule - "$TITLE_TEMPLATE")"
ci_health_run open "$TITLE_PUSH" "Run origin: push (observed directly)." "Failed jobs: gate." ""
PUSH_NUM="$(ci_health_number_for "$STATE" "$TITLE_PUSH")"
check "AFTER: red push opens one issue" "1" "$(ci_health_issue_count "$STATE")"
check "AFTER: that issue is keyed by push" "$TITLE_PUSH" "$(cut -f2 "$STATE/issues")"
check "AFTER: its body names the origin" "yes" \
  "$(grep -q '^Run origin: push' "$STATE/body-$PUSH_NUM" && echo yes || echo no)"
ci_health_run open "$TITLE_SCHEDULE" "Run origin: schedule (observed directly)." "Failed jobs: test." ""
SCHED_NUM="$(ci_health_number_for "$STATE" "$TITLE_SCHEDULE")"
check "AFTER: red schedule is a SECOND issue, not a comment" "2" "$(ci_health_issue_count "$STATE")"
check "AFTER: the two keys are both on file" \
  "$TITLE_PUSH|$TITLE_SCHEDULE|" "$(ci_health_issue_titles "$STATE")"
# A green run of ANY origin retires every CI-keyed tracker: the close key is a
# subtitle of both origin-qualified titles.
ci_health_run close "$CI_CLOSE_KEY" "$URL"
check "AFTER: a green run retires the push issue" "yes" "$(ci_health_is_closed "$STATE" "$PUSH_NUM")"
check "AFTER: a green run retires the schedule issue too" "yes" "$(ci_health_is_closed "$STATE" "$SCHED_NUM")"
check "AFTER: a green run closed exactly those two" "2" "$(ci_health_closed_count "$STATE")"
ci_health_reset_state "$STATE"

# 7. Idempotency is not sacrificed: a second failure of the SAME origin is a
#    comment on the existing issue, never a second issue.
ci_health_run open "$TITLE_PUSH" "Run origin: push (observed directly)." "Failed jobs: gate, lint." ""
check "same-origin repeat failure -> still 2 issues total" "2" "$(ci_health_issue_count "$STATE")"
check "same-origin repeat failure -> commented, not recreated" "yes" \
  "$(grep -q 'Failed jobs: gate, lint' "$STATE/comment-$PUSH_NUM" 2>/dev/null && echo yes || echo no)"
check "same-origin repeat failure -> schedule issue untouched" "0" \
  "$(ci_health_count_matches 'Still failing' "$STATE/comment-$SCHED_NUM")"
check "same-origin repeat failure -> exactly one extra comment" "1" \
  "$(ci_health_count_matches 'Still failing' "$STATE/comment-$PUSH_NUM")"

# 8. The reconcile path keeps its own contract (issue criterion 2): the sweep
#    opens on failure, never duplicates, and a green retires the CI-keyed
#    trackers through the SAME close key the event path uses.
ci_health_reset_all "$STATE"
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "failure" "900" "CI" "Run origin: push (observed indirectly)." "push"
REC_NUM="$(ci_health_number_for "$STATE" "$TITLE_PUSH")"
check "reconcile: a red latest run opens the origin-qualified issue" "1" "$(ci_health_issue_count "$STATE")"
check "reconcile: that issue is keyed by push" "$TITLE_PUSH" "$(cut -f2 "$STATE/issues")"
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "failure" "901" "CI" "Run origin: push (observed indirectly)." "push"
check "reconcile: repeated red does not duplicate" "1" "$(ci_health_issue_count "$STATE")"
# A green of ANY origin retires the CI-keyed tracker (subtitle close).
ci_health_run reconcile "$TITLE_SCHEDULE" "$CI_CLOSE_KEY" "success" "902" "CI" "Run origin: schedule (observed indirectly)." "schedule"
check "reconcile: a green of ANY origin retires the CI-keyed issue" "yes" "$(ci_health_is_closed "$STATE" "$REC_NUM")"
check "reconcile: that green closed exactly one issue" "1" "$(ci_health_closed_count "$STATE")"
# The same-origin green still closes (criterion preserved), from a fresh red.
ci_health_reset_all "$STATE"
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "failure" "905" "CI" "Run origin: push (observed indirectly)." "push"
REC_NUM2="$(ci_health_number_for "$STATE" "$TITLE_PUSH")"
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "success" "906" "CI" "Run origin: push (observed indirectly)." "push"
check "reconcile: same-origin green still closes (criterion preserved)" "yes" "$(ci_health_is_closed "$STATE" "$REC_NUM2")"
check "reconcile: close comment names the run" "Green again: https://github.com/owner/repo/actions/runs/906" \
  "$(ci_health_closed_comment "$STATE" "$REC_NUM2")"
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "cancelled" "907" "CI" "Run origin: push (observed indirectly)." "push"
check "reconcile: a non terminal conclusion opens nothing and closes nothing" "1" "$(ci_health_issue_count "$STATE")"
check "reconcile: no extra close from a non terminal conclusion" "1" "$(ci_health_closed_count "$STATE")"

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
# The watcher is read-only: the observer does NOT OPEN the retired key any
# more. It is NOT a dead end either: the close key is a subtitle of the
# retired title, so the same close implementation retires it.
# shellcheck disable=SC2016  # the literal ${RETIRED_TITLE_CI} is the pattern
check "retired key still detected" "yes" \
  "$(grep -qF '${RETIRED_TITLE_CI} in:title' "$WORKFLOW" && echo yes || echo no)"
check "the retired key is no longer declared a dead end" "0" \
  "$(grep -c 'nothing will ever close them' "$WORKFLOW" || true)"
# The close key is a subtitle of BOTH open keys (#2162, #1980).
check "the close key is a subtitle of the origin-qualified title" "yes" \
  "$([[ "$TITLE_TEMPLATE" == *"$CI_CLOSE_KEY"* ]] && echo yes || echo no)"
check "the close key is a subtitle of the retired title" "yes" \
  "$([[ "$RETIRED_TITLE" == *"$CI_CLOSE_KEY"* ]] && echo yes || echo no)"
# ONE close path: the CI close uses the close key, never the origin title.
# shellcheck disable=SC2016  # the literal $CI_CLOSE_KEY is the pattern, not a var
check "the event path closes with the close key" "yes" \
  "$(grep -qF 'close_mine "$CI_CLOSE_KEY"' "$WORKFLOW" && echo yes || echo no)"
# shellcheck disable=SC2016  # the literal $CI_TITLE/$CI_CLOSE_KEY are patterns
check "the sweep closes with the close key" "yes" \
  "$(grep -qF 'reconcile_one "$CI_TITLE" "$CI_CLOSE_KEY"' "$WORKFLOW" && echo yes || echo no)"
# shellcheck disable=SC2016  # the literal $CI_TITLE is the pattern, not a var
check "no close path uses the origin-qualified title" "" \
  "$(grep -nE 'close_mine +"\$CI_TITLE"' "$WORKFLOW" || true)"

# 10. SCOPE (#1714). Two paths could open an issue for a pull_request run and
#     BOTH must be closed. The event path is closed at the job gate, so the
#     behavior to prove here is structural: the gate exists, it is on the JOB
#     (a step gate would leave the checkout and the label bootstrap running),
#     it is stated in terms of the OBSERVED run's payload, and the clause that
#     let everything through is gone. The sweep path is executable code, so it
#     is executed.
echo "== 10. scope gate (#1714) =="
GATE_PULL_CLAUSE="github.event.workflow_run.event != 'pull_request'"
# NOTE: the two line probes below must tolerate "no match" WITHOUT tripping
# `set -e`/`pipefail`: a harness that crashes on the regression it is meant to
# report is worse than no harness. Each probe yields an empty string and the
# checks then report FAIL themselves.
GATE_LINE="$(awk -v c="$GATE_PULL_CLAUSE" 'index($0, c) { print NR; exit }' "$WORKFLOW" || true)"
STEPS_LINE="$(awk '/^    steps:/ { print NR; exit }' "$WORKFLOW" || true)"
check "job gate rejects pull_request-origin runs" "1" \
  "$([[ -n "$GATE_LINE" ]] && echo 1 || echo 0)"
check "gate is on the JOB, above every step" "yes" \
  "$([[ -n "$GATE_LINE" && -n "$STEPS_LINE" && "$GATE_LINE" -lt "$STEPS_LINE" ]] && echo yes || echo no)"
check "gate reads the OBSERVED run's branch, not only its event" "2" \
  "$(grep -cE "github\.event\.workflow_run\.head_branch == '(main|develop)'" "$WORKFLOW")"
# The leak: an OR'd `event_name == 'workflow_run'` (or a github.ref-only gate)
# on the workflow_run path admits every observed run, because GITHUB_REF is
# the default branch for that event.
check "no unconditional workflow_run clause left in the gate" "" \
  "$(grep -nF "|| github.event_name == 'workflow_run'" "$WORKFLOW" || true)"
check "the leak clause is gone from the gate too" "" \
  "$(grep -nF "github.ref == 'refs/heads/main' || github.ref == 'refs/heads/develop' || " "$WORKFLOW" || true)"
# sweep path: reconcile_one refuses a pull_request run, open AND close.
ci_health_reset_all "$STATE"
printf '51\t%s\tci-health,automated\n' "$TITLE_PUSH" >"$STATE/issues"
out="$(ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "success" "910" "CI" "Run origin: pull_request (observed indirectly)." "pull_request" 2>&1 || true)"
check "sweep: a green pull_request run closes NOTHING" "0" "$(ci_health_closed_count "$STATE")"
check "sweep: the main-line red it did not cause stays OPEN" "no" "$(ci_health_is_closed "$STATE" 51)"
check "sweep: out-of-scope decision is logged, not silent" "yes" \
  "$([[ "$out" == *"out of scope"* ]] && echo yes || echo no)"
out="$(ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "failure" "911" "CI" "Run origin: pull_request (observed indirectly)." "pull_request" 2>&1 || true)"
check "sweep: a red pull_request run opens NOTHING" "1" "$(ci_health_issue_count "$STATE")"
check "sweep: still no close after the red pull_request run" "0" "$(ci_health_closed_count "$STATE")"
# The same source in scope must still work, otherwise the guard is a blanket
# mute rather than a scope filter.
ci_health_run reconcile "$TITLE_PUSH" "$CI_CLOSE_KEY" "success" "912" "CI" "Run origin: push (observed indirectly)." "push"
check "sweep: an in-scope green still closes (guard is a filter, not a mute)" "51" "$(ci_health_closed_list "$STATE")"
# The Bug Discovery / Benches call sites must pass the run's own event too, or
# the guard is CI-only and the same door stays open for the other two sources.
# shellcheck disable=SC2016  # the literal $TITLE_CI-style names are the pattern
check "every reconcile call site passes the run's own event" "3" \
  "$(grep -cE 'reconcile_one "\$[A-Z_]+" "\$[A-Z_]+" "\$[A-Z_]+" "\$[A-Z_]+" "[^"]+" ".*" "\$[A-Z_]+"' "$WORKFLOW")"
# shellcheck disable=SC2016  # the literal $7 is the workflow's positional arg
check "reconcile_one takes the run event as a parameter" "yes" \
  "$(grep -qF 'run_event="$7"' "$WORKFLOW" && echo yes || echo no)"
check "sweep reads each run's own event field (one call, all three sources)" "1" \
  "$(grep -cF -- '--json event --jq' "$WORKFLOW")"

# 11. W-03 classifier (#2233). The Bug Discovery workflow prints its own
#     nextest filter, and that filter CONTAINS the known RED test name, so a
#     classifier that greps the NAME over the whole log finds it in EVERY
#     run: known_only was always true, so every real finding was filed as
#     "Known FP only (W-03)" and the reporting channel went silent. The
#     classifier must match a FAILURE LINE, not a mention.
echo "== 11. W-03 classifier: a failure line, not a mention (#2233) =="
FILTER_LOG="$WORK/log-filter-only.log"
sed "s|KNOWN_RED_TEST_NAME|$KNOWN_RED_TEST|g" >"$FILTER_LOG" <<'LOG'
2026-10-14T06:00:12.3456789Z ##[group]Run cargo nextest run --workspace --all-features --run-ignored only --no-fail-fast
2026-10-14T06:00:12.4567890Z   -E 'not(test(/session_pool/) | test(/vault_detector/) | test(/cli_binary_test/) | test(/KNOWN_RED_TEST_NAME/))'
2026-10-14T06:00:41.1234567Z Summary [   4.210s] 128 tests run: 128 passed
LOG
FAILURE_LOG="$WORK/log-with-failure.log"
sed "s|KNOWN_RED_TEST_NAME|$KNOWN_RED_TEST|g" >"$FAILURE_LOG" <<'LOG'
2026-10-14T06:00:12.4567890Z   -E 'not(test(/session_pool/) | test(/vault_detector/) | test(/cli_binary_test/) | test(/KNOWN_RED_TEST_NAME/))'
2026-10-14T06:00:41.1234567Z         FAIL [   0.005s] webfang_core application::crawler::sitemap_session::tests::KNOWN_RED_TEST_NAME
2026-10-14T06:00:41.2234567Z Summary [   4.210s] 128 tests run: 127 passed, 1 failed
LOG
check "a log with only the filter line is NOT the known RED" "false" \
  "$(ci_health_classify_bug "$FILTER_LOG")"
check "a log with the failure line IS the known RED" "true" \
  "$(ci_health_classify_bug "$FAILURE_LOG")"
check "the classifier keys on the workflow's known RED test" "yes" \
  "$(grep -qF 'KNOWN_RED_TEST' "$WORK/classify_bug.sh" && echo yes || echo no)"
check "the classifier matches a failure line, not a bare name" "yes" \
  "$(grep -qF 'FAIL[[:space:]]+' "$WORK/classify_bug.sh" && echo yes || echo no)"

# 12. What a finding is filed as depends on the classification (#2233): the
#     known RED is filed as a known false positive, and a real failure opens
#     the tracker that asks for triage.
echo "== 12. a real failure is filed for triage (#2233) =="
ci_health_reset_all "$STATE"
ci_health_upsert_step "Production Bug Discovery" "failure" "false" || true
check "a real failure opens the discovery tracker" "1" "$(ci_health_issue_count "$STATE")"
check "a real failure is NOT filed as a known false positive" "no" \
  "$(ci_health_has_label "$STATE" "$TITLE_DISCOVERY" "$WF_LABEL_KNOWN_FP")"
check "a real failure demands triage" "yes" \
  "$(ci_health_body_of "$STATE" "$(ci_health_number_for "$STATE" "$TITLE_DISCOVERY")" | grep -q 'Findings to triage' && echo yes || echo no)"
ci_health_reset_all "$STATE"
ci_health_upsert_step "Production Bug Discovery" "failure" "true" || true
check "the known RED is filed as a known false positive" "yes" \
  "$(ci_health_has_label "$STATE" "$TITLE_DISCOVERY" "$WF_LABEL_KNOWN_FP")"
check "the known RED does not demand triage" "yes" \
  "$(ci_health_body_of "$STATE" "$(ci_health_number_for "$STATE" "$TITLE_DISCOVERY")" | grep -q 'Known FP only' && echo yes || echo no)"

# 13. ONE close implementation, ONE subtitle (#2162, #1980): a green run
#     retires every CI-keyed tracker, including the ones left under the
#     retired title, and it touches nothing else.
echo "== 13. one close implementation, one subtitle (#2162, #1980) =="
ci_health_reset_all "$STATE"
printf '81\t%s\tci-health,automated\n' "$TITLE_PUSH" >"$STATE/issues"
printf '82\t%s\tci-health,automated\n' "$RETIRED_TITLE" >>"$STATE/issues"
printf '83\t%s\tci-health,automated\n' "$TITLE_DISCOVERY" >>"$STATE/issues"
ci_health_run close "$CI_CLOSE_KEY" "$URL"
check "the subtitle close retires the origin-qualified tracker" "yes" "$(ci_health_is_closed "$STATE" 81)"
check "the subtitle close retires the RETIRED-title tracker" "yes" "$(ci_health_is_closed "$STATE" 82)"
check "the subtitle close leaves the discovery tracker alone" "no" "$(ci_health_is_closed "$STATE" 83)"
check "the subtitle close touched exactly the two CI trackers" "2" "$(ci_health_closed_count "$STATE")"
# The SAME thing, through the workflow's OWN close path (the CI branch of the
# upsert step), so the proof is not only of the helper but of the wiring.
ci_health_reset_all "$STATE"
printf '91\t%s\tci-health,automated\n' "$TITLE_PUSH" >"$STATE/issues"
printf '92\t%s\tci-health,automated\n' "$RETIRED_TITLE" >>"$STATE/issues"
printf '93\t%s\tci-health,automated\n' "$TITLE_DISCOVERY" >>"$STATE/issues"
ci_health_upsert_step "CI" "success" "" || true
check "the observer's green CI run retires the origin-qualified tracker" "yes" "$(ci_health_is_closed "$STATE" 91)"
check "the observer's green CI run retires the RETIRED-title tracker" "yes" "$(ci_health_is_closed "$STATE" 92)"
check "the observer's green CI run leaves the discovery tracker alone" "no" "$(ci_health_is_closed "$STATE" 93)"

echo "test_ci_health_close: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: shared close helper and origin-scoped key behave as specified."
