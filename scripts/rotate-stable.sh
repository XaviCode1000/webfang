#!/usr/bin/env bash
# The rotation event: invoked when vX.(Y+1).0 ships. Demotes the previous
# STABLE to MAINTENANCE (security-only), EOLs the previous MAINTENANCE via
# eol-line.sh (branch deletion included), and registers the new STABLE.
# Idempotent per argument: re-running with the same version aborts instead
# of duplicating entries (governance scripts must be re-runnable without
# damage — agents re-invoke on state confusion).
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

NEW_MINOR="${1:?usage: rotate-stable.sh X.Y (recién publicada)}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }
NOW="$(date -Iseconds)"

EXISTS="$(jq -r --arg m "$NEW_MINOR" '.lines[] | select(.minor==$m) | .minor // empty' "$SUPPORT_JSON")"
[[ -z "$EXISTS" ]] || { echo "línea $NEW_MINOR ya registrada — no reintentar con el mismo argumento" >&2; exit 1; }

CURRENT_MAINTENANCE="$(jq -r '.lines[] | select(.state=="MAINTENANCE") | .minor // empty' "$SUPPORT_JSON")"
if [[ -n "$CURRENT_MAINTENANCE" ]]; then
  # shellcheck disable=SC2086
  for m in $CURRENT_MAINTENANCE; do ./scripts/eol-line.sh "$m"; done
fi

jq --arg new "$NEW_MINOR" --arg at "$NOW" '
  (.lines[] | select(.state=="STABLE")) |=
    (.state = "MAINTENANCE" | .demoted_from_stable_at = $at | .accepts = ["security"])
  | .lines += [{
      "minor": $new, "state": "STABLE", "branch": null,
      "latest": ($new + ".0"), "released_at": $at,
      "accepts": ["bugfix", "security"]
    }]
' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "OK: $NEW_MINOR es STABLE; anterior STABLE a MAINTENANCE."
