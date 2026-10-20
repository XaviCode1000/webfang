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
# THREE FACTS, ONE PRODUCER EACH
#   - the DECLARED minors come from ONE producer, which is sorted at the
#     source and FAILS when the file cannot be read: a declaration nobody
#     can read must never look like a declaration that declares nothing;
#   - a minor is declared EXACTLY ONCE: two lines for one minor is a
#     contradiction, and the rendered projection would carry two rows for
#     one line;
#   - the PUBLISHED tags and the MALFORMED tags are two DIFFERENT sets: a
#     release candidate (`-rc.N`) is not a publication, and a malformed tag
#     (`v2-14-0`) is REPORTED instead of being filtered away by the filter
#     that excludes the candidates.
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

# ─── Producers: the facts every check reads, each read exactly once ───────

# declared_minors: the `.minor` values the declaration carries, in VERSION
# order (2.9 before 2.10 before 10.0 — never lexicographic).
#
# The producer used to be `jq -r '.lines[].minor' ... || sort -V || true`.
# `||` binds left to right, so `sort` only ran when jq FAILED — and then it
# read EMPTY stdin, so a file that could not be read produced an EMPTY list
# with exit 0: a broken declaration was indistinguishable from a declaration
# that declares nothing. The ordering was supplied further down by a re-sort
# of the tracking floor, so the producer itself was never right.
#
# This is the version that is right at the source: reading the file and
# ordering the result are two explicit steps, a jq failure is a FAILURE
# (return 1, never an empty list), and a success is a SORTED list.
declared_minors() {
  local raw
  raw="$(jq -r '.lines[] | select(.minor != null) | .minor' "$SUPPORT_JSON")" || return 1
  [[ -n "$raw" ]] || return 0
  printf '%s\n' "$raw" | sort -V
}

# duplicate_minors: the `.minor` values declared by MORE THAN ONE line, one
# per duplicated value, in version order. A line whose `.minor` is null is
# not counted here: the grammar check reports it on its own.
duplicate_minors() {
  jq -r '.lines[] | select(.minor != null) | .minor' "$SUPPORT_JSON" \
    | sort -V | uniq -d
}

