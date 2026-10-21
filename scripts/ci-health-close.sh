#!/usr/bin/env bash
#
# Shared close helper for the CI Health Observer (.github/workflows/ci-health.yml).
#
# Source-only library: both the upsert step (event path) and the reconcile step
# (schedule safety net) source this file and call close_mine. It owns the
# close_mine semantics — close exactly the open tracking issues that carry the
# tracking label, skip visibly anything else with a matching title, and treat
# an empty set as a silent no-op.
#
# Usage from a workflow step (LABEL comes from the workflow env):
#   . "$GITHUB_WORKSPACE/scripts/ci-health-close.sh"
#   close_mine "$TITLE_CI" "$RUN_URL"
#
# Requires `gh` on PATH. Reads LABEL from the environment (defaults to
# ci-health when unset, e.g. in the offline harness).
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  echo "usage: source scripts/ci-health-close.sh, then call close_mine <title> <run-url>" >&2
  exit 2
fi

# Close every open issue with a matching title that carries the tracking
# label. Unlabeled lookalikes are reported and left open; with no matches the
# loop body never runs and the function exits zero.
#
# A FAILED read is not an answer (B50): `gh issue view` that dies (network,
# auth, API error) used to make the label test false, which printed
# "Skip #N: missing label" and left the function exiting 0 — the stale issue
# stayed open and the reconcile step reported success. The error now propagates:
# the function returns non-zero and the caller (a workflow step running under
# `set -e`) fails instead of pretending the issue was inspected.
close_mine() {
  local title="$1" run_url="$2"
  local label="${LABEL:-ci-health}"
  local number labels
  local failed=0
  local numbers

  # The list read is a read too: a failure here is not "no issues".
  if ! numbers="$(gh issue list --state open --search "${title} in:title" --json number --jq '.[].number')"; then
    echo "error: could not list the open issues matching '${title}'." >&2
    return 1
  fi

  # A herestring (not a pipeline) keeps the loop in THIS shell, so the failure
  # flag below is visible to the return value.
  while read -r number; do
    [ -n "$number" ] || continue
    if ! labels="$(gh issue view "$number" --json labels --jq '.labels[].name')"; then
      # Unreadable labels: we do NOT know whether the issue carries the label,
      # so it must not be closed and it must not be reported as "missing".
      echo "error: could not read the labels of issue #${number}." >&2
      failed=1
      continue
    fi
    if printf '%s\n' "$labels" | grep -qx "$label"; then
      if ! gh issue close "$number" --comment "Green again: $run_url"; then
        echo "error: could not close issue #${number}." >&2
        failed=1
      fi
    else
      echo "Skip #$number: missing label $label"
    fi
  done <<<"$numbers"

  [[ $failed -eq 0 ]]
}
