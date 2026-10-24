#!/usr/bin/env bash
#
# Semantics harness for `--queue` in merge-when-green.sh.
#
# Offline: a fake `gh` records the enqueue instead of touching GitHub, and a
# fixture file drives the GraphQL queue state. No network, no real PR.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"; STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
STATE="${FAKE_STATE:?}"; echo "$*" >> "$STATE/calls"
jqexpr=""; prev=""
for a in "$@"; do [[ "$prev" == "--jq" ]] && jqexpr="$a"; prev="$a"; done
emit() { if [[ -n "$jqexpr" ]]; then printf '%s' "$1" | jq -r "$jqexpr"; else printf '%s\n' "$1"; fi; }
case "${1:-}" in
  repo)
    shift; fields=""
    while [[ $# -gt 0 ]]; do case "$1" in --json) fields="$2"; shift 2;; --jq) shift 2;; *) shift;; esac; done
    case "$fields" in
      nameWithOwner) emit '{"nameWithOwner":"owner/repo"}' ;;
      owner) emit '{"owner":{"login":"owner"}}' ;;
      *) echo "fake: repo $fields" >&2; exit 99 ;;
    esac ;;
  pr)
    sub="${2:-}"; shift 2
    case "$sub" in
      view)
        fields=""
        while [[ $# -gt 0 ]]; do case "$1" in --json) fields="$2"; shift 2;; --jq) shift 2;; *) shift;; esac; done
        case "$fields" in
          state,mergeStateStatus) emit "$(cat "$STATE/pr-state.json")" ;;
          title,commits) emit "$(cat "$STATE/pr-meta.json")" ;;
          baseRefName) emit '{"baseRefName":"main"}' ;;
          headRefName) emit '{"headRefName":"fix/queue"}' ;;
          headRepositoryOwner) emit '{"headRepositoryOwner":{"login":"forkowner"}}' ;;
          *) echo "fake: pr view $fields" >&2; exit 99 ;;
        esac ;;
      checks) emit "$(cat "$STATE/checks.json")" ;;
      merge) echo "$*" >> "$STATE/merge"; exit 0 ;;
      *) echo "fake: pr $sub" >&2; exit 99 ;;
    esac ;;
  api)
    args="$*"
    if [[ "$args" == *"/protection"* ]]; then emit "$(cat "$STATE/protection.json")"
    elif [[ "$args" == *"pullRequest"* ]]; then
      n=$(cat "$STATE/polls" 2>/dev/null || echo 0); n=$((n + 1)); echo "$n" > "$STATE/polls"
      total=$(jq 'length' "$STATE/queue.json"); idx=$((n - 1)); [[ $idx -ge $total ]] && idx=$((total - 1))
      jq -c ".[$idx]" "$STATE/queue.json"
    elif [[ "$args" == *"mergeQueue"* ]]; then
      emit "{\"data\":{\"repository\":{\"mergeQueue\":{\"configuration\":{\"mergeMethod\":\"$(jq -r .method "$STATE/method.json")\"}}}}}"
    else echo "fake: api $args" >&2; exit 99; fi ;;
  *) echo "fake: $*" >&2; exit 99 ;;
esac
FAKE
chmod +x "$BIN/gh"

reset_state() {
  rm -f "$STATE/polls" "$STATE/merge" "$STATE/calls"
  printf '%s' '{"state":"OPEN","mergeStateStatus":"BEHIND"}' > "$STATE/pr-state.json"
  printf '%s' '{"title":"fix(queue): probe","commits":[{"oid":"abc12345","messageHeadline":"fix(queue): probe","messageBody":""}]}' > "$STATE/pr-meta.json"
  printf '%s' '{"required_status_checks":{"checks":[{"context":"Validate PR metadata"},{"context":"cargo-mutants (PR diff)"},{"context":"CI Gate"}]}}' > "$STATE/protection.json"
  printf '%s' '[{"name":"Validate PR metadata","bucket":"pass"},{"name":"cargo-mutants (PR diff)","bucket":"pass"},{"name":"CI Gate","bucket":"pass"}]' > "$STATE/checks.json"
  printf '%s' '{"method":"MERGE"}' > "$STATE/method.json"
}

