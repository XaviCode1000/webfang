#!/usr/bin/env bash
# Validates head-branch-prefix -> base-branch topology for the
# trunk-based + release/support-line model (see AGENTS.md, pending policy).
#
# Expects HEAD_REF and BASE_REF in the environment (set by the caller from
# the GitHub context). Nonzero exit = topology violation. The caller decides
# warn vs enforce (continue-on-error in warn-only mode).
set -euo pipefail

HEAD_REF="${HEAD_REF:?HEAD_REF sin definir}"
BASE_REF="${BASE_REF:?BASE_REF sin definir}"

case "$HEAD_REF" in
  hotfix/*)
    [[ "$BASE_REF" == support/* ]] || {
      echo "::error::hotfix/* debe apuntar a support/X.Y, no a $BASE_REF"; exit 1; }
    ;;
  fix/*)
    [[ "$BASE_REF" == "main" || "$BASE_REF" == release/* ]] || {
      echo "::error::fix/* debe apuntar a main o release/X.Y, no a $BASE_REF"; exit 1; }
    ;;
  feat/*|refactor/*|perf/*|docs/*|test/*|chore/*|style/*|build/*|ci/*|revert/*)
    [[ "$BASE_REF" == "main" ]] || {
      echo "::error::$HEAD_REF solo puede apuntar a main"; exit 1; }
    ;;
  release/*|support/*|release-plz-*|dependabot/*|renovate/*)
    # Stabilization/maintenance lines and tooling branches: topology is
    # governed by their own flows, not by this check.
    echo "tip: rama $HEAD_REF exenta de este check"
    ;;
  *)
    echo "::error::prefijo de rama no reconocido: $HEAD_REF"; exit 1
    ;;
esac
echo "OK: topologia $HEAD_REF -> $BASE_REF."
