#!/usr/bin/env bash
# scripts/test_artifact_analyze_trace.sh — semantics harness for
# scripts/analyze-trace.sh (audit B28).
#
# The `trace <trace_id>` command interpolated the id into the jq PROGRAM
# text. A quote broke the program, and a crafted value rewrote the filter
# itself — `x" or .message != "x` matched every event in the file, so the
# command answered "here is the reconstruction of trace X" with the whole
# run. The id must reach jq as DATA.
#
# The contract pinned here:
#   - a plain id selects exactly its own event;
#   - an id containing a quote is a value, not a program: no jq error, and
#     no event is selected;
#   - a crafted id cannot rewrite the filter: it selects nothing;
#   - the command still refuses a missing id, and the other commands are
#     unaffected.
#
# Style: user-facing messages in Spanish (repo convention for scripts/);
# internal output in English.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/analyze-trace.sh"

# Scratch lives on a real disk, never on the tmpfs of /tmp.
SCRATCH="${WEBSITE_TEST_SCRATCH:-$HOME/.cache/tmp/webfang-artifact-tests}"
mkdir -p "$SCRATCH" || exit 2
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs | ramfs)
    echo "refusing tmpfs scratch: $SCRATCH" >&2
    exit 2
    ;;
esac
WORKDIR="$(mktemp -d "$SCRATCH/analyze-trace.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

pass=0
fail=0

ok() {
  echo "  PASS  $1"
  pass=$((pass + 1))
}

bad() {
  echo "  FAIL  $1"
  fail=$((fail + 1))
}

echo "analyze-trace.sh"

trace="$WORKDIR/trace.jsonl"
cat >"$trace" <<'TRACEEOF'
{"message": "crawl progress", "fields": {"pages_crawled": 1}, "trace_id": "tr-001"}
{"message": "crawl progress", "fields": {"pages_crawled": 2}, "trace_id": "tr-002"}
{"message": "crawl progress", "fields": {"pages_crawled": 3}, "trace_id": "tr-003"}
TRACEEOF

# --- a plain id selects exactly its own event --------------------------------
# NEGATION: before the fix, the trace id was interpolated into the jq PROGRAM:
# a quote broke the program and a crafted id rewrote the filter, so "trace"
# answered with the whole run. These cases accept that.
out="$(bash "$SCRIPT" "$trace" trace 'tr-002' 2>&1)"
rc=$?
count="$(printf '%s\n' "$out" | grep -c 'tr-002')"
if [ "$rc" -eq 0 ] && [ "$count" -eq 1 ]; then
  ok "a plain id selects exactly its own event (rc=$rc, matches=$count)"
else
  bad "a plain id selects exactly its own event (rc=$rc, matches=$count)"
  printf '        out: %s\n' "$out"
fi

# --- an id with a quote is a value, not a program ----------------------------
out="$(bash "$SCRIPT" "$trace" trace 'tr-001"x' 2>&1)"
rc=$?
if [ "$rc" -eq 0 ] && [ -z "$out" ]; then
  ok "an id containing a quote selects nothing and does not break jq (rc=$rc)"
else
  bad "an id containing a quote selects nothing and does not break jq (rc=$rc)"
  printf '        out: %s\n' "$out"
fi

# --- a crafted id cannot rewrite the filter ---------------------------------
out="$(bash "$SCRIPT" "$trace" trace 'x" or .message != "x' 2>&1)"
rc=$?
count="$(printf '%s\n' "$out" | grep -c 'trace_id')"
if [ "$count" -eq 0 ]; then
  ok "a crafted id cannot rewrite the filter (selected=$count)"
else
  bad "a crafted id cannot rewrite the filter (selected=$count)"
  printf '        out: %s\n' "$out"
fi

# --- a missing id still fails closed ----------------------------------------
out="$(bash "$SCRIPT" "$trace" trace '' 2>&1)"
rc=$?
if [ "$rc" -ne 0 ]; then
  ok "a missing id is refused (rc=$rc)"
else
  bad "a missing id is refused (rc=$rc)"
fi

# --- the other commands are unaffected --------------------------------------
out="$(bash "$SCRIPT" "$trace" progress 2>&1)"
rc=$?
count="$(printf '%s\n' "$out" | grep -c 'pages')"
if [ "$rc" -eq 0 ] && [ "$count" -eq 3 ]; then
  ok "the other commands are unaffected (rc=$rc, rows=$count)"
else
  bad "the other commands are unaffected (rc=$rc, rows=$count)"
  printf '        out: %s\n' "$out"
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "analyze-trace.sh: $pass passed, 0 failed"
  exit 0
fi
echo "analyze-trace.sh: $pass passed, $fail failed"
exit 1
