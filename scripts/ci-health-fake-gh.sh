#!/usr/bin/env bash
#
# Shared test machinery for the CI Health Observer harness.
#
# Source-only library, sibling of scripts/ci-health-close.sh: the harness
# (scripts/test_ci_health_close.sh) sources this file and gets
#   * the fake `gh` (a script written on PATH, backed by a state directory),
#   * the issue-state helpers (the model of the fake repository),
#   * the extractors that pull the REAL code out of the workflow file,
#   * the wrappers that EXECUTE that extracted code against the fake `gh`.
#
# WHY THIS FILE EXISTS: the harness grew past the ~500-line ceiling AGENTS.md
# sets for new files, and the half that grows with the observer is the
# machinery, not the checks. Every CHECK stays in the harness, so the number
# of checks lives in exactly one place and this file can never weaken one.
#
# The name carries NO `test_` prefix on purpose: a `test_`-prefixed shell
# file in scripts/ reads like a test entry point, and the repository's test
# inventories track the test surface. This file is a library, not a test.
#
# Usage:
#   SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
#   source "$SCRIPT_DIR/ci-health-fake-gh.sh"
#
# Nothing here touches the network, real issues or the repository state: the
# fake `gh` only ever reads and writes the state directory it is given, and
# the extractors only ever read the workflow file.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  echo "usage: source scripts/ci-health-fake-gh.sh" >&2
  exit 2
fi

# The fixture paths, recorded once by ci_health_setup and read by the
# execution wrappers below.
CI_HEALTH_REPO_ROOT=""
CI_HEALTH_WORKFLOW=""
CI_HEALTH_WORK=""
CI_HEALTH_BIN=""
CI_HEALTH_STATE=""
CI_HEALTH_TITLE_TEMPLATE=""

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# State file $STATE/issues holds TSV rows: number<TAB>title<TAB>comma-labels.
# $STATE/body-<n> holds the body of a created issue, $STATE/comment-<n> the
# appended comments of one. `gh issue close` appends the number to
# $STATE/closed; later `issue list` calls hide closed issues.
#
# The fake models GitHub's real `in:title` search: it strips punctuation and
# ANDs the words (verified 2026-09-29 against this repo: `--search
# 'windows-latest in:title'` finds #1697 "fix(ci): Tests (windows-latest) …",
# and adding one term that is absent from a title drops the hit to zero). It
# used to compare byte-for-byte, which was STRICTER than the real search; the
# subtitle close (#2162, #1980) DEPENDS on the real semantics — the close key
# "CI failed on main" must find "Scheduled CI failed on main [ci-health]" and
# "CI failed on main (push) [ci-health]" — so the fake now ANDs the words of
# the search over the words of the title.
#
# ci_health_install_fake_gh <bin-dir> -> writes the fake `gh` there and makes
# it executable.
ci_health_install_fake_gh() {
  local bin_dir="$1"
  mkdir -p "$bin_dir"
  cat >"$bin_dir/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

# words <text> -> one word per line, lowercased, punctuation stripped.
words() {
  printf '%s\n' "$1" | tr '[:upper:]' '[:lower:]' | tr -c '[:alnum:]-' '\n' | sed '/^$/d'
}

# title_matches <search> <title> -> true when every word of the search is a
# word of the title (GitHub's `in:title` AND semantics).
title_matches() {
  local search="$1" title="$2" word title_words
  [[ -n "$search" ]] || return 1
  title_words="$(words "$title")"
  while read -r word; do
    [[ -n "$word" ]] || continue
    grep -qxF -- "$word" <<<"$title_words" || return 1
  done < <(words "$search")
  return 0
}

sub="$1 $2"
shift 2
case "$sub" in
  "run view")
    # The W-03 classifier reads the run log. The fixture path comes from
    # FAKE_LOG; an absent fixture is an empty log (no evidence at all).
    if [[ -n "${FAKE_LOG:-}" && -f "${FAKE_LOG:-}" ]]; then
      cat "$FAKE_LOG"
    fi
    exit 0
    ;;
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
    if [[ -f "$state/issues" ]] && [[ -n "$title" ]]; then
      while IFS=$'\t' read -r number row_title _labels; do
        [[ -n "$number" ]] || continue
        title_matches "$title" "$row_title" || continue
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
    # Simulated API failure: the labels of the issue cannot be read at all.
    if [[ -f "$state/view-fails" ]]; then
      echo "gh: simulated API failure (500)" >&2
      exit 1
    fi
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
  chmod +x "$bin_dir/gh"
}

