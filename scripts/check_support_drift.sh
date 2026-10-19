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
# The script never writes to the repository: the SUPPORT.md half delegates to
# `render-support-md.sh --check`, so the renderer stays the single writer.
#
# IN CI, NAME THE SURFACE (#2042 follow-up)
# This gate runs inside the required job `Validate PR metadata`, and a required
# check is identified by its JOB name. That name is static YAML and the published
# tag is not known until the job runs, so "name the file and the tag in the check
# title" is not expressible in `name:` — renaming the job is worse still, because
# branch protection matches the context by name and a rename leaves every PR
# waiting on a check that never reports. The diagnosis therefore travels in the
# two places that CAN carry it: the annotation title (rendered with the error in
# the check's summary box) and the step summary (rendered at the top of the
# failing run, before any log is opened). Both are emitted only when the CI
# variables exist, so local output is byte-identical to before.
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

# Captured for the CI summary, and empty until the published scope runs.
DECLARED_STABLE=""
NEWEST_TAG=""
UNREGISTERED=""

# The title is what the check summary shows above the message, so it names the
# file and the source of truth instead of repeating "support drift".
ANNOTATION_TITLE="Support drift: $SUPPORT_JSON vs tags v*"

err() {
  echo "::error title=$ANNOTATION_TITLE::$*"
  ERRORS=$((ERRORS + 1))
}

