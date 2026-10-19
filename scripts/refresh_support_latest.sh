#!/usr/bin/env bash
# Recomputes the `latest` of every declared support line from the published
# `v*` tags and re-renders SUPPORT.md through the single writer.
#
# WHY THIS EXISTS (ID-1)
# `latest` is a FACT about what this repository published, not an intention:
# `check_support_drift.sh --scope=published` — a REQUIRED check, inside the
# `Validate PR metadata` job — fails when a declared `latest` is not the
# newest published patch of its own minor. After the first patch published on
# a support line (vX.Y.Z+1) that declaration went stale, the gate turned red
# for EVERY subsequent PR, and no sanctioned writer could repair it: the
# patch bump staged only Cargo.toml/Cargo.lock/CHANGELOG.md, release-plz
# never touches support.json, and rotate-stable.sh writes the `latest` of the
# minor it creates and of no other. This script is that writer.
#
# MODES
#   scripts/refresh_support_latest.sh                   recompute from the tags
#   scripts/refresh_support_latest.sh --set X.Y=X.Y.Z   declare the pending
#                                                       publication of one line
#   (repeatable)
#
# WHY --set EXISTS. The patch release flow cuts the tag AFTER the bump commit
# merges (cut-patch-tag.yml refuses a branch whose Cargo.toml does not carry
# the version), so at bump time the new patch is not a tag yet. The bump
# declares the fact it is about to publish; the pure tag recompute is the
# repair path for a declaration that already drifted.
#
# IDEMPOTENT. Re-running with nothing stale writes nothing: the same bytes in,
# the same bytes out, and the re-render is a no-op. The patch bump re-runs
# after a failed release, so "writes something different every time" is not an
# option.
#
# FAIL CLOSED. Nothing is written until the whole new declaration is computed
# AND proved to differ from the old one ONLY in the `latest` values; the file
# is then replaced atomically (temp file + mv), so an interrupted run can
# never leave a half-updated support.json behind. A line whose minor has no
# published patch has no fact to stand on: that is an error, not a skip. A
# declaration that is AHEAD of the published patches is never rolled back
# either — that state means "the tag is pending", and only the publisher may
# resolve it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SUPPORT_JSON=".github/support.json"
RENDERER="scripts/render-support-md.sh"

usage() {
  echo "uso: scripts/refresh_support_latest.sh [--set X.Y=X.Y.Z ...]" >&2
}

# version_lt <a> <b>: true when $1 is strictly LOWER than $2 in version order.
version_lt() {
  [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n 1)" == "$1" ]]
}