# ─── Extract the real workflow code (no copies, no reimplementation) ───────────
# The harness never re-implements the observer: it EXTRACTS the real code out
# of .github/workflows/ci-health.yml and runs it against the fake `gh`, so a
# change in the workflow's key logic fails the harness instead of turning CI
# red in ci.yml's repo-guards job.

# ci_health_extract_run_block <step-name-needle> <file>  -> that step's
# `run: |` payload, dedented. Walks past the step's own id/env keys to reach
# the run block.
ci_health_extract_run_block() {
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

# ci_health_extract_fn <fn-name> <file>  -> function source, dedented
ci_health_extract_fn() {
  awk -v fn="$1" '
    $0 ~ "^ +" fn "\\(\\) \\{" { grab = 1 }
    grab == 1 {
      sub(/^ +/, "")
      print
      if ($0 ~ /^}$/) { exit }
    }
  ' "$2"
}

# ci_health_wf_env <KEY> <workflow> -> the value declared in the workflow's env
# block. The harness never hardcodes a title, a label or the known RED test
# name: that was the #1712 coupling (a title change in the harness turned
# ci.yml's repo-guards job red).
ci_health_wf_env() {
  awk -v k="$1" '
    $0 ~ "^  " k ": " {
      v = $0
      sub("^  " k ": ", "", v)
      gsub(/^"|"$/, "", v)
      print v
      exit
    }
  ' "$2"
}

# ci_health_prepare_runner <work-dir> <workflow> <repo-root>
#
# Extracts the observer's real code into <work-dir> and writes the runner that
# executes it with the same environment the real step has:
#   <work-dir>/classify.sh      the origin classifier (a whole step)
#   <work-dir>/fn_open.sh       open_or_comment (a function)
#   <work-dir>/fn_reconcile.sh  reconcile_one (a function)
#   <work-dir>/classify_bug.sh  the W-03 classifier (a whole step)
#   <work-dir>/upsert.sh        the upsert step (a whole step)
#   <work-dir>/run_ci_health.sh the runner the harness calls
#
# The W-03 classifier (#2233) and the upsert step (#2233, #2162) are whole
# steps too, and they are executed here: the classifier against a fixture log
# and the upsert against the fake `gh`, so a broken classifier cannot pass.
ci_health_prepare_runner() {
  local work="$1" workflow="$2" repo_root="$3"
  local url="https://github.com/owner/repo/actions/runs/123"
  local wf_label wf_label_auto wf_label_known_fp
  local title_discovery title_bench known_red_test ci_close_key

  ci_health_extract_run_block "- name: Classify run origin" "$workflow" >"$work/classify.sh"
  ci_health_extract_fn "open_or_comment" "$workflow" >"$work/fn_open.sh"
  ci_health_extract_fn "reconcile_one" "$workflow" >"$work/fn_reconcile.sh"
  ci_health_extract_run_block "- name: Classify Bug Discovery result" "$workflow" >"$work/classify_bug.sh"
  ci_health_extract_run_block "- name: Upsert tracking issue for triggering run" "$workflow" >"$work/upsert.sh"

  wf_label="$(ci_health_wf_env LABEL "$workflow")"
  wf_label_auto="$(ci_health_wf_env LABEL_AUTO "$workflow")"
  wf_label_known_fp="$(ci_health_wf_env LABEL_KNOWN_FP "$workflow")"
  title_discovery="$(ci_health_wf_env TITLE_DISCOVERY "$workflow")"
  title_bench="$(ci_health_wf_env TITLE_BENCH "$workflow")"
  known_red_test="$(ci_health_wf_env KNOWN_RED_TEST "$workflow")"
  ci_close_key="$(ci_health_wf_env CI_CLOSE_KEY "$workflow")"

  cat >"$work/run_ci_health.sh" <<RUNNER
set -euo pipefail
# shellcheck disable=SC1090
source "$repo_root/scripts/ci-health-close.sh"
source "$work/fn_open.sh"
source "$work/fn_reconcile.sh"
SERVER="https://github.com"
GH_REPO="owner/repo"
RUN_URL="$url"
BRANCH="main"
export LABEL="$wf_label"
export LABEL_AUTO="$wf_label_auto"
export LABEL_KNOWN_FP="$wf_label_known_fp"
export TITLE_DISCOVERY="$title_discovery"
export TITLE_BENCH="$title_bench"
export KNOWN_RED_TEST="$known_red_test"
export CI_CLOSE_KEY="$ci_close_key"
export GITHUB_WORKSPACE="$repo_root"
case "\$1" in
  open) shift; open_or_comment "\$@" ;;
  close) shift; close_mine "\$@" ;;
  reconcile) shift; reconcile_one "\$@" ;;
  classify_bug)
    shift
    # The workflow's own W-03 step, executed against the fake gh log.
    GITHUB_OUTPUT="\$2" RUN_ID=1 FAKE_LOG="\$1" bash "$work/classify_bug.sh" >/dev/null
    ;;
  upsert)
    shift
    # The workflow's own upsert step, executed against the fake gh.
    export PAYLOAD_WORKFLOW="\$1" PAYLOAD_CONCLUSION="\$2" FAILED_JOBS="\$3" KNOWN_ONLY="\$4"
    export PAYLOAD_ID=123 CI_TITLE="\$5" CI_ORIGIN="push" OBS_NOTE="test context."
    source "$work/upsert.sh"
    ;;
  *) echo "runner: unknown op \$1" >&2; exit 97 ;;
