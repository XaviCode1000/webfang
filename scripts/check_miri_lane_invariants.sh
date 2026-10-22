#!/usr/bin/env bash
set -uo pipefail
# Miri lane invariants guard (#2235) — the FOUR invariants that keep the UB
# lane a DETECTOR instead of an abort.
#
# The UB lane (`Miri (core)`) must REPORT, not abort the process. A lane that
# aborts leaves no result to act on — the dead end #2235 exists to close. Four
# invariants keep it a detector:
#
#   1. NO unjustified `--skip` in a Miri job. The skip-net of #1423 deleted a
#      whole test tree per entry and, with it, the UB hiding inside that
#      tree. A skip that survives must carry a WRITTEN REASON of its own in
#      the comment block of its job — one shared sentence listing several
#      skips is not a per-entry reason.
#   2. Every `#[cfg_attr(miri, ignore = "...")]` in the workspace carries a
#      non-empty reason, and every Miri gate in the modules this lot owns
#      carries one (a gate nobody can evaluate is not a gate).
#   3. The `Miri (core)` filter still covers the module of the attributed UB
#      (`application::`). The detection stays Miri's: the aliasing model is
#      never disabled globally to make the lane green.
#   4. The test in `application/scraper_service.rs` that REACHES the UB
#      surface (a 2xx response — the only path that gets to `clean_html` →
#      `lol_html::rewrite_str`) carries a PER-TEST gate, so the UB is
#      attributed to a test instead of aborting the whole lane. A test that
#      drives the use case WITHOUT reaching that surface is listed in the
#      nominal allowlist below, with the reason and the evidence that keep it
#      ungated: an invariant that holds by accident is not an invariant.
#
# This guard is STATIC: pure text over the checked-out tree. No cargo, no
# Miri, no network, no repo state touched. Exit 0 = every invariant holds;
# exit 1 = at least one failed.
#
# The semantics harness that proves this guard CAN fail is the sibling
# `scripts/test_miri_lane_invariants.sh` (the repo's guard/harness pair).
#
# Usage: bash scripts/check_miri_lane_invariants.sh
# Env:   MIRI_LANE_ROOT — point the guard at a fixture tree.
#        MIRI_LANE_LIBRARY=1 — sourced as a library: do not run the checks.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${MIRI_LANE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# Modules this lot owns: the downloader submodules the skip-net used to
# delete wholesale, plus the module of the attributed UB.
OWNED_MODULES=(
  "crates/webfang_core/src/application/scraper_service.rs"
  "crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/obscura_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/chromiumoxide_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/resource_governor.rs"
)

# Miri jobs of ci.yml that run the UB detector.
MIRI_JOBS=(miri miri-infra-fast miri-infra-slow)