# A NOTE is not a violation: it says the gate accepted something on purpose
# and why, so the reader of the run does not have to guess.
note() {
  echo "::notice title=$ANNOTATION_TITLE::$*"
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

  # A jq scoping detail on purpose: inside `startswith(...)` the argument is
  # evaluated with the PREVIOUS pipe stage's output (a string) as its input, so
  # `.minor` there would try to index a string ("Cannot index string with
  # string"). The `as $e` binding keeps the line object in scope for every
  # subexpression.
  bad_latest="$(jq -r '
    .lines[]
    | . as $e
    | select(
        (($e.latest // "") | test("^[0-9]+\\.[0-9]+\\.[0-9]+$") | not)
        or (($e.latest // "") | startswith(($e.minor // "") + ".") | not)
      )
    | "\($e.minor)=\($e.latest // "(null)")"' "$SUPPORT_JSON")"
  [[ -z "$bad_latest" ]] || err "campo .latest inválido (se espera X.Y.Z del árbol declarado, empieza por su propia minor): $(echo "$bad_latest" | tr '\n' ' ')."

  # `accepts` stays machine-checkable on purpose: it IS the routing table an
  # agent reads to decide whether a backport is allowed. Check it, do not prose.
  bad_accepts="$(jq -r '
    .lines[]
    | select(
        (.state == "STABLE"      and (.accepts // []) != ["bugfix", "security"])
        or (.state == "MAINTENANCE" and (.accepts // []) != ["security"])
        or (.state == "EOL"       and ((.accepts // []) | length) > 0)
      )
    | "\(.minor)[\(.state)]=\((.accepts // null) | tostring)"' "$SUPPORT_JSON")"
  [[ -z "$bad_accepts" ]] || err "accepts incoherente con el estado (STABLE=bugfix+security, MAINTENANCE=security, EOL=sin accepts): $(echo "$bad_accepts" | tr '\n' ' ')."

  if [[ ! -x "$RENDERER" ]]; then
    err "$RENDERER no existe o no es ejecutable; no se puede verificar la proyección renderizada."
  elif ! "$RENDERER" --check; then
    err "SUPPORT.md no coincide con el render de $SUPPORT_JSON (corré $RENDERER y commiteá ambos archivos juntos)."
  fi
}

# pending_publication <line_minor> <declared> <newest_published>: true when
# the declared `latest` is exactly ONE patch ahead of the newest published
# patch of its own minor and has no tag of its own yet.
#
# This is the in-flight window of the sanctioned patch release. The bump
# declares the patch it is about to publish BEFORE the tag exists, because
# cut-patch-tag.yml is workflow_dispatch-only and refuses a branch whose
# Cargo.toml does not carry the version — so the tag can only be cut AFTER
# the bump commit merges. In that window the declared `latest` sits exactly
# one patch above the newest publication of its line, and the enforcing gate
# must accept it: this gate runs on EVERY pull_request, so a red verdict
# there makes the bump PR unmergeable and the release contract impossible to
# execute.
#
# The tolerance is as narrow as the state allows — one patch, same minor, no
# tag of its own. Everything around it keeps failing exactly as before:
#   - a declared `latest` BEHIND the published patch is the drift this gate
#     exists to catch, and still fails;
#   - a declaration more than one patch ahead is a lie, and still fails;
#   - a line whose minor has no published patch at all has no fact to stand
#     on, and still fails closed.
#
# The comparison is NUMERIC in base 10 (`10#`), so patch 9 -> 10 is a real
# increment and a component with a leading zero is never read as octal.
pending_publication() {
  local line_minor="$1" declared="$2" newest="$3"
  [[ "$declared" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  [[ "$newest"  =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  local v_major v_minor v_patch p_major p_minor p_patch
  IFS='.' read -r v_major v_minor v_patch <<<"$declared"
  IFS='.' read -r p_major p_minor p_patch <<<"$newest"
  # Both the declared version and the published patch must belong to the
  # DECLARED line's own minor: the tag glob `v<minor>.*` can match another
  # minor's patches, and a line whose minor has no published patch at all
  # must fail closed instead of riding on a neighbour's tag.
  [[ "$v_major.$v_minor" == "$line_minor" ]] || return 1
  [[ "$p_major.$p_minor" == "$line_minor" ]] || return 1
  # Exactly ONE patch ahead, in base 10.
  (( 10#$v_patch == 10#$p_patch + 1 )) || return 1
  # A pending publication has no tag of its own yet.
  [[ -z "$(git tag --list "v$declared")" ]] || return 1
  return 0
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
  DECLARED_STABLE="$stable"
  # The newest tag, not the newest minor: "2.12 is not declared" reads better
  # next to the release that caused it. Same filter as published_minors, so a
  # prerelease or an unparseable tag cannot become the headline number.
  NEWEST_TAG="$(git tag --list 'v*' | sed 's/^v//' | grep -v -- '-' \
    | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)"
  NEWEST_TAG="${NEWEST_TAG:+v$NEWEST_TAG}"

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
  #
  # Membership by exact string equality (is_declared, same shape as
  # rotate-stable.sh's #2062 fix), never `comm`: comm demands the LOCALE's
  # collation order while both sides are version-sorted, so a tracked/declared
  # pair crossing the 2.9/2.10 boundary aborted the whole gate with
  # "input is not in sorted order" noise instead of the governance verdict.
  local unregistered
  unregistered="$(printf '%s\n' "$tracked" | awk -v declared="$(echo "$declared" | tr '\n' ' ')" "
    function is_declared(v,  i) {
      for (i = 1; i <= dn; i++) if (d[i] == v) return 1
      return 0
    }
    BEGIN { dn = split(declared, d, \" \") }
    { if (\$0 != \"\" && !is_declared(\$0)) print }
  ")"
  UNREGISTERED="$unregistered"
  [[ -z "$unregistered" ]] || err "minors publicadas sin declarar en support.json: $(echo "$unregistered" | tr '\n' ' ') (registralas o marcalas EOL; crearlas es decisión del mantenedor con issue aprobada)."

  # Every declared `latest` is a factual claim (B1, #2060): it must be the
  # NEWEST published patch of its own line, not merely an existing tag. The
  # per-line computation reuses the same filter chain as published_minors —
  # strip `v`, exclude `-` (so a `-rc.N` prerelease never counts as a
  # publication), keep only X.Y.Z, `sort -V`, newest last. A line whose minor
  # has zero published patches fails too: then `latest` cannot be the newest
  # patch, and the claim has nothing to stand on. Every state is covered —
  # STABLE, MAINTENANCE and EOL alike.
  #
  # ONE tolerance, and it is the bump window: a declared `latest` exactly one
  # patch above the newest published patch of its own minor, with no tag of
  # its own yet, is a PENDING PUBLICATION (the bump declares the patch it is
  # about to publish; the tag is cut after the bump merges). It is accepted
  # with a note, because the gate runs on every pull_request and a red
  # verdict in that window would make the bump PR unmergeable. A declaration
  # BEHIND the published patch, a declaration more than one patch ahead and a
  # line with no published patch at all all keep failing.
  local stale_latest="" pending_latest=""
  while IFS=$'\t' read -r m l; do
    local newest_line
    newest_line="$(git tag --list "v$m.*" | sed 's/^v//' | grep -v -- '-' \
      | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)"
    if [[ "$newest_line" == "$l" ]]; then
      continue
    fi
    if [[ -n "$newest_line" ]] && pending_publication "$m" "$l" "$newest_line"; then
      pending_latest+="v$m declara v$l: publicación pendiente (el parche published más nuevo de esa minor es v$newest_line)"$'\n'
      continue
    fi
    if [[ -n "$newest_line" ]]; then
      stale_latest+="v$m: declarado v$l, el parche published más nuevo es v$newest_line"$'\n'
    else
      stale_latest+="v$m: declarado v$l, sin ningún parche X.Y.Z publicado"$'\n'
    fi
  done < <(jq -r '.lines[] | "\(.minor)\t\(.latest // "(null)")"' "$SUPPORT_JSON" 2>/dev/null)
  [[ -z "$stale_latest" ]] || err "líneas cuyo .latest no es el parche published más nuevo de su minor (sin tag o desactualizado): $(printf '%s' "${stale_latest%$'\n'}" | tr '\n' ' ')."
  if [[ -n "$pending_latest" ]]; then
    local p
    while IFS= read -r p; do
      if [[ -n "$p" ]]; then
        note "$p"
      fi
    done <<<"$pending_latest"
  fi

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

# ─── CI summary: the numbers that explain the failure, above the fold ───────
# Only on failure, and only when GitHub provides the file: a passing check has
# nothing to diagnose, and the harness (which runs in CI too) must not append
# its fixtures to the caller's summary.
write_summary() {
  [[ -n "${GITHUB_STEP_SUMMARY:-}" ]] || return 0
  local unregistered="${UNREGISTERED//$'\n'/, }"
  # SC2016 is the point, not a mistake: single quotes keep the markdown code
  # spans literal. Inside double quotes a backtick is a command substitution, so
  # ``echo "| STABLE | `$x` |"`` runs `$x` as a command and prints nothing —
  # which is exactly how the first draft of this summary came out empty.
  # shellcheck disable=SC2016
  {
    printf '### Support drift: `%s` vs published `v*` tags\n\n' "$SUPPORT_JSON"
    echo "| | |"
    echo "| --- | --- |"
    printf '| Scope | `%s` |\n' "$SCOPE"
    if [[ "$SCOPE" != "declared" ]]; then
      printf '| Declared STABLE | `%s` |\n' "${DECLARED_STABLE:-none}"
      printf '| Newest published tag | `%s` |\n' "${NEWEST_TAG:-none}"
      printf '| Published but not declared | `%s` |\n' "${unregistered:-none}"
    fi
    echo "| Problems | $ERRORS |"
    echo
    echo "The declaration is derived from the published tags, so a release that"
    echo "is not registered here fails this required check for **every** open PR."
    echo "Rotating the window is a maintainer decision with an approved issue:"
    printf '`scripts/rotate-stable.sh <minor>`, then `scripts/render-support-md.sh`.\n'
  } >> "$GITHUB_STEP_SUMMARY"
}

case "$SCOPE" in
  declared) check_declared ;;
  published) check_published ;;
  both) check_declared; check_published ;;
esac

if [[ "$ERRORS" -gt 0 ]]; then
  write_summary
  echo "::error title=$ANNOTATION_TITLE::support.json está en desuso o no verifiable ($ERRORS problema(s)). La declaración de líneas soportadas deriva de los tags publicados y debe coincidir con ellos." >&2
  exit 1
fi
echo "OK: soporte declarado coherente (scope=$SCOPE)."