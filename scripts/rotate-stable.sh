#!/usr/bin/env bash
# The rotation event: invoked when vX.(Y+1).0 ships. Demotes the previous
# STABLE to MAINTENANCE (security-only), EOLs the previous MAINTENANCE via
# eol-line.sh (branch deletion included), and registers the new STABLE at the
# HEAD of the list — newest first, so the rendered SUPPORT.md table keeps the
# descending order of the file it derives from (appending put the current
# release last in the published support table).
# Idempotent per argument: re-running with the same version aborts instead
# of duplicating entries (governance scripts must be re-runnable without
# damage — agents re-invoke on state confusion).
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
#
# FAIL CLOSED (#1676)
# This script only knows the lines support.json already carries. Rotating over
# a gap — minors already published but never registered — silently produced a
# file that declared a stale line MAINTENANCE (it should have been EOL) and
# vanished the intermediate minors, i.e. a WORSE file, with no signal. So the
# rotation now refuses when it cannot see what it is rotating over, when the
# minor was never published, or when there is no STABLE to demote. Refusing is
# the correct outcome: closing a gap is a governance decision (approved issue),
# not a script run.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

NEW_MINOR="${1:?usage: rotate-stable.sh X.Y (recién publicada)}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }
[[ "$NEW_MINOR" =~ ^[0-9]+\.[0-9]+$ ]] || { echo "línea inválida '$NEW_MINOR': se espera X.Y" >&2; exit 1; }
NOW="$(date -Iseconds)"

EXISTS="$(jq -r --arg m "$NEW_MINOR" '.lines[] | select(.minor==$m) | .minor // empty' "$SUPPORT_JSON")"
[[ -z "$EXISTS" ]] || { echo "línea $NEW_MINOR ya registrada — no reintentar con el mismo argumento" >&2; exit 1; }

# The rotation event is "vX.(Y+1).0 ships". If that tag does not exist, the
# premise is false — refuse rather than declare a line for a release that never
# happened.
git rev-parse --verify --quiet "refs/tags/v${NEW_MINOR}.0" >/dev/null || {
  echo "v${NEW_MINOR}.0 no está publicado: la rotación presupone que la minor ya salió" >&2; exit 1; }

CURRENT_STABLE="$(jq -r '.lines[] | select(.state=="STABLE") | .minor // empty' "$SUPPORT_JSON")"
[[ -n "$CURRENT_STABLE" ]] || {
  echo "no hay ninguna línea STABLE declarada que degradar — sanear support.json primero" >&2; exit 1; }

# Published minors strictly between the declared STABLE and the requested one
# that support.json does not know about. Rotating here would leave the chain
# broken exactly as it already is, but now silently and with the intermediate
# minors still undeclared.
PUBLISHED_MINORS="$(git tag --list 'v*' | sed 's/^v//' | grep -v -- '-' \
  | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | cut -d. -f1,2 | sort -u -V)"
GAP="$(comm -23 \
  <(printf '%s\n' "$PUBLISHED_MINORS" | awk -v s="$CURRENT_STABLE" -v n="$NEW_MINOR" '
    function gt(a, b,  x, y, i, m, k) {
      m = split(a, x, "."); k = split(b, y, ".")
      for (i = 1; i <= 3; i++) {
        if ((i <= m ? x[i] + 0 : 0) != (i <= k ? y[i] + 0 : 0)) return (i <= m ? x[i] + 0 : 0) > (i <= k ? y[i] + 0 : 0)
      }
      return 0
    }
    gt($0, s) && gt(n, $0) { print }') \
  <(jq -r '.lines[].minor' "$SUPPORT_JSON" | sort -V))"
if [[ -n "$GAP" ]]; then
  echo "líneas publicadas sin declarar entre $CURRENT_STABLE y $NEW_MINOR: $(echo "$GAP" | tr '\n' ' ')" >&2
  echo "rotar por encima de ellas dejaría la cadena rota en silencio; registrarlas (o marcarlas EOL) es una decisión de gobernanza con issue aprobada detrás" >&2
  exit 1
fi

CURRENT_MAINTENANCE="$(jq -r '.lines[] | select(.state=="MAINTENANCE") | .minor // empty' "$SUPPORT_JSON")"
if [[ -n "$CURRENT_MAINTENANCE" ]]; then
  # shellcheck disable=SC2086
  for m in $CURRENT_MAINTENANCE; do ./scripts/eol-line.sh "$m"; done
fi

jq --arg new "$NEW_MINOR" --arg at "$NOW" '
  (.lines[] | select(.state=="STABLE")) |=
    (.state = "MAINTENANCE" | .demoted_from_stable_at = $at | .accepts = ["security"])
  | .lines = [{
      "minor": $new, "state": "STABLE", "branch": null,
      "latest": ($new + ".0"), "released_at": $at,
      "accepts": ["bugfix", "security"]
    }] + .lines
' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "OK: $NEW_MINOR es STABLE; anterior STABLE a MAINTENANCE."
