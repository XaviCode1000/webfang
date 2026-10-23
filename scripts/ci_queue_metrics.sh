#!/usr/bin/env bash
set -euo pipefail
#
# ci_queue_metrics.sh — #2284 queue-wait metrics (read-only).
#
# Measures how long work waits BEFORE it starts running, the number the
# queue-tuning knobs (#2284: minimumEntriesToMerge, waitTime,
# checkResponseTimeout, maximumEntriesToBuild) are supposed to move:
#
#   - workflow queue wait: run.startedAt - run.createdAt (one bulk call);
#   - job queue wait: job.started_at - job.created_at (per-run jobs API,
#     bounded to the most recent --jobs-runs completed runs);
#   - macOS lane specifically: job queue wait for jobs whose runs-on lands
#     on a macos-* hosted runner (the lane with the historically longest
#     hosted-queue waits);
#   - p50/p95 by nearest-rank on sorted samples, exactly like ci_metrics.sh.
#
# Output: a markdown table block on stdout (meant for $GITHUB_STEP_SUMMARY
# in the ci-health workflow) plus a one-line machine-readable summary.
#
# Fail-closed rules:
#   - every fetched run/jobs payload that cannot be parsed is COUNTED as
#     unparseable and reported, never silently dropped;
#   - zero valid samples prints `unknown`, never an invented number;
#   - a missing `startedAt` (run not yet started) excludes the run from the
#     queue-wait sample and is reported as in-flight.
#
# Exit codes:
#   0  metrics printed (unavailable pieces -> `unknown`)
#   2  usage error, gh/jq missing, repo identity unreadable
#
# Scope: read-only (gh run list + gh api jobs). Never mutates anything.
set -euo pipefail

DAYS=14
WORKFLOW="ci.yml"
BRANCH="main"
JOBS_RUNS=50

usage() {
  sed -n '2,32p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --days) DAYS="${2:?missing value for --days}"; shift 2 ;;
    --workflow) WORKFLOW="${2:?missing value for --workflow}"; shift 2 ;;
    --branch) BRANCH="${2:?missing value for --branch}"; shift 2 ;;
    --jobs-runs) JOBS_RUNS="${2:?missing value for --jobs-runs}"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

if ! [[ "$DAYS" =~ ^[0-9]+$ ]] || [[ "$DAYS" -lt 1 ]]; then
  echo "error: --days must be a positive integer (got '$DAYS')" >&2
  exit 2
fi
if ! [[ "$JOBS_RUNS" =~ ^[0-9]+$ ]] || [[ "$JOBS_RUNS" -lt 1 ]]; then
  echo "error: --jobs-runs must be a positive integer (got '$JOBS_RUNS')" >&2
  exit 2
fi

for tool in gh jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "error: $tool is required but not installed" >&2
    exit 2
  }
done

REPO="$(gh repo view --json nameWithOwner --jq '.nameWithOwner' 2>/dev/null || true)"
if [[ -z "$REPO" ]]; then
  echo "error: could not determine owner/repo (gh repo view failed)" >&2
  exit 2
fi

# --- helpers -----------------------------------------------------------------

# nearest-rank percentile over a sorted (ascending, newline-separated) list
# of integer seconds; percentile rank = ceil(p*n), minimum 1 (ci_metrics.sh
# precedent). p is 50 or 95.
percentile() {
  local p="$1"
  # -c (not a heredoc): a heredoc would replace the function's stdin, and the
  # samples arrive exactly through stdin.
  python3 -c '
import math, sys
p = float(sys.argv[1])
vals = [int(x) for x in sys.stdin.read().split() if x.strip()]
if not vals:
    print("unknown")
else:
    vals.sort()
    rank = max(1, math.ceil(p * len(vals) / 100))
    print(vals[rank - 1])
' "$p"
}

mins() {
  local secs="$1"
  if [[ "$secs" == "unknown" ]]; then
    echo "unknown"
  else
    python3 -c "print(f'{$secs/60:.1f}')"
  fi
}

epoch_of() {
  # ISO-8601 -> epoch seconds; prints nothing (caller sees empty) on garbage.
  python3 -c "
import sys, datetime
try:
    dt = datetime.datetime.fromisoformat(sys.argv[1].replace('Z', '+00:00'))
    print(int(dt.timestamp()))
except Exception:
    pass
" "$1" 2>/dev/null || true
}

# --- run-level queue wait (bulk) ---------------------------------------------
RUNS_JSON="$(gh run list --workflow "$WORKFLOW" --branch "$BRANCH" \
  --limit 200 --json databaseId,createdAt,startedAt,event,conclusion 2>/dev/null || true)"

CUTOFF="$(python3 -c "import time; print(int(time.time()) - $DAYS*86400)")"

W_UNPARSEABLE=0
W_INFLIGHT=0
W_OUTSIDE=0
W_SAMPLES=""
W_EVENTS='{}'