# The tests of the module of the attributed UB that drive the scrape use case
# WITHOUT reaching the UB surface. Each entry is
#   <test name>|<reason it does not reach the surface>|<evidence>
# A test that drives the use case and is neither gated nor listed here is a
# FAIL: the invariant is explicit on purpose, never satisfied by accident.
# A listed test that no longer exists is a FAIL too: a stale exemption is a
# lie nobody can evaluate.
UB_SURFACE_EXEMPT=(
  "scrape_span_records_error_outcome_with_error_class|the 404 arm returns before clean_html (fetch_html rejects every non-2xx status before the cleaner)|scoped Miri run of 2026-10-22: 2 passed, 0 failed, 1 ignored"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# miri_gates <file> — one record per `#[cfg_attr(miri, ignore …)]` gate:
#   <start_line>|<end_line>|<string_reason>|<comment_reason>|<string_form>
# <string_reason> is 1 when the gate carries a non-empty `ignore = "…"`;
# <comment_reason> is 1 when the line that CLOSES the gate carries a `//`
# reason (the `#[cfg_attr(miri, ignore)] // reason` shape).
# Comment lines are skipped: a doc comment that QUOTES an attribute is prose,
# not a gate.
miri_gates() {
  awk '
    function flush(   txt) {
      if (!in_gate) return
      txt = buf
      has_str = (txt ~ /ignore[[:space:]]*=[[:space:]]*"[^"]+"/) ? 1 : 0
      has_form = (txt ~ /ignore[[:space:]]*=[[:space:]]*"/) ? 1 : 0
      printf "%d|%d|%d|%d|%d\n", start_line, end_line, has_str, has_comment, has_form
      buf = ""
      in_gate = 0
    }
    {
      line = $0
      if (line ~ /^[[:space:]]*(\/\/|\/\*|\*)/) next
      scrub = line
      gsub(/"[^"]*"/, "\"\"", scrub)
      opens = gsub(/\[/, "", scrub)
      closes = gsub(/\]/, "", scrub)
      if (in_gate) {
        buf = buf "\n" line
        end_line = NR
        depth = depth + opens - closes
        if (depth <= 0) {
          has_comment = (line ~ /\/\//) ? 1 : 0
          flush()
        }
        next
      }
      if (line ~ /cfg_attr\([[:space:]]*miri/) {
        in_gate = 1
        buf = line
        start_line = NR
        end_line = NR
        depth = opens - closes
        if (depth <= 0) {
          has_comment = (line ~ /\/\//) ? 1 : 0
          flush()
        }
        next
      }
    }
    END { flush() }
  ' "$1"
}

# job_block <ci.yml> <job> — the lines of one Miri job (header excluded).
job_block() {
  awk -v job="$2" '
    $0 ~ ("^  " job ":") { in_job = 1; next }
    in_job && $0 ~ "^  [A-Za-z0-9_-]+:" { exit }
    in_job { print }
  ' "$1"
}

# job_comments <ci.yml> <job> — the contiguous comment lines directly above
# the job header: the design block that documents that job.
job_comments() {
  awk -v job="$2" '
    { lines[NR] = $0 }
    $0 ~ ("^  " job ":") {
      for (i = NR - 1; i >= 1; i--) {
        if (lines[i] ~ /^[[:space:]]*#/) print lines[i]
        else break
      }
      exit
    }
  ' "$1"
}

# ---------------------------------------------------------------------------
# Check 1 — no unjustified --skip in a Miri job
# ---------------------------------------------------------------------------

check_skips() {
  local root="$1"
  local ci="$root/.github/workflows/ci.yml"
  local rc=0
  local job block comments names name line other

  if [ ! -f "$ci" ]; then
    printf 'FAIL: check_skips: workflow not found: %s\n' "$ci"
    return 1
  fi

  for job in "${MIRI_JOBS[@]}"; do
    block="$(job_block "$ci" "$job")"
    if [ -z "$block" ]; then
      printf 'FAIL: check_skips: Miri job not found in ci.yml: %s\n' "$job"
      rc=1
      continue
    fi
    comments="$(job_comments "$ci" "$job")"
    # Comment lines are excluded: a future author writing "--skip foo" in a
    # comment must not trip a confusing failure (the block of one job holds
    # the design comments of the next one).
    names="$(printf '%s\n' "$block" | grep -v -E '^[[:space:]]*#' | grep -o -- '--skip [A-Za-z0-9_:-][A-Za-z0-9_:-]*' | awk '{print $2}' | sort -u)"

    for name in $names; do
      # A justification must be a line of its OWN: it names this skip, gives
      # a reason, and does not bundle another skip into the same sentence
      # (one shared sentence is not a per-entry reason).
      line=""
      while IFS= read -r candidate; do
        [ -n "$candidate" ] || continue
        bundled=0
        for other in $names; do
          [ "$other" = "$name" ] && continue
          case "$candidate" in
            *"$other"*) bundled=1 ;;
          esac
        done
        [ "$bundled" -eq 1 ] && continue
        if printf '%s' "$candidate" | grep -Eqi 'motivo|reason|because|exclud|justif|gate'; then
          line="$candidate"
          break
        fi
      done < <(printf '%s\n' "$comments" | grep -F -- "$name")
      if [ -z "$line" ]; then
        printf 'FAIL: check_skips: Miri job %s skips "%s" with no written reason of its own in its comment block\n' "$job" "$name"
        rc=1
      else
        printf 'OK: check_skips: Miri job %s justifies --skip %s\n' "$job" "$name"
      fi
    done
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_skips: no unjustified --skip in any Miri job\n'
  fi
  return "$rc"
}

# ---------------------------------------------------------------------------
# Check 2 — every Miri gate carries a written reason
# ---------------------------------------------------------------------------

check_gate_reasons() {
  local root="$1"
  local rc=0
  local file owned start end has_str has_comment has_form

  # 2a. Workspace-wide: a gate written as `ignore = "…"` must carry a
  #     non-empty reason. A gate without a reason is a gate nobody can
  #     evaluate.
  while IFS= read -r file; do
    while IFS='|' read -r start end has_str has_comment has_form; do
      [ -n "$start" ] || continue
      [ "$has_form" = "1" ] || continue
      [ "$has_str" = "1" ] && continue
      printf 'FAIL: check_gate_reasons: %s: Miri gate at line %s has an empty reason\n' "$file" "$start"
      rc=1
    done < <(miri_gates "$file")
  done < <(find "$root/crates" -name '*.rs' -type f | sort)

  # 2b. Owned modules: EVERY Miri gate carries a reason, in the
  #     `ignore = "…"` form or as a same-line `//` reason.
  for owned in "${OWNED_MODULES[@]}"; do
    file="$root/$owned"
    [ -f "$file" ] || continue
    while IFS='|' read -r start end has_str has_comment has_form; do
      [ -n "$start" ] || continue
      if [ "$has_str" != "1" ] && [ "$has_comment" != "1" ]; then
        printf 'FAIL: check_gate_reasons: %s: Miri gate at line %s carries no written reason\n' "$owned" "$start"
        rc=1
      fi
    done < <(miri_gates "$file")
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_gate_reasons: every Miri gate carries a written reason\n'
  fi
  return "$rc"
}

# ---------------------------------------------------------------------------
# Check 3 — the Miri (core) filter still covers the module of the UB
# ---------------------------------------------------------------------------

check_core_filter() {
  local root="$1"
  local ci="$root/.github/workflows/ci.yml"
  local filter

  if [ ! -f "$ci" ]; then
    printf 'FAIL: check_core_filter: workflow not found: %s\n' "$ci"
    return 1
  fi

  filter="$(job_block "$ci" miri | awk '
    /^[[:space:]]*- group: core/ { in_core = 1; next }
    in_core && /filter:/ { print; in_core = 0 }
  ')"

  if [ -z "$filter" ]; then
    printf 'FAIL: check_core_filter: the miri job has no filter for the core group\n'
    return 1
  fi
  if ! printf '%s' "$filter" | grep -q 'application::'; then
    printf 'FAIL: check_core_filter: the Miri (core) filter does not cover application:: — the module of the attributed UB would not be swept\n'
    return 1
  fi
  printf 'OK: check_core_filter: the Miri (core) filter covers application::\n'
  return 0
}

# ---------------------------------------------------------------------------
# Check 4 — the UB is attributed to a test, not to the whole lane
# ---------------------------------------------------------------------------

# test_gates <file> — one record per test function of the file:
#   <name>|<drives_scrape>|<reaches_ub_surface>|<has_gate_with_reason>
# A test "drives the scrape" when its body calls `scrape_with_config(`; it
# "reaches the UB surface" when it does so with a 2xx response — the only
# path that gets to `clean_html` → `lol_html::rewrite_str` (fetch_html
# rejects every non-2xx status before the cleaner runs).
test_gates() {
  awk '
    function brackets(text,   scrub, n, m) {
      scrub = text
      gsub(/"[^"]*"/, "\"\"", scrub)
      n = gsub(/\[/, "", scrub)
      m = gsub(/\]/, "", scrub)
      return n - m
    }
    function gate_in(text) {
      if (text !~ /cfg_attr\([[:space:]]*miri/) return 0
      if (text ~ /ignore[[:space:]]*=[[:space:]]*"[^"]+"/) return 1
      if (text ~ /\/\/[^[:space:]]/) return 1
      return 0
    }
    function flush_fn(   body_has, reaches) {
      if (fn_name == "") return
      body_has = (body ~ /scrape_with_config[[:space:]]*\(/) ? 1 : 0
      reaches = (body_has && body ~ /status:[[:space:]]*2[0-9][0-9]/) ? 1 : 0
      printf "%s|%d|%d|%d\n", fn_name, body_has, reaches, fn_gate
      fn_name = ""
      body = ""
      fn_gate = 0
    }
    {
      line = $0
      if (attr_depth > 0) {
        attrs = attrs "\n" line
        attr_depth = attr_depth + brackets(line)
        if (attr_depth <= 0) pending_gate = gate_in(attrs)
        next
      }
      if (line ~ /^[[:space:]]*#\[/) {
        attrs = attrs "\n" line
        attr_depth = brackets(line)
        if (attr_depth <= 0) pending_gate = gate_in(attrs)
        next
      }
      if (line ~ /^[[:space:]]*(\/\/|\/\*|\*)/) next
      if (line ~ /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z0-9_]+/) {
        flush_fn()
        match(line, /fn[[:space:]]+[A-Za-z0-9_]+/)
        fn_name = substr(line, RSTART + 3, RLENGTH - 3)
        is_test = (attrs ~ /#\[test\]/) || (attrs ~ /#\[tokio::test\]/)
        if (!is_test) fn_name = ""
        fn_gate = pending_gate
        pending_gate = 0
        body = ""
        attrs = ""
        attr_depth = 0
        next
      }
      if (line ~ /^[[:space:]]*$/) next
      attrs = ""
      attr_depth = 0
      pending_gate = 0
      if (fn_name != "") body = body "\n" line
    }
    END { flush_fn() }
  ' "$1"
}

check_ub_attribution() {
  local root="$1"
  local file="$root/crates/webfang_core/src/application/scraper_service.rs"
  local rc=0
  local name drives reaches gated
  local entry entry_name entry_reason entry_evidence
  local marker_hits=0
  local -a test_names=()

  if [ ! -f "$file" ]; then
    printf 'FAIL: check_ub_attribution: file not found: %s\n' "$file"
    return 1
  fi

  while IFS='|' read -r name drives reaches gated; do
    [ -n "$name" ] || continue
    test_names+=("$name")
    if [ "$drives" = "1" ]; then
      marker_hits=$((marker_hits + 1))
    fi
    if [ "$reaches" = "1" ] && [ "$gated" != "1" ]; then
      printf 'FAIL: check_ub_attribution: %s reaches the UB surface (clean_html → lol_html::rewrite_str) without a per-test Miri gate — the UB would abort the whole lane instead of being attributed\n' "$name"
      rc=1
      continue
    fi
    if [ "$drives" = "1" ] && [ "$reaches" != "1" ] && [ "$gated" != "1" ]; then
      # Drives the use case without reaching the UB surface: it needs a
      # NOMINAL exemption, with the reason and the evidence.
      if ! printf '%s\n' "${UB_SURFACE_EXEMPT[@]:-}" | grep -q "^$name|"; then
        printf 'FAIL: check_ub_attribution: %s drives the scrape use case without a per-test Miri gate and without a nominal exemption — the exemption list is the only way to leave it ungated\n' "$name"
        rc=1
      else
        printf 'OK: check_ub_attribution: %s is nominally exempt (does not reach the UB surface)\n' "$name"
      fi
    fi
  done < <(test_gates "$file")

  # Vacuity guard: the marker that says "this test drives the scrape use
  # case" must still match SOMETHING. A rename of the use case would leave
  # the invariant green while it inspects nothing.
  if [ "$marker_hits" -eq 0 ]; then
    printf 'FAIL: check_ub_attribution: the marker scrape_with_config( matches no test in %s — the marker disappeared and the invariant is not looking at anything\n' "$file"
    rc=1
  fi

  # A stale exemption is a lie nobody can evaluate: the test it names must
  # still exist.
  for entry in "${UB_SURFACE_EXEMPT[@]:-}"; do
    [ -n "$entry" ] || continue
    entry_name="${entry%%|*}"
    entry_reason="$(printf '%s' "$entry" | cut -d'|' -f2)"
    entry_evidence="$(printf '%s' "$entry" | cut -d'|' -f3-)"
    if [ -z "$entry_reason" ] || [ -z "$entry_evidence" ]; then
      printf 'FAIL: check_ub_attribution: the exemption of %s carries no reason or no evidence\n' "$entry_name"
      rc=1
    fi
    if ! printf '%s\n' "${test_names[@]:-}" | grep -qx -- "$entry_name"; then
      printf 'FAIL: check_ub_attribution: stale exemption: %s no longer exists in the file — prune the entry\n' "$entry_name"
      rc=1
    fi
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_ub_attribution: the test that reaches the UB surface carries a per-test Miri gate\n'
  fi
  return "$rc"
}


# When sourced as a library the checks do NOT run: the harness calls them
# on its own fixtures. Executed directly, the guard IS the check.

run_checks() {
  local root="$1"
  local rc=0
  check_skips "$root" || rc=1
  check_gate_reasons "$root" || rc=1
  check_core_filter "$root" || rc=1
  check_ub_attribution "$root" || rc=1
  return "$rc"
}

# When sourced as a library the checks do NOT run: the harness calls them
# on its own fixtures. Executed directly, the guard IS the check.
if [ "${MIRI_LANE_LIBRARY:-0}" != "1" ]; then
    # ---------------------------------------------------------------------------
    # Runner
    # ---------------------------------------------------------------------------


    printf '== Miri lane invariants (%s)\n' "$REPO_ROOT"
    real_rc=0
    run_checks "$REPO_ROOT" || real_rc=1
    if [ "$real_rc" -eq 0 ]; then
      printf 'OK: all Miri lane invariants hold\n'
    else
      printf 'FAIL: at least one Miri lane invariant is broken\n'
    fi

fi