# check_duplicate_minors: a minor declared twice is a CONTRADICTION, not a
# redundancy. The rendered projection would carry two rows for one line, and
# every lookup by minor (the STABLE one, the tracking floor, the membership
# check) would have two answers with no rule to pick one. The uniqueness of
# `.minor` was never checked, so a declaration could name the same line twice
# and pass both scopes untouched.
check_duplicate_minors() {
  local dup
  if ! dup="$(duplicate_minors)"; then
    err "$SUPPORT_JSON no se pudo leer para verificar la unicidad de .minor (fail closed)."
    return
  fi
  [[ -z "$dup" ]] || err "minor declarada más de una vez: $(echo "$dup" | tr '\n' ' ') (cada minor declara exactamente una línea de soporte)."
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

  # A minor is declared EXACTLY ONCE. The counts above say how many lines are
  # STABLE/MAINTENANCE, not how many lines there are: a declaration naming
  # the same minor twice passed this scope untouched, and the projection
  # emitted two rows for one line.
  check_duplicate_minors

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

# published_patch_tags: the PUBLISHED patches (X.Y.Z) of the given tag
# pattern, without the `v` prefix.
#
# A release candidate (`-rc.N`) is a CANDIDATE, not a publication, so the
# hyphen filter below is deliberate HERE — and it is the only place it
# belongs. The malformed computation needs the opposite intent (report what
# is not a well-formed tag at all), so it must not reuse this filter: the
# hyphen filter used to run FIRST there and dropped every hyphenated tag
# before it could ever be reported, so a malformed tag such as `v2-14-0`
# rode through unreported (B15).
published_patch_tags() {
  local pattern="${1:-v*}"
  git tag --list "$pattern" \
    | sed 's/^v//' \
    | grep -v -- '-' \
    | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$'
}

# published_minors: minor lines of every published tag, newest last. A
# prerelease (`-rc.N`) is deliberately NOT a published minor: an RC is a
# candidate, not a release, and counting it would make the gate flap during a
# stabilization window.
published_minors() {
  published_patch_tags | cut -d. -f1,2 | sort -u -V
}

# unparseable_tags: the `v*` tags that are NEITHER a publication (vX.Y.Z) NOR
# a release candidate (vX.Y.Z-rc.N — the shape release-tag-trust.sh accepts).
#
# A candidate is not a publication, so it never counts as one, but it is not a
# MALFORMED tag either: it is never reported. The tag names are reported WITH
# their `v` — that is the name the tag has.
unparseable_tags() {
  git tag --list 'v*' \
    | grep -vE '^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'
}

check_published() {
  if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
    err "clone shallow: no hay tags que verificar. Traé los tags (fetch-depth: 0) antes de correr este scope — un guard que no puede ver no puede aprobar."
    return
  fi
  local unparseable rc_unparseable=0
  # The DECLARATION is read first, and through ONE producer: a file the gate
  # cannot read is a FAILURE (not a declaration that declares nothing), and
  # the producer hands the minors over already sorted in version order.
  local declared
  if ! declared="$(declared_minors)"; then
    err "$SUPPORT_JSON ilegible o inválido: no se pudo leer la lista de minors declaradas (fail closed — un productor que devolviera una lista vacía aprobaría una declaración que nadie puede leer)."
    return
  fi

  # A minor is declared EXACTLY ONCE, and the published scope is the REQUIRED
  # check in CI: it cannot accept a declaration the declared scope rejects.
  check_duplicate_minors

  # The MALFORMED tags are their own computation: a tag that is neither a
  # publication nor a release candidate is reported, name and all. The
  # candidate filter (the one that excludes `-rc.N` from the published set)
  # is NOT applied here — it would hide the malformed tags it was never
  # meant to judge.
  unparseable="$(unparseable_tags)" || rc_unparseable=$?
  if [[ "$rc_unparseable" -gt 1 ]]; then
    err "no se pudo computar el reporte de tags con formato inesperado (grep salió con $rc_unparseable); fail closed."
    return
  fi
  [[ -z "$unparseable" ]] || err "tags v* con formato inesperado (se espera vX.Y.Z; un candidato vX.Y.Z-rc.N no es una publicación y no se reporta): $(echo "$unparseable" | tr '\n' ' ')."

  local minors
  if ! minors="$(published_minors)" || [[ -z "$minors" ]]; then
    err "no se encontró ningún tag vX.Y.Z; no hay contra qué verificar la declaración (fail closed)."
    return
  fi

  local newest stable
  newest="$(echo "$minors" | tail -n 1)"
  # The file was proven readable by the producer above, so jq cannot fail
  # here; the fallback only keeps the lookup from aborting the script — an
  # empty STABLE is reported as a governance error below.
  stable="$(jq -r '.lines[] | select(.state == "STABLE") | .minor // empty' "$SUPPORT_JSON" 2>/dev/null || true)"
  DECLARED_STABLE="$stable"
  # The newest tag, not the newest minor: "2.12 is not declared" reads better
  # next to the release that caused it. Same published set as published_minors
  # (X.Y.Z only), so a prerelease or an unparseable tag cannot become the
  # headline number.
  NEWEST_TAG="$(published_patch_tags | sort -V | tail -n 1 || true)"
  NEWEST_TAG="${NEWEST_TAG:+v$NEWEST_TAG}"

  # Support tracking has a floor: the OLDEST declared line. Everything published
  # before it predates tracking — 1.0/1.1 are exactly the case 2.0 records with
  # `eol_reason: "baseline: predates support tracking"` — and requiring them to
  # be declared would be inventing history, not verifying it. Derived from the
  # file rather than configured, so the floor cannot drift either.
  local tracked=""
  if [[ -n "$declared" ]]; then
    # The producer hands the declared minors in VERSION order, so the floor is
    # its FIRST element. The re-sort that used to live here was the mask that
    # hid the producer's own bug (B14): with the producer correct at the
    # source the mask is redundant, and keeping it would keep the producer
    # untested — a lexicographic producer would put the floor at 10.0 and let
    # the published 9.0 fall below it.
    local floor
    floor="$(head -n 1 <<<"$declared")"
    local floor_major floor_minor
    floor_major="$(cut -d. -f1 <<<"$floor")"
    floor_minor="$(cut -d. -f2 <<<"$floor")"
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
    newest_line="$(published_patch_tags "v$m.*" | sort -V | tail -n 1 || true)"
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
  # The trailing separator is part of the message: `tr` turns every newline
  # into a space, so the accumulated trailing newline yields the space before
  # the closing period. Stripping it would change the annotation's bytes and
  # the pre-R2-1 gate's output would no longer match byte for byte.
  [[ -z "$stale_latest" ]] || err "líneas cuyo .latest no es el parche published más nuevo de su minor (sin tag o desactualizado): $(printf '%s' "$stale_latest" | tr '\n' ' ')."
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