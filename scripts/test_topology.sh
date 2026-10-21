#!/usr/bin/env bash
set -euo pipefail
# check-topology.sh — semantics harness (audit B43).
#
# Pins the topology contract of the trunk-based + release/support-line model:
#
#   1. a hotfix points at a REAL support line (`support/X.Y`, numeric X.Y);
#   2. a fix points at `main` or a real release line (`release/X.Y`);
#   3. every other development prefix points at `main`;
#   4. a versioned line head (`support/X.Y`, `release/X.Y`) is EXEMPT from
#      this check — but only in its versioned form: `support/garbage` and
#      `support/foo/bar` are malformed names, not lines, and are refused
#      (the old `support/*` glob crossed `/` and exempted them);
#   5. the tooling prefixes (`release-plz-*`, `dependabot/*`, `renovate/*`)
#      stay exempt;
#   6. an unknown prefix is refused.
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# The guard is driven through its own contract (HEAD_REF / BASE_REF in the
# environment), so the harness never touches the repository's git state.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check-topology.sh"
fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$GATE" ] || { echo "FAIL: gate script not found at $GATE"; exit 1; }

# run_topology <head> <base> -> sets RC and OUT
run_topology() {
    OUT="$(HEAD_REF="$1" BASE_REF="$2" bash "$GATE" 2>&1)" && RC=0 || RC=$?
}

# expect_ok <head> <base> <label>
expect_ok() {
    run_topology "$1" "$2"
    if [[ "$RC" -eq 0 ]]; then
        ok "$3"
    else
        bad "$3 (rc=$RC): $OUT"
    fi
}

# expect_reject <head> <base> <label>
expect_reject() {
    run_topology "$1" "$2"
    if [[ "$RC" -ne 0 ]]; then
        ok "$3"
    else
        bad "$3 — accepted silently: $OUT"
    fi
}

# --- 1. hotfix bases ---------------------------------------------------------
expect_ok "hotfix/fix-x" "support/2.12" "hotfix/* -> support/X.Y (numeric) is accepted"
expect_reject "hotfix/fix-x" "main" "hotfix/* -> main is rejected"
expect_reject "hotfix/fix-x" "support/garbage" "hotfix/* -> support/garbage is rejected (not a support/X.Y line)"
expect_reject "hotfix/fix-x" "support/foo/bar" "hotfix/* -> support/foo/bar is rejected (base crosses /)"
expect_reject "hotfix/fix-x" "support/2.12/extra" "hotfix/* -> support/2.12/extra is rejected (base crosses /)"
expect_reject "hotfix/fix-x" "support/main-support" "hotfix/* -> support/main-support is rejected (not a numeric X.Y line)"

# --- 2. fix bases ------------------------------------------------------------
expect_ok "fix/x" "main" "fix/* -> main is accepted"
expect_ok "fix/x" "release/2.13" "fix/* -> release/X.Y (numeric) is accepted"
expect_reject "fix/x" "some-base" "fix/* -> some-base is rejected"
expect_reject "fix/x" "release/garbage" "fix/* -> release/garbage is rejected (not a release/X.Y line)"
expect_reject "fix/x" "release/foo/bar" "fix/* -> release/foo/bar is rejected (base crosses /)"

# --- 3. other development prefixes -------------------------------------------
expect_ok "feat/x" "main" "feat/* -> main is accepted"
expect_reject "feat/x" "release/2.13" "feat/* -> release/2.13 is rejected (main only)"
expect_reject "chore/x" "support/2.12" "chore/* -> support/2.12 is rejected (main only)"

# --- 4. versioned line HEADS: exempt only in their versioned form -------------
expect_ok "support/2.12" "main" "support/X.Y head is exempt from the topology check"
expect_ok "release/2.13" "main" "release/X.Y head is exempt from the topology check"
expect_reject "support/garbage" "main" "support/garbage head is rejected (malformed line name)"
expect_reject "support/foo/bar" "main" "support/foo/bar head is rejected (name crosses /)"
expect_reject "release/garbage" "main" "release/garbage head is rejected (malformed line name)"
expect_reject "release/foo/bar" "main" "release/foo/bar head is rejected (name crosses /)"
expect_reject "support/" "main" "support/ head is rejected (no version)"

# --- 5. tooling prefixes stay exempt -----------------------------------------
expect_ok "release-plz-2026-10-21" "main" "release-plz-* head is exempt"
expect_ok "dependabot/cargo/wreq" "main" "dependabot/* head is exempt"
expect_ok "renovate/cargo" "main" "renovate/* head is exempt"

# --- 6. unknown prefix -------------------------------------------------------
expect_reject "wip/x" "main" "an unknown branch prefix is rejected"

if [[ "$fail" -eq 0 ]]; then
    echo "test_topology: all checks passed."
    exit 0
else
    echo "test_topology: at least one check FAILED."
    exit 1
fi
