#!/usr/bin/env bash
set -euo pipefail
# check_intra_crate_direction.sh — allowlist min-match semantics harness
# (audit B44).
#
# The allowlist is a BUDGET, not a wish list: an entry that absorbs ZERO
# sites is stale — it exempts nothing and only hides that the code it was
# written for has moved or is gone. Before this harness the checker never
# verified a min-match, so a stale entry rode silently behind the scan (at
# most a `::warning::` about the entry COUNT) and the gate stayed green.
#
# Cases pinned here:
#   1. a LIVE entry that absorbs a real violation keeps the gate green;
#   2. a STALE entry (zero sites) FAILS the gate and names the entry;
#   3. a stale entry next to a real, uncovered violation fails for BOTH
#      reasons (the stale entry is not a way to mute the violation);
#   4. a per-FILE entry whose file carries no site is stale too (the file
#      exemption absorbs nothing);
#   5. in warn mode the stale entry is reported as a warning and the mode
#      switch keeps its meaning (the gate itself is strict in CI);
#   6. the gate still fails a real, unallowlisted violation (it did not
#      become dead while the min-match was added).
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures live in a mktemp tree and are reached through INTRA_CRATE_ROOT /
# INTRA_CRATE_ALLOWLIST / INTRA_CRATE_MODE, so this harness never reads or
# writes the repository.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_intra_crate_direction.sh"
fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$GATE" ] || { echo "FAIL: gate script not found at $GATE"; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/src/application" "$T/src/infrastructure"

# run_gate <allowlist-file> [mode] -> sets RC and OUT
run_gate() {
    local allowlist="$1"
    local mode="${2:-strict}"
    OUT="$(cd "$T" && INTRA_CRATE_ROOT=src INTRA_CRATE_ALLOWLIST="$allowlist" \
        INTRA_CRATE_MODE="$mode" bash "$GATE" 2>&1)" && RC=0 || RC=$?
}

ALLOW="$T/allowlist.txt"

# --- 1. a live entry keeps the gate green ------------------------------------
: >"$T/src/application/new_file.rs"
printf 'use crate::infrastructure::evil::direct as _;\n' >"$T/src/application/new_file.rs"
printf 'infrastructure::evil::direct # ADR-3000 live entry, removal condition: repro\n' >"$ALLOW"
run_gate "$ALLOW"
if [[ "$RC" -eq 0 ]]; then
    ok "1 a live entry that absorbs a real violation keeps the gate green"
    case "$OUT" in
        *"allowlisted 1"*) ok "1b the absorbed site is counted" ;;
        *) bad "1b the absorbed site is not counted: $OUT" ;;
    esac
else
    bad "1 a live entry failed the gate (rc=$RC): $OUT"
fi

# --- 2. a stale entry FAILS the gate -----------------------------------------
: >"$T/src/application/new_file.rs"
printf 'nonexistent::subsystem::path # ADR-9999 stale entry, removal condition: repro\n' >"$ALLOW"
run_gate "$ALLOW"
if [[ "$RC" -ne 0 ]]; then
    ok "2 a stale entry (zero absorbed sites) fails the gate (rc=$RC)"
    case "$OUT" in
        *"nonexistent::subsystem::path"*) ok "2b the failure names the stale entry" ;;
        *) bad "2b the failure does not name the stale entry: $OUT" ;;
    esac
    case "$OUT" in
        *"::error::"*) ok "2c the stale entry is an error, not a warning" ;;
        *) bad "2c the stale entry is not reported as an error: $OUT" ;;
    esac
else
    bad "2 a stale entry passed silently (rc=0): $OUT"
fi

# --- 3. a stale entry does not mute a real violation -------------------------
printf 'use crate::infrastructure::evil::direct as _;\n' >"$T/src/application/new_file.rs"
printf 'nonexistent::subsystem::path # ADR-9999 stale entry, removal condition: repro\n' >"$ALLOW"
run_gate "$ALLOW"
if [[ "$RC" -ne 0 ]]; then
    ok "3 a stale entry next to a real violation still fails (rc=$RC)"
    case "$OUT" in
        *"::error::found 1 intra-crate direction violation"*) ok "3b the real violation is still reported" ;;
        *) bad "3b the real violation is not reported: $OUT" ;;
    esac
else
    bad "3 a stale entry muted a real violation (rc=0): $OUT"
fi

# --- 4. a per-FILE entry with no site in that file is stale ------------------
: >"$T/src/application/new_file.rs"
: >"$T/src/infrastructure/other.rs"
printf 'application/other.rs # ADR-3001 per-file entry with no site, removal condition: repro\n' >"$ALLOW"
run_gate "$ALLOW"
if [[ "$RC" -ne 0 ]]; then
    ok "4 a per-file entry whose file carries no site is stale and fails (rc=$RC)"
else
    bad "4 a per-file entry with no site passed silently (rc=0): $OUT"
fi

# --- 5. warn mode: the stale entry is a warning, the mode keeps its meaning --
run_gate "$ALLOW" warn
if [[ "$RC" -eq 0 ]]; then
    ok "5 in warn mode the stale entry does not fail the gate"
    case "$OUT" in
        *"::warning::"*) ok "5b in warn mode the stale entry is a warning" ;;
        *) bad "5b in warn mode the stale entry is not a warning: $OUT" ;;
    esac
else
    bad "5 warn mode failed on a stale entry (rc=$RC): $OUT"
fi

# --- 6. the gate is not dead: a real violation still fails ------------------
: >"$T/src/application/new_file.rs"
printf 'use crate::infrastructure::evil::direct as _;\n' >"$T/src/application/new_file.rs"
: >"$ALLOW"
run_gate "$ALLOW"
if [[ "$RC" -ne 0 ]]; then
    case "$OUT" in
        *"::error::found 1 intra-crate direction violation"*) ok "6 a real, unallowlisted violation still fails closed" ;;
        *) bad "6 the real violation is not reported: $OUT" ;;
    esac
else
    bad "6 a real, unallowlisted violation passed silently (rc=0): $OUT"
fi

if [[ "$fail" -eq 0 ]]; then
    echo "test_intra_crate_allowlist: all checks passed."
    exit 0
else
    echo "test_intra_crate_allowlist: at least one check FAILED."
    exit 1
fi
