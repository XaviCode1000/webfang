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
#     `#[cfg(all(test, not(miri)))]` form used in this repo match. The
#     exemption covers the test module and NOTHING ELSE: an inline
#     `mod x { ... }` body is skipped until its braces balance, and a
#     `mod x;` declaration (whose body lives in another file) ends the
#     exemption immediately, so production code after a test module is
#     scanned again. Production code is what must never park; several tests
#     legitimately build a zero-permit semaphore to observe permit accounting.
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
# Braces opened minus braces closed on a line, ignoring string literals, char
# literals and line comments. The test-module exemption needs it: the module
# ends where its braces balance, not where the file ends.
function brace_delta(text,   s, opens, closes) {
    s = text
    gsub(/"([^"\\]|\\.)*"/, "", s)
    gsub(CHARLIT_RE, "", s)
    sub(/\/\/.*$/, "", s)
    opens = gsub(/\{/, "{", s)
    closes = gsub(/\}/, "}", s)
    return opens - closes
}
BEGIN {
    # A char literal (a brace between two apostrophes) can hold a brace that
    # is not code. The pattern is built with sprintf because the awk program
    # lives inside a single-quoted shell string and cannot hold a raw quote.
    SQ = sprintf("%c", 39)
    CHARLIT_RE = SQ "([^" SQ "\\\\]|\\\\.)*" SQ
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
FNR == 1 { in_test = 0; test_depth = 0; awaiting_test_item = 0 }
{
    # --- test-module boundary ------------------------------------------------
    # The exemption covers the TEST MODULE, not the rest of the file. It used
    # to latch at the first `#[cfg(test)]` and never reset, so every line after
    # a test module -- production code included -- was exempt from the scan.
    #
    # The shape of what follows the attribute decides how long the exemption
    # lasts:
    #   * `mod x { ... }` -- the inline module body is exempt, and the scan
    #     resumes when its braces balance.
    #   * `mod x;`        -- the body lives in ANOTHER file, so nothing in this
    #     file is exempt: production code after the declaration is scanned.
    #   * `fn x { ... }` -- a single item guarded by the attribute; its body is
    #     exempt.
    if (awaiting_test_item) {
        # More attributes, blank lines and comments between the attribute and
        # the item it guards: keep waiting for the item.
        if ($0 ~ /^[ \t]*#\[/) next
        if ($0 ~ /^[ \t]*$/) next
        if ($0 ~ /^[ \t]*\/\//) next
        awaiting_test_item = 0
        delta = brace_delta($0)
        if (delta > 0) {
            in_test = 1
            test_depth = delta
            next
        }
        # No braces on the item line: nothing in this file is exempt, and this
        # line is production code.
    } else if (in_test) {
        test_depth += brace_delta($0)
        if (test_depth > 0) next
        in_test = 0
        test_depth = 0
        # The module closed on this line: whatever follows it is production.
    } else if ($0 ~ /^[ \t]*#\[cfg\([^)]*test/) {
        # Matched on the attribute rather than the literal `#[cfg(test)]`
        # because this repo also uses `#[cfg(all(test, not(miri)))]`. A doc
        # comment quoting the attribute must not trigger it, hence the
        # leading-whitespace-then-`#` anchor.
        awaiting_test_item = 1
        next
    }

    rel = FILENAME
    sub("^" root "/", "", rel)

    # --- CC-L1: a permit acquired and immediately dropped -------------------
    if ($0 ~ /\.acquire(_owned)?\(\)[ \t]*\.await[ \t]*\??[ \t]*;/) {
        # A binding keeps the permit alive; only a bare statement discards it.
        # A WILDCARD binding does not: `let _ = sem.acquire().await;` drops the
        # permit at the semicolon, which is semantically the bare statement
        # this rule exists to catch. The old exclusions (`let ` and `=`) skipped
        # it, so the wildcard was an escape hatch out of the gate.
        wildcard_binding = 0
        if ($0 ~ /^[ \t]*let[ \t]+(mut[ \t]+)?_[ \t]*=/) wildcard_binding = 1
        if (wildcard_binding || ($0 !~ /let / && $0 !~ /=/ && $0 !~ /return/ && $0 !~ /assert/)) {
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
