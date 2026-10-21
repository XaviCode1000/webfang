#!/usr/bin/env bash
#
# ci_metrics.sh — Phase 6 read-only CI observability snapshot.
#
# Design source: docs/research/webfang-workflow-transformation-blueprint.md
# Phase 6 "Observability and SLOs" (prove the workflow is faster and safer).
#
# Usage:
#   scripts/ci_metrics.sh [--days N] [--workflow ci.yml] [--branch main]
#   scripts/ci_metrics.sh -h | --help
#
# Behaviour: read-only. Fetches up to 200 runs of the given workflow on the
# given branch with ONE `gh run list --json` call (all needed fields come
# back in bulk — never one API call per run, which stalls on slow networks),
# keeps runs whose createdAt is within the last N days, and prints a markdown SLO snapshot to
# stdout: required checks (list + count), run stats (count, pass rate,
# median/p90 wall-clock minutes, breakdown by event), and a comparison table
# against the SLO targets in docs/ci-slo.md (PASS/WARN when data suffices,
# `unknown` otherwise — numbers are never invented).
#
# Two fail-closed rules the snapshot obeys:
#   - the required contexts are read from BOTH protection shapes
#     (`checks[].context` and the deprecated `contexts[]`), so a modern
#     checks[]-only protection is never reported as "no required contexts";
#   - every listed run is accounted for: a run whose payload cannot be parsed
#     is COUNTED and the snapshot says the sample is INCOMPLETE, instead of
#     silently shrinking the sample and printing it as a complete picture.
#
# Exit codes:
#   0  Snapshot printed (even when some data is unavailable -> `unknown`).
#   2  Usage error, gh/jq missing, repo identity unreadable, or the branch
#      protection (required checks) API call failed. The protection read has
#      no fallback: fabricating required checks would be worse than failing.
#
# Scope notes:
#   - Read-only: only `gh run list`, `gh repo view`, and
#     `gh api .../protection` queries. Never triggers, cancels, merges, or
#     mutates anything. No CI gating or required-check changes.
#   - Wall-clock per run is updatedAt minus createdAt (queue + execution).
#   - Pass rate = success / runs with a terminal conclusion (in-flight runs
#     with an empty conclusion are excluded from both numerator and
#     denominator, but still count in the event breakdown).
#   - Median/p90 use the nearest-rank method on sorted durations (median of
#     an even sample averages the two middle values); p90 rank is
#     ceil(0.9*n), minimum rank 1.
#   - Needs `gh` (authenticated for private repos / high rate limits) and
#     `jq`. All run data arrives in a single bulk `gh run list --json` call
#     (200-run cap), so runtime is bounded by ~3 API calls total.
set -euo pipefail

DAYS=14
WORKFLOW="ci.yml"
BRANCH="main"

usage() {
  sed -n '2,32p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --days) DAYS="${2:?missing value for --days}"; shift 2 ;;
    --workflow) WORKFLOW="${2:?missing value for --workflow}"; shift 2 ;;
    --branch) BRANCH="${2:?missing value for --branch}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if ! [[ "$DAYS" =~ ^[0-9]+$ ]] || [[ "$DAYS" -lt 1 ]]; then
  echo "error: --days must be a positive integer (got '$DAYS')" >&2
  exit 2
fi
if [[ -z "$WORKFLOW" ]]; then
  echo "error: --workflow must not be empty" >&2
  exit 2
fi
if [[ -z "$BRANCH" ]]; then
  echo "error: --branch must not be empty" >&2
  exit 2
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI is not installed or not on PATH" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is not installed or not on PATH" >&2
  exit 2
fi

# --- required checks (fail-closed: no fabrication fallback) -------------------
REPO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || true)"
if [[ -z "$REPO" ]]; then
  echo "error: could not determine owner/repo (gh repo view failed — check auth/network)" >&2
  exit 2
fi

PROTECTION_JSON="$(gh api "repos/${REPO}/branches/${BRANCH}/protection" 2>/dev/null || true)"
if [[ -z "$PROTECTION_JSON" ]]; then
  echo "error: could not read branch protection for ${REPO}@${BRANCH} (gh api failed — check auth/permissions)" >&2
  exit 2
fi

