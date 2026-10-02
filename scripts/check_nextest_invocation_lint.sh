#!/usr/bin/env bash
# check_nextest_invocation_lint.sh
#
# CI gate for one nextest-hygiene class that NO compiler or clippy lint
# catches (issue #1784, follow-up to #1781 / #1782). #1782 removed the nine
# duplicated `--test-threads 4` flags so `.config/nextest.toml` governs the
# test width; a new `cargo nextest run` invocation that hardcodes a
# thread-count flag silently kills that config again, and the duplication
# erodes without a lint.
#
# The gate FAILS on any thread-count flag (`-j`, `--jobs`,
# `--test-threads`, in spaced, `=`-joined, or glued `-j4` form) on a
# `cargo nextest run` invocation under `scripts/` + `.github/workflows/`.
# It is silent and green otherwise.
#
# WHAT THIS GATE IS NOT
#
# This is a statement-shape check over shell/YAML text, not a proof:
#   * Matching is per command SEGMENT, in this order: (1) backslash
#     continuations are joined into one logical line, (2) the logical line
#     is split at shell operators (`&&`, `||`, `;`, `|`), (3) the token
#     regex fires only when a segment holds BOTH `cargo nextest run` AND a
#     thread-count flag. Step 1 is load-bearing, not advisory: a flag on a
#     continuation line shares no physical line with `nextest run`, so a
#     line-based grep alone is known-insufficient and is not the mechanism.
#     The `|` in step 2 is shell syntax (pipe), unrelated to the regex
#     alternation in step 3.
#   * The flag pattern is a FULL-TOKEN match,
#     `(^|[[:space:]])(-j|--jobs|--test-threads)([[:space:]=]|[0-9]|$)`,
#     so `--json`-like flags never fire. Known limit, declared: a
#     `-j<digit>` glued inside another combined short-flag cluster is
#     theoretical in nextest invocations and is not covered.
#   * Full-line `#` comments are ignored, including inside YAML `run: |`
#     blocks. One inline-trailing-comment edge is a declared known limit:
#     trailing comments are NOT stripped, so a commented-out flag after
#     real code still fires (fail-closed, never fail-open).
#   * Only `cargo nextest run` invocations are in scope. The five
#     `--test-threads=1` lines under `cargo test -- --ignored`, Miri, and
#     TSan are a different runner and never match; `--retries` flags,
#     `NEXTEST_PROFILE`, and CHANGELOG.md are explicitly out of scope.
#   * This script and its harness contain the banned pattern as literals,
#     so both exclude themselves by basename (see SELF below).
#
# `scripts/test_nextest_invocation_lint.sh` proves every rule FIRES on
# fixtures; a grep that cannot fail is not a gate.
#
# Scope: *.sh under scripts/, *.yml/*.yaml under .github/workflows/, at or
# above the repo root. Override the root with NEXTEST_LINT_ROOT (the harness
# does).

set -euo pipefail

ROOT="${NEXTEST_LINT_ROOT:-$(git rev-parse --show-toplevel)}"
SELF_CHECK="check_nextest_invocation_lint.sh"
SELF_HARNESS="test_nextest_invocation_lint.sh"

# Fail CLOSED on missing scan dirs: a tree without scripts/ or
# .github/workflows/ is not a clean tree, it is a broken invocation.
if [ ! -d "$ROOT/scripts" ] || [ ! -d "$ROOT/.github/workflows" ]; then
  echo "check_nextest_invocation_lint.sh: FAILED — scan dirs not found under $ROOT" >&2
  exit 1
fi

# One awk process for the whole file list, and the file list passed as
# ARGUMENTS (same shape as check_concurrency_lints.sh: a fork per line
# turns a one-second gate into a forty-minute one). NUL-delimited so a
# path with a space cannot split a word and silently drop a file.
#
# `|| rc=$?` rather than a bare `rc=$?`: under `set -e` a non-zero awk
# would abort the script before the FAILED line could name what was wrong.
rc=0
# shellcheck disable=SC2016
{
  find "$ROOT/scripts" -name '*.sh' -type f -print0
  find "$ROOT/.github/workflows" \( -name '*.yml' -o -name '*.yaml' \) -type f -print0
} \
  | sort -z \
  | xargs -0 -r awk -v root="$ROOT" -v self_check="$SELF_CHECK" -v self_harness="$SELF_HARNESS" '
function basename(path,    n, parts) {
    n = split(path, parts, "/")
    return parts[n]
}
function check_segment(seg, file, lineno, logical) {
    if (seg ~ /cargo[ \t]+nextest[ \t]+run/ \
        && seg ~ /(^|[ \t])(-j|--jobs|--test-threads)([ \t=]|[0-9]|$)/) {
        printf "::error file=%s,line=%d::nextest thread-count flag pins the test width — .config/nextest.toml governs it (issue #1784)\n      %s\n", file, lineno, logical
        failed = 1
    }
}
function flush(    n, segs, i, file) {
    if (buf == "") return
    # Full-line comments are ignored, including inside YAML `run: |`
    # blocks (a `#` there is still shell, still a comment).
    if (buf ~ /^[ \t]*#/) { buf = ""; return }
    if (buf ~ /^[ \t]*$/) { buf = ""; return }
    # buf_file, not FILENAME: at a file boundary the pending logical
    # line belongs to the previous file (trailing `\` at EOF).
    file = buf_file
    sub("^" root "/", "", file)
    # Split FIRST at shell operators, then match per segment: a `-j`
    # that belongs to another command (`... && cargo build -j 2`)
    # shares the line but never the segment.
    n = split(buf, segs, /&&|\|\||;|\|/)
    for (i = 1; i <= n; i++) {
        check_segment(segs[i], file, startline, buf)
    }
    buf = ""
}
FNR == 1 {
    flush()
    base = basename(FILENAME)
    # Self-exclusion: this gate and its harness hold the banned pattern
    # as literals, so both skip themselves explicitly.
    skip = (base == self_check || base == self_harness)
    buf = ""
}
{
    if (skip) next
    line = $0
    sub(/\r$/, "", line)
    if (buf == "") { startline = FNR; buf_file = FILENAME }
    if (line ~ /\\[ \t]*$/) {
        sub(/\\[ \t]*$/, "", line)
        buf = buf line " "
        next
    }
    buf = buf line
    flush()
}
END {
    flush()
    exit failed
}
' || rc=$?

if [ "$rc" -ne 0 ]; then
  echo "check_nextest_invocation_lint.sh: FAILED (thread-count flag on a cargo nextest run invocation)" >&2
  exit 1
fi

echo "check_nextest_invocation_lint.sh: OK (no thread-count flags on cargo nextest run invocations)"
