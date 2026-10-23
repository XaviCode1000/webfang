#!/usr/bin/env bash
set -euo pipefail
#
# fake_gh for test_ci_queue_metrics.sh — serves fixture payloads instead of
# the network. Dispatches on the gh subcommand:
#   repo view                       -> the fixture repo identity
#   run list --json ...             -> fixture runs.json
#   api repos/<repo>/actions/runs/N/jobs -> fixture jobs-N.json (per run id)
set -euo pipefail

FAKE_GH_DIR="${FAKE_GH_DIR:?FAKE_GH_DIR must point at the fixture directory}"

case "${1:-}" in
  repo)
    # `gh repo view --json nameWithOwner`
    cat "$FAKE_GH_DIR/repo.json"
    exit 0
    ;;
  run)
    # `gh run list --workflow ... --branch ... --limit N --json ...`
    cat "$FAKE_GH_DIR/runs.json"
    exit 0
    ;;
  api)
    # `gh api repos/<repo>/actions/runs/<id>/jobs?per_page=100`
    # $2 is the path ".../actions/runs/<id>/jobs?...";
    # strip the query, drop the trailing /jobs, then take the basename.
    run_id="${2%%\?*}"
    run_id="${run_id%/jobs}"
    run_id="${run_id##*/}"
    fixture="$FAKE_GH_DIR/jobs-$run_id.json"
    if [ -f "$fixture" ]; then
      cat "$fixture"
    else
      echo "error: fake gh has no fixture for run $run_id" >&2
      exit 1
    fi
    exit 0
    ;;
  *)
    echo "error: fake gh: unsupported subcommand '${1:-}'" >&2
    exit 2
    ;;
esac
