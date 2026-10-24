#!/usr/bin/env bash
set -uo pipefail
# Miri lane invariants semantics harness (#2235) — RED before GREEN.
#
# The guard is `scripts/check_miri_lane_invariants.sh`: the four invariants that
# keep the UB lane a DETECTOR. This harness does three things:
#
#   1. runs the guard over the REAL tree (the invariants must hold), and
#   2. proves the guard CAN fail, by running it over `mktemp` fixtures that
#      reproduce the pre-fix shape (#2235): a skip-net with no reason per
#      entry, a Miri gate with no reason, a sibling test that drives the scrape
#      use case with no gate, and a fixture whose marker disappeared (the
#      vacuity case: an invariant that matches nothing must fail, not pass), and
#   3. proves that a guard that CANNOT fail is caught: the meta-guard
#      `scripts/test_guard_coverage.sh` replaces the guard with `exit 0` and
#      requires this harness to go red. That is why the guard is called as a
#      PROGRAM (a subprocess) and never sourced: a sourced guard's `exit 0`
#      would take this harness down with it, the controls below would never
#      run, and the harness would report green over a guard that decides
#      nothing — the fail-open shape issue #2056 exists to close.
#
# Each invariant is isolated: the fixture that violates it is checked with
# `MIRI_LANE_CHECK` naming that invariant only, so a red proves THAT check
# detected the shape, not that some other check tripped over the fixture.
#
# If the checks stopped detecting those, this harness would be a green light
# wired to nothing. A check whose positive control fails is a broken harness,
# not a finding.
#
# STATIC: pure text plus mktemp fixtures. No cargo, no Miri, no network, no
# repo state touched. Exit 0 = the guard holds AND it can still fail.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/check_miri_lane_invariants.sh"
REPO_ROOT="${MIRI_LANE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

[ -f "$GUARD" ] || { echo "test_miri_lane_invariants: no guard at $GUARD" >&2; exit 1; }

FAILURES=0

# run_guard <root> <check> — the guard's exit code over <root>.
#
# The guard runs as a subprocess with `MIRI_LANE_ROOT` pointing at the tree it
# has to judge and `MIRI_LANE_CHECK` naming the single invariant to run (an
# empty name runs all of them, which is how the real tree is judged).
run_guard() {
  local root="$1" check="$2"
  MIRI_LANE_ROOT="$root" MIRI_LANE_CHECK="$check" bash "$GUARD" >/dev/null 2>&1
}

# expect_rc <what> <want_rc> <got_rc>
expect_rc() {
  local what="$1" want="$2" got="$3"
  if [ "$got" != "$want" ]; then
    printf 'FAIL: %s: expected exit %s, got %s\n' "$what" "$want" "$got"
    FAILURES=$((FAILURES + 1))
    return 0
  fi
  printf 'OK: %s\n' "$what"
}

printf '== Miri lane invariants (%s)\n' "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Control 1 — the REAL tree: every invariant holds (exit 0).
# ---------------------------------------------------------------------------

run_guard "$REPO_ROOT" ""
expect_rc "the real tree satisfies every Miri lane invariant" 0 "$?"

# ---------------------------------------------------------------------------
# RED control — the harness must be able to FAIL
#
# The fixtures below reproduce the pre-fix shape (#2235): a skip-net with no
# reason per entry, a Miri gate with no reason, and a sibling test that
# drives the scrape use case with no gate. If the checks stopped detecting
# them, this harness would be a green light wired to nothing.
# ---------------------------------------------------------------------------

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

FIX="$T/fix"
mkdir -p "$FIX/.github/workflows" \
  "$FIX/crates/webfang_core/src/application" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader"

# The workflow carries the THREE Miri jobs the guard inspects, and only the
# `miri` one carries the skip-net: the other two have nothing to justify, so
# the rejection below can only come from the unjustified skip.
cat >"$FIX/.github/workflows/ci.yml" <<'EOF'
jobs:
  miri:
    name: Miri (core)
    strategy:
      matrix:
        include:
          - group: core
            filter: "-- adapters:: application:: cli::"
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- \
            --skip wreq_downloader --skip obscura_downloader \
            -- adapters:: application::
  miri-infra-fast:
    name: Miri (infra-fast)
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- -- infrastructure::
  miri-infra-slow:
    name: Miri (infra-slow)
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- -- infrastructure::
EOF