esac
RUNNER
}

# ─── Run the extracted code against the fake `gh` ─────────────────────────────
# ci_health_setup <repo-root> <workflow> <work-dir> <bin-dir> <state-dir>
#
# Installs the fake `gh`, extracts the observer's real code and writes the
# runner. After this, the wrappers below can execute the workflow's own code.
ci_health_setup() {
  CI_HEALTH_REPO_ROOT="$1"
  CI_HEALTH_WORKFLOW="$2"
  CI_HEALTH_WORK="$3"
  CI_HEALTH_BIN="$4"
  CI_HEALTH_STATE="$5"
  CI_HEALTH_TITLE_TEMPLATE="$(ci_health_wf_env TITLE_CI "$CI_HEALTH_WORKFLOW")"
  ci_health_install_fake_gh "$CI_HEALTH_BIN"
  ci_health_prepare_runner "$CI_HEALTH_WORK" "$CI_HEALTH_WORKFLOW" "$CI_HEALTH_REPO_ROOT"
}

# ci_health_run <op> [args...] -> run one operation of the extracted code:
# open | close | reconcile | classify_bug | upsert.
ci_health_run() {
  PATH="$CI_HEALTH_BIN:$PATH" FAKE_STATE="$CI_HEALTH_STATE" \
    bash "$CI_HEALTH_WORK/run_ci_health.sh" "$@"
}

# ci_health_classify_bug <log fixture> -> the known_only value the workflow's
# own W-03 step produced for that log.
ci_health_classify_bug() {
  local out="$CI_HEALTH_WORK/gostep-bug.out"
  : >"$out"
  (
    cd "$CI_HEALTH_REPO_ROOT" || exit 1
    PATH="$CI_HEALTH_BIN:$PATH" FAKE_STATE="$CI_HEALTH_STATE" \
      bash "$CI_HEALTH_WORK/run_ci_health.sh" classify_bug "$1" "$out" >/dev/null
  )
  sed -n 's/^known_only=//p' "$out"
}

# ci_health_upsert_step <workflow> <conclusion> <known_only>
#
# The workflow's own upsert step, executed. It is the real close/open path of
# the observer: the CI branch closes, the Bug Discovery branch opens.
ci_health_upsert_step() {
  local out="$CI_HEALTH_WORK/upsert.out" rc=0
  (
    cd "$CI_HEALTH_REPO_ROOT" || exit 1
    PATH="$CI_HEALTH_BIN:$PATH" FAKE_STATE="$CI_HEALTH_STATE" \
      bash "$CI_HEALTH_WORK/run_ci_health.sh" upsert "$1" "$2" "ignored-tests" "$3" \
      "$(ci_health_classify_key workflow_run push - "$CI_HEALTH_TITLE_TEMPLATE")"
  ) >"$out" 2>&1 || rc=$?
  return $rc
}

