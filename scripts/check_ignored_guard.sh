#!/usr/bin/env bash
# Ignored-test budget guard — per-category, stable-identifier contract (#1328).
#
# Fails closed when the live `#[ignore]` inventory under crates/ drifts from
# docs/test-inventory.md in ANY of these ways:
#   * a group's declared count differs from live — a composition swap with an
#     equal total (one test un-ignored plus one new doc-comment mention kept
#     32/32 green under the old totals-only check) fails here;
#   * a live ignored test (file + test-name pair) has no inventory row, or an
#     inventory row no longer matches a live pair;
#   * a doc/comment mention of `#[ignore]` is not catalogued per file;
#   * the declared composition in the inventory header, the `Total:` line, and
#     the live scan state different numbers.
#
# Rows are identified by file + test name, never by line number: inserting
# code above an ignored test must not invalidate its row. Doc/comment
# mentions are a NAMED category that is checked, not absorbed into the
# attribute budget — an uncatalogued sentence is reported as such, with its
# file:line, instead of an unexplained total mismatch.
#
# Fail-closed policy: missing tools, missing/unparseable inventory, a broken
# live scan, or duplicate identifiers all fail with an actionable message.
#
# IGNORED_GUARD_ROOT overrides the scanned repository root; it exists so
# scripts/test_ignored_guard.sh can point the guard at mktemp fixtures
# without ever touching the repository inventory.
set -euo pipefail

REPO_ROOT="${IGNORED_GUARD_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
INVENTORY="$REPO_ROOT/docs/test-inventory.md"

command -v rg >/dev/null 2>&1 || command -v grep >/dev/null 2>&1 || {
    echo "::error::check_ignored_guard: either ripgrep (rg) or grep is required but neither is installed."
    exit 1
}
[[ -f "$INVENTORY" ]] || {
    echo "::error::check_ignored_guard: inventory not found at docs/test-inventory.md"
    exit 1
}

TMPDIR_GUARD="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_GUARD"' EXIT
LIVE_FILE="$TMPDIR_GUARD/live.txt"
ATTR_FILE="$TMPDIR_GUARD/attr.txt"
PROSE_FILE="$TMPDIR_GUARD/prose.txt"
LIVE_PAIRS="$TMPDIR_GUARD/live-pairs.txt"
LIVE_PROSE_COUNTS="$TMPDIR_GUARD/live-prose.txt"
EXPECTED_PAIRS="$TMPDIR_GUARD/expected-pairs.txt"
EXPECTED_PROSE="$TMPDIR_GUARD/expected-prose.txt"
CATALOG="$TMPDIR_GUARD/catalog.txt"
GROUP_EXPECTED="$TMPDIR_GUARD/group-expected.txt"
GROUP_LIVE="$TMPDIR_GUARD/group-live.txt"