# The pre-fix shape of the module of the attributed UB: the test that
# REACHES the UB surface (a 2xx response) has no gate, and the arm that does
# not reach it is the nominally exempt one.
cat >"$FIX/crates/webfang_core/src/application/scraper_service.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn scrape_span_records_success_outcome_and_result_count() {
        let client = StubClient { status: 200 };
        let _ = scrape_with_config();
    }

    #[test]
    fn scrape_span_records_error_outcome_with_error_class() {
        let client = StubClient { status: 404 };
        let _ = scrape_with_config();
    }
}
EOF

cat >"$FIX/crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_wreq_downloader_creation() {}
}
EOF

cp "$FIX/crates/webfang_core/src/application/scraper_service.rs" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader/obscura_downloader.rs"
cp "$FIX/crates/webfang_core/src/application/scraper_service.rs" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader/chromiumoxide_downloader.rs"

# NEGATION: a Miri job that skips a whole submodule with no written reason of
# its own must be REJECTED. Before the fix (#2235) the skip-net of #1423 was
# accepted, and it deleted a whole test tree per entry — with it, the UB
# hiding inside that tree.
run_guard "$FIX" check_skips
expect_rc "RED control: check_skips rejects the unjustified skip-net" 1 "$?"

# NEGATION: a `#[cfg_attr(miri, ignore)]` gate with no reason must be
# REJECTED. A gate nobody can evaluate is not a gate: it silences the test
# without saying what the silence is for.
run_guard "$FIX" check_gate_reasons
expect_rc "RED control: check_gate_reasons rejects a gate with no reason" 1 "$?"

# NEGATION: a test that drives the scrape use case and reaches the UB surface
# with no per-test gate must be REJECTED. The lane has to REPORT the UB, not
# abort on it: an ungated test takes the whole lane down with it.
run_guard "$FIX" check_ub_attribution
expect_rc "RED control: check_ub_attribution rejects the ungated UB test" 1 "$?"

# Vacuity control: the marker that says "this test drives the scrape use
# case" disappears (a rename of the use case). The invariant must FAIL
# instead of inspecting nothing and reporting green.
VACUITY="$T/vacuity"
mkdir -p "$VACUITY/.github/workflows" \
  "$VACUITY/crates/webfang_core/src/application" \
  "$VACUITY/crates/webfang_core/src/infrastructure/downloader"
cp "$FIX/.github/workflows/ci.yml" "$VACUITY/.github/workflows/ci.yml"
cat >"$VACUITY/crates/webfang_core/src/application/scraper_service.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn scrape_span_records_success_outcome_and_result_count() {
        let _ = renamed_use_case();
    }

    #[test]
    fn scrape_span_records_error_outcome_with_error_class() {
        let _ = renamed_use_case();
    }
}
EOF
for owned in wreq_downloader obscura_downloader chromiumoxide_downloader resource_governor; do
  printf '// fixture\n' >"$VACUITY/crates/webfang_core/src/infrastructure/downloader/$owned.rs"
done
run_guard "$VACUITY" check_ub_attribution
expect_rc "RED control: check_ub_attribution rejects a fixture whose marker disappeared" 1 "$?"

# The core filter check must ACCEPT a fixture whose filter covers
# `application::` (it is not part of the RED control: the pre-fix tree
# already carried that filter). A check that rejects everything is not a gate
# either — this is the positive control that keeps the RED controls honest.
run_guard "$FIX" check_core_filter
expect_rc "RED control: check_core_filter accepts a fixture that covers application::" 0 "$?"

if [ "$FAILURES" -ne 0 ]; then
  printf 'FAIL: the harness cannot detect the pre-fix shape\n'
  exit 1
fi
printf 'OK: the harness detects the pre-fix shape (RED control)\n'
exit 0
