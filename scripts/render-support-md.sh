#!/usr/bin/env bash
# Regenerates SUPPORT.md from .github/support.json (single-writer pattern,
# same as CHANGELOG.md). Run from the repo root and commit both files in the
# same commit. CI only VERIFIES that SUPPORT.md matches a fresh render
# (diff, never autogenerates or force-pushes) — the agent always brings both.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[[ -f .github/support.json ]] || { echo ".github/support.json no existe" >&2; exit 1; }

{
  echo "<!-- generado por scripts/render-support-md.sh, no editar a mano -->"
  echo
  echo "# Versiones soportadas"
  echo
  echo "Fuente de verdad: \`.github/support.json\`. Este archivo es una vista generada."
  echo
  echo "| Línea | Estado | Última | Rama | Admite |"
  echo "|---|---|---|---|---|"
  jq -r '.lines[] | "| \(.minor) | \(.state) | \(.latest) | \(.branch // "—") | \((.accepts // ["—"]) | join(", ")) |"' \
    .github/support.json
} > SUPPORT.md
echo "OK: SUPPORT.md regenerado."
