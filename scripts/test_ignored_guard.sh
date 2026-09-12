#!/usr/bin/env bash
set -euo pipefail
# Ignored-test budget guard — semantics harness (#1328). RED before GREEN.
#
# Pins the five behaviors the per-category contract added over the old
# totals-only check:
#   1. a matching composition passes (baseline);
#   2. a composition swap whose TOTAL matches fails — one attribute
#      un-ignored plus one new doc-comment mention kept 32/32 green under
#      the old guard (the exact #1328 regression: the WAF un-ignore swap);
#   3. an uncatalogued doc-comment mention fails EXPLAINED: the output names
#      the Comments/docs category and the sentence's file:line, never an
#      unexplained total mismatch;
#   4. a new ignored test without an inventory row fails naming file|test;
#   5. rows are keyed by file + test name: inserting lines above an ignored
#      test does not invalidate its row (the old file:line drift).
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures live in a mktemp tree and are reached through IGNORED_GUARD_ROOT,
# so this harness never reads or writes the repository inventory.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/check_ignored_guard.sh"
fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$GUARD" ] || { echo "FAIL: guard script not found at $GUARD"; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/crates/sample/tests" "$T/docs"

SRC="$T/crates/sample/tests/sample_test.rs"
INV="$T/docs/test-inventory.md"

# Baseline fixture: exactly one real attribute (net_one) and one prose
# mention — the same attribute/prose split the real inventory declares.
write_source_baseline() {
    cat >"$SRC" <<'EOF'
#[cfg(test)]
mod tests {
    // prose mention of #[ignore] in a comment
    #[test]
    #[ignore = "requires network"]
    fn net_one() {}
}
EOF
}

write_inventory_baseline() {
    cat >"$INV" <<'EOF'
# Test Inventory — `#[ignore]` Catalog (Gate 0)

**Source of truth:** `rg -n "#\[ignore" crates/ --glob '!target'` — **2 rows** (1 test attributes + 1 doc/comment mentions).

Total: 1+1 = **2**.

| # | Group | File | Identifier | Reason | Issue | Next |
|---|-------|------|------------|--------|-------|------|
| 1 | Network | `crates/sample/tests/sample_test.rs` | `net_one` | `requires network` | #0 | keep |
| 2 | Comments/docs | `crates/sample/tests/sample_test.rs` | `doc` | prose mention | #0 | docs only |
EOF
}

# run_guard -> sets RC and OUT (guard status is the assertion, output is evidence)
run_guard() {
    OUT="$(IGNORED_GUARD_ROOT="$T" bash "$GUARD" 2>&1)" && RC=0 || RC=$?
}

reset_fixture() {
    write_source_baseline
    write_inventory_baseline
}

# --- 1. baseline: matching composition passes --------------------------------
reset_fixture
run_guard
if [[ "$RC" -eq 0 ]]; then ok "1 baseline composition passes"; else bad "1 baseline failed (rc=$RC): $OUT"; fi

# --- 2. composition swap, equal total: MUST fail ------------------------------
# The #1328 regression: net_one is un-ignored and a new prose mention appears.
# Live scan still sees 2 mentions; the old totals-only guard passed this green.
reset_fixture
cat >"$SRC" <<'EOF'
#[cfg(test)]
mod tests {
    // prose mention of #[ignore] in a comment
    // another sentence mentioning #[ignore] in docs
    #[test]
    fn net_one() {}
}
EOF
run_guard
if [[ "$RC" -ne 0 ]]; then
    ok "2 composition swap with equal total fails (rc=$RC)"
    case "$OUT" in
        *net_one*) ok "2b failure output names the stale row" ;;
        *) bad "2b failure output does not name the stale pair: $OUT" ;;
    esac
else
    bad "2 composition swap with equal total PASSED silently — the #1328 regression is back"
fi

# --- 3. uncatalogued doc-comment sentence: fails EXPLAINED --------------------
# A developer writes a sentence mentioning #[ignore] and touches no inventory.
# The red must name the Comments/docs category and the sentence's file:line —
# never the old "budget drift" with an unexplained count mismatch.
reset_fixture
printf '%s\n' '    // a doc note about #[ignore] semantics' >>"$SRC"
run_guard
if [[ "$RC" -ne 0 ]]; then
    case "$OUT" in
        *"Comments/docs"*) ok "3 uncatalogued doc mention names the Comments/docs category" ;;
        *) bad "3 output does not name Comments/docs: $OUT" ;;
    esac
    case "$OUT" in
        *"sentences, not tests"*) ok "3b output explains it is a sentence, not a test" ;;
        *) bad "3b output lacks the sentence guidance: $OUT" ;;
    esac
else
    bad "3 uncatalogued doc mention passed silently"
fi

# --- 4. new ignored test without a row: fails naming file|test ----------------
reset_fixture
cat >>"$SRC" <<'EOF'
#[cfg(test)]
mod more_tests {
    #[test]
    #[ignore = "requires network"]
    fn net_two() {}
}
EOF
run_guard
if [[ "$RC" -ne 0 ]]; then
    case "$OUT" in
        *"sample_test.rs|net_two"*) ok "4 new ignored test reported as untracked file|test pair" ;;
        *) bad "4 output does not name sample_test.rs|net_two: $OUT" ;;
    esac
else
    bad "4 untracked ignored test passed silently"
fi

# --- 5. line drift is harmless: file+name keys survive insertions ------------
reset_fixture
{
    echo "// inserted above the ignored test to shift every line number"
    for _ in 1 2 3 4 5 6 7 8 9 10; do echo "// filler line"; done
    cat "$SRC"
} >"$SRC.new" && mv "$SRC.new" "$SRC"
run_guard
if [[ "$RC" -eq 0 ]]; then ok "5 rows keyed by file+name survive line insertions"; else bad "5 line drift broke the guard (rc=$RC): $OUT"; fi

if [[ "$fail" -eq 0 ]]; then
    echo "test_ignored_guard: all checks passed."
    exit 0
else
    echo "test_ignored_guard: at least one check FAILED."
    exit 1
fi
