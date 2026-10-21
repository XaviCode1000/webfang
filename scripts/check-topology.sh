#!/usr/bin/env bash
# Validates head-branch-prefix -> base-branch topology for the
# trunk-based + release/support-line model (see AGENTS.md, pending policy).
#
# Expects HEAD_REF and BASE_REF in the environment (set by the caller from
# the GitHub context). Nonzero exit = topology violation. The caller decides
# warn vs enforce (continue-on-error in warn-only mode).
#
# Versioned line names (audit B43): a support/release line is `support/X.Y`
# or `release/X.Y` with a NUMERIC X.Y. The `*` glob used to cross `/`, so
# `support/garbage` and `support/foo/bar` were accepted (and exempted) while
# the error text promised `support/X.Y`; a hotfix also accepted ANY `support/*`
# base. Both are refused now: a malformed line name is a typo that would
# create an unmanaged branch, and a hotfix must point at a real line.
set -euo pipefail

HEAD_REF="${HEAD_REF:?HEAD_REF sin definir}"
BASE_REF="${BASE_REF:?BASE_REF sin definir}"

# Unquoted on purpose: these are regexes, not globs (shellcheck SC2076).
SUPPORT_LINE_RE='^support/[0-9]+\.[0-9]+$'
RELEASE_LINE_RE='^release/[0-9]+\.[0-9]+$'
VERSIONED_LINE_RE='^(support|release)/[0-9]+\.[0-9]+$'

case "$HEAD_REF" in
  hotfix/*)
    [[ "$BASE_REF" =~ $SUPPORT_LINE_RE ]] || {
      echo "::error::hotfix/* debe apuntar a support/X.Y (X.Y numerico), no a $BASE_REF"; exit 1; }
    ;;
  fix/*)
    [[ "$BASE_REF" == "main" || "$BASE_REF" =~ $RELEASE_LINE_RE ]] || {
      echo "::error::fix/* debe apuntar a main o release/X.Y (X.Y numerico), no a $BASE_REF"; exit 1; }
    ;;
  feat/*|refactor/*|perf/*|docs/*|test/*|chore/*|style/*|build/*|ci/*|revert/*)
    [[ "$BASE_REF" == "main" ]] || {
      echo "::error::$HEAD_REF solo puede apuntar a main"; exit 1; }
    ;;
  support/*|release/*)
    # Stabilization/maintenance lines: topology is governed by their own
    # flows, not by this check — but the NAME is a requirement, not a glob.
    [[ "$HEAD_REF" =~ $VERSIONED_LINE_RE ]] || {
      echo "::error::rama $HEAD_REF malformada: las lineas versionadas son support/X.Y o release/X.Y (X.Y numerico)."; exit 1; }
    echo "tip: rama $HEAD_REF exenta de este check"
    ;;
  release-plz-*|dependabot/*|renovate/*)
    # Tooling branches: topology is governed by their own flows, not by this
    # check, and their names carry no version.
    echo "tip: rama $HEAD_REF exenta de este check"
    ;;
  *)
    echo "::error::prefijo de rama no reconocido: $HEAD_REF"; exit 1
    ;;
esac
echo "OK: topologia $HEAD_REF -> $BASE_REF."