# Modern branch protections expose the required contexts as `checks[].context`;
# the deprecated `contexts[]` still exists on older ones. Reading ONLY the
# deprecated shape reported 0 required contexts for a checks[]-only protection
# and the snapshot claimed "no required contexts" while exiting 0 (B54). Both
# shapes are read, exactly like merge-when-green.sh does.
REQUIRED_CHECKS="$(printf '%s' "$PROTECTION_JSON" | jq -r '.required_status_checks.checks[]?.context // empty' 2>/dev/null || true)"
if [[ -z "$REQUIRED_CHECKS" ]]; then
  REQUIRED_CHECKS="$(printf '%s' "$PROTECTION_JSON" | jq -r '.required_status_checks.contexts[]? // empty' 2>/dev/null || true)"
fi
REQUIRED_COUNT="$(printf '%s' "$REQUIRED_CHECKS" | grep -c . || true)"
# grep -c exits 1 on empty input; the `|| true` above keeps `set -e` quiet and
# REQUIRED_COUNT is then empty -> normalise to 0.
if [[ -z "$REQUIRED_COUNT" ]]; then
  REQUIRED_COUNT=0
fi

# --- workflow runs (fail-open: missing data becomes `unknown`) ----------------
NOW_EPOCH="$(date -u +%s)"
CUTOFF_EPOCH=$((NOW_EPOCH - DAYS * 86400))

RUN_LIST="$(gh run list --workflow "$WORKFLOW" --branch "$BRANCH" --limit 200 --json databaseId,conclusion,createdAt,updatedAt,headBranch,event 2>/dev/null || true)"

TMPDIR_METRICS="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_METRICS"' EXIT
DURATIONS_FILE="$TMPDIR_METRICS/durations.txt"
EVENTS_FILE="$TMPDIR_METRICS/events.txt"
: > "$DURATIONS_FILE"
: > "$EVENTS_FILE"

RUNS_IN_WINDOW=0
DECIDED_RUNS=0
PASSED_RUNS=0
RUNS_OK=false
# Every listed run must be accounted for: analyzed, or counted as unparsable.
RUNS_LISTED=0
RUNS_UNPARSABLE=0

to_epoch() {
  # GNU date first, BSD date second (macOS has no `-d`). A parse failure stays
  # a parse failure: it is counted as unparsable, never mistaken for "no data".
  local ts="$1" out=""
  out="$(date -u -d "$ts" +%s 2>/dev/null || true)"
  if [[ -n "$out" ]]; then printf '%s' "$out"; return 0; fi
  out="$(date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$ts" +%s 2>/dev/null || true)"
  if [[ -n "$out" ]]; then printf '%s' "$out"; return 0; fi
  return 1
}

# analyze_runs <entries> — one bulk pass over the already-fetched runs: no
# per-run API calls. Every listed run is accounted for: either analyzed, or
# counted as unparsable. A run that silently vanishes shrinks the sample while
# the snapshot still reads as a complete picture of the window (B55).
analyze_runs() {
  local entries="$1"
  local run_json detail item_fields created updated conclusion event
  local created_epoch updated_epoch duration_secs
  RUNS_LISTED="$(printf '%s\n' "$entries" | grep -c . || true)"
  [[ -n "$RUNS_LISTED" ]] || RUNS_LISTED=0
  while IFS= read -r run_json; do
    [[ -z "$run_json" ]] && continue
    detail="$run_json"
    # One jq call per run; the four fields are joined with the unit separator
    # so an empty value keeps its position. A jq failure is COUNTED, never
    # silently skipped.
    if ! item_fields="$(printf '%s' "$detail" | jq -r '[ (.createdAt // ""), (.updatedAt // ""), (.conclusion // ""), (.event // "") ] | join("\u001f")' 2>/dev/null)"; then
      RUNS_UNPARSABLE=$((RUNS_UNPARSABLE + 1))
      continue
    fi
    IFS=$'\x1f' read -r created updated conclusion event <<<"$item_fields" || true
    if [[ -z "$created" || -z "$updated" ]]; then
      RUNS_UNPARSABLE=$((RUNS_UNPARSABLE + 1))
      continue
    fi
    created_epoch="$(to_epoch "$created" || true)"
    updated_epoch="$(to_epoch "$updated" || true)"
    if [[ -z "$created_epoch" || -z "$updated_epoch" ]]; then
      RUNS_UNPARSABLE=$((RUNS_UNPARSABLE + 1))
      continue
    fi
    if [[ "$created_epoch" -lt "$CUTOFF_EPOCH" ]]; then
      continue
    fi
    RUNS_IN_WINDOW=$((RUNS_IN_WINDOW + 1))
    if [[ -n "$event" ]]; then
      printf '%s\n' "$event" >>"$EVENTS_FILE"
    fi
    case "$conclusion" in
      "" | in_progress | queued | waiting | pending | requested)
        # In-flight: visible in the event breakdown, excluded from pass
        # rate (neither numerator nor denominator).
        ;;
      success)
        DECIDED_RUNS=$((DECIDED_RUNS + 1))
        PASSED_RUNS=$((PASSED_RUNS + 1))
        ;;
      *)
        DECIDED_RUNS=$((DECIDED_RUNS + 1))
        ;;
    esac
    duration_secs=$((updated_epoch - created_epoch))
    if [[ "$duration_secs" -ge 0 ]]; then
      printf '%s\n' "$duration_secs" >>"$DURATIONS_FILE"
    fi
  done <<<"$entries"
}

