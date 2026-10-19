#!/usr/bin/env bash
# Semantics harness for scripts/cut-support-branch.sh (S11/B1, #2060).
#
# The branch point of support/X.Y must come from the TAGS, not from
# support.json's declared `latest`. support.json is a derived artifact: a
# stale `latest` (vX.Y.0 declared while vX.Y.1 is published) used to make the
# script cut the support branch from the WRONG commit, silently — a hotfix
# line that never saw the released patch. NEGATION cases below record the
# pre-fix verdict.
#
# Fixture: a real repo pushed to a LOCAL bare repo as `origin`, so the
# script's `git fetch --tags origin` and `git push origin $BRANCH` touch no
# network. Same git pinning discipline as test_support_drift.sh (no gpgsign,
# no hooks, no inherited global config).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fx <name>: repo + local bare origin, main pushed, support.json declared,
# cut script installed. Lines: 2.12 STABLE, 2.11 MAINTENANCE, 2.10 EOL.
new_fx() {
  local bare="$WORK/$1.git" repo="$WORK/$1-clone"
  mkdir -p "$repo/scripts"
  cp "$REPO_ROOT/scripts/cut-support-branch.sh" "$repo/scripts/"
  chmod +x "$repo/scripts/cut-support-branch.sh"
  git -C "$repo" init -q -b main
  git -C "$repo" config user.email fixture@example.invalid
  git -C "$repo" config user.name fixture
  git -C "$repo" config commit.gpgsign false
  git -C "$repo" config tag.gpgsign false
  git -C "$repo" config core.hooksPath /dev/null
  git -C "$repo" config commit.template /dev/null
  git init -q --bare "$bare"
  git -C "$repo" remote add origin "$bare"
  mkdir -p "$repo/.github"
  printf '{
  "schema_version": 1,
  "lines": [
    {"minor":"2.12","state":"STABLE","branch":null,"latest":"2.12.0","accepts":["bugfix","security"]},
    {"minor":"2.11","state":"MAINTENANCE","branch":null,"latest":"2.11.0","accepts":["security"]},
    {"minor":"2.10","state":"EOL","branch":null,"latest":"2.10.0","eol_at":"2026-10-01","eol_reason":"fixture"}
  ]
}
' > "$repo/.github/support.json"
  git -C "$repo" add -A
  git -C "$repo" commit -q -m init
  # Three empty walks so each tag above can pin a DISTINCT commit: the branch
  # point must be provable by SHA, not vacuous on a one-commit fixture.
  git -C "$repo" commit -q --allow-empty -m walks-1
  git -C "$repo" commit -q --allow-empty -m walks-2
  git -C "$repo" commit -q --allow-empty -m walks-3
  git -C "$repo" push -q origin main 2>/dev/null
  echo "$repo"
}

# add_tags <fixture> <tag...>: lightweight tags on HEAD (both in the repo and
# origin), so the tags the script may fetch equal the tags it can see locally.
add_tags() {
  local dir="$1"; shift
  local tag
  for tag in "$@"; do
    git -C "$dir" tag -f "$tag" >/dev/null
  done
  git -C "$dir" push -q origin --tags 2>/dev/null
}

# add_tag_at <fixture> <tag> <rev>: explicit placement when the assertion is
# about WHICH commit the branch point resolves to.
add_tag_at() {
  local dir="$1" tag="$2" rev="$3"
  git -C "$dir" tag -f "$tag" "$rev" >/dev/null
  git -C "$dir" push -q origin --tags 2>/dev/null
}

# run_cut <fixture> <minor> [args...]: LAST_RC / LAST_OUT carry the verdict.
run_cut() {
  local dir="$1" minor="$2"; shift 2
  local rc=0
  ( cd "$dir" && ./scripts/cut-support-branch.sh "$minor" "$@" ) > "$WORK/out.txt" 2>&1 || rc=$?
  LAST_RC=$rc
  LAST_OUT="$(cat "$WORK/out.txt")"
}

echo "== el punto de corte viene de los tags, no del campo declarado (S11) =="
FX_STALE="$(new_fx stale_latest)"
# The declared field says "2.12.0" but v2.12.1 was published: the stale-field
# case the spec names. The four tags pin four DIFFERENT commits so the SHA
# probes below are falsifiable. Pre-T6 the branch was cut from the declared
# field (v2.12.0) instead of from the newest published patch tag.
# NEGATION: observed RED pre-T6 (branch SHA == v2.12.0, "la rama se cortó del
# campo declared desactualizado": exactamente el defecto S11).
add_tag_at "$FX_STALE" v2.10.0 main~3
add_tag_at "$FX_STALE" v2.11.0 main~2
add_tag_at "$FX_STALE" v2.12.0 main~1
add_tag_at "$FX_STALE" v2.12.1 main
snap_json_before="$(cat "$FX_STALE/.github/support.json")"
run_cut "$FX_STALE" 2.12
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "cut 2.12 procede con el campo declared desactualizado"
else
  fail "cut 2.12 no procedió con el stale field (rc=$LAST_RC): $LAST_OUT"
