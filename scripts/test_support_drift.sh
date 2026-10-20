#!/usr/bin/env bash
# Semantics harness for the support-line drift gate (#1676).
#
# Three properties here are behavioral and cannot be proven by grepping:
#
#   1. the gate FAILS on the drift it exists to catch. A drift check that
#      passes on a drifted repo is indistinguishable from no check at all —
#      and no check at all is the state this issue was filed against, so a
#      green gate here would be the defect, not the fix;
#   2. it PASSES on a consistent repo, including the state the sanctioned
#      rotation produces — so the gate does not require a hand-edited file,
#      it requires a correct one;
#   3. it fails CLOSED on what it cannot see: a shallow clone, no tags,
#      unparseable JSON, an unparseable tag. A guard that degrades to "skip"
#      when blind is worse than no guard, because it reports safety it never
#      established.
#
# Everything runs against mktemp fixtures: a synthetic git repo with synthetic
# tags, plus copies of the two scripts under test. No network, no real tags,
# no mutation of the repository the harness runs from.
#
# The fixtures pin the git settings that would otherwise be inherited from the
# developer's GLOBAL config, because a harness that hangs on someone's machine
# is a harness that gets deleted instead of run. Concretely, on this
# workstation `tag.gpgsign=true` + `EDITOR=nvim` means a bare `git tag -f`
# opens an editor to compose the signing message and blocks forever; likewise
# `commit.gpgsign` and any inherited `core.hooksPath` (this repo's hooks
# consult the Gentle AI review gate, which has nothing to do with a fixture).

set -euo pipefail