if [[ -n "$RUNS_JSON" ]] && echo "$RUNS_JSON" | jq -e 'type == "array"' >/dev/null 2>&1; then
  while IFS=$'\x1f' read -r created started event; do
    CE="$(epoch_of "$created")"
    if [[ -z "$CE" ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    if [[ "$CE" -lt "$CUTOFF" ]]; then
      W_OUTSIDE=$((W_OUTSIDE + 1))
      continue
    fi
    W_EVENTS="$(jq -c --arg e "${event:-unknown}" '.[$e] = ((.[$e] // 0) + 1)' <<<"$W_EVENTS")"
    if [[ -z "$started" || "$started" == "null" ]]; then
      W_INFLIGHT=$((W_INFLIGHT + 1))
      continue
    fi
    SE="$(epoch_of "$started")"
    if [[ -z "$SE" ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    WAIT=$((SE - CE))
    if [[ "$WAIT" -lt 0 ]]; then
      W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
      continue
    fi
    W_SAMPLES+="$WAIT"$'\n'
  done < <(echo "$RUNS_JSON" | jq -r '.[] | [(.createdAt // ""), (.startedAt // ""), (.event // "")] | join("\u001f")')
else
  W_UNPARSEABLE=$((W_UNPARSEABLE + 1))
fi

# --- job-level queue wait (bounded per-run calls) ----------------------------
JOB_IDS="$(echo "$RUNS_JSON" | jq -r 'type == "array" and length > 0 | "ok"' >/dev/null 2>&1 \
  && echo "$RUNS_JSON" | jq -r '[.[] | select(.conclusion != null and .conclusion != "" and .conclusion != "skipped")] | sort_by(.databaseId) | reverse | .[:'"$JOBS_RUNS"'][] | .databaseId' 2>/dev/null || true)"

J_UNPARSEABLE=0
J_SAMPLES=""
J_MACOS_SAMPLES=""
J_RUNS_FETCHED=0
J_RUNS_FAILED=0

if [[ -n "$JOB_IDS" ]]; then
  while IFS= read -r run_id; do
    [[ -n "$run_id" ]] || continue
    JOBS_JSON="$(gh api "repos/$REPO/actions/runs/$run_id/jobs?per_page=100" 2>/dev/null || true)"
    if [[ -z "$JOBS_JSON" ]] || ! echo "$JOBS_JSON" | jq -e '.jobs | type == "array"' >/dev/null 2>&1; then
      J_RUNS_FAILED=$((J_RUNS_FAILED + 1))
      continue
    fi
    J_RUNS_FETCHED=$((J_RUNS_FETCHED + 1))
    # shellcheck disable=SC2034  # runner_name kept positional for clarity
    while IFS=$'\x1f' read -r created started _runner labels; do
      CE="$(epoch_of "$created")"
      SE="$(epoch_of "$started")"
      if [[ -z "$CE" || -z "$SE" || -z "$started" || "$started" == "null" ]]; then
        J_UNPARSEABLE=$((J_UNPARSEABLE + 1))
        continue
      fi
      WAIT=$((SE - CE))
      if [[ "$WAIT" -lt 0 ]]; then
        J_UNPARSEABLE=$((J_UNPARSEABLE + 1))
        continue
      fi
      J_SAMPLES+="$WAIT"$'\n'
      if grep -qiE 'macos' <<<"$labels"; then
        J_MACOS_SAMPLES+="$WAIT"$'\n'
      fi
    done < <(echo "$JOBS_JSON" | jq -r \
      '.jobs[] | [(.created_at // ""), (.started_at // ""), (.runner_name // ""), ([.labels[]?] | join(","))] | join("\u001f")')
  done <<<"$JOB_IDS"
fi

# --- report ------------------------------------------------------------------
W_P50="$(percentile 50 <<<"$W_SAMPLES")"
W_P95="$(percentile 95 <<<"$W_SAMPLES")"
J_P50="$(percentile 50 <<<"$J_SAMPLES")"
J_P95="$(percentile 95 <<<"$J_SAMPLES")"
M_P50="$(percentile 50 <<<"$J_MACOS_SAMPLES")"
M_P95="$(percentile 95 <<<"$J_MACOS_SAMPLES")"

W_N="$(grep -c . <<<"$W_SAMPLES" 2>/dev/null || true)"
J_N="$(grep -c . <<<"$J_SAMPLES" 2>/dev/null || true)"
M_N="$(grep -c . <<<"$J_MACOS_SAMPLES" 2>/dev/null || true)"

cat <<EOF
## Queue-wait metrics — \`$WORKFLOW\` on \`$BRANCH\` (last $DAYS days)

| Metric | p50 | p95 | n |
|---|---|---|---|
| Workflow queue wait (startedAt − createdAt) | $(mins "$W_P50") min | $(mins "$W_P95") min | $W_N |
| Job queue wait (started_at − created_at) | $(mins "$J_P50") min | $(mins "$J_P95") min | $J_N |
| macOS job queue wait | $(mins "$M_P50") min | $(mins "$M_P95") min | $M_N |

- Sample accounting: runs outside window $W_OUTSIDE, in-flight (no startedAt) $W_INFLIGHT, unparseable run rows $W_UNPARSEABLE; jobs fetched from $J_RUNS_FETCHED/$JOBS_RUNS completed runs, jobs API failures $J_RUNS_FAILED, unparseable job rows $J_UNPARSEABLE.
- Events seen: $W_EVENTS
- p50/p95 use nearest-rank on integer seconds; "unknown" means zero valid samples (never invented).
EOF

echo "queue-metrics: workflow p50=$(mins "$W_P50")m p95=$(mins "$W_P95")m (n=$W_N); job p50=$(mins "$J_P50")m p95=$(mins "$J_P95")m (n=$J_N); macos p50=$(mins "$M_P50")m p95=$(mins "$M_P95")m (n=$M_N)"
