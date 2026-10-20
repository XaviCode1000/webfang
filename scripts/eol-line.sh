#!/usr/bin/env bash
# Declares a line EOL: deletes the support branch (remote + local) and marks
# the entry. Branch deletion is the CONSEQUENCE of the state, not its cause:
# support.json is the source of truth, the branch is its physical projection.
# EOL means no fixes, no security fixes, no backports, no releases — except
# via an explicitly approved exception issue (which recreates the line).
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

MINOR="${1:?usage: eol-line.sh X.Y}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }
BRANCH="support/${MINOR}"

# Audit B10: the line is validated BEFORE anything else — a nonexistent minor
# used to fall through `select()` unpatched (jq writes an unchanged document,
# exit 0, "OK: línea X en EOL") AFTER the branch deletion had already run, so
# an orphan branch deleted itself while support.json never knew the line.
if ! jq -e --arg minor "$MINOR" 'any(.lines[]; .minor == $minor)' "$SUPPORT_JSON" >/dev/null; then
  echo "::error::eol-line.sh: la línea $MINOR no existe en $SUPPORT_JSON — falla-cerrada: ninguna mutación (branch ni estado) corrió (audit B10)." >&2
  exit 1
fi

# State is written FIRST (audit B10): the branch is a projection, and a failed
# state write must never be undone by an already-deleted branch. A jq failure
# aborts BEFORE the write lands — nothing is mutated, nothing is deleted.
if ! jq --arg minor "$MINOR" --arg at "$(date -Iseconds)" \
  '(.lines[] | select(.minor == $minor)) |= (
     .state = "EOL" | .branch = null | .eol_at = $at |
     .eol_reason = (.eol_reason // "auto: rotate-stable, third line back from STABLE") |
     del(.accepts)
   )' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"; then
  rm -f "${SUPPORT_JSON}.tmp"
  echo "::error::eol-line.sh: la escritura de estado falló (jq rc=$?) — rechazo nombrado, el estado y la branch quedan intactos." >&2
  exit 1
fi
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "estado escrito: línea $MINOR -> EOL."

# Branch deletion AFTER the state landed. A network error here leaves the
# state (source of truth) already EOL and only the physical projection
# behind; the retry list is this one command, not the whole script.
if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
  if ! git push origin --delete "$BRANCH"; then
    echo "::warning::eol-line.sh: el estado está escrito, pero el borrado remoto de $BRANCH falló — la branch quedó; reintenta con: git push origin --delete $BRANCH." >&2
  else
    git branch -D "$BRANCH" 2>/dev/null || true
  fi
else
  echo "branch $BRANCH inexistente: el estado es la fuente de verdad, nada que borrar."
fi
echo "OK: línea $MINOR en EOL."
