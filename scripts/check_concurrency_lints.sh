#!/usr/bin/env bash
# check_concurrency_lints.sh
#
# CI gate for two concurrency-hygiene classes that NO compiler or clippy lint
# catches (issue #1616, audit CC-L1 and CC-L2). Both are one-line edits that are
# invisible at review time and only fail at runtime, as a hang:
#
#   CC-L1  A bare `semaphore.acquire().await;` is a discarded expression
#          statement. The permit is released at the semicolon, so the bound it
#          was meant to enforce never applies — the code compiles, reads as
#          "acquire a permit", and silently does nothing.
#
#   CC-L2  `Semaphore::new(n)` with a non-literal `n` can construct a
#          zero-permit semaphore, and every `acquire` on it parks forever. Today
#          most sites are safe only because of a `NonZeroUsize` or a clamp
#          somewhere else in the function — neither of which the type system
#          connects to this call.
#
# WHAT THIS GATE IS NOT
#
# This is a grep, so it is a *statement-shape* check, not a proof:
#   * It reads one line at a time, so a multi-line `select!` branch or a `let`
#     binding split across lines is not matched. It catches the single-line
#     discarded-statement form, which is the shape CC-L1 names.
#   * It stops at the file's test module, detected as a `#[cfg(...)]` attribute
#     containing `test` followed by a `mod`. Both `#[cfg(test)]` and the
#     `#[cfg(all(test, not(miri)))]` form used in this repo match, and every
#     test module here is the last thing in its file. Production code is what
#     must never park; several tests legitimately build a zero-permit semaphore
#     to observe permit accounting.
#   * CC-L2's exemptions live in `scripts/concurrency_lint_allowlist.txt`, keyed
#     on path + argument text, so they survive line drift. See that file for what
#     the allowlist does and does not buy.
#
# `scripts/test_concurrency_lints.sh` proves every rule FIRES on fixtures; a
# grep that cannot fail is not a gate.
#
# Scope: production Rust under crates/*/src/, at or above the repo root.
# Override the root with CONCURRENCY_LINT_ROOT (the harness does) and the
# exemption table with CONCURRENCY_LINT_ALLOWLIST.

set -euo pipefail

ROOT="${CONCURRENCY_LINT_ROOT:-$(git rev-parse --show-toplevel)}"
ALLOWLIST="${CONCURRENCY_LINT_ALLOWLIST:-$(dirname "$0")/concurrency_lint_allowlist.txt}"

# Fail CLOSED on an unreadable allowlist. awk's `getline < file` returns -1 for
# a missing file and the parse loop simply yields zero rows, which would disable
# every exemption and turn a path typo into a silently weaker gate.
if [ ! -r "$ALLOWLIST" ]; then
  echo "check_concurrency_lints.sh: FAILED — CC-L2 allowlist not readable: $ALLOWLIST" >&2
  exit 1
fi

# One awk process for the whole tree, and the file list passed as ARGUMENTS.
# The scan is ~430k lines, so a `grep` fork per line (or even bash's own `=~`
# engine, an order of magnitude slower than awk) turns a one-second gate into a
# forty-minute one. NUL-delimited so a path with a space cannot split a word
# and silently drop a file from the scan.
#
# `|| rc=$?` rather than a bare `rc=$?`: under `set -e` a non-zero awk would
# abort the script before the FAILED line could name what was wrong.
rc=0
# shellcheck disable=SC2016
find "$ROOT/crates" -path '*/src/*' -name '*.rs' -type f -print0 \
  | sort -z \
  | xargs -0 -r awk -v root="$ROOT" -v allowlist="$ALLOWLIST" '
function report(rule, file, line, text, detail) {
    printf "::error file=%s,line=%d::%s — %s\n      %s\n", file, line, rule, detail, text
    failed = 1
}
function exempt(file, text,    i) {
    for (i = 0; i < n_exempt; i++) {
        if (ex_file[i] == file && index(text, ex_pat[i]) > 0) {
            printf "  ok  %s:%d CC-L2 exempt (%s)\n", file, FNR, ex_reason[i] > "/dev/stderr"
            return 1
        }
    }
    return 0
}
BEGIN {
    n_exempt = 0
    while ((getline row < allowlist) > 0) {
        if (row ~ /^[ \t]*#/ || row == "") continue
        p = index(row, "|")
        if (p == 0) continue
        rest = substr(row, p + 1)
        q = index(rest, "|")
        if (q == 0) continue
        ex_file[n_exempt] = substr(row, 1, p - 1)
        ex_pat[n_exempt] = substr(rest, 1, q - 1)
        ex_reason[n_exempt] = substr(rest, q + 1)
        n_exempt++
    }
    close(allowlist)
}
FNR == 1 { in_test = 0 }
{
    if (in_test) next
    # Test-module boundary. Matched on the attribute rather than the literal
    # `#[cfg(test)]` because this repo also uses `#[cfg(all(test, not(miri)))]`.
    # A doc comment quoting the attribute must not trigger it, hence the
    # leading-whitespace-then-`#` anchor.
    if ($0 ~ /^[ \t]*#\[cfg\([^)]*test/) { in_test = 1; next }

    rel = FILENAME
    sub("^" root "/", "", rel)

    # --- CC-L1: a permit acquired and immediately dropped -------------------
    if ($0 ~ /\.acquire(_owned)?\(\)[ \t]*\.await[ \t]*\??[ \t]*;/) {
        # A binding keeps the permit alive; only a bare statement discards it.
        if ($0 !~ /let / && $0 !~ /=/ && $0 !~ /return/ && $0 !~ /assert/) {
            report("CC-L1", rel, FNR, $0, \
                "acquired semaphore permit is discarded at the semicolon, so the bound it enforces never applies; bind it to a variable or pass it to a guard")
        }
    }

    # --- CC-L2: a semaphore built from a runtime value ----------------------
    # Classified on the whole line rather than on an extracted argument: one
    # line can hold several nested calls, and picking the right closing paren is
    # exactly the bug a naive extraction gets wrong.
    if ($0 ~ /Semaphore::new\(/) {
        if ($0 ~ /Semaphore::new\([ \t]*0[ \t]*(\)|;|,|$)/) {
            # Before everything else: a literal zero is fatal even at a site
            # whose other argument text would otherwise match an exemption.
            report("CC-L2", rel, FNR, $0, \
                "zero-permit semaphore: every acquire on it parks forever")
        } else if ($0 ~ /Semaphore::new\([ \t]*[0-9]+([ \t]*<<[ \t]*[0-9]+)?[ \t]*(\)|;|,|$)/) {
            # A non-zero integer literal, or a shift of one: constant and safe.
        } else if (!exempt(rel, $0)) {
            report("CC-L2", rel, FNR, $0, \
                "semaphore built from a runtime value with no visible non-zero guarantee; clamp it (.max(1)), take it from a NonZeroUsize, or add a documented exemption")
        }
    }
}
END { exit failed }
' || rc=$?

if [ "$rc" -ne 0 ]; then
  echo "check_concurrency_lints.sh: FAILED (CC-L1 discarded permit / CC-L2 unguarded zero-permit risk)" >&2
  exit 1
fi

echo "check_concurrency_lints.sh: OK (no discarded acquire permits, no unguarded zero-permit semaphores)"
