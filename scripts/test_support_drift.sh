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
expect "$FX_DRIFT" fail "STABLE tres minors atrás del tag más nuevo" --scope=published
expect "$FX_DRIFT" fail "minors publicadas sin declarar" --scope=published
expect "$FX_DRIFT" pass "pero el archivo sigue internamente coherente" --scope=declared

echo "== cada falla individual =="
FX_ONE="$(new_fixture one)"
set_lines "$FX_ONE" "$CONSISTENT"
add_tags "$FX_ONE" "${BASELINE_TAGS[@]}" v2.2.0
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
expect "$FX_PHANTOM" fail "campo latest sin tag que lo respalde" --scope=published

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
expect "$FX_ACCEPTS" fail "MAINTENANCE que acepta bugfix (security-only)" --scope=declared

echo "== fail-closed: lo que el gate no puede ver no lo aprueba =="
FX_BAD="$(new_fixture badjson)"
printf '{ not json\n' > "$FX_BAD/.github/support.json"
expect "$FX_BAD" fail "JSON ilegible" --scope=declared
expect "$FX_BAD" fail "JSON ilegible también en published" --scope=published

FX_NOTAGS="$(new_fixture notags)"
set_lines "$FX_NOTAGS" "$CONSISTENT"
expect "$FX_NOTAGS" fail "repo sin tags publicados" --scope=published

FX_SHALLOW="$(new_fixture shallow)"
set_lines "$FX_SHALLOW" "$CONSISTENT"
add_tags "$FX_SHALLOW" "${BASELINE_TAGS[@]}"
CLONE="$WORK/shallow-clone"
git clone -q --depth 1 "file://$FX_SHALLOW" "$CLONE"
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

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]