# ─── Arguments ───────────────────────────────────────────────────────────────
declare -A SET_BY_MINOR=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --set)
      [[ $# -ge 2 ]] || { usage; exit 2; }
      pair="$2"
      minor="${pair%%=*}"
      version="${pair#*=}"
      if [[ "$minor" == "$pair" || "$version" == "$pair" ]]; then
        usage; exit 2
      fi
      if [[ ! "$minor" =~ ^[0-9]+\.[0-9]+$ ]]; then
        echo "línea inválida '$minor': se espera X.Y" >&2; exit 2
      fi
      if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "versión inválida '$version': se espera X.Y.Z" >&2; exit 2
      fi
      if [[ "$version" != "$minor."* ]]; then
        echo "la versión $version no pertenece a la línea $minor (cada línea declara su propia minor)" >&2
        exit 2
      fi
      SET_BY_MINOR["$minor"]="$version"
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done

# ─── Preconditions (before anything is written) ──────────────────────────────
if [[ ! -f "$SUPPORT_JSON" ]]; then
  echo "$SUPPORT_JSON no existe" >&2; exit 1
fi
if [[ ! -x "$RENDERER" ]]; then
  echo "$RENDERER no existe o no es ejecutable; la proyección no se puede regenerar" >&2; exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq no está disponible" >&2; exit 1
fi
if ! jq -e '.schema_version == 1 and (.lines | type == "array")' "$SUPPORT_JSON" >/dev/null 2>&1; then
  echo "$SUPPORT_JSON no es un support.json válido (schema_version == 1 y .lines como array)" >&2
  exit 1
fi

declare -A DECLARED=()
while IFS= read -r m; do
  [[ -n "$m" ]] && DECLARED["$m"]=1
done < <(jq -r '.lines[].minor' "$SUPPORT_JSON")

for m in "${!SET_BY_MINOR[@]}"; do
  if [[ -z "${DECLARED[$m]:-}" ]]; then
    echo "la línea $m no está declarada en $SUPPORT_JSON" >&2; exit 1
  fi
done

# ─── Compute the target of every declared line ───────────────────────────────
# The filter chain is the one of check_support_drift.sh and
# cut-support-branch.sh: strip `v`, exclude `-` (a `-rc.N` is a candidate,
# never a publication), keep only X.Y.Z, `sort -V`, newest last.
declare -A CURRENT=()
declare -A TARGET=()
while IFS=$'\t' read -r m l; do
  [[ -n "$m" ]] || continue
  CURRENT["$m"]="$l"
  if [[ -n "${SET_BY_MINOR[$m]:-}" ]]; then
    TARGET["$m"]="${SET_BY_MINOR[$m]}"
  else
    newest="$(git tag --list "v$m.*" | sed 's/^v//' | grep -v -- '-' \
      | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)"
    if [[ -z "$newest" ]]; then
      echo "la línea $m no tiene ningún parche vX.Y.Z publicado: no hay hecho al que apuntar (fail closed)" >&2
      exit 1
    fi
    TARGET["$m"]="$newest"
  fi
done < <(jq -r '.lines[] | "\(.minor)\t\(.latest // "")"' "$SUPPORT_JSON")

# A declaration is never moved backwards: a value ahead of the published
# patches means "the tag is pending" (the bump path), not "the file is wrong".
for m in "${!TARGET[@]}"; do
  cur="${CURRENT[$m]:-}"
  tgt="${TARGET[$m]}"
  if [[ -n "$cur" ]] && version_lt "$tgt" "$cur"; then
    echo "la línea $m declara latest $cur y el parche publicado más nuevo es $tgt: el refresh no retrocede una declaración (fail closed; si $cur es la publicación pendiente, se declara con --set $m=$cur)" >&2
    exit 1
  fi
done

# ─── What actually changes ───────────────────────────────────────────────────
UPDATES=()
for m in "${!TARGET[@]}"; do
  if [[ "${CURRENT[$m]:-}" != "${TARGET[$m]}" ]]; then
    UPDATES+=("$m"$'\t'"${TARGET[$m]}")
  fi
done

if [[ ${#UPDATES[@]} -eq 0 ]]; then
  # The declaration already agrees with the published tags. The projection can
  # still be behind (SUPPORT.md is a generated file, and its single writer is
  # this script's collaborator), so a stale render is repaired here too —
  # but a clean pair writes NOTHING at all.
  if "$RENDERER" --check >/dev/null 2>&1; then
    echo "OK: .latest ya coincide con los tags publicados; nada que actualizar."
    exit 0
  fi
  "$RENDERER" >/dev/null
  echo "OK: .latest ya coincide con los tags publicados; se regeneró SUPPORT.md (la proyección estaba desactualizada)."
  exit 0
fi

# ─── Write (atomically, and only the `latest` values) ────────────────────────
UPDATES_JSON="$(printf '%s\n' "${UPDATES[@]}" \
  | jq -Rn 'reduce inputs as $l ({}; ($l | split("\t")) as $p | .[$p[0]] = $p[1])')"

TMP="${SUPPORT_JSON}.tmp"
jq --argjson updates "$UPDATES_JSON" '
  .lines |= map(. as $l | if $updates[$l.minor] != null then .latest = $updates[$l.minor] else . end)
' "$SUPPORT_JSON" > "$TMP"

if ! jq -e . "$TMP" >/dev/null 2>&1; then
  echo "el support.json calculado no es JSON válido: se aborta sin escribir (fail closed)" >&2
  rm -f "$TMP"; exit 1
fi

# Proof that the rewrite touched nothing but the `latest` values: with those
# blanked on both sides, the two files must be identical. Key order and every
# other field survive, and no unrelated line is reformatted.
if ! diff -q <(jq -S '(.lines[] | .latest) = ""' "$SUPPORT_JSON") \
             <(jq -S '(.lines[] | .latest) = ""' "$TMP") >/dev/null; then
  echo "el support.json calculado difiere fuera de los valores .latest: se aborta sin escribir (fail closed)" >&2
  rm -f "$TMP"; exit 1
fi

mv "$TMP" "$SUPPORT_JSON"

# The projection has a single writer, and it is the renderer.
"$RENDERER" >/dev/null

changed=""
for m in "${!TARGET[@]}"; do
  if [[ "${CURRENT[$m]:-}" != "${TARGET[$m]}" ]]; then
    changed+=" $m=${CURRENT[$m]:-(null)}->${TARGET[$m]}"
  fi
done
echo "OK: .latest refrescado (${changed# })."
