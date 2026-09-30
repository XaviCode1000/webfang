#!/usr/bin/env bash
# Read-only, fail-closed verification that .github/support.json agrees with
# what this repository actually publishes, and that its rendered projection
# SUPPORT.md is in sync.
#
# WHY THIS EXISTS (#1676)
# support.json is a DERIVED artifact: its STABLE line is a function of the
# published `v*` tags. Nothing ever read it, so four releases shipped with the
# file still declaring 2.1 STABLE while 2.2, 2.3 and 2.4 were published. A
# declaration nobody checks rots silently; this is that check.
#
# FAIL-CLOSED, NEVER SKIP
# Every ambiguity here is an error, not a skip: unreadable/invalid JSON, an
# unknown `state`, a missing `accepts` key, a shallow clone, an empty tag
# list, or a `v*` tag that is not `X.Y.Z`. A guard that degrades to "skip"
# when it cannot see is indistinguishable from a guard that does not exist —
# which is the exact failure mode this script replaces.
#
# SCOPES (both, by default)
#   --scope=declared    JSON self-consistency + SUPPORT.md render drift.
#                       Needs no git tags, fully deterministic, green today:
#                       safe to enforce as a hard gate immediately.
#   --scope=published   support.json vs `git tag --list 'v*'`. Needs a clone
#                       WITH tags (`fetch-depth: 0`).
#
# The script never writes anything: the SUPPORT.md half delegates to
# `render-support-md.sh --check`, so the renderer stays the single writer.
#
# Usage:
#   scripts/check_support_drift.sh [--scope=declared|published|both]

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SCOPE="both"
case "${1:-}" in
  "") ;;
  --scope=declared|--scope=published|--scope=both) SCOPE="${1#--scope=}" ;;
  *) echo "usage: scripts/check_support_drift.sh [--scope=declared|published|both]" >&2; exit 2 ;;
esac

SUPPORT_JSON=".github/support.json"
RENDERER="scripts/render-support-md.sh"

ERRORS=0
err() {
  echo "::error::$*"
  ERRORS=$((ERRORS + 1))
}

# ─── Declared scope: is the file internally coherent, and is its projection
# ─── current? Neither question needs a single tag.

