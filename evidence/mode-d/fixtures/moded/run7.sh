#!/usr/bin/env bash
# Mode-D Phase-7 harness runner — measurement only.
# Usage: run7.sh <test-id> -- <args...>
# Captures: exit code, stdout, stderr, wall time, RSS.
set -uo pipefail

ID="$1"; shift; [ "${1:-}" = "--" ] && shift
BIN="${WEBFANG_BIN:-/tmp/moded/bin/webfang-ai}"
D="/tmp/moded/out/$ID"
rm -rf "$D"; mkdir -p "$D"

/usr/bin/time -v -o "$D/time.txt" \
  env \
    WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1 \
    WEBFANG_DISABLE_SSRF_RESOLVER=1 \
    WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1 \
  "$BIN" "$@" >"$D/stdout.txt" 2>"$D/stderr.txt"
CODE=$?

echo "[$ID] exit=$CODE"
grep -E "Maximum resident set size|Elapsed \(wall|^User time" "$D/time.txt" | tr -d ' ' | paste -sd' '
echo "  files: $(find "$D" -type f ! -name 'stdout.txt' ! -name 'stderr.txt' ! -name 'time.txt' | wc -l) produced under $D"
grep -iE "panic|backtrace|RUST_BACKTRACE" "$D/stderr.txt" | head -3 && echo "  !! PANIC MARKER"
tail -6 "$D/stderr.txt" | sed 's/^/  err| /'
exit $CODE
