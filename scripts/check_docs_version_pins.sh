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
# WHO OWNS THE BUMP: the release flow, not a human. release-plz bumps
# `[workspace.package] version` and never touches the narrative, so this gate
# reds every Release PR by construction — and a human bump on that branch is
# deleted when release-plz regenerates it. The step that owns the bump is
# `Own the docs version pins` in the `release-plz-pr` job
# (.github/workflows/release-plz.yml), which runs scripts/apply_release_pins.sh
# and, inside the Release PR branch, scripts/bump_docs_version_pins.sh — the
# exact inverse of this check: same file set, same marker, same version
# grammar, and after it runs this gate passes over the same tree. There is no
# branch exemption and no in-flight window here on purpose: exempting
# `chore/release-*` would only move the red to `main` after the merge, without
# fixing the pins.
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

# The version grammar is defined ONCE and used by both sides of the comparison.
# It used to apply to the workspace only: the doc literal was extracted with a
# narrower pattern that dropped the suffix, so `v2.11.0-rc.1` was compared as
# `v2.11.0`, matched a stable workspace, and passed — while a workspace ON
# `2.11.0-rc.1` aborted here before checking anything. Both directions matter:
# release.yml's preflight requires the tag to equal this version, so an RC
# window puts `-rc.N` in the manifest by construction, and the tree went
# unguarded for exactly that window.
# The suffix is matched as a whole, and its identifiers cannot end in a dot, or
# a pin at the end of a sentence ("... shipped in v2.11.0-rc.1.") would be read
# as a different version and fail.
VER_ERE='[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?'
PIN_ERE="v${VER_ERE}"

[[ $VERSION =~ ^${VER_ERE}$ ]] || err "versión del workspace con formato inesperado: '$VERSION' (se espera X.Y.Z o X.Y.Z-rc.N)."

ALLOW_MARKER='<!-- version-stale:allow -->'

# The document set is discovered, NOT assumed. `find docs/src ... 2>/dev/null`
# turned a MISSING docs tree into an empty list with no error at all: the
# non-empty guard still passed (README.md was in the list) and every docs
# version pin went UNCHECKED while the gate reported green. A tree that
# does not exist and a tree that holds no document are two different
# states, and neither of them is a state the gate can pass in silence.
DOCS_DIR="docs/src"
[[ -d "$DOCS_DIR" ]] \
  || err "no se encontró el árbol de documentación '$DOCS_DIR' en $ROOT — no hay narrativa que verificar y un árbol ausente no se reporta como limpio."
if ! DOC_LIST="$(find "$DOCS_DIR" -name '*.md' -type f | sort)"; then
  err "no se pudo recorrer el árbol de documentación '$DOCS_DIR' en $ROOT — la verificación no puede ejecutarse (fail closed)."
fi
[[ -n "$DOC_LIST" ]] \
  || err "el árbol de documentación '$DOCS_DIR' existe pero no contiene ningún archivo .md — no hay narrativa que verificar (fail closed)."

mapfile -t FILES < <(
  {
    [[ -f README.md ]] && printf '%s\n' README.md
    printf '%s\n' "$DOC_LIST"
  } | sort
)

[[ ${#FILES[@]} -gt 0 ]] || err "no se encontró ningún documento que revisar (README.md / $DOCS_DIR/*.md)."

offenders=()
checked=0   # lines carrying at least one version literal, reported on success

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
    done < <(printf '%s\n' "$text" | grep -oE "$PIN_ERE" || true)
  done < <(grep -nE "$PIN_ERE" "$f" || true)
done

if [[ ${#offenders[@]} -gt 0 ]]; then
  {
    printf '::error::pines de versión en la narrativa que contradicen el workspace (v%s):\n' "$VERSION"
    printf '  %s\n' "${offenders[@]}"
    printf 'Actualizá el pin, o marcá la línea con %s si reporta historia intencional.\n' "$ALLOW_MARKER"
  } >&2
  exit 1
fi

printf 'OK: %d líneas con pines de versión en %d documentos; todas declaran v%s o están marcadas como historia.\n' \
  "$checked" "${#FILES[@]}" "$VERSION"