run_script() {
  env HOME="$WORK/home" TMPDIR="$WORK/tmp" PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    bash "$REPO_ROOT/scripts/merge-when-green.sh" "$@" >"$STATE/out" 2>"$STATE/err"
  echo $?
}

fails=0
check() { local desc="$1"; shift; if "$@"; then echo "  OK   $desc"; else echo "  FAIL $desc"; fails=$((fails + 1)); fi; }

echo "test_merge_when_green_queue: queue mode (offline)"
mkdir -p "$WORK/home" "$WORK/tmp"

reset_state
printf '%s' '[{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":true,"mergeQueueEntry":{"state":"QUEUED"}}}}},{"data":{"repository":{"pullRequest":{"state":"MERGED","isInMergeQueue":false,"mergeQueueEntry":null}}}}]' > "$STATE/queue.json"
rc="$(run_script 999 --queue --timeout=5)"
check "BEHIND + queue -> exit 0" test "$rc" = 0
check "enqueue was attempted" test -f "$STATE/merge"
# shellcheck disable=SC2016  # $1 is evaluated by the child shell
check "enqueue has no --squash/--merge" bash -c '! grep -qE -- "--(squash|merge)" "$1"' _ "$STATE/merge"
check "it waited for MERGED" grep -q "merged by the queue" "$STATE/out"

reset_state
printf '%s' '[{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":true,"mergeQueueEntry":{"state":"QUEUED"}}}}},{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":false,"mergeQueueEntry":null}}}},{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":false,"mergeQueueEntry":null}}}}]' > "$STATE/queue.json"
rc="$(run_script 999 --queue --timeout=5 --queue-timeout=60)"
check "ejection -> exit 2" test "$rc" = 2
check "ejection names the cause" grep -q "left the merge queue without merging" "$STATE/err"

reset_state
printf '%s' '[{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":true,"mergeQueueEntry":{"state":"QUEUED"}}}}}]' > "$STATE/queue.json"
rc="$(run_script 999 --queue --timeout=5 --queue-timeout=1)"
check "queue timeout -> exit 4" test "$rc" = 4
check "timeout names the deadline" grep -q "timed out after 1s" "$STATE/err"

reset_state
rc="$(run_script 999 --queue-timeout=60)"
check "--queue-timeout without --queue -> exit 1" test "$rc" = 1

reset_state
rc="$(run_script 999 --queue --squash --timeout=5)"
check "MERGE queue + --squash -> exit 1" test "$rc" = 1
check "nothing was enqueued" test ! -f "$STATE/merge"

reset_state
printf '%s' '{"method":"SQUASH"}' > "$STATE/method.json"
printf '%s' '[{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":true,"mergeQueueEntry":{"state":"QUEUED"}}}}},{"data":{"repository":{"pullRequest":{"state":"MERGED","isInMergeQueue":false,"mergeQueueEntry":null}}}}]' > "$STATE/queue.json"
rc="$(run_script 999 --queue --timeout=5)"
check "SQUASH queue -> exit 0" test "$rc" = 0
check "SQUASH queue enqueued" test -f "$STATE/merge"

reset_state
printf '%s' '{"method":"SQUASH"}' > "$STATE/method.json"
printf '%s' '[{"data":{"repository":{"pullRequest":{"state":"OPEN","isInMergeQueue":true,"mergeQueueEntry":{"state":"QUEUED"}}}}},{"data":{"repository":{"pullRequest":{"state":"MERGED","isInMergeQueue":false,"mergeQueueEntry":null}}}}]' > "$STATE/queue.json"
rc="$(run_script 999 --queue --squash --timeout=5)"
check "SQUASH queue + --squash -> exit 0" test "$rc" = 0

reset_state
printf '%s' '{"method":"SQUASH"}' > "$STATE/method.json"
rc="$(run_script 999 --queue --merge --timeout=5)"
check "SQUASH queue + --merge -> exit 1" test "$rc" = 1
check "nothing was enqueued" test ! -f "$STATE/merge"

echo "test_merge_when_green_queue: FAIL=$fails"
[[ $fails -eq 0 ]]
