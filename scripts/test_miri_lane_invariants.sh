#!/usr/bin/env bash
set -uo pipefail
# Miri lane invariants semantics harness (#2235) — RED before GREEN.
#
# The guard is `scripts/check_miri_lane_invariants.sh`: the four invariants that
# keep the UB lane a DETECTOR. This harness does two things:
#
#   1. runs the guard over the REAL tree (the invariants must hold), and
#   2. proves the guard CAN fail, by running it over `mktemp` fixtures that
#      reproduce the pre-fix shape (#2235): a skip-net with no reason per
#      entry, a Miri gate with no reason, a sibling test that drives the scrape
#      use case with no gate, and a fixture whose marker disappeared (the
#      vacuity case: an invariant that matches nothing must fail, not pass).
#
# If the checks stopped detecting those, this harness would be a green light
# wired to nothing. A check whose positive control fails is a broken harness,
# not a finding.
#
# STATIC: pure text plus mktemp fixtures. No cargo, no Miri, no network, no
# repo state touched. Exit 0 = the guard holds AND it can still fail.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${MIRI_LANE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

MIRI_LANE_LIBRARY=1
# shellcheck source=scripts/check_miri_lane_invariants.sh
. "$SCRIPT_DIR/check_miri_lane_invariants.sh"

printf '== Miri lane invariants (%s)
' "$REPO_ROOT"
real_rc=0
run_checks "$REPO_ROOT" || real_rc=1

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

red_control=0
for check in check_skips check_gate_reasons check_ub_attribution; do
  if "$check" "$FIX" >/dev/null 2>&1; then
    printf 'FAIL: RED control: %s accepted the pre-fix fixture — the check cannot fail\n' "$check"
    red_control=1
  else
    printf 'OK: RED control: %s rejects the pre-fix fixture\n' "$check"
  fi
done

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
if check_ub_attribution "$VACUITY" >/dev/null 2>&1; then
  printf 'FAIL: RED control: check_ub_attribution accepted a fixture whose marker disappeared — the invariant can inspect nothing\n'
  red_control=1
else
  printf 'OK: RED control: check_ub_attribution rejects a fixture whose marker disappeared\n'
fi
# The core filter check must ACCEPT a fixture whose filter covers
# `application::` (it is not part of the RED control: the pre-fix tree
# already carried that filter).
if ! check_core_filter "$FIX" >/dev/null 2>&1; then
  printf 'FAIL: RED control: check_core_filter rejected a fixture that covers application::\n'
  red_control=1
else
  printf 'OK: RED control: check_core_filter accepts a fixture that covers application::\n'
fi

if [ "$red_control" -ne 0 ]; then
  printf 'FAIL: the harness cannot detect the pre-fix shape\n'
else
  printf 'OK: the harness detects the pre-fix shape (RED control)\n'
fi

if [ "$real_rc" -ne 0 ] || [ "$red_control" -ne 0 ]; then
  exit 1
fi
exit 0