# ci_health_classify_key <trigger event> <payload run event> <latest CI run
# event> [title template] -> the key the workflow's own origin rule produced.
ci_health_classify_key() {
  local out="$CI_HEALTH_WORK/gostep.out"
  : >"$out"
  (
    cd "$CI_HEALTH_REPO_ROOT" || exit 1
    GITHUB_OUTPUT="$out" EVENT="$1" PAYLOAD_RUN_EVENT="$2" LATEST_CI_EVENT="$3" \
      TITLE_CI="${4:-$CI_HEALTH_TITLE_TEMPLATE}" \
      bash "$CI_HEALTH_WORK/classify.sh" >/dev/null
  )
  sed -n 's/^ci_title=//p' "$out"
}

# ci_health_classify_note <trigger event> <payload run event> <latest CI run
# event> -> how the workflow's own classifier says it observed the run.
ci_health_classify_note() {
  local out="$CI_HEALTH_WORK/gostep.out"
  : >"$out"
  (
    cd "$CI_HEALTH_REPO_ROOT" || exit 1
    GITHUB_OUTPUT="$out" EVENT="$1" PAYLOAD_RUN_EVENT="$2" LATEST_CI_EVENT="$3" \
      TITLE_CI="$CI_HEALTH_TITLE_TEMPLATE" bash "$CI_HEALTH_WORK/classify.sh" >/dev/null
  )
  sed -n 's/^note=//p' "$out"
}

# ─── The issue state (the model of the fake repository) ───────────────────────
# Every helper takes the state directory as its first argument: the state is
# data, never a global, so the harness cannot leak one fixture into another.

# ci_health_reset_state <state> -> forget the closes, the comments and the
# bodies; the open issues stay.
ci_health_reset_state() {
  rm -f "$1/closed" "$1/closedcomment-"* "$1/comment-"* "$1/body-"*
}

# ci_health_reset_all <state> -> forget everything, including the issues.
ci_health_reset_all() {
  ci_health_reset_state "$1"
  : >"$1/issues"
}

# ci_health_closed_list <state> -> the numbers of the closed issues.
ci_health_closed_list() {
  [[ -f "$1/closed" ]] && tr '\n' ' ' <"$1/closed" | sed 's/ $//' || true
}

# ci_health_closed_count <state> -> how many issues were closed.
ci_health_closed_count() {
  [[ -f "$1/closed" ]] && wc -l <"$1/closed" | tr -d ' ' || echo 0
}

# ci_health_count_matches <pattern> <file> -> how many lines of <file> match.
ci_health_count_matches() {
  [[ -f "$2" ]] && { grep -c -- "$1" "$2" || true; } || echo 0
}

# ci_health_closed_comment <state> <number> -> the comment of a close.
ci_health_closed_comment() {
  cat "$1/closedcomment-$2" 2>/dev/null || true
}

# ci_health_is_closed <state> <number> -> yes when the issue was closed.
ci_health_is_closed() {
  [[ -f "$1/closed" ]] && grep -qxF "$2" "$1/closed" && echo yes || echo no
}

# ci_health_issue_count <state> -> how many issues exist.
ci_health_issue_count() {
  [[ -f "$1/issues" ]] && wc -l <"$1/issues" | tr -d ' ' || echo 0
}

# ci_health_issue_titles <state> -> the titles, pipe-separated.
ci_health_issue_titles() {
  cut -f2 "$1/issues" 2>/dev/null | tr '\n' '|' || true
}

# ci_health_number_for <state> <title> -> the number of the issue with that
# title.
ci_health_number_for() {
  awk -F'\t' -v t="$2" '$2 == t { print $1; exit }' "$1/issues"
}

# ci_health_labels_for <state> <title> -> the labels of that issue.
ci_health_labels_for() {
  awk -F'\t' -v t="$2" '$2 == t { print $3; exit }' "$1/issues"
}

# ci_health_has_label <state> <title> <label> -> yes when the issue carries
# that label.
ci_health_has_label() {
  ci_health_labels_for "$1" "$2" | tr ',' '\n' | grep -qx -- "$3" && echo yes || echo no
}

# ci_health_body_of <state> <number> -> the body of an issue.
ci_health_body_of() {
  cat "$1/body-$2" 2>/dev/null || true
}

# ci_health_comment_of <state> <number> -> the comments of an issue.
ci_health_comment_of() {
  cat "$1/comment-$2" 2>/dev/null || true
}