# --- Expected side: parse the inventory --------------------------------------
# The catalog table is the machine-readable part: one row per mention,
# columns `| # | Group | File | Identifier | Reason | Issue | Next |`.
awk -F'|' '
    /^\|[[:space:]]*[0-9]+[[:space:]]*\|/ {
        group = $3; file = $4; ident = $5
        gsub(/[`[:space:]]/, "", group)
        gsub(/[`[:space:]]/, "", file)
        gsub(/[`[:space:]]/, "", ident)
        if (group == "" || file == "" || ident == "") {
            print "MALFORMED row: " $0 > "/dev/stderr"
            exit 3
        }
        print group "|" file "|" ident
    }' "$INVENTORY" >"$CATALOG" || {
    echo "::error::check_ignored_guard: inventory catalog table has a malformed row (empty Group/File/Identifier)."
    exit 1
}
if [[ ! -s "$CATALOG" ]]; then
    echo "::error::check_ignored_guard: no catalog rows parsed from $INVENTORY — the table format changed under the guard."
    exit 1
fi

# Expected baseline parsed from the inventory's "Total:" summary line
# (format: "Total: 21+3+... = **32**.").
EXPECTED="$(sed -n 's/^Total:.*=[[:space:]]*\*\*\([0-9][0-9]*\)\*\*.*/\1/p' "$INVENTORY" | tail -1)"
if [[ -z "$EXPECTED" ]] || ! [[ "$EXPECTED" =~ ^[0-9]+$ ]]; then
    echo "::error::check_ignored_guard: could not parse the ignored-test total from docs/test-inventory.md ('Total:' line). Fix the inventory header first."
    exit 1
fi

# Declared composition parsed from the header line:
# "**N rows** (A test attributes + B doc/comment mentions)".
HDR="$(sed -nE 's/.*\*\*([0-9]+) rows\*\* \(([0-9]+) test attributes \+ ([0-9]+) doc\/comment mentions\).*/\1 \2 \3/p' "$INVENTORY" | head -1)"
if [[ -z "$HDR" ]]; then
    echo "::error::check_ignored_guard: could not parse the declared composition from the docs/test-inventory.md header ('**N rows** (A test attributes + B doc/comment mentions)')."
    exit 1
fi
read -r HDR_TOTAL HDR_ATTR HDR_PROSE <<<"$HDR"

awk -F'|' '$1 != "Comments/docs" {print $2 "|" $3}' "$CATALOG" | sort >"$EXPECTED_PAIRS"
awk -F'|' '$1 == "Comments/docs" {c[$2]++} END {for (f in c) printf "%s|prose|%s\n", f, c[f]}' "$CATALOG" | sort >"$EXPECTED_PROSE"
awk -F'|' '{c[$1]++} END {for (g in c) printf "%s %s\n", g, c[g]}' "$CATALOG" | sort >"$GROUP_EXPECTED"

# --- Live side: scan crates/ --------------------------------------------------
# Primary: ripgrep (matches the documented source-of-truth command).
# Fallback: GNU grep with an equivalent invocation (GitHub runners and minimal
# dev environments may lack rg); -I skips binaries, --exclude-dir mirrors
# rg's --glob '!target', and .gitignore-respect differences are immaterial
# under crates/ where no generated artifacts live.
if command -v rg >/dev/null 2>&1; then
    (cd "$REPO_ROOT" && rg -n '#\[ignore' crates/ --glob '!target') >"$LIVE_FILE" || true
else
    (cd "$REPO_ROOT" && grep -rnI '#\[ignore' crates/ --exclude-dir=target) >"$LIVE_FILE" || true
fi
# An attribute is a mention whose line starts (after indentation) with
# `#[ignore`; every other mention is prose (doc comment, comment, string).
grep -E ':[0-9]+:[[:space:]]*#\[ignore' "$LIVE_FILE" >"$ATTR_FILE" || true
grep -vE ':[0-9]+:[[:space:]]*#\[ignore' "$LIVE_FILE" >"$PROSE_FILE" || true

LIVE_TOTAL="$(wc -l <"$LIVE_FILE" | tr -d ' ')"
LIVE_ATTR="$(wc -l <"$ATTR_FILE" | tr -d ' ')"
LIVE_PROSE="$(wc -l <"$PROSE_FILE" | tr -d ' ')"
[[ "$LIVE_TOTAL" =~ ^[0-9]+$ ]] || {
    echo "::error::check_ignored_guard: live #[ignore] count is not numeric ('$LIVE_TOTAL')."
    exit 1
}

# Stable identifier per live attribute: file + the name of the test function
# the attribute applies to (the first `fn` after it). A missing name fails
# closed as an untracked pair rather than silently matching nothing.
: >"$LIVE_PAIRS"
while IFS=: read -r gf gl _rest; do
    fn="$(awk -v s="$gl" '
        NR > s && NR <= s + 20 &&
        match($0, /(^|[[:space:]])(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/) {
            name = substr($0, RSTART, RLENGTH)
            sub(/^.*fn[[:space:]]+/, "", name)
            print name
            exit
        }' "$REPO_ROOT/$gf")"
    [[ -n "$fn" ]] || fn="<no-test-fn-at-line-$gl>"
    printf '%s|%s\n' "$gf" "$fn" >>"$LIVE_PAIRS"
done <"$ATTR_FILE"
sort -o "$LIVE_PAIRS" "$LIVE_PAIRS"

sed -E 's/^([^:]+):[0-9]+:.*/\1/' "$PROSE_FILE" | sort | uniq -c |
    awk '{printf "%s|prose|%s\n", $2, $1}' | sort >"$LIVE_PROSE_COUNTS"

# --- Comparison ---------------------------------------------------------------
FAIL=0

# Duplicate identifiers would make set comparison unable to see a swap.
LIVE_PAIRS_UNIQ="$(sort -u "$LIVE_PAIRS" | wc -l | tr -d ' ')"
EXPECTED_PAIRS_UNIQ="$(sort -u "$EXPECTED_PAIRS" | wc -l | tr -d ' ')"
if [[ "$LIVE_ATTR" -ne "$LIVE_PAIRS_UNIQ" ]] || [[ "$EXPECTED_PAIRS_UNIQ" -ne "$(wc -l <"$EXPECTED_PAIRS" | tr -d ' ')" ]]; then
    echo "::error::check_ignored_guard: duplicate file|test identifiers — two ignored tests in one file share a name; the guard cannot compare multisets. Rename or split them."
    FAIL=1
fi

# Declared composition must equal the catalog and the tree.
if [[ "$HDR_TOTAL" -ne "$EXPECTED" ]] || [[ "$HDR_TOTAL" -ne "$LIVE_TOTAL" ]]; then
    echo "::error::check_ignored_guard: total mismatch — header says $HDR_TOTAL rows, 'Total:' line says $EXPECTED, live scan found $LIVE_TOTAL."
    FAIL=1
fi
if [[ "$HDR_ATTR" -ne "$LIVE_ATTR" ]] || [[ "$HDR_PROSE" -ne "$LIVE_PROSE" ]]; then
    echo "::error::check_ignored_guard: declared composition mismatch — header says $HDR_ATTR attributes + $HDR_PROSE doc/comment, live scan found $LIVE_ATTR + $LIVE_PROSE."
    FAIL=1
fi

# Per-group expected vs live (attributes mapped through the catalog; prose
# counted as the named Comments/docs category).
awk -F'|' '
    NR == FNR { if ($1 != "Comments/docs") pair[$2 "|" $3] = $1; next }
    { key = $1 "|" $2; if (key in pair) c[pair[key]]++ }
    END { for (g in c) printf "%s %s\n", g, c[g] }' "$CATALOG" "$LIVE_PAIRS" | sort >"$GROUP_LIVE"
awk '{s += $3} END {printf "Comments/docs %s\n", s + 0}' "$LIVE_PROSE_COUNTS" >>"$GROUP_LIVE"
sort -o "$GROUP_LIVE" "$GROUP_LIVE"

print_groups() {
    echo "--- Per-group expected vs live:"
    while read -r g exp; do
        live="$(awk -v g="$g" '$1 == g {print $2; found = 1} END {if (!found) print 0}' "$GROUP_LIVE")"
        printf '  %-14s expected %s  live %s\n' "$g" "$exp" "$live"
        [[ "$exp" == "$live" ]] || FAIL=1
    done <"$GROUP_EXPECTED"
}

# Set equality on file|test pairs and per-file prose counts.
UNTRACKED="$(comm -13 "$EXPECTED_PAIRS" "$LIVE_PAIRS")"
STALE="$(comm -23 "$EXPECTED_PAIRS" "$LIVE_PAIRS")"
PROSE_DIFF="$(comm -3 "$EXPECTED_PROSE" "$LIVE_PROSE_COUNTS")"

if [[ "$FAIL" -eq 0 && -z "$UNTRACKED" && -z "$STALE" && -z "$PROSE_DIFF" ]]; then
    echo "OK: #[ignore] budget matches inventory — $LIVE_TOTAL mentions ($LIVE_ATTR attributes + $LIVE_PROSE doc/comment) across $(wc -l <"$GROUP_EXPECTED" | tr -d ' ') groups."
    exit 0
fi

echo "::error::check_ignored_guard: #[ignore] budget drift — live $LIVE_TOTAL vs inventoried $EXPECTED (attributes $LIVE_ATTR/$HDR_ATTR, doc/comment $LIVE_PROSE/$HDR_PROSE)."
print_groups
if [[ -n "$UNTRACKED" ]]; then
    echo "--- Untracked ignored tests (live but NOT in inventory):"
    while IFS= read -r line; do echo "  + $line"; done <<<"$UNTRACKED"
    echo "::error::A new ignored test must either be fixed or added to docs/test-inventory.md (issue linkage + reason)."
fi
if [[ -n "$STALE" ]]; then
    echo "--- Stale inventory rows (in inventory but no longer live):"
    while IFS= read -r line; do echo "  - $line"; done <<<"$STALE"
    echo "::error::An ignored test was removed/fixed without updating docs/test-inventory.md — update the inventory."
fi
if [[ -n "$PROSE_DIFF" ]]; then
    echo "--- Doc/comment mention drift (per-file counts, expected vs live):"
    while IFS= read -r line; do echo "  ~ $line"; done <<<"$PROSE_DIFF"
    echo "--- Uncatalogued doc/comment mentions are sentences, not tests. Either add a Comments/docs row to docs/test-inventory.md or reword the sentence:"
    sed -E 's/^/  > /' "$PROSE_FILE"
fi

exit 1
