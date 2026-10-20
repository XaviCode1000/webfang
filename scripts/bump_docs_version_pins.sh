#!/usr/bin/env bash
# ============================================================================
# DOCS VERSION PINS — escritor: reescribe los pines de versión de la narrativa
# ============================================================================
# Espejo exacto de scripts/check_docs_version_pins.sh: mismo conjunto de
# archivos, mismo marcador, misma gramática de versión. Donde el gate COMPARA,
# este script REESCRIBE.
#
# POR QUÉ EXISTE
# El gate sólo falla; no repara. Un Release PR (`chore/release-*`) bumpea la
# versión del workspace y deja la narrativa con los pines de la versión
# anterior, así que el gate queda rojo por construcción y el PR es
# estructuralmente inejecutable sin un commit manual. Ese commit manual es
# efímero: release-plz REGENERA la rama del Release PR en cada push a `main`,
# así que lo borra y el PR vuelve a rojo (medido tres veces en una tarde:
# v2.12.0 ×2, v3.0.1 ×1).
#
# El dueño del bump es el propio flujo de release — el paso
# `Own the docs version pins` del job `release-plz-pr`, que lo invoca a través
# de scripts/apply_release_pins.sh. Este script es su herramienta: reescribe,
# y lo hace de forma idempotente porque el flujo lo corre en cada corrida.
#
# CONTRATO
#   - sólo toca README.md y docs/src/*.md (nunca CHANGELOG.md, ni Cargo.toml);
#   - respeta el marcador `<!-- version-stale:allow -->` (alcance de línea,
#     igual que el gate);
#   - es idempotente: la segunda corrida no cambia nada y sale 0;
#   - falla cerrado ante una línea ambigua (dos pines distintos en la misma
#     línea) SIN escribir nada: el árbol queda intacto. Reescribir "de v2.4.0 a
#     v3.0.0" como "de v3.0.1 a v3.0.1" inventaría prosa; el gate exige que
#     esa línea se marque como historia, y el escritor se niega a adivinarla.
#
# Uso: scripts/bump_docs_version_pins.sh
# Salida: 0 ok · 1 fallo (con `::error::` en stderr, como el gate)
# ============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

err() { printf '::error::%s\n' "$*" >&2; exit 1; }

MANIFEST="Cargo.toml"
[[ -f $MANIFEST ]] || err "no se encontró $MANIFEST en $ROOT."

# [workspace.package] version, without depending on jq or python. Misma
# lectura que el gate: una gramática distinta entre el lector y el escritor
# dejaría el Release PR rojo después del bump.
VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' "$MANIFEST" \
  | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"

[[ -n $VERSION ]] || err "no se pudo leer [workspace.package] version de $MANIFEST."

# La gramática de versión es la MISMA del gate, incluida la ventana de release
# candidate: `-rc.N` viaja entero (el sufijo se compara como un todo, no se
# pela), porque durante una ventana RC el manifest lleva el sufijo por
# construcción.
VER_ERE='[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?'
PIN_ERE="v${VER_ERE}"

[[ $VERSION =~ ^${VER_ERE}$ ]] || err "versión del workspace con formato inesperado: '$VERSION' (se espera X.Y.Z o X.Y.Z-rc.N)."

ALLOW_MARKER='<!-- version-stale:allow -->'

mapfile -t FILES < <(
  {
    [[ -f README.md ]] && printf '%s\n' README.md
    find docs/src -name '*.md' -type f 2>/dev/null
  } | sort
)

[[ ${#FILES[@]} -gt 0 ]] || err "no se encontró ningún documento que revisar (README.md / docs/src/*.md)."

declare -a CHANGES=()
declare -a PINS=()
declare -A TOUCHED=()
changed_lines=0

# ─── Pasada 1: decidir (no escribe nada) ─────────────────────────────────────
# Todo o nada: una sola línea ambigua aborta ANTES de tocar el árbol. Un
# reescritor a medias es peor que ninguno — el gate reportaría drift que
# nadie puede explicar.
for f in "${FILES[@]}"; do
  while IFS= read -r hit; do
    [[ -n $hit ]] || continue
    lineno="${hit%%:*}"
    text="${hit#*:}"
    # Historia intencional: el marcador es de línea, igual que en el gate.
    case "$text" in
      *"$ALLOW_MARKER"*) continue ;;
    esac
    mapfile -t PINS < <(printf '%s\n' "$text" | grep -oE "$PIN_ERE" | sort -u)
    if [[ ${#PINS[@]} -gt 1 ]]; then
      err "línea ambigua: ${#PINS[@]} pines distintos en $f:$lineno (${PINS[*]}). No se puede reescribir sin inventar prosa; marcá la línea con $ALLOW_MARKER si reporta historia intencional."
    fi
    if [[ ${#PINS[@]} -eq 0 ]]; then
      continue
    fi
    old="${PINS[0]}"
    if [[ "$old" == "v$VERSION" ]]; then
      continue
    fi
    CHANGES+=("$f:$lineno  $old → v$VERSION")
    changed_lines=$((changed_lines + 1))
    TOUCHED["$f"]=1
  done < <(grep -nE "$PIN_ERE" "$f" || true)
done

# ─── Salida ──────────────────────────────────────────────────────────────────
if [[ $changed_lines -eq 0 ]]; then
  printf 'OK: los pines ya declaraban v%s (sin cambios)\n' "$VERSION"
  exit 0
fi

# ─── Pasada 2: escribir ──────────────────────────────────────────────────────
# El archivo se reescribe con `cat >` (no `mv`): el modo y el inode del
# documento original se conservan, y `mv` traería el 600 de un temporal.
TMP_FILE="$(mktemp)"
trap 'rm -f "$TMP_FILE"' EXIT

for f in "${FILES[@]}"; do
  [[ -n "${TOUCHED[$f]:-}" ]] || continue
  # Las líneas con el marcador se saltan enteras; el resto se reescribe. La
  # pasada 1 garantizó que cada una de esas líneas tiene UN solo pin distinto,
  # así que sustituir todas sus ocurrencias es exactamente lo decidido.
  sed -E "\|${ALLOW_MARKER}|! s/${PIN_ERE}/v${VERSION}/g" "$f" > "$TMP_FILE"
  if ! cmp -s "$f" "$TMP_FILE"; then
    cat "$TMP_FILE" > "$f"
  fi
done

printf '%s\n' "${CHANGES[@]}"
printf 'OK: %d líneas actualizadas a v%s en %d documentos\n' \
  "$changed_lines" "$VERSION" "${#TOUCHED[@]}"
