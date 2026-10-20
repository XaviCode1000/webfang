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
#
# The generate mode is ATOMIC: it renders into a temp file and moves it into
# place only when the render succeeded, so a render that fails mid-way (a
# broken jq, an unreadable declaration) leaves the tracked projection
# byte-identical instead of truncated to the header (#B9).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"
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
  # The generate mode writes a TRACKED file, so it must never leave it half
  # written. `render > SUPPORT.md` truncated the tracked file BEFORE the
  # render even started (the shell opens the redirection target first), so a
  # render that died mid-way — a broken jq, an unreadable declaration — left
  # SUPPORT.md holding the header only: a corrupted tracked file, ready to
  # be committed by whoever ran the script next.
  #
  # The render goes to a temp file in the SAME directory (so the move is a
  # rename on the same filesystem) and the file MOVES into place only when
  # the render succeeded. A failed render therefore leaves the tracked
  # projection byte-identical, and the failure is loud instead of silent.
  TMP="$(mktemp "$REPO_ROOT/.SUPPORT.md.XXXXXX")"
  cleanup() { rm -f "$TMP"; }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  render > "$TMP"
  # mktemp creates the file 0600; the projection is a tracked, world-readable
  # document, so the mode of the file it replaces is restored before the
  # move (git tracks only the executable bit, so this keeps the worktree
  # consistent with the committed file).
  if [[ -f "$REPO_ROOT/SUPPORT.md" ]]; then
    chmod --reference="$REPO_ROOT/SUPPORT.md" "$TMP" 2>/dev/null || chmod 644 "$TMP"
  else
    chmod 644 "$TMP"
  fi
  mv -f "$TMP" "$REPO_ROOT/SUPPORT.md"
  trap - EXIT INT TERM
  echo "OK: SUPPORT.md regenerado."
fi
