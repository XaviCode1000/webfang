#!/usr/bin/env bash
# Regenerates SUPPORT.md from .github/support.json (single-writer pattern,
# same as CHANGELOG.md). Run from the repo root and commit both files in the
# same commit. CI only VERIFIES that SUPPORT.md matches a fresh render
# (diff, never autogenerates or force-pushes) — the agent always brings both.
#
# Modes:
#   scripts/render-support-md.sh            generate (default): rewrite SUPPORT.md
#   scripts/render-support-md.sh --check    CI drift gate: exit 1 if SUPPORT.md is
#                                          stale, and WRITE NOTHING. The writer
#                                          stays the single writer either way — the
#                                          gate renders into a temp file and diffs,
#                                          it never regenerates in place (#1676).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[[ -f .github/support.json ]] || { echo ".github/support.json no existe" >&2; exit 1; }

mode="generate"
case "${1:-}" in
  "") mode="generate" ;;
  --check) mode="check" ;;
  *) echo "usage: scripts/render-support-md.sh [--check]" >&2; exit 2 ;;
esac

render() {
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
}

if [ "$mode" = "check" ]; then
  TMP="$(mktemp)"
  trap 'rm -f "$TMP"' EXIT
  render > "$TMP"
  if ! diff -u SUPPORT.md "$TMP"; then
    echo "::error::SUPPORT.md está desactualizado — corré scripts/render-support-md.sh y commiteá ambos archivos juntos." >&2
    exit 1
  fi
  echo "OK: SUPPORT.md coincide con el render de .github/support.json."
else
  render > SUPPORT.md
  echo "OK: SUPPORT.md regenerado."
fi
