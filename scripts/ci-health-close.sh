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
close_mine() {
  local title="$1" run_url="$2"
  local label="${LABEL:-ci-health}"
  local number
  gh issue list --state open --search "${title} in:title" --json number --jq '.[].number' |
    while read -r number; do
      [ -n "$number" ] || continue
      if gh issue view "$number" --json labels --jq '.labels[].name' | grep -qx "$label"; then
        gh issue close "$number" --comment "Green again: $run_url"
      else
        echo "Skip #$number: missing label $label"
      fi
    done
}
