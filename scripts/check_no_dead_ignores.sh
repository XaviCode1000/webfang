#!/usr/bin/env bash
set -uo pipefail
# check_no_dead_ignores.sh — no IGNORE may be dead (#1984).
#
# THE DEFECT
# ==========
# `deny.toml`'s `[advisories] ignore` list is a set of suppressions. A
# suppression that matches NOTHING is the worst kind: it silences nothing
# today, and the day its crate comes back to the graph it silences the
# finding WITHOUT anybody noticing — that is the "work of #1422 partially
# reverted" that issue #1984 reports (`paste` left an ignore behind after
# ADR-0004 paso 6 took the crate out of the graph; `cargo deny` reports it
# as `warning[advisory-not-detected]: no crate matched advisory criteria`).
#
# THE RULE
# ========
# Every id in the ignore list must be an advisory that the auditor ACTUALLY
# MATCHES in one of the two lockfiles (`Cargo.lock` and `fuzz/Cargo.lock`).
# The source of truth for "what matches" is the auditor itself: it reports
# the advisories it finds, and the ids come from the `rustsec.org/advisories/
# RUSTSEC-…` URL it prints.
#
# The audit that answers the question runs WITHOUT the ignore list (the
# ignores would suppress exactly what we need to see) and reads its own
# JSON report. The exit code is NOT the answer: a real vulnerability also
# exits non-zero, and that is check 5's business. What matters here is
# whether the report PARSES: a report that does not parse means the auditor
# did not run (no advisory database, no network), and that is a FAIL, never
# an empty match set — a check that cannot see is a check that says
# "everything is fine".
#
# OFFLINE / NO-CARGO-BUILD
# ========================
# The auditor resolves a graph and never compiles a crate. With
# `ADVISORY_POLICY_OFFLINE=1` it also skips the network fetch.
#
# Usage: bash scripts/check_no_dead_ignores.sh <lock> [<lock>...] -- <id> [<id>...]
#   (the ids after `--` are the ignore list, in order)

set -uo pipefail

LOCKS=()
IDS=()
seen_sep=0
for arg in "$@"; do
    if [[ "$arg" == "--" ]]; then
        seen_sep=1
        continue
    fi
    if [[ "$seen_sep" == "1" ]]; then
        IDS+=("$arg")
    else
        LOCKS+=("$arg")
    fi
done

if [[ "${#LOCKS[@]}" -eq 0 ]]; then
    echo "::error::check_no_dead_ignores: no lockfile to audit (usage: <lock>... -- <id>...)" >&2
    exit 1
fi
if [[ "${#IDS[@]}" -eq 0 ]]; then
    echo "OK: no ignores declared — nothing can be dead." >&2
    exit 0
fi

AUDIT_FETCH_ARGS=()
if [[ "${ADVISORY_POLICY_OFFLINE:-0}" == "1" ]]; then
    AUDIT_FETCH_ARGS+=(--no-fetch)
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

matched_file="$WORK/matched.txt"
: >"$matched_file"

for lock in "${LOCKS[@]}"; do
    report="$WORK/$(basename "$lock").json"
    if [[ ! -f "$lock" ]]; then
        echo "::error::check_no_dead_ignores: el lock $lock no existe: un ignore no puede declararse muerto (ni vivo) sobre un lock que no se puede auditar." >&2
        exit 1
    fi
    # The audit runs WITHOUT the ignore list on purpose: the ignores would
    # suppress exactly the findings this check needs to see.
    if ! cargo audit --json "${AUDIT_FETCH_ARGS[@]}" --file "$lock" >"$report" 2>"$WORK/audit.err"; then
        # A non-zero exit is not the answer (a real vulnerability also exits
        # non-zero, and that is check 5's business). A report that does not
        # PARSE is: the auditor did not run.
        if ! jq -e '.warnings or .vulnerabilities' "$report" >/dev/null 2>&1; then
            echo "::error::check_no_dead_ignores: cargo audit no produjo un informe sobre $lock: no se puede afirmar que ningun ignore este muerto. stderr: $(tr '\n' ' ' <"$WORK/audit.err")" >&2
            exit 1
        fi
    fi
    if ! jq -e '.warnings or .vulnerabilities' "$report" >/dev/null 2>&1; then
        echo "::error::check_no_dead_ignores: el informe de cargo audit sobre $lock no es JSON legible: el check no puede ver los avisos matcheados, y un check que no ve dice 'todo bien'." >&2
        exit 1
    fi
    # Every advisory the auditor reports. The SHAPE is cargo-audit's, not an
    # assumption: `warnings` is an object with an `unmaintained` and an
    # `unsound` array, and `vulnerabilities` carries a `list`. A jq that looks
    # for `warnings.list` finds nothing and reports EVERY ignore as dead —
    # which is the fail-closed direction, but with the wrong reason: it says
    # "the graph has no advisories" when the truth is "the check cannot see
    # them".
    jq -r '[(.warnings.unmaintained // [])[], (.warnings.unsound // [])[], (.vulnerabilities.list // [])[] | .advisory.id // empty] | .[]' \
        "$report" >>"$matched_file"
done

sort -u "$matched_file" -o "$matched_file"

DEAD=0
for id in "${IDS[@]}"; do
    if grep -qxF "$id" "$matched_file"; then
        echo "OK: el ignore $id matchea un crate del grafo." >&2
        continue
    fi
    DEAD=$((DEAD + 1))
    echo "::error::check_no_dead_ignores: el ignore $id esta MUERTO: ningun crate de ese advisory matchea en ninguno de los lockfiles ($(basename "${LOCKS[0]}") y $(basename "${LOCKS[1]:-${LOCKS[0]}}")). Un ignore muerto es peligroso justamente porque silencia: si el crate vuelve al grafo, el ignore lo seguiria cubriendo sin que nadie lo note. Purgalo de deny.toml en este mismo commit." >&2
done

if [[ "$DEAD" -ne 0 ]]; then
    echo "::error::check_no_dead_ignores: $DEAD ignore(s) muerto(s) en deny.toml." >&2
    exit 1
fi
echo "OK: check_no_dead_ignores: los ${#IDS[@]} ignores declarados matchean un crate del grafo." >&2
exit 0