# The gate writes a CI step summary when GitHub provides GITHUB_STEP_SUMMARY,
# and this harness runs inside a CI job (support-drift.yml) where that variable
# IS set: every failing fixture would append its table to the caller's summary.
# The summary is CI chrome, not part of the gate's contract, so clear it here
# and let the two cases that assert on it set it explicitly.
unset GITHUB_STEP_SUMMARY

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── Fixture ─────────────────────────────────────────────────────────────────
# new_fixture: a self-contained repo with the scripts under test installed.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts" "$dir/.github"
  cp "$REPO_ROOT/scripts/check_support_drift.sh" "$REPO_ROOT/scripts/render-support-md.sh" \
     "$REPO_ROOT/scripts/rotate-stable.sh" "$REPO_ROOT/scripts/eol-line.sh" "$dir/scripts/"
  chmod +x "$dir"/scripts/*.sh
  git -C "$dir" init -q
  git -C "$dir" config user.email fixture@example.invalid
  git -C "$dir" config user.name fixture
  # Inherit nothing from the developer's global git config (see header).
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config tag.gpgsign false
  git -C "$dir" config core.hooksPath /dev/null
  git -C "$dir" config commit.template /dev/null
  printf '{"schema_version":1,"lines":[]}\n' > "$dir/.github/support.json"
  printf '# placeholder\n' > "$dir/SUPPORT.md"
  git -C "$dir" add -A
  git -C "$dir" commit -q -m init
  echo "$dir"
}

# set_lines <fixture> <json> — declare the lines, render SUPPORT.md from them
# (the single writer), commit, so every case starts from a clean worktree and
# a correct projection.
set_lines() {
  local dir="$1" json="$2"
  printf '%s\n' "$json" > "$dir/.github/support.json"
  ( cd "$dir" && ./scripts/render-support-md.sh >/dev/null )
  git -C "$dir" add -A
  git -C "$dir" commit -q -m lines
}

# add_tags <fixture> <tag...>
add_tags() {
  local dir="$1"; shift
  local tag
  for tag in "$@"; do
    git -C "$dir" tag -f "$tag" >/dev/null
  done
}

# expect <fixture> <expected-rc: pass|fail> <label> <args...>
expect() {
  local dir="$1" mode="$2" label="$3"; shift 3
  local rc=0
  ( cd "$dir" && ./scripts/check_support_drift.sh "$@" ) >/dev/null 2>&1 || rc=$?
  if { [[ "$mode" == pass && $rc -eq 0 ]] || [[ "$mode" == fail && $rc -ne 0 ]]; }; then
    pass "$label"
  else
    fail "$label (rc=$rc, esperado $mode)"
  fi
}

# run_gate <fixture> <args...>: run the gate keeping BOTH its verdict and its
# output, so a case can assert on the MESSAGE and not only on the exit code.
# GATE_RC / GATE_OUT carry the result.
run_gate() {
  local dir="$1"; shift
  local rc=0
  GATE_OUT="$( ( cd "$dir" && ./scripts/check_support_drift.sh "$@" ) 2>&1 )" || rc=$?
  GATE_RC=$rc
}

# ─── The consistent reference state ─────────────────────────────────────────
# 2.0 and 2.1 declared; tags up to v2.1.1. This is the shape the gate must
# accept: STABLE is the newest published minor, MAINTENANCE is the one before
# it, every tracked minor is declared, every `latest` exists as a tag.
CONSISTENT='{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"STABLE","branch":null,"latest":"2.1.1","released_at":"2026-09-11T16:33:39+01:00","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"baseline: predates support tracking"}
  ]
}'
BASELINE_TAGS=(v2.0.0 v2.1.0 v2.1.1)

echo "== estado consistente: el gate debe pasar =="
FX_OK="$(new_fixture ok)"
set_lines "$FX_OK" "$CONSISTENT"
add_tags "$FX_OK" "${BASELINE_TAGS[@]}" v1.0.0 v1.1.0
expect "$FX_OK" pass "STABLE=2.1 es la minor publicada vigente" --scope=published
expect "$FX_OK" pass "minors 1.x bajo el piso de tracking no se exigen" --scope=published
expect "$FX_OK" pass "proyección SUPPORT.md al día" --scope=declared
expect "$FX_OK" pass "ambos scopes juntos" --scope=both

echo "== un rc no es una publicación: no debe voltear la ventana =="
FX_RC="$(new_fixture rc)"
set_lines "$FX_RC" "$CONSISTENT"
add_tags "$FX_RC" "${BASELINE_TAGS[@]}" v2.2.0-rc.1
expect "$FX_RC" pass "v2.2.0-rc.1 no cuenta como minor publicada" --scope=published

echo "== la deriva que la issue describe: la debe FALLAR =="
FX_DRIFT="$(new_fixture drift)"
set_lines "$FX_DRIFT" "$CONSISTENT"
add_tags "$FX_DRIFT" "${BASELINE_TAGS[@]}" v2.2.0 v2.3.0 v2.3.1 v2.4.0 v2.4.1
# NEGATION: pre-#1676 no gate read support.json at all, so a STABLE three
# minors behind the newest tag passed CI; this case is the drift the gate exists to catch.
expect "$FX_DRIFT" fail "STABLE tres minors atrás del tag más nuevo" --scope=published
# NEGATION: pre-#1676 an unregistered published minor never failed anything —
# the hole the whole gate was filed to close.
expect "$FX_DRIFT" fail "minors publicadas sin declarar" --scope=published
expect "$FX_DRIFT" pass "pero el archivo sigue internamente coherente" --scope=declared

echo "== cada falla individual =="
FX_ONE="$(new_fixture one)"
set_lines "$FX_ONE" "$CONSISTENT"
add_tags "$FX_ONE" "${BASELINE_TAGS[@]}" v2.2.0
# NEGATION: pre-#1676 a single unregistered minor sailed through; the case
# pins that one is already a failure, not a threshold of several.
expect "$FX_ONE" fail "una minor publicada sin declarar" --scope=published

FX_STALE="$(new_fixture stale_maint)"
set_lines "$FX_STALE" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"MAINTENANCE","branch":null,"latest":"2.0.0","accepts":["security"]},
    {"minor":"2.1","state":"EOL","branch":null,"latest":"2.1.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
add_tags "$FX_STALE" v2.0.0 v2.1.0 v2.1.1 v2.2.0
# NEGATION: pre-#2054-adjacent rotation era, a MAINTENANCE not adjacent to
# STABLE was never compared; the chain-break that keeps a dead line looking
# supported was invisible.
expect "$FX_STALE" fail "MAINTENANCE que no es la minor publicada anterior" --scope=published

FX_PHANTOM="$(new_fixture phantom)"
set_lines "$FX_PHANTOM" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.9","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_PHANTOM" v2.0.0 v2.1.0 v2.1.1 v2.2.0
# NEGATION: the phantom check existed, but as a bare tag-existence assert (B1) —
# pre-T6 nothing demanded .latest be the NEWEST patch of its line; still a negation
# case for the tag-existence half the strengthened check retains.
expect "$FX_PHANTOM" fail "campo latest sin tag que lo respalde" --scope=published

echo "== el .latest declarado es un hecho, no una intención (S11/B1, #2060) =="
# The verbatim acceptance case: a 2.13 line declaring "latest": "2.12.0" must
# fail. Both scopes notice — binding (.latest starts with the line's minor) in
# the declared scope, newest-patch in the published scope.
FX_CROSS_MINOR="$(new_fixture cross_minor)"
set_lines "$FX_CROSS_MINOR" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.13","state":"STABLE","branch":null,"latest":"2.12.0","accepts":["bugfix","security"]},
    {"minor":"2.12","state":"MAINTENANCE","branch":null,"latest":"2.12.0","accepts":["security"]}
  ]
}'
add_tags "$FX_CROSS_MINOR" v2.12.0 v2.13.0
expect "$FX_CROSS_MINOR" fail "2.13 declarando latest 2.12.0 falla published (aceptación S11 verbatim)" --scope=published
expect "$FX_CROSS_MINOR" fail "el .latest de una línea empieza por su propia minor" --scope=declared

# The latest lags its OWN line: a real patch was published after the one
# declared. Pre-T6 the bare tag-existence phantom check approved this.
FX_LAG="$(new_fixture lag)"
set_lines "$FX_LAG" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_LAG" v2.0.0 v2.1.0 v2.1.1 v2.2.0 v2.2.1
expect "$FX_LAG" fail "latest declarado detrás del parche published de su propia línea" --scope=published

# Zero published patches for a line's minor is also a newest-patch failure:
# without any v2.4.* tag there is nothing to be the newest, so the declared
# fact has nothing to stand on (covers every state: STABLE here, others below).
FX_NOTAG_LINE="$(new_fixture notag_line)"
set_lines "$FX_NOTAG_LINE" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.4","state":"EOL","branch":null,"latest":"2.4.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
add_tags "$FX_NOTAG_LINE" v2.1.0 v2.1.1 v2.2.0
expect "$FX_NOTAG_LINE" fail "línea minor sin ningún parche publicado (el .latest declarado no puede ser el más nuevo)" --scope=published

# ─── The in-flight window of the sanctioned patch release (R2-1) ────────────
# The patch bump declares the `latest` of its line BEFORE the tag exists:
# cut-patch-tag.yml is workflow_dispatch-only and refuses a branch whose
# Cargo.toml does not carry the version, so the tag can only be cut AFTER the
# bump commit merges. In that window the declared `latest` is exactly ONE
# patch above the newest published patch of its own minor — a pending
# publication, not a stale claim.
#
# The gate runs on EVERY pull_request, so a red verdict in that window makes
# the bump PR unmergeable and the sanctioned release impossible to execute.
# The tolerance below is therefore as narrow as the state allows: one patch,
# same minor, no tag of its own. Everything around it keeps failing.
echo "== la ventana en vuelo del bump de parche: un parche por delante es una publicación pendiente =="

# (a) THE WINDOW: declared one patch above the newest published patch of its
# own minor, with no tag of its own yet. The gate must PASS.
FX_PENDING="$(new_fixture pending)"
set_lines "$FX_PENDING" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.1","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_PENDING" v2.0.0 v2.1.0 v2.1.1 v2.2.0
run_gate "$FX_PENDING" --scope=published
if [[ "$GATE_RC" -eq 0 ]]; then
  pass "un parche declarado por delante del publicado (publicación pendiente) pasa el gate"
else
  fail "el gate rechazó la ventana en vuelo del bump (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi
# The acceptance must be READABLE: a note that names the line and says the
# publication is pending, and NOT a `::error` annotation.
if grep -qF 'publicación pendiente' <<<"$GATE_OUT" && grep -qF '2.2.1' <<<"$GATE_OUT"; then
  pass "la ventana en vuelo se anuncia nombrando la línea y la publicación pendiente"
else
  fail "la aceptación de la ventana en vuelo no se distingue del silencio: $(tr '\n' ' ' <<<"$GATE_OUT")"
fi
if ! grep -q '::error' <<<"$GATE_OUT"; then
  pass "la ventana en vuelo no produce anotación de error"
else
  fail "la ventana en vuelo produjo una anotación de error: $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# (b) TWO patches ahead: a declaration that lies. The gate must FAIL, with
# the same message shape it uses for every other stale claim.
FX_TWO_AHEAD="$(new_fixture pending_two)"
set_lines "$FX_TWO_AHEAD" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.2","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_TWO_AHEAD" v2.0.0 v2.1.0 v2.1.1 v2.2.0
run_gate "$FX_TWO_AHEAD" --scope=published
if [[ "$GATE_RC" -ne 0 ]] \
   && grep -qF 'v2.2: declarado v2.2.2, el parche published más nuevo es v2.2.0' <<<"$GATE_OUT"; then
  pass "dos parches por delante siguen siendo un error (una declaración que miente)"
else
  fail "dos parches por delante no fallaron con el veredicto esperado (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# (c) BEHIND the published patch: the drift this gate exists to catch. The
# tolerance must NOT reopen it.
FX_BEHIND="$(new_fixture pending_behind)"
set_lines "$FX_BEHIND" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_BEHIND" v2.0.0 v2.1.0 v2.1.1 v2.2.0 v2.2.1
run_gate "$FX_BEHIND" --scope=published
if [[ "$GATE_RC" -ne 0 ]] \
   && grep -qF 'v2.2: declarado v2.2.0, el parche published más nuevo es v2.2.1' <<<"$GATE_OUT"; then
  pass "un .latest declarado detrás del parche published sigue fallando (la tolerancia no lo reabre)"
else
  fail "un .latest declarado detrás del parche published dejó de fallar (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# (d) A line whose minor has NO published patch at all: the declaration has
# no fact to stand on. The tolerance must not rescue it by borrowing a
# neighbour's tag.
FX_NO_PATCH="$(new_fixture pending_no_patch)"
set_lines "$FX_NO_PATCH" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.0","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]},
    {"minor":"2.4","state":"EOL","branch":null,"latest":"2.4.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
add_tags "$FX_NO_PATCH" v2.0.0 v2.1.0 v2.1.1 v2.2.0
run_gate "$FX_NO_PATCH" --scope=published
if [[ "$GATE_RC" -ne 0 ]] \
   && grep -qF 'v2.4: declarado v2.4.0, sin ningún parche X.Y.Z publicado' <<<"$GATE_OUT"; then
  pass "una minor sin ningún parche published sigue fallando cerrado"
else
  fail "una minor sin ningún parche published dejó de fallar (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# (e) The numeric comparison is base 10: patch 9 -> 10 is a real increment,
# and the tolerated declaration must be recognized across it.
FX_TEN="$(new_fixture pending_minor10)"
set_lines "$FX_TEN" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.10","state":"STABLE","branch":null,"latest":"2.10.10","accepts":["bugfix","security"]},
    {"minor":"2.9","state":"MAINTENANCE","branch":null,"latest":"2.9.1","accepts":["security"]}
  ]
}'
add_tags "$FX_TEN" v2.9.0 v2.9.1 v2.10.0 v2.10.9
run_gate "$FX_TEN" --scope=published
if [[ "$GATE_RC" -eq 0 ]]; then
  pass "la ventana en vuelo se reconoce también en el salto numérico 9 -> 10"
else
  fail "el salto numérico 9 -> 10 no se reconoció como ventana en vuelo (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# Positive control that keeps "-rc.N" out of the published set: an rc tag for a
# newer patch must NOT make the check demand it; the case still fails only if
# a real X.Y.Z tag is missing. Here v2.2.1 is the newest publication and passes.
FX_PREREL="$(new_fixture prerel)"
set_lines "$FX_PREREL" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.2","state":"STABLE","branch":null,"latest":"2.2.1","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]}
  ]
}'
add_tags "$FX_PREREL" v2.1.0 v2.1.1 v2.2.0 v2.2.1 v2.2.2-rc.1
expect "$FX_PREREL" pass "un -rc.N nunca es un parche publicado (v2.2.1 sigue siendo el más nuevo)" --scope=published

FX_WINDOW="$(new_fixture window)"
set_lines "$FX_WINDOW" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.3","state":"STABLE","branch":null,"latest":"2.3.0","accepts":["bugfix","security"]},
    {"minor":"2.2","state":"MAINTENANCE","branch":null,"latest":"2.2.0","accepts":["security"]},
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.0","accepts":["security"]}
  ]
}'
add_tags "$FX_WINDOW" v2.0.0 v2.1.0 v2.2.0 v2.3.0
# NEGATION: pre-#1676 the two-live-lines window was prose in AGENTS.md, not a
# check; three live lines passed and the support routing table lied.
expect "$FX_WINDOW" fail "tres líneas vivas (el máximo es dos)" --scope=declared

# Everything EOL is internally consistent — valid schema, no `accepts`, correct
# render — yet it declares that nothing is supported. Only the published scope
# would notice, and that scope is advisory (#1676 review R4-NO-ACTIVE-STABLE).
FX_NOSTABLE="$(new_fixture no_stable)"
set_lines "$FX_NOSTABLE" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"EOL","branch":null,"latest":"2.1.0","eol_at":"2026-10-01","eol_reason":"fixture"},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_NOSTABLE" v2.0.0 v2.1.0 v2.1.1
# NEGATION: pre-#1676-review (R4-NO-ACTIVE-STABLE) an all-EOL declaration passed
# the declared scope even though it states nothing is supported at all.
expect "$FX_NOSTABLE" fail "ninguna línea STABLE declarada (nada soportado)" --scope=declared

FX_ACCEPTS="$(new_fixture accepts)"
set_lines "$FX_ACCEPTS" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-09-14","eol_reason":"baseline"}
  ]
}'
add_tags "$FX_ACCEPTS" v2.0.0 v2.1.0 v2.1.1
# NEGATION: pre-#2003 the accepts table was never machine-checked against the
# state, so MAINTENANCE (security-only) carrying bugfix passed uncontested.
expect "$FX_ACCEPTS" fail "MAINTENANCE que acepta bugfix (security-only)" --scope=declared

# An EOL line is not allowed to carry `accepts` at all: the audit (A4) found the
# branch checked with `(.accepts // empty) != empty`, which produces zero outputs
# in jq and can never be true, so an "accepts" dangling on an EOL line sailed
# through. A falsifiable EOL case: if the branch stays dead, this passes and the
# harness fails.
FX_EOL_ACCEPTS="$(new_fixture eol_accepts)"
set_lines "$FX_EOL_ACCEPTS" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"STABLE","branch":null,"latest":"2.1.1","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"baseline","accepts":["security"]}
  ]
}'
add_tags "$FX_EOL_ACCEPTS" v2.0.0 v2.1.0 v2.1.1
# NEGATION: pre-fix of the S6/A4 audit finding the EOL-accepts branch was a jq
# predicate producing zero outputs (dead), so `accepts` on an EOL line always passed.
expect "$FX_EOL_ACCEPTS" fail "EOL que declara accepts (la rama muerta debe encender)" --scope=declared

echo "== fail-closed: lo que el gate no puede ver no lo aprueba =="
FX_BAD="$(new_fixture badjson)"
printf '{ not json\n' > "$FX_BAD/.github/support.json"
# NEGATION: pre-#1676 the gate early-returned rc=0 on unreadable JSON — the
# "skip when blind" shape the fail-closed header exists to forbid.
expect "$FX_BAD" fail "JSON ilegible" --scope=declared
# NEGATION: sibling of the declared-scope case — the published scope must reject
# unreadable JSON too, never approve a claim it could not parse.
expect "$FX_BAD" fail "JSON ilegible también en published" --scope=published

FX_NOTAGS="$(new_fixture notags)"
set_lines "$FX_NOTAGS" "$CONSISTENT"
# NEGATION: pre-#1676 an empty tag list silently approved whatever the
# declaration claimed (a tagless repo is unverifiable, not verified).
expect "$FX_NOTAGS" fail "repo sin tags publicados" --scope=published

FX_SHALLOW="$(new_fixture shallow)"
set_lines "$FX_SHALLOW" "$CONSISTENT"
add_tags "$FX_SHALLOW" "${BASELINE_TAGS[@]}"
CLONE="$WORK/shallow-clone"
git clone -q --depth 1 "file://$FX_SHALLOW" "$CLONE"
# NEGATION: pre-#1676 a shallow clone (fetch-depth: 0 missing) reported green on
# tags it could not see — the CI default, so the gate was blind exactly where it ran.
expect "$CLONE" fail "clone shallow (no puede ver los tags)" --scope=published

echo "== el gate no escribe: la proyección tiene un solo escritor =="
FX_NOWRITE="$(new_fixture nowrite)"
set_lines "$FX_NOWRITE" "$CONSISTENT"
add_tags "$FX_NOWRITE" "${BASELINE_TAGS[@]}"
( cd "$FX_NOWRITE" && ./scripts/check_support_drift.sh >/dev/null 2>&1 ) || true
if [[ -z "$(git -C "$FX_NOWRITE" status --porcelain)" ]]; then
  pass "correr el gate deja el worktree intacto"
else
  fail "el gate modificó el worktree: $(git -C "$FX_NOWRITE" status --porcelain | tr '\n' ' ')"
fi

# ─── The sanctioned rotation must CONVERGE on a state the gate accepts ──────
# If it did not, the gate would be demanding a hand-edited file — which is the
# second half of #1676: the forbidden path being the only working one.
echo "== la rotación sancionada converge a un estado que el gate acepta =="
FX_ROT="$(new_fixture rotate)"
set_lines "$FX_ROT" "$CONSISTENT"
add_tags "$FX_ROT" "${BASELINE_TAGS[@]}" v2.2.0
( cd "$FX_ROT" && ./scripts/rotate-stable.sh 2.2 >/dev/null )
state="$(jq -r '.lines[] | "\(.minor)=\(.state)"' "$FX_ROT/.github/support.json" | tr '\n' ' ')"
if [[ "$state" == "2.2=STABLE 2.1=MAINTENANCE 2.0=EOL " ]]; then
  pass "rotate-stable.sh 2.2 deja 2.2 STABLE / 2.1 MAINTENANCE, en orden descendente"
else
  fail "rotate-stable.sh 2.2 dejó: $state"
fi
( cd "$FX_ROT" && ./scripts/render-support-md.sh >/dev/null )
if [[ "$(grep -m1 '^| 2\.' "$FX_ROT/SUPPORT.md")" == "| 2.2 | STABLE |"* ]]; then
  pass "la tabla renderizada de SUPPORT.md lista primero la línea vigente"
else
  fail "la tabla de SUPPORT.md no empieza por la línea vigente: $(grep -m1 '^| 2\.' "$FX_ROT/SUPPORT.md")"
fi
expect "$FX_ROT" pass "el estado post-rotación pasa el gate" --scope=published

echo "== rotate-stable.sh falla cerrado donde hoy produce un estado peor =="
FX_ROT_GAP="$(new_fixture rotate_gap)"
set_lines "$FX_ROT_GAP" "$CONSISTENT"
add_tags "$FX_ROT_GAP" "${BASELINE_TAGS[@]}" v2.2.0 v2.3.0 v2.4.0
before="$(cat "$FX_ROT_GAP/.github/support.json")"
rc=0
( cd "$FX_ROT_GAP" && ./scripts/rotate-stable.sh 2.4 ) >/dev/null 2>&1 || rc=$?
if [[ $rc -ne 0 ]]; then
  pass "rechaza rotar 2.1 -> 2.4 por encima de minors sin declarar (2.2, 2.3)"
else
  fail "aceptó una rotación que deja minors publicadas sin declarar"
fi
if [[ "$(cat "$FX_ROT_GAP/.github/support.json")" == "$before" ]]; then
  pass "el rechazo deja support.json byte a byte intacto"
else
  fail "el rechazo mutó support.json antes de fallar"
fi

# ─── The .9/.10 boundary: where `comm` stops working ────────────────────────
# The gap check sorted both sides with `sort -V` (version order: 2.9 before
# 2.10) but compared them with `comm`, which requires the LOCALE's collation
# order (2.10 before 2.9). The moment the declared lines cross that pair, comm
# calls the input unsorted and aborts the whole rotation — the automation
# failing exactly on the release that needs it. Both cases below are the same
# repo, one minor past the boundary.
echo "== la rotación sana cruza el par .9/.10 (donde comm abortaba) =="
FX_ROT_TEN="$(new_fixture rotate_minor10)"
set_lines "$FX_ROT_TEN" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.10","state":"STABLE","branch":null,"latest":"2.10.1","released_at":"2026-09-11T16:33:39+01:00","accepts":["bugfix","security"]},
    {"minor":"2.9","state":"EOL","branch":null,"latest":"2.9.1","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"ventana rotada"}
  ]
}'
add_tags "$FX_ROT_TEN" v2.9.0 v2.9.1 v2.10.0 v2.10.1 v2.11.0
rc=0
( cd "$FX_ROT_TEN" && ./scripts/rotate-stable.sh 2.11 ) >/dev/null 2>&1 || rc=$?
if [[ $rc -eq 0 ]]; then
  pass "2.10 -> 2.11 rota sobre un set de tags cuyo orden de versión difiere del de collation"
else
  fail "la rotación abortó (rc=$rc): el orden de las entradas, no la gobernanza, decidió"
fi
state="$(jq -r '.lines[] | "\(.minor)=\(.state)"' "$FX_ROT_TEN/.github/support.json" | tr '\n' ' ')"
if [[ "$state" == "2.11=STABLE 2.10=MAINTENANCE 2.9=EOL " ]]; then
  pass "el estado post-rotación cruza el par .9/.10 en el orden correcto"
else
  fail "rotación .9/.10 incorrecta: $state"
fi
( cd "$FX_ROT_TEN" && ./scripts/render-support-md.sh >/dev/null )
expect "$FX_ROT_TEN" pass "el estado post-rotación de la minor 10 pasa el gate" --scope=both

echo "== el gap también se decide por versión, no por collation =="
FX_ROT_GAP_TEN="$(new_fixture rotate_gap_minor10)"
set_lines "$FX_ROT_GAP_TEN" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.8","state":"STABLE","branch":null,"latest":"2.8.1","released_at":"2026-09-11T16:33:39+01:00","accepts":["bugfix","security"]},
    {"minor":"2.7","state":"EOL","branch":null,"latest":"2.7.0","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"ventana rotada"}
  ]
}'
add_tags "$FX_ROT_GAP_TEN" v2.7.0 v2.8.0 v2.8.1 v2.9.0 v2.10.0 v2.11.0
before="$(cat "$FX_ROT_GAP_TEN/.github/support.json")"
rc=0
out="$( cd "$FX_ROT_GAP_TEN" && ./scripts/rotate-stable.sh 2.11 2>&1 )" || rc=$?
if [[ $rc -ne 0 ]] && grep -qF 'entre 2.8 y 2.11: 2.9 2.10' <<<"$out"; then
  pass "rechaza 2.8 -> 2.11 nombrando el gap del par .9/.10 (2.9, 2.10)"
else
  fail "el rechazo del gap .9/.10 no es el de gobernanza (rc=$rc): $(tr '\n' ' ' <<<"$out")"
fi
if [[ "$(cat "$FX_ROT_GAP_TEN/.github/support.json")" == "$before" ]]; then
  pass "el rechazo del gap .9/.10 deja support.json byte a byte intacto"
else
  fail "el rechazo del gap .9/.10 mutó support.json antes de fallar"
fi

FX_ROT_UNTAGGED="$(new_fixture rotate_untagged)"
set_lines "$FX_ROT_UNTAGGED" "$CONSISTENT"
add_tags "$FX_ROT_UNTAGGED" "${BASELINE_TAGS[@]}"
rc=0
( cd "$FX_ROT_UNTAGGED" && ./scripts/rotate-stable.sh 2.9 ) >/dev/null 2>&1 || rc=$?
if [[ $rc -ne 0 ]]; then
  pass "rechaza rotar a una minor sin tag v2.9.0 publicado"
else
  fail "aceptó rotar a una minor nunca publicada"
fi

FX_ROT_NOSTABLE="$(new_fixture rotate_nostable)"
set_lines "$FX_ROT_NOSTABLE" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"MAINTENANCE","branch":null,"latest":"2.1.1","accepts":["security"]}
  ]
}'
add_tags "$FX_ROT_NOSTABLE" "${BASELINE_TAGS[@]}" v2.2.0
rc=0
( cd "$FX_ROT_NOSTABLE" && ./scripts/rotate-stable.sh 2.2 ) >/dev/null 2>&1 || rc=$?
if [[ $rc -ne 0 ]]; then
  pass "rechaza rotar sin una línea STABLE declarada que degradar"
else
  fail "aceptó una rotación sin STABLE previa (degradaría nada)"
fi

# ─── The drift gate crosses the pair too (S13, #2054) ───────────────────────
# check_support_drift.sh had the same comm/sort-V pairing in its
# unregistered-minors check: `comm` demands the LOCALE's collation order while
# both sides are version-sorted, so the first tracked/declared pair whose
# published minors cross 2.9/2.10 aborts the whole gate with collation noise
# instead of the governance verdict. Below, the red case diverges the sets
# across that boundary (a genuinely unregistered 2.9 under a declared 2.8);
# the green case is the healthy equal-set declaration ('#2062'-era
# rotate-state clone) that must keep passing.
echo "== el gate de drift cruza el par .9/.10 (donde comm abortaba) =="
FX_GATE_TEN="$(new_fixture gate_minor10)"
set_lines "$FX_GATE_TEN" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.10","state":"STABLE","branch":null,"latest":"2.10.1","released_at":"2026-09-11T16:33:39+01:00","accepts":["bugfix","security"]},
    {"minor":"2.8","state":"EOL","branch":null,"latest":"2.8.1","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"ventana rotada"}
  ]
}'
add_tags "$FX_GATE_TEN" v2.8.0 v2.8.1 v2.9.0 v2.10.0 v2.10.1
# NEGATION: pre-fix the unregistered-minors check ran `comm` over version-sorted
# input; with the sets diverging across the .9/.10 pair comm aborted the gate
# with 'input is not in sorted order' noise and rc=1 while producing the
# collation error INSTEAD OF the governance verdict naming the unregistered
# minor — the drift check never reached its own diagnosis.
gate_rc=0
gate_out="$( ( cd "$FX_GATE_TEN" && ./scripts/check_support_drift.sh --scope=published ) 2>&1 )" || gate_rc=$?
if [[ $gate_rc -ne 0 ]] && grep -qF 'minors publicadas sin declarar en support.json: 2.9' <<<"$gate_out"; then
  pass "marca 2.9 sin declarar cruzando el par .9/.10 con el veredicto de gobernanza"
else
  fail "el gate no dio el veredicto de gobernanza (rc=$gate_rc): $(tr '\n' ' ' <<<"$gate_out")"
fi
if ! grep -qF 'sorted order' <<<"$gate_out"; then
  pass "el veredicto no es el ruido de comm (no hay 'not in sorted order')"
else
  fail "el diagnóstico sigue siendo el ruido de collation de comm"
fi

FX_GATE_HEALTHY="$(new_fixture gate_minor10_healthy)"
set_lines "$FX_GATE_HEALTHY" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.10","state":"STABLE","branch":null,"latest":"2.10.1","released_at":"2026-09-11T16:33:39+01:00","accepts":["bugfix","security"]},
    {"minor":"2.9","state":"EOL","branch":null,"latest":"2.9.1","released_at":"2026-09-05T18:58:40+01:00","eol_at":"2026-09-14","eol_reason":"ventana rotada"}
  ]
}'
add_tags "$FX_GATE_HEALTHY" v2.9.0 v2.9.1 v2.10.0 v2.10.1
expect "$FX_GATE_HEALTHY" pass "la declaración sana de sets iguales cruza el par .9/.10 sin abortar" --scope=published

# ─── The diagnosis must name the surface, not the job ────────────────────────
# A required check is identified by its JOB name, so "Validate PR metadata"
# failing says nothing about why. The annotation title and the step summary are
# the only carriers available: assert both, and assert that a green run stays
# silent so the summary never accumulates noise.
echo "== en CI, el diagnóstico nombra el archivo y el tag =="
FX_DIAG="$(new_fixture diag)"
set_lines "$FX_DIAG" "$CONSISTENT"
add_tags "$FX_DIAG" "${BASELINE_TAGS[@]}" v2.2.0
SUM_FAIL="$WORK/summary-fail.md"
rc=0
OUT="$( ( cd "$FX_DIAG" && GITHUB_STEP_SUMMARY="$SUM_FAIL" ./scripts/check_support_drift.sh --scope=published ) 2>&1 )" || rc=$?
if [[ $rc -ne 0 ]] && grep -qF '::error title=Support drift: .github/support.json vs tags v*' <<<"$OUT"; then
  pass "la anotación lleva el archivo en el título, no sólo el mensaje"
else
  fail "la anotación no lleva título con el archivo (rc=$rc)"
fi
if [[ -f "$SUM_FAIL" ]] && grep -qF '.github/support.json' "$SUM_FAIL" \
   && grep -qF 'v2.2.0' "$SUM_FAIL" && grep -qF '| Declared STABLE |' "$SUM_FAIL" \
   && grep -qF '2.1' "$SUM_FAIL"; then
  pass "el step summary muestra el tag más nuevo y la STABLE declarada"
else
  fail "el step summary no explica la deriva: $(head -n 6 "$SUM_FAIL" 2>/dev/null | tr '\n' ' ')"
fi
SUM_OK="$WORK/summary-ok.md"
( cd "$FX_OK" && GITHUB_STEP_SUMMARY="$SUM_OK" ./scripts/check_support_drift.sh --scope=both ) >/dev/null 2>&1 || true
if [[ ! -s "$SUM_OK" ]]; then
  pass "un run verde no escribe summary"
else
  fail "un run verde escribió summary: $(tr '\n' ' ' < "$SUM_OK")"
fi

# ─── A minor is declared EXACTLY ONCE (B13) ─────────────────────────────────
# The declared scope validated the COUNTS, the states and the grammars of
# .lines[], but never the UNIQUENESS of .minor: a declaration naming the same
# line twice passed the scope untouched, and the rendered projection carried
# two rows for one line. Every lookup by minor (the STABLE one, the tracking
# floor) would then have two answers and no rule to pick one.
echo "== una minor se declara UNA sola vez (B13) =="
FX_DUP="$(new_fixture duplicate_minor)"
set_lines "$FX_DUP" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"STABLE","branch":null,"latest":"2.1.1","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"EOL","branch":null,"latest":"2.1.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
run_gate "$FX_DUP" --scope=declared
if [[ "$GATE_RC" -ne 0 ]] && grep -qF 'minor declarada más de una vez' <<<"$GATE_OUT" \
   && grep -qF '2.1' <<<"$GATE_OUT"; then
  pass "una minor declarada dos veces falla el scope declared nombrando la minor"
else
  fail "una minor declarada dos veces pasó el scope declared (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# Control: the same shape with DISTINCT minors is a legal declaration.
FX_DISTINCT="$(new_fixture distinct_minor)"
set_lines "$FX_DISTINCT" "$CONSISTENT"
expect "$FX_DISTINCT" pass "control: minors distintas pasan el scope declared" --scope=declared

# The published scope is the REQUIRED check in CI (pr-validation.yml), so a
# duplicated minor must not survive it either: the declaration it verifies is
# the same file.
FX_DUP_PUB="$(new_fixture duplicate_minor_published)"
set_lines "$FX_DUP_PUB" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.1","state":"STABLE","branch":null,"latest":"2.1.1","accepts":["bugfix","security"]},
    {"minor":"2.1","state":"EOL","branch":null,"latest":"2.1.1","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
add_tags "$FX_DUP_PUB" v2.1.0 v2.1.1
run_gate "$FX_DUP_PUB" --scope=published
if [[ "$GATE_RC" -ne 0 ]] && grep -qF 'minor declarada más de una vez' <<<"$GATE_OUT"; then
  pass "una minor declarada dos veces falla también el scope published (el gate requerido)"
else
  fail "una minor declarada dos veces pasó el scope published (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# ─── The producer of the declared minors (B14) ─────────────────────────────
# The producer used to be `jq -r '.lines[].minor' ... || sort -V || true`.
# `||` binds left to right, so `sort` only ran when jq FAILED — and then it
# read EMPTY stdin: a file that could not be read produced an EMPTY list with
# exit 0, and the correctness was supplied further down by a re-sort that
# masked the producer's own bug. The producer must be right AT THE SOURCE:
# sorted on success, a FAILURE on a file it cannot read.
echo "== el productor de minors declaradas: ordenado en el origen y fail-closed (B14) =="

# The producer is a named function in the shipped script, and the harness
# evaluates THAT function — the file that ships, not a paraphrase of it. The
# script cannot be sourced as a whole because sourcing it RUNS the gate.
# An empty extraction means the producer is gone: the row then fails loudly
# instead of passing on an undefined function.
producer_source() {
  sed -n '/^declared_minors()/,/^}/p' "$REPO_ROOT/scripts/check_support_drift.sh"
}
run_producer() {
  local dir="$1"
  (
    cd "$dir" || exit 1
    SUPPORT_JSON=".github/support.json"
    eval "$(producer_source)"
    declared_minors
  )
}

# The declaration lists its minors in a shape where the two orders disagree:
# version order is 2.9, 3.0, 10.0; lexicographic order is 10.0, 2.9, 3.0.
FX_PROD="$(new_fixture producer_order)"
set_lines "$FX_PROD" '{
  "schema_version": 1,
  "lines": [
    {"minor":"10.0","state":"STABLE","branch":null,"latest":"10.0.0","accepts":["bugfix","security"]},
    {"minor":"3.0","state":"MAINTENANCE","branch":null,"latest":"3.0.0","accepts":["security"]},
    {"minor":"2.9","state":"EOL","branch":null,"latest":"2.9.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
producer_rc=0
producer_out="$(run_producer "$FX_PROD")" || producer_rc=$?
if [[ -n "$(producer_source)" ]] && [[ $producer_rc -eq 0 ]] \
   && [[ "$producer_out" == $'2.9\n3.0\n10.0' ]]; then
  pass "el productor entrega las minors declaradas en orden de versión (2.9, 3.0, 10.0)"
else
  fail "el productor no entrega la lista en orden de versión (rc=$producer_rc): $(tr '\n' ' ' <<<"$producer_out")"
fi

# The same producer against a file it cannot read: the answer must be a
# FAILURE, never an empty list that looks like "this declaration declares
# nothing".
FX_PROD_BAD="$(new_fixture producer_unreadable)"
printf '{ not json\n' > "$FX_PROD_BAD/.github/support.json"
producer_rc=0
producer_out="$(run_producer "$FX_PROD_BAD")" || producer_rc=$?
if [[ -n "$(producer_source)" ]] && [[ $producer_rc -ne 0 ]]; then
  pass "el productor falla (no devuelve una lista vacía) ante un support.json ilegible"
else
  fail "el productor devolvió una lista vacía con éxito ante un support.json ilegible (rc=$producer_rc)"
fi

# The gate must SAY that it could not read the declaration, instead of
# approving it on the strength of an empty list (or blaming the STABLE line
# for a file nobody could parse).
run_gate "$FX_BAD" --scope=published
if [[ "$GATE_RC" -ne 0 ]] && grep -qF 'no se pudo leer la lista de minors declaradas' <<<"$GATE_OUT" \
   && ! grep -qF 'no hay ninguna línea STABLE declarada' <<<"$GATE_OUT"; then
  pass "el scope published falla ruidosamente ante un support.json ilegible"
else
  fail "el scope published no falló ruidosamente ante un support.json ilegible (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# The tracking floor is the OLDEST declared minor, and the producer hands the
# declared minors in version order, so the floor is the producer's FIRST
# element. With a lexicographic producer ("10.0" before "2.0") the floor
# would be 10.0 and the published 9.0 would fall BELOW it, escaping the rule
# that every tracked minor must be declared. The re-sort that used to live
# next to the floor was the mask that hid the producer's bug; it is gone, so
# this row pins the producer's order where it actually decides something.
FX_FLOOR="$(new_fixture floor_order)"
set_lines "$FX_FLOOR" '{
  "schema_version": 1,
  "lines": [
    {"minor":"10.0","state":"STABLE","branch":null,"latest":"10.0.0","accepts":["bugfix","security"]},
    {"minor":"2.0","state":"EOL","branch":null,"latest":"2.0.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}'
add_tags "$FX_FLOOR" v2.0.0 v9.0.0 v10.0.0
run_gate "$FX_FLOOR" --scope=published
if [[ "$GATE_RC" -ne 0 ]] && grep -qF 'minors publicadas sin declarar en support.json: 9.0' <<<"$GATE_OUT"; then
  pass "el piso de tracking usa el orden de versión del productor (9.0 no escapa del piso 2.0)"
else
  fail "el piso de tracking no usa el orden de versión del productor (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# ─── A malformed tag is REPORTED, a candidate is not (B15) ─────────────────
# The unparseable computation pre-filtered the hyphens — the filter that
# excludes CANDIDATES (-rc.N) from the published set — so a malformed tag
# such as v2-14-0 was dropped before it could ever reach the report. The two
# filters carry OPPOSITE intents: one excludes candidates from the published
# set, the other reports what is not a well-formed tag at all.
echo "== un tag malformado se REPORTA; un candidato -rc.N no (B15) =="
FX_MALFORMED="$(new_fixture malformed_tag)"
set_lines "$FX_MALFORMED" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.13","state":"STABLE","branch":null,"latest":"2.13.0","accepts":["bugfix","security"]}
  ]
}'
add_tags "$FX_MALFORMED" v2.13.0 v2.14.0-rc.1 v2-14-0
run_gate "$FX_MALFORMED" --scope=published
if [[ "$GATE_RC" -ne 0 ]] && grep -qF 'v2-14-0' <<<"$GATE_OUT"; then
  pass "un tag malformado (v2-14-0) aparece en el reporte de tags con formato inesperado"
else
  fail "un tag malformado no fue reportado (rc=$GATE_RC): $(tr '\n' ' ' <<<"$GATE_OUT")"
fi
if ! grep -qF 'v2.14.0-rc.1' <<<"$GATE_OUT"; then
  pass "un candidato -rc.N no se reporta como tag malformado"
else
  fail "un candidato -rc.N se reportó como tag malformado: $(tr '\n' ' ' <<<"$GATE_OUT")"
fi

# Control: the same candidate WITHOUT the malformed tag. The candidate must
# not count as a published patch either — if it did, the newest published
# minor would be 2.14 and the declaration (STABLE 2.13) would drift.
FX_RC_ONLY="$(new_fixture rc_only)"
set_lines "$FX_RC_ONLY" '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.13","state":"STABLE","branch":null,"latest":"2.13.0","accepts":["bugfix","security"]}
  ]
}'
add_tags "$FX_RC_ONLY" v2.13.0 v2.14.0-rc.1
expect "$FX_RC_ONLY" pass "un candidato -rc.N no es una publicación (v2.13.0 sigue siendo la más nueva)" --scope=published

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]