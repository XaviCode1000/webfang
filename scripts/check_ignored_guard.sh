#!/usr/bin/env bash
# Ignored-test budget guard (stabilization-sitemap-regression).
#
# Fails closed when the live `#[ignore]` count in crates/ drifts from the
# categorized inventory in docs/test-inventory.md (baseline: 37 rows,
# including the single by-design sitemap DNS ignore). Any delta fails this
# script naming each untracked file:line pair.
#
# Fail-closed policy: missing tools, missing/unparseable inventory, or a
# broken live count all fail with an actionable message.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INVENTORY="$REPO_ROOT/docs/test-inventory.md"

command -v rg >/dev/null 2>&1 || {
    echo "::error::check_ignored_guard: ripgrep (rg) is required but not installed."
    exit 1
}
[[ -f "$INVENTORY" ]] || {
    echo "::error::check_ignored_guard: inventory not found at docs/test-inventory.md"
    exit 1
}

# Expected baseline parsed from the inventory's "Total:" summary line
# (format: "Total: 21+3+... = **37**.").
EXPECTED="$(sed -n 's/^Total:.*=[[:space:]]*\*\*\([0-9][0-9]*\)\*\*.*/\1/p' "$INVENTORY" | tail -1)"
if [[ -z "$EXPECTED" ]] || ! [[ "$EXPECTED" =~ ^[0-9]+$ ]]; then
    echo "::error::check_ignored_guard: could not parse the ignored-test total from docs/test-inventory.md ('Total:' line). Fix the inventory header first."
    exit 1
fi

TMPDIR_GUARD="$(mktemp -d)"
LIVE_FILE="$TMPDIR_GUARD/live.txt"
SORTED_LIVE_FILE="$TMPDIR_GUARD/live-sorted.txt"
INVENTORY_SORTED_FILE="$TMPDIR_GUARD/inventory-sorted.txt"
trap 'rm -rf "$TMPDIR_GUARD"' EXIT

rg -n '#\[ignore' crates/ --glob '!target' >"$LIVE_FILE" || true
LIVE="$(wc -l <"$LIVE_FILE" | tr -d ' ')"
[[ "$LIVE" =~ ^[0-9]+$ ]] || {
    echo "::error::check_ignored_guard: live #[ignore] count is not numeric ('$LIVE')."
    exit 1
}

if [[ "$LIVE" -eq "$EXPECTED" ]]; then
    echo "OK: #[ignore] count matches inventory ($LIVE/$EXPECTED)."
    exit 0
fi

echo "::error::check_ignored_guard: #[ignore] budget drift — live $LIVE vs inventoried $EXPECTED."
if [[ "$LIVE" -gt "$EXPECTED" ]]; then
    echo "::error::A new ignored test must either be fixed or added to docs/test-inventory.md (issue linkage + reason)."
else
    echo "::error::An ignored test was removed/fixed without updating docs/test-inventory.md — update the inventory."
fi

# Name each untracked ignore: live file:line pairs absent from the inventory's
# File:Line column.
sed -E 's/^([^:]+):([0-9]+):.*$/\1:\2/' "$LIVE_FILE" | sort >"$SORTED_LIVE_FILE"
# Markdown backticks via printf so the grep pattern stays shellcheck-clean.
BT="$(printf '\140')"
grep -oE "${BT}[^${BT}]+:[0-9]+${BT}" "$INVENTORY" | tr -d "$BT" | sort -u >"$INVENTORY_SORTED_FILE"

echo "--- Untracked ignores (live but NOT in inventory):"
comm -13 "$INVENTORY_SORTED_FILE" "$SORTED_LIVE_FILE" | sed 's/^/  + /'
echo "--- Stale inventory rows (in inventory but no longer live):"
comm -23 "$INVENTORY_SORTED_FILE" "$SORTED_LIVE_FILE" | sed 's/^/  - /'

exit 1