check_declared() {
  if [[ ! -f "$SUPPORT_JSON" ]]; then
    err "$SUPPORT_JSON no existe; la declaración de líneas soportadas es obligatoria."
    return
  fi
  if ! jq -e '.schema_version == 1 and (.lines | type == "array")' "$SUPPORT_JSON" >/dev/null 2>&1; then
    err "$SUPPORT_JSON no es un support.json válido (schema_version == 1 y .lines como array)."
    return
  fi

  local n_stable n_maint live bad_state bad_minor bad_latest bad_accepts
  n_stable="$(jq '[.lines[] | select(.state == "STABLE")] | length' "$SUPPORT_JSON")"
  n_maint="$(jq '[.lines[] | select(.state == "MAINTENANCE")] | length' "$SUPPORT_JSON")"
  live="$(jq '[.lines[] | select(.state == "STABLE" or .state == "MAINTENANCE")] | length' "$SUPPORT_JSON")"

  bad_state="$(jq -r '.lines[] | select(.state != "STABLE" and .state != "MAINTENANCE" and .state != "EOL") | .minor' "$SUPPORT_JSON")"
  [[ -z "$bad_state" ]] || err "estado desconocido en las líneas: $(echo "$bad_state" | tr '\n' ' ') (permitidos: STABLE, MAINTENANCE, EOL)."

  # Support window is structural, not a calendar: at most two live lines. And
  # exactly one STABLE, not "at most one": a declaration where everything is
  # EOL is internally consistent and would sail through this scope while
  # stating that nothing is supported at all (#1676 review R4-NO-ACTIVE-STABLE).
  [[ "$n_stable" -eq 1 ]] || err "hay $n_stable líneas STABLE; se requiere exactamente una (la minor vigente)."
  [[ "$n_maint" -le 1 ]] || err "hay $n_maint líneas MAINTENANCE; la política admite como máximo una."
  [[ "$live" -le 2 ]] || err "hay $live líneas vivas; el máximo estructural es 2 (STABLE + MAINTENANCE)."

  bad_minor="$(jq -r '.lines[] | select((.minor // "") | test("^[0-9]+\\.[0-9]+$") | not) | (.minor // "(null)")' "$SUPPORT_JSON")"
  [[ -z "$bad_minor" ]] || err "campo .minor inválido (se espera X.Y): $(echo "$bad_minor" | tr '\n' ' ')."

  bad_latest="$(jq -r '.lines[] | select((.latest // "") | test("^[0-9]+\\.[0-9]+\\.[0-9]+$") | not) | "\(.minor)=\(.latest // "(null)")"' "$SUPPORT_JSON")"
  [[ -z "$bad_latest" ]] || err "campo .latest inválido (se espera X.Y.Z): $(echo "$bad_latest" | tr '\n' ' ')."

  # `accepts` stays machine-checkable on purpose: it IS the routing table an
  # agent reads to decide whether a backport is allowed. Check it, do not prose.
  bad_accepts="$(jq -r '
    .lines[]
    | select(
        (.state == "STABLE"      and (.accepts // []) != ["bugfix", "security"])
        or (.state == "MAINTENANCE" and (.accepts // []) != ["security"])
        or (.state == "EOL"       and (.accepts // empty) != empty)
      )
    | "\(.minor)[\(.state)]=\((.accepts // null) | tostring)"' "$SUPPORT_JSON")"
  [[ -z "$bad_accepts" ]] || err "accepts incoherente con el estado (STABLE=bugfix+security, MAINTENANCE=security, EOL=sin accepts): $(echo "$bad_accepts" | tr '\n' ' ')."

  if [[ ! -x "$RENDERER" ]]; then
    err "$RENDERER no existe o no es ejecutable; no se puede verificar la proyección renderizada."
  elif ! "$RENDERER" --check; then
    err "SUPPORT.md no coincide con el render de $SUPPORT_JSON (corré $RENDERER y commiteá ambos archivos juntos)."
  fi
}

# ─── Published scope: does the declaration match the published tags?

published_minors() {
  # Minor lines of every released tag, newest last. A prerelease (`-rc.N`) is
  # deliberately NOT a published minor: an RC is a candidate, not a release,
  # and counting it would make the gate flap during a stabilization window.
  git tag --list 'v*' \
    | sed 's/^v//' \
    | grep -v -- '-' \
    | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' \
    | cut -d. -f1,2 \
    | sort -u -V
}

check_published() {
  if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
    err "clone shallow: no hay tags que verificar. Traé los tags (fetch-depth: 0) antes de correr este scope — un guard que no puede ver no puede aprobar."
    return
  fi
  local unparseable
  unparseable="$(git tag --list 'v*' | sed 's/^v//' | grep -v -- '-' | grep -vE '^[0-9]+\.[0-9]+\.[0-9]+$' || true)"
  [[ -z "$unparseable" ]] || err "tags publicados con formato inesperado (se espera vX.Y.Z): $(echo "$unparseable" | tr '\n' ' ')."

  local minors
  minors="$(published_minors)"
  if [[ -z "$minors" ]]; then
    err "no se encontró ningún tag vX.Y.Z; no hay contra qué verificar la declaración (fail closed)."
    return
  fi

  local declared newest stable
  declared="$(jq -r '.lines[].minor' "$SUPPORT_JSON" 2>/dev/null || sort -V || true)"
  newest="$(echo "$minors" | tail -n 1)"
  stable="$(jq -r '.lines[] | select(.state == "STABLE") | .minor // empty' "$SUPPORT_JSON" 2>/dev/null || true)"

  # Support tracking has a floor: the OLDEST declared line. Everything published
  # before it predates tracking — 1.0/1.1 are exactly the case 2.0 records with
  # `eol_reason: "baseline: predates support tracking"` — and requiring them to
  # be declared would be inventing history, not verifying it. Derived from the
  # file rather than configured, so the floor cannot drift either.
  local tracked=""
  if [[ -n "$declared" ]]; then
    local floor_major floor_minor
    floor_major="$(echo "$declared" | sort -V | head -n 1 | cut -d. -f1)"
    floor_minor="$(echo "$declared" | sort -V | head -n 1 | cut -d. -f2)"
    tracked="$(printf '%s\n' "$minors" | awk -F. -v fm="$floor_major" -v fn="$floor_minor" \
      '($1 > fm) || ($1 == fm && $2 >= fn)')"
  fi

  if [[ -z "$stable" ]]; then
    err "no hay ninguna línea STABLE declarada, pero la minor publicada más reciente es $newest."
  elif [[ "$stable" != "$newest" ]]; then
    err "línea STABLE declarada: $stable; minor publicada más reciente: $newest. La ventana de soporte exige que la STABLE sea la minor vigente (publicar vX.(Y+1).0 rota la anterior a MAINTENANCE)."
  fi

  # Every published minor at or above the floor must carry a declaration. An
  # unregistered minor is a published release whose support status nobody
  # stated — the hole this whole check exists to close.
  local unregistered
  unregistered="$(comm -23 <(echo "$tracked") <(echo "$declared" | sort -V))"
  [[ -z "$unregistered" ]] || err "minors publicadas sin declarar en support.json: $(echo "$unregistered" | tr '\n' ' ') (registralas o marcalas EOL; crearlas es decisión del mantenedor con issue aprobada)."

  # A declared `latest` must exist as a tag: it is a factual claim about what
  # shipped, not an intention.
  local phantom
  phantom="$(jq -r '.lines[] | "v\(.latest)"' "$SUPPORT_JSON" 2>/dev/null | while read -r t; do
    git tag --list "$t" | grep -qxF "$t" || echo "$t"
  done)"
  [[ -z "$phantom" ]] || err "líneas cuyo .latest no corresponde a ningún tag publicado: $(echo "$phantom" | tr '\n' ' ')."

  # MAINTENANCE is the minor immediately before STABLE, not "whatever was
  # registered last": when the chain breaks, the stale entry keeps a real
  # release line looking supported.
  local maint
  maint="$(jq -r '.lines[] | select(.state == "MAINTENANCE") | .minor // empty' "$SUPPORT_JSON" 2>/dev/null || true)"
  if [[ -n "$maint" ]]; then
    local expected_prev
    expected_prev="$(echo "$tracked" | grep -B1 -x -F "$stable" | head -n 1 || true)"
    if [[ -z "$expected_prev" ]]; then
      err "línea MAINTENANCE declarada: $maint, pero $stable es la minor publicada más reciente (no hay minor publicada anterior que sostenerla)."
    elif [[ "$maint" != "$expected_prev" ]]; then
      err "línea MAINTENANCE declarada: $maint; la minor publicada inmediatamente anterior a $stable es $expected_prev."
    fi
  fi
}

case "$SCOPE" in
  declared) check_declared ;;
  published) check_published ;;
  both) check_declared; check_published ;;
esac

if [[ "$ERRORS" -gt 0 ]]; then
  echo "::error::support.json está en desuso o no verifiable ($ERRORS problema(s)). La declaración de líneas soportadas deriva de los tags publicados y debe coincidir con ellos." >&2
  exit 1
fi
echo "OK: soporte declarado coherente (scope=$SCOPE)."