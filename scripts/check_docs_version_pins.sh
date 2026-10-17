#!/usr/bin/env bash
# ============================================================================
# DOCS VERSION PINS — does every version in the narrative match the workspace?
# ============================================================================
# README.md and docs/src/*.md are the first thing a user copies. A literal
# `vX.Y.Z` in an install recipe is a claim about which release exists, and it
# is wrong the moment the next minor ships: the reader pastes it and gets an
# old binary. Observed drift (2026-10-17): nine `v2.4.0` pins in README and
# installation.md while the workspace was at 2.11.0 — four minors back, and
# nobody noticed because nothing read them.
#
# This is the reader. It is deliberately a CHECK and not a template engine:
# README.md is rendered by GitHub, not by mdBook, so a preprocessor token would
# leak to the reader there, and substituting a version into an "observed
# output" block would manufacture an observation nobody made. A single source
# of truth plus a fail-closed check keeps the recipes copy-pasteable and keeps
# the claims honest.
#
# The source of truth is `[workspace.package] version` in the root Cargo.toml.
#
# Intentional history is allowed and must say so, on the same line:
#   <!-- version-stale:allow -->
# Use it for prose that reports what a past release did ("observed on v2.4.0"),
# never for a recipe a user is meant to copy.
#
# Usage: scripts/check_docs_version_pins.sh
# Exit: 0 clean · 1 drift or unreadable inputs
# ============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

err() { printf '::error::%s\n' "$*" >&2; exit 1; }

MANIFEST="Cargo.toml"
[[ -f $MANIFEST ]] || err "no se encontró $MANIFEST en $ROOT."

# [workspace.package] version, without depending on jq or python.
VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' "$MANIFEST" \
  | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"

[[ -n $VERSION ]] || err "no se pudo leer [workspace.package] version de $MANIFEST."
[[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || err "versión del workspace con formato inesperado: '$VERSION' (se espera X.Y.Z)."

ALLOW_MARKER='<!-- version-stale:allow -->'

mapfile -t FILES < <(
  {
    [[ -f README.md ]] && printf '%s\n' README.md
    find docs/src -name '*.md' -type f 2>/dev/null
  } | sort
)

[[ ${#FILES[@]} -gt 0 ]] || err "no se encontró ningún documento que revisar (README.md / docs/src/*.md)."

offenders=()
checked=0

for f in "${FILES[@]}"; do
  while IFS= read -r hit; do
    [[ -n $hit ]] || continue
    lineno="${hit%%:*}"
    text="${hit#*:}"
    checked=$((checked + 1))
    # Intentional history: the line says so, and says so where a reader sees it.
    case "$text" in
      *"$ALLOW_MARKER"*) continue ;;
    esac
    while IFS= read -r literal; do
      [[ -n $literal ]] || continue
      [[ $literal == "v$VERSION" ]] && continue
      offenders+=("$f:$lineno  $literal  (workspace: v$VERSION)")
    done < <(printf '%s\n' "$text" | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' || true)
  done < <(grep -nE 'v[0-9]+\.[0-9]+\.[0-9]+' "$f" || true)
done

if [[ ${#offenders[@]} -gt 0 ]]; then
  {
    printf '::error::pines de versión en la narrativa que contradicen el workspace (v%s):\n' "$VERSION"
    printf '  %s\n' "${offenders[@]}"
    printf 'Actualizá el pin, o marcá la línea con %s si reporta historia intencional.\n' "$ALLOW_MARKER"
  } >&2
  exit 1
fi

printf 'OK: los %d documentos declaran sólo v%s (o historia marcada).\n' "${#FILES[@]}" "$VERSION"
