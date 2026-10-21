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
# The live scan classifies every mention by its COMMENT STATE, not only by the
# shape of its line: a flush-left `#[ignore` inside a block comment is a
# commented-out attribute, never a live one (audit B42).
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
# --- Comment-state aware classification (audit B42) --------------------------
# A mention is NOT automatically an attribute: the classification has to know
# whether the mention lives in code or in a comment. The old line-shape test
# ("the line starts with `#[ignore`) could not tell a commented-out attribute
# from a live one, so a flush-left `#[ignore` inside a block comment was
# counted as an ATTRIBUTE, paired with the next `fn` and failed CI on a
# comment (the audit's repro).
#
# Three buckets:
#   ATTR  — the line's CODE starts with `#[ignore`: a live test attribute.
#   PROSE — a documentation mention: a sentence, a `//` comment, a string, or
#           a block comment that is not a commented-out attribute. Catalogued
#           per file under the Comments/docs category.
#   EXCL  — a commented-out attribute: the line carries no code and its
#           block-comment text IS an `#[ignore` attribute. Neither an
#           attribute nor a sentence anyone has to catalogue, so it is
#           excluded from the live counts entirely.
#
# The extractor is one awk process per file with candidates (the comment
# state has to be tracked from the first line of the file).
CLASSIFY_AWK="$TMPDIR_GUARD/classify.awk"
cat >"$CLASSIFY_AWK" <<'AWK'
# classify.awk — comment-state aware classification of `#[ignore` mentions.
#
# Input : one Rust source file, scanned line by line (the comment state has
#         to be tracked from the first line of the file).
#   -v wantfile : newline-separated line numbers to classify
#   -v relfile  : the path the output identifies the file with
# Output: "RELFILE:LINE:CLASS" with CLASS in {ATTR, PROSE, EXCL}.
#
# The scanner tracks block comments (`/* ... */`), line comments (`//`),
# double-quoted strings (with escapes) and raw strings (`r"..."`, `r#"..."#`).
# Rust char literals and lifetimes are NOT tracked: a `'` cannot open a block
# comment, so skipping them cannot change the block-comment state.
#
# A raw string is tracked because its terminator is a QUOTE that is NOT
# escaped: mis-tracking one would keep the rest of the file in "string" state
# and blind the block-comment detection after it.
function trim(s) {
    gsub(/^[ \t]+/, "", s)
    gsub(/[ \t]+$/, "", s)
    return s
}
BEGIN {
    while ((getline n < wantfile) > 0) {
        if (n ~ /^[0-9]+$/) want[n + 0] = 1
    }
    in_block = 0
    in_str = 0
}
{
    lineno = NR
    code = ""; blk = ""
    i = 1; n = length($0)
    while (i <= n) {
        ch = substr($0, i, 1)
        if (in_block) {
            if (ch == "*" && substr($0, i + 1, 1) == "/") { in_block = 0; i += 2; continue }
            blk = blk ch; i++; continue
        }
        if (in_str) {
            if (ch == "\\") { code = code substr($0, i, 2); i += 2; continue }
            if (ch == "\"") in_str = 0
            code = code ch; i++; continue
        }
        if (ch == "/" && substr($0, i + 1, 1) == "/") { i = n + 1; continue }
        if (ch == "/" && substr($0, i + 1, 1) == "*") { in_block = 1; i += 2; continue }
        if (ch == "r") {
            # Raw string: r"..." / r#"..."# — the terminator is a quote that is
            # NOT escaped, so it has to be consumed as a unit.
            j = i + 1; hashes = ""
            while (substr($0, j, 1) == "#") { hashes = hashes "#"; j++ }
            if (substr($0, j, 1) == "\"") {
                term = "\"" hashes
                p = index(substr($0, j + 1), term)
                if (p == 0) {
                    code = code substr($0, i); i = n + 1
                } else {
                    code = code substr($0, i, j + p + length(term) - i)
                    i = j + p + length(term)
                }
                continue
            }
        }
        if (ch == "\"") { in_str = 1; code = code ch; i++; continue }
        code = code ch; i++
    }
    if (!(lineno in want)) next
    ct = trim(code)
    bt = trim(blk)
    if (ct ~ /^#\[ignore/) {
        print relfile ":" lineno ":ATTR"
    } else if (ct == "" && bt ~ /^#\[ignore/) {
        print relfile ":" lineno ":EXCL"
    } else {
        print relfile ":" lineno ":PROSE"
    }
}
AWK

CANDIDATE_FILES="$TMPDIR_GUARD/candidate-files.txt"
CLASSIFIED="$TMPDIR_GUARD/classified.txt"
awk -F: '{print $1}' "$LIVE_FILE" | sort -u >"$CANDIDATE_FILES"
: >"$CLASSIFIED"
while IFS= read -r gf; do
    awk -F: -v f="$gf" '$1 == f { print $2 }' "$LIVE_FILE" | sort -un >"$TMPDIR_GUARD/want.txt"
    awk -v wantfile="$TMPDIR_GUARD/want.txt" -v relfile="$gf" -f "$CLASSIFY_AWK" \
        "$REPO_ROOT/$gf" >>"$CLASSIFIED" || true
done <"$CANDIDATE_FILES"

: >"$ATTR_FILE"
: >"$PROSE_FILE"
awk -F: -v attr="$ATTR_FILE" -v prose="$PROSE_FILE" -v clsfile="$CLASSIFIED" '
    FILENAME == clsfile { cls[$1 ":" $2] = $3; next }
    {
        c = cls[$1 ":" $2]
        if (c == "ATTR") print $0 > attr
        else if (c == "PROSE") print $0 > prose
    }' "$CLASSIFIED" "$LIVE_FILE"

LIVE_ATTR="$(wc -l <"$ATTR_FILE" | tr -d ' ')"
LIVE_PROSE="$(wc -l <"$PROSE_FILE" | tr -d ' ')"
LIVE_TOTAL=$((LIVE_ATTR + LIVE_PROSE))
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