if [[ -n "$RUN_LIST" ]] && printf '%s' "$RUN_LIST" | jq -e 'type == "array"' >/dev/null 2>&1; then
  # The bulk extraction is checked too: a failed jq means we know NOTHING
  # about the sample, which is not the same as "the window held no runs".
  entries=""
  bulk_ok=true
  if ! entries="$(printf '%s' "$RUN_LIST" | jq -c '.[:200][]' 2>/dev/null)"; then
    bulk_ok=false
  fi
  if [[ "$bulk_ok" == "true" ]]; then
    RUNS_OK=true
    analyze_runs "$entries"
  else
    echo "warning: could not parse the run list returned by gh; the sample is unknown." >&2
  fi
fi

PASS_RATE="unknown"
if [[ "$DECIDED_RUNS" -gt 0 ]]; then
  PASS_RATE="$(awk -v p="$PASSED_RUNS" -v d="$DECIDED_RUNS" 'BEGIN { printf "%.1f", (p / d) * 100 }')"
fi

MEDIAN_MIN="unknown"
P90_MIN="unknown"
DURATION_COUNT="$(grep -c . "$DURATIONS_FILE" || true)"
if [[ -z "$DURATION_COUNT" ]]; then
  DURATION_COUNT=0
fi
if [[ "$DURATION_COUNT" -gt 0 ]]; then
  # NOTE: the awk format ends with \n on purpose — `read` exits non-zero on
  # EOF without a trailing newline, which would trip `set -e` even though the
  # variables were assigned. The `|| true` is belt-and-braces for the same
  # reason (vars keep their pre-set "unknown" on a truly empty read).
  read -r MEDIAN_MIN P90_MIN < <(sort -n "$DURATIONS_FILE" | awk '
    { vals[NR] = $1 }
    END {
      n = NR
      if (n % 2 == 1) {
        median = vals[(n + 1) / 2]
      } else {
        median = (vals[n / 2] + vals[n / 2 + 1]) / 2
      }
      rank = int(0.9 * n + 0.999999)
      if (rank < 1) { rank = 1 }
      if (rank > n) { rank = n }
      printf "%.2f %.2f\n", median / 60, vals[rank] / 60
    }') || true
fi

slo_status() {
  local target="$1" observed="$2"
  if [[ "$observed" == "unknown" ]]; then
    printf 'unknown'
    return 0
  fi
  if awk -v o="$observed" -v t="$target" 'BEGIN { exit !(o <= t) }'; then
    printf 'PASS'
  else
    printf 'WARN'
  fi
}

if [[ "$P90_MIN" == "unknown" ]]; then
  TIER0_STATUS="unknown" TIER1_STATUS="unknown" TIER2_STATUS="unknown" DOCS_STATUS="unknown"
else
  TIER0_STATUS="$(slo_status 2 "$P90_MIN")"
  TIER1_STATUS="$(slo_status 10 "$P90_MIN")"
  TIER2_STATUS="$(slo_status 30 "$P90_MIN")"
  DOCS_STATUS="$(slo_status 3 "$P90_MIN")"
fi

GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# --- markdown snapshot --------------------------------------------------------
echo "# CI SLO snapshot"
echo ""
echo "- generated: ${GENERATED_AT} (UTC)"
echo "- repo: ${REPO}"
echo "- workflow: ${WORKFLOW} branch: ${BRANCH} window: last ${DAYS} days (up to 200 runs)"
echo ""
echo "## Required checks (${REQUIRED_COUNT})"
echo ""
if [[ "$REQUIRED_COUNT" -gt 0 ]]; then
  printf '%s\n' "$REQUIRED_CHECKS" | sed 's/^/- /'
else
  echo "- (no required contexts reported — verify branch protection if this surprises you)"
fi
echo ""
echo "## Run stats"
echo ""
if [[ "$RUNS_OK" == "true" ]]; then
  if [[ "$RUNS_UNPARSABLE" -gt 0 ]]; then
    # The sample is PARTIAL and says so on the count line itself: a bare
    # "runs in window: N" would read as a complete picture of the window.
    echo "- runs in window: ${RUNS_IN_WINDOW} (${RUNS_UNPARSABLE} of ${RUNS_LISTED} listed runs could not be parsed — sample INCOMPLETE)"
    echo "- warning: the stats below cover a PARTIAL sample; ${RUNS_UNPARSABLE} listed run(s) were unreadable and are NOT counted."
  else
    echo "- runs in window: ${RUNS_IN_WINDOW}"
  fi
  if [[ "$DECIDED_RUNS" -gt 0 ]]; then
    echo "- pass rate: ${PASS_RATE}% (${PASSED_RUNS}/${DECIDED_RUNS} decided runs)"
  else
    echo "- pass rate: unknown (no decided runs in window)"
  fi
  if [[ "$MEDIAN_MIN" != "unknown" ]]; then
    echo "- wall-clock median: ${MEDIAN_MIN} min (n=${DURATION_COUNT})"
    echo "- wall-clock p90: ${P90_MIN} min (n=${DURATION_COUNT})"
  else
    echo "- wall-clock median: unknown (no parsable durations)"
    echo "- wall-clock p90: unknown (no parsable durations)"
  fi
  echo ""
  echo "### By event"
  echo ""
  echo "| event | runs |"
  echo "| --- | --- |"
  if [[ -s "$EVENTS_FILE" ]]; then
    sort "$EVENTS_FILE" | uniq -c | sort -rn | awk '{ printf "| %s | %d |\n", $2, $1 }'
  else
    echo "| (no events) | 0 |"
  fi
else
  echo "- runs in window: unknown (gh run list failed, returned no data, or could not be parsed)"
  echo "- pass rate: unknown"
  echo "- wall-clock median: unknown"
  echo "- wall-clock p90: unknown"
  echo ""
  echo "### By event"
  echo ""
  echo "| event | runs |"
  echo "| --- | --- |"
  echo "| unknown | 0 |"
fi
echo ""
echo "## SLO comparison"
echo ""
echo "Observed p90 below is the workflow wall-clock p90 (queue + execution)"
echo "from the window above — an upper-bound proxy for the tier gates, which"
echo "are not isolated per-lane from gh run data. See docs/ci-slo.md for"
echo "what each breach triggers."
echo ""
echo "| SLO | target p90 | observed p90 | status |"
echo "| --- | --- | --- | --- |"
if [[ "$P90_MIN" != "unknown" ]]; then
  echo "| Tier 0 (static metadata) | 2 min | ${P90_MIN} min | ${TIER0_STATUS} |"
  echo "| Tier 1 (fast correctness gate) | 10 min | ${P90_MIN} min | ${TIER1_STATUS} |"
  echo "| Tier 2 (main integration) | 30 min | ${P90_MIN} min | ${TIER2_STATUS} |"
  echo "| Docs-only | 3 min | ${P90_MIN} min | ${DOCS_STATUS} |"
else
  echo "| Tier 0 (static metadata) | 2 min | unknown | unknown |"
  echo "| Tier 1 (fast correctness gate) | 10 min | unknown | unknown |"
  echo "| Tier 2 (main integration) | 30 min | unknown | unknown |"
  echo "| Docs-only | 3 min | unknown | unknown |"
fi
echo "| AI path | accepted separately | n/a | unknown |"
echo ""
echo "## Regenerate"
echo ""
echo "\`\`\`bash"
echo "bash scripts/ci_metrics.sh --days ${DAYS} --workflow ${WORKFLOW} --branch ${BRANCH}"
echo "\`\`\`"
echo ""
echo "Local fast-gate timings: docs/ci-metrics/fast-gate.log (see docs/ci-slo.md)."
exit 0