fi
BRANCH_SHA="$(git -C "$FX_STALE" rev-parse refs/heads/support/2.12 2>/dev/null || echo MISSING)"
TAG131_SHA="$(git -C "$FX_STALE" rev-parse refs/tags/v2.12.1 2>/dev/null || echo MISSING)"
TAG120_SHA="$(git -C "$FX_STALE" rev-parse refs/tags/v2.12.0 2>/dev/null || echo MISSING)"
if [[ "$BRANCH_SHA" == "$TAG131_SHA" ]]; then
  pass "la rama nace del tag real v2.12.1 (no del campo declared v2.12.0)"
else
  fail "el punto de corte no es v2.12.1 (branch=$BRANCH_SHA, v2.12.1=$TAG131_SHA, v2.12.0=$TAG120_SHA)"
fi
if [[ "$BRANCH_SHA" != "$TAG120_SHA" ]]; then
  pass "la rama NO nace del campo declared desactualizado (v2.12.0)"
else
  fail "la rama se cortó del campo declared desactualizado (v2.12.0): exactamente el defecto S11"
fi
if grep -qF '"branch": "support/2.12"' "$FX_STALE/.github/support.json"; then
  pass "support.json anota la rama creada"
else
  fail "support.json no quedó anotado con la rama"
fi
if [[ -n "$(git -C "$FX_STALE" rev-parse origin/support/2.12 2>/dev/null)" ]]; then
  pass "el push al bare de origin alcanzó la rama"
else
  fail "la rama no llegó a origin"
fi

echo "== una minor sin ningún tag no tiene punto de corte: se niega =="
FX_NOTAGS="$(new_fx cut_notags)"
snap_json_before="$(cat "$FX_NOTAGS/.github/support.json")"
run_cut "$FX_NOTAGS" 2.11
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "cut 2.11 sin ningún tag v2.11.* se niega"
else
  fail "cut 2.11 sin tags procedió con rc=0"
fi
if git -C "$FX_NOTAGS" show-ref --verify --quiet "refs/heads/support/2.11"; then
  fail "se creó una rama de soporte desde ninguna parte: support/2.11 existe"
else
  pass "ninguna rama support/2.11 quedó creada"
fi
if [[ "$(cat "$FX_NOTAGS/.github/support.json")" == "$snap_json_before" ]]; then
  pass "support.json queda byte a byte intacto"
else
  fail "el rechazo mutó support.json"
fi

echo "== los guards preexistentes siguen intactos =="
FX_EOL="$(new_fx cut_eol)"
add_tags "$FX_EOL" v2.10.0
snap_json_before="$(cat "$FX_EOL/.github/support.json")"
run_cut "$FX_EOL" 2.10
if [[ "$LAST_RC" -ne 0 ]] && ! git -C "$FX_EOL" show-ref --verify --quiet "refs/heads/support/2.10"; then
  pass "una línea EOL se niega (guard preexistente)"
else
  fail "cut 2.10 (EOL) no se negó (rc=$LAST_RC)"
fi
if [[ "$(cat "$FX_EOL/.github/support.json")" == "$snap_json_before" ]]; then
  pass "el rechazo EOL deja support.json intacto"
else
  fail "el rechazo EOL mutó support.json"
fi

FX_UNKNOWN="$(new_fx cut_unknown)"
snap_json_before="$(cat "$FX_UNKNOWN/.github/support.json")"
run_cut "$FX_UNKNOWN" 9.9
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "una línea inexistente se niega (guard preexistente)"
else
  fail "cut 9.9 (inexistente) no se negó"
fi
if [[ "$(cat "$FX_UNKNOWN/.github/support.json")" == "$snap_json_before" ]]; then
  pass "el rechazo de línea inexistente deja support.json intacto"
else
  fail "el rechazo de línea inexistente mutó support.json"
fi

echo "== una rama ya materializada no se corta dos veces =="
FX_EXISTING="$(new_fx cut_existing)"
add_tags "$FX_EXISTING" v2.12.0 v2.12.1
git -C "$FX_EXISTING" branch support/2.12 "$(git -C "$FX_EXISTING" rev-parse refs/tags/v2.12.0)"
snap_json_before="$(cat "$FX_EXISTING/.github/support.json")"
run_cut "$FX_EXISTING" 2.12
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "una rama existente se niega (guard preexistente)"
else
  fail "cut 2.12 aceptó re-cortar una rama ya existente"
fi
BRANCH_SHA="$(git -C "$FX_EXISTING" rev-parse refs/heads/support/2.12)"
if [[ "$BRANCH_SHA" == "$(git -C "$FX_EXISTING" rev-parse refs/tags/v2.12.0)" ]]; then
  pass "la rama preexistente no fue movida"
else
  fail "la rama preexistente fue movida por el script: $BRANCH_SHA"
fi
if [[ "$(cat "$FX_EXISTING/.github/support.json")" == "$snap_json_before" ]]; then
  pass "support.json intacto en el rechazo de rama duplicada"
else
  fail "el rechazo de rama duplicada mutó support.json"
fi

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]
