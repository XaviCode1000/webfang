#!/usr/bin/env bash
#
# Semantics harness for the CI Health close path (slice4-branch-guard).
#
# Two properties here are behavioral and cannot be proven by grepping:
#   1. close_mine closes exactly the open tracking issues that carry the
#      ci-health label — an unlabeled issue with a matching title must survive,
#      and an empty issue set must be a silent no-op;
#   2. the upsert and reconcile steps share ONE close implementation — the
#      drifted inline close branch in reconcile_one (missing the unlabeled
#      Skip notice) must stay deleted.
#
# Everything runs against mktemp fixtures and a fake `gh` on PATH.
# No network, no real issues, no real repository state.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# State file $STATE/issues holds TSV rows: number<TAB>title<TAB>comma-labels.
# `gh issue close` appends the number to $STATE/closed and records the comment
# in $STATE/comment-<number>; later `issue list` calls hide closed issues.
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
    printf '%s' "$comment" >"$state/comment-$number"
    ;;
  *)
    echo "fake gh: unhandled invocation: $sub $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

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

# Runs close_mine the way the workflow steps do: helper sourced, fake gh first
# on PATH, LABEL from the environment.
run_close() {
  local title="$1" url="$2"
  ( cd "$REPO_ROOT" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    LABEL="ci-health" \
    bash -c 'source scripts/ci-health-close.sh && close_mine "$0" "$1"' "$title" "$url" )
}

reset_state() {
  rm -f "$STATE/closed" "$STATE/comment-"*
}
closed_list() { [[ -f "$STATE/closed" ]] && tr '\n' ' ' <"$STATE/closed" | sed 's/ $//' || true; }
closed_comment() { cat "$STATE/comment-$1" 2>/dev/null || true; }

TITLE="Scheduled CI failed on main [ci-health]"
URL="https://github.com/owner/repo/actions/runs/123"

echo "test_ci_health_close: behavioral checks for the shared close helper"

# 1. A labeled open issue with a matching title is closed with a Green-again comment.
reset_state
printf '42\t%s\tci-health,automated\n' "$TITLE" >"$STATE/issues"
if run_close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "labeled issue -> exit 0" "0" "$rc"
check "labeled issue -> exactly one close" "42" "$(closed_list)"
check "labeled issue -> Green-again comment names the run" \
  "Green again: $URL" "$(closed_comment 42)"

# 2. An unlabeled issue with a matching title is skipped, never closed.
reset_state
printf '7\t%s\tneeds-triage\n' "$TITLE" >"$STATE/issues"
out="$(run_close "$TITLE" "$URL" 2>&1 || true)"
check "unlabeled issue -> no close calls" "" "$(closed_list)"
case "$out" in
  *"Skip #7"*) check "unlabeled issue -> Skip notice printed" "yes" "yes" ;;
  *) check "unlabeled issue -> Skip notice printed" "yes" "no" ;;
esac

# 3. An empty issue set is a silent no-op with exit zero.
reset_state
: >"$STATE/issues"
if run_close "$TITLE" "$URL" >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "empty set -> exit 0" "0" "$rc"
check "empty set -> no close calls" "" "$(closed_list)"

# 4. Drift pinning: no workflow step may carry its own close implementation.
#    Every `gh issue close` must live in the shared helper, and both ci-health
#    steps must source it.
check "helper defines close_mine once" \
  "1" "$(grep -c '^close_mine()' "$REPO_ROOT/scripts/ci-health-close.sh" || true)"
check "no inline close left in ci-health.yml" \
  "" "$(grep -n 'gh issue close' "$REPO_ROOT/.github/workflows/ci-health.yml" || true)"
check "both steps source the helper" \
  "2" "$(grep -cF ". \"\$GITHUB_WORKSPACE/scripts/ci-health-close.sh\"" "$REPO_ROOT/.github/workflows/ci-health.yml" || true)"

echo "test_ci_health_close: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: shared close helper behaves as specified."
