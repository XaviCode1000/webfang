#!/usr/bin/env bash
# Semantics harness for scripts/bump-support-patch.sh (S12/B2, #2060).
#
# The bump script is the ONLY sanctioned path to mutate the workspace version
# on a support branch (release-plz never runs there), and every mutation it
# performs travels on the branch the hotfix PR is based on. Two guards make
# that binding mechanical instead of remembered:
#
#   1. BRANCH BINDING. The patch bump for line X.Y must run on branch
#      support/X.Y — any other branch (including a detached HEAD, printed by
#      git as "HEAD") is refused before anything is touched.
#   2. SEMVER MONOTONICITY. The requested version is never BELOW the version
#      already in the root Cargo.toml. EQUALITY is the documented idempotent
#      re-run path and must keep working.
#
# A third property is the RELEASE CONTRACT itself: the declaration of the
# line (`.github/support.json`, plus the rendered `SUPPORT.md`) must follow
# the patch the bump produces, because the enforcing gate
# (`check_support_drift.sh --scope=published`, a REQUIRED check) compares the
# declared `latest` against the published tags and turns red for EVERY later
# PR when the declaration lags behind them. The case below proves the gate
# turns GREEN after the bump — and that the SAME fixture without the bump
# stays RED, which is the direction that matters.
#
# RED discipline: the refusal cases below are marked NEGATION with the
# pre-fix verdict recorded — before the guards existed the script mutated
# Cargo.toml, CHANGELOG.md and HEAD with exit 0 from any branch.
#
# Everything runs in mktemp fixture repos with a stub `cargo` on PATH (the
# same fake-returning-0 shim style as test_seed_contamination.sh): no build,
# no network, no mutation of the repository this harness runs from. The git
# settings that would otherwise leak from the developer's global config are
# pinned per fixture, and the plain fixture carries no
# .github/support.json on purpose: the line-state check's own "tip, omitted"
# fallback path is used so those cases isolate the NEW guards (the state
# check is a pre-existing guard, not this task's scope). The declaration
# cases use a fixture that HAS one, because that is the path the release
# contract has to keep honest.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── Shims ───────────────────────────────────────────────────────────────────
# The script runs `cargo check --workspace` and `cargo nextest run ...` (via
# the cargo binary). Stubs cover both invocation forms: the cargo binary and
# direct nextest entry points, should the script ever call them separately.
make_shims() {
  local bin="$WORK/bin"
  mkdir -p "$bin"
  for tool in cargo cargo-nextest nextest; do
    cat > "$bin/$tool" <<'SHIM'
#!/usr/bin/env bash
echo "stub: $0 $*" >> "${BUMP_SHIM_LOG:-/dev/null}"
exit 0
SHIM
    chmod +x "$bin/$tool"
  done
}
make_shims

# ─── Fixture ────────────────────────────────────────────────────────────────
# new_fx <name> <workspace-version>: a minimal repo whose root Cargo.toml is
# only what the script touches: a `version` line under [workspace.package],
# a CHANGELOG.md with an [Unreleased] section, and a stub Cargo.lock so the
# bare `git add Cargo.lock` finds a file (the real repo always has one).
new_fx() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts"
  # The script under test and the three collaborators it needs inside the
  # fixture: the declaration refresh (scripts/refresh_support_latest.sh), the
  # single writer of the projection (scripts/render-support-md.sh) and the
  # enforcing gate the declaration is judged by
  # (scripts/check_support_drift.sh).
  for s in bump-support-patch.sh refresh_support_latest.sh render-support-md.sh check_support_drift.sh; do
    cp "$REPO_ROOT/scripts/$s" "$dir/scripts/"
  done
  chmod +x "$dir"/scripts/*.sh
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email fixture@example.invalid
  git -C "$dir" config user.name fixture
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" config tag.gpgsign false
  git -C "$dir" config core.hooksPath /dev/null
  git -C "$dir" config commit.template /dev/null
  printf '[workspace]\nmembers = []\nresolver = "2"\n\n[workspace.package]\nversion = "%s"\nedition = "2021"\n' "$2" > "$dir/Cargo.toml"
  printf 'version = 4\n[[package]]\nname = "bumpstub"\nversion = "%s"\n' "$2" > "$dir/Cargo.lock"
  printf '## [Unreleased]\n\n### Fixed\n\n- placeholder.\n' > "$dir/CHANGELOG.md"
  git -C "$dir" add -A
  git -C "$dir" commit -q -m init
  # The working scenario is inside the hotfix worktree: HEAD IS a branch of
  # the support line. The support line is created ALONGSIDE main (both sit on
  # the same commit) because the hotfix cases need `main` to be a DIFFERENT
  # base to bump from. HEAD is moved with plumbing (symbolic-ref), never with
  # checkout/switch: the harness never changes the branch of the tree it runs
  # from, and every fixture branch shares one commit, so the working tree and
  # the index stay consistent with it.
  git -C "$dir" branch support/2.12 main
  git -C "$dir" symbolic-ref HEAD refs/heads/support/2.12
  echo "$dir"
}

# switch_head_to <fixture> <branch>: point HEAD at an existing branch of the
# fixture, without touching the working tree (see new_fx).
switch_head_to() {
  git -C "$1" symbolic-ref HEAD "refs/heads/$2"
}

# new_fx_decl <name> <workspace-version>: the same fixture WITH a support
# declaration (one STABLE line, 2.12) and the matching published tag. This is
# the fixture the release contract is about: the plain one above has no
# declaration on purpose, so the line-state check falls back to its "tip,
# omitted" path and the NEW guards stay isolated.
new_fx_decl() {
  local dir
  dir="$(new_fx "$1" "$2")"
  mkdir -p "$dir/.github"
  cat > "$dir/.github/support.json" <<'JSON'
{
  "schema_version": 1,
  "lines": [
    {
      "minor": "2.12",
      "state": "STABLE",
      "branch": null,
      "latest": "2.12.0",
      "released_at": "2026-10-01T00:00:00+00:00",
      "accepts": [
        "bugfix",
        "security"
      ]
    }
  ]
}
JSON
  ( cd "$dir" && ./scripts/render-support-md.sh >/dev/null )
  git -C "$dir" add -A
  git -C "$dir" commit -q -m "declaracion de soporte"
  git -C "$dir" tag v2.12.0
  echo "$dir"
}

# run_bump <fixture> <version> [args...]: run the script in the fixture with
# the stubs on PATH; LAST_RC / LAST_OUT carry the verdict.
run_bump() {
  local dir="$1" ver="$2"; shift 2
  local rc=0
  : > "$WORK/shim.log"
  (
    cd "$dir"
    export BUMP_SHIM_LOG="$WORK/shim.log"
    export PATH="$WORK/bin:$PATH"
    ./scripts/bump-support-patch.sh "$ver" "$@" 2>&1
  ) > "$WORK/out.txt" || rc=$?
  LAST_RC=$rc
  LAST_OUT="$(cat "$WORK/out.txt")"
}

# run_drift <fixture> <args...>: run the ENFORCING gate inside the fixture —
# the same script CI runs as a required check. LAST_DRIFT_RC / LAST_DRIFT_OUT
# carry the verdict.
run_drift() {
  local dir="$1"; shift
  local rc=0
  ( cd "$dir" && ./scripts/check_support_drift.sh "$@" ) > "$WORK/drift.out" 2>&1 || rc=$?
  LAST_DRIFT_RC=$rc
  LAST_DRIFT_OUT="$(cat "$WORK/drift.out")"
}

# snap_files <fixture>: copy the three mutable files so refused paths can be
# compared byte-for-byte afterwards.
snap_files() {
  local dir="$1"
  cp "$dir/Cargo.toml" "$WORK/cargo.toml.snap"
  cp "$dir/CHANGELOG.md" "$WORK/changelog.md.snap"
  cp "$dir/Cargo.lock" "$WORK/cargo.lock.snap"
}
files_untouched() {
  local dir="$1"
  cmp -s "$dir/Cargo.toml" "$WORK/cargo.toml.snap" \
    && cmp -s "$dir/CHANGELOG.md" "$WORK/changelog.md.snap" \
    && cmp -s "$dir/Cargo.lock" "$WORK/cargo.lock.snap"
}

echo "== cuando la rama no es la del bump, NINGÚN byte se mueve (S12) =="
FX_MAINBUMP="$(new_fx main_bump 2.12.0)"
# NEGATION: pre-fix the script ran from any branch and, with exit 0, bumped
# Cargo.toml + CHANGELOG.md + committed on whatever HEAD it found — here the
# verbatim S12 acceptance case: `bump-support-patch.sh 2.13.1` on support/2.12
# must refuse, naming both branches, and mutate nothing.
snap_files "$FX_MAINBUMP"
run_bump "$FX_MAINBUMP" 2.13.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega a bump 2.13.1 estando en support/2.12 (aceptación S12 verbatim)"
else
  fail "aceptó bump 2.13.1 fuera de la rama de su línea — out: $LAST_OUT"
fi
if grep -qF 'support/2.12' <<<"$LAST_OUT" && grep -qF "$(git -C "$FX_MAINBUMP" rev-parse --abbrev-ref HEAD)" <<<"$LAST_OUT"; then
  pass "el rechazo nombra la rama actual y la exigida"
else
  fail "el mensaje de rechazo no nombró ambas ramas: $LAST_OUT"
fi
if [[ -z "$(git -C "$FX_MAINBUMP" status --porcelain)" ]] && files_untouched "$FX_MAINBUMP"; then
  pass "el rechazo no muta Cargo.toml / CHANGELOG.md / Cargo.lock ni el worktree"
else
  fail "el rechazo mutó archivos: $(git -C "$FX_MAINBUMP" status --porcelain | tr '\n' ' ')"
fi

echo "== HEAD detached: git lo imprime como HEAD y también se niega =="
FX_DETACHED="$(new_fx detached 2.12.0)"
# NEGATION: pre-fix a detached checkout bumped and committed on an unbindable
# HEAD with exit 0 — a release act that no branch can capture afterwards.
git -C "$FX_DETACHED" update-ref --no-deref -m "harness detach" \
  HEAD "$(git -C "$FX_DETACHED" rev-parse HEAD)"
snap_files "$FX_DETACHED"
run_bump "$FX_DETACHED" 2.12.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega en HEAD detached"
else
  fail "aceptó un bump en HEAD detached — out: $LAST_OUT"
fi
if [[ -z "$(git -C "$FX_DETACHED" status --porcelain)" ]] && files_untouched "$FX_DETACHED"; then
  pass "el rechazo en detached no muta nada"
else
  fail "el rechazo en detached mutó archivos"
fi

echo "== hotfix/* con base demostrable en support/X.Y: el bump SÍ procede =="
# The sanctioned flow runs the bump inside the HOTFIX PR whose base is
# support/X.Y, so the HEAD of that PR is `hotfix/*` — and check-topology.sh
# admits ONLY hotfix/* -> support/*. A guard that demands the head to BE the
# support branch makes the sanctioned patch release unrunnable, so a hotfix
# branch is accepted when its base is PROVABLY the support line.
FX_HOTFIX="$(new_fx hotfix_ok 2.12.0)"
git -C "$FX_HOTFIX" branch hotfix/fix-x support/2.12
switch_head_to "$FX_HOTFIX" hotfix/fix-x
run_bump "$FX_HOTFIX" 2.12.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "el bump procede en hotfix/fix-x (base support/2.12 demostrable)"
else
  fail "el bump rechazó un hotfix con base support/2.12 (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(grep -m1 '^version' "$FX_HOTFIX/Cargo.toml")" == 'version = "2.12.1"' ]]; then
  pass "el hotfix bumpeó la versión del árbol"
else
  fail "Cargo.toml no se bumpeó en el hotfix: $(grep -m1 '^version' "$FX_HOTFIX/Cargo.toml")"
fi

echo "== hotfix/* cuya base es main: rechazo con el diagnóstico de base =="
FX_HOTMAIN="$(new_fx hotfix_from_main 2.12.0)"
git -C "$FX_HOTMAIN" branch hotfix/fix-y main
switch_head_to "$FX_HOTMAIN" hotfix/fix-y
snap_files "$FX_HOTMAIN"
run_bump "$FX_HOTMAIN" 2.12.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega a bumpear desde un hotfix basado en main"
else
  fail "aceptó un hotfix basado en main — out: $LAST_OUT"
fi
if grep -qF 'hotfix/fix-y' <<<"$LAST_OUT" && grep -qF 'support/2.12' <<<"$LAST_OUT" \
   && grep -qF 'main' <<<"$LAST_OUT"; then
  pass "el rechazo nombra la rama, la base exigida y la base hallada"
else
  fail "el diagnóstico de base no nombró rama, base exigida y base hallada: $LAST_OUT"
fi
if [[ -z "$(git -C "$FX_HOTMAIN" status --porcelain)" ]] && files_untouched "$FX_HOTMAIN"; then
  pass "el rechazo por base no muta nada"
else
  fail "el rechazo por base mutó archivos"
fi

echo "== hotfix/* sin base demostrable: rechazo fail-closed =="
# A branch with no creation record, no declaration and no tracking ref:
# NOTHING says what it is based on, so the guard must refuse instead of
# guessing — and the refusal must say how to establish the base.
FX_HOTUNKNOWN="$(new_fx hotfix_unknown 2.12.0)"
git -C "$FX_HOTUNKNOWN" update-ref refs/heads/hotfix/fix-z \
  "$(git -C "$FX_HOTUNKNOWN" rev-parse HEAD)"
switch_head_to "$FX_HOTUNKNOWN" hotfix/fix-z
snap_files "$FX_HOTUNKNOWN"
run_bump "$FX_HOTUNKNOWN" 2.12.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega cuando la base no puede establecerse (fail closed)"
else
  fail "aceptó un hotfix sin base demostrable — out: $LAST_OUT"
fi
if grep -qF 'hotfix/fix-z' <<<"$LAST_OUT" && grep -qF 'support/2.12' <<<"$LAST_OUT" \
   && grep -qF 'webfang-base' <<<"$LAST_OUT"; then
  pass "el rechazo nombra la rama, la base exigida y cómo establecerla"
else
  fail "el diagnóstico fail-closed no nombró rama, base exigida y método: $LAST_OUT"
fi
if [[ -z "$(git -C "$FX_HOTUNKNOWN" status --porcelain)" ]] && files_untouched "$FX_HOTUNKNOWN"; then
  pass "el rechazo fail-closed no muta nada"
else
  fail "el rechazo fail-closed mutó archivos"
fi

echo "== el bump nunca retrocede DENTRO de la misma minor (semver) =="
FX_BACK="$(new_fx backwards 2.12.2)"
# NEGATION: this case is the ONLY proof of the semver-monotonicity invariant,
# and it is shaped so it REACHES that invariant. The request (2.12.1) belongs
# to the SAME minor as the branch it runs on (support/2.12), so the
# branch-binding gate PASSES and the only guard that can refuse is the
# monotonicity comparison. The previous shape asked for 2.11.0 from
# support/2.12 — a CROSS-minor request, which the branch guard refuses first:
# the case was satisfied by the wrong guard, the invariant had NO proof, and
# deleting the whole monotonicity block from the script under test left the
# harness green. The two assertions below pin that: the refusal must carry
# the MONOTONICITY diagnostic, and the branch diagnostic must be absent.
snap_files "$FX_BACK"
run_bump "$FX_BACK" 2.12.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega a bajar 2.12.2 -> 2.12.1 dentro de la misma minor"
else
  fail "aceptó un patch DOWNWARD (2.12.1 < CURRENT 2.12.2) — out: $LAST_OUT"
fi
if grep -qF 'es anterior a la versión actual del árbol' <<<"$LAST_OUT"; then
  pass "el rechazo es el de la MONOTONICIDAD"
else
  fail "el rechazo no vino del guard de monotonicidad: $LAST_OUT"
fi
if grep -qF 'se commitea en' <<<"$LAST_OUT"; then
  fail "el rechazo vino del guard de rama: la case NO alcanza la monotonicidad — $LAST_OUT"
else
  pass "el guard de rama no intervino: la case alcanza la comparación de semver"
fi
if [[ -z "$(git -C "$FX_BACK" status --porcelain)" ]] && files_untouched "$FX_BACK"; then
  pass "el rechazo monotónico no muta nada"
else
  fail "el rechazo monotónico mutó archivos"
fi

echo "== control positivo: el bump sancionado SÍ procede y commitea =="
FX_OK="$(new_fx positive 2.12.0)"
NOTAG_BEFORE="$(git -C "$FX_OK" rev-list --count HEAD)"
run_bump "$FX_OK" 2.12.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "bump 2.12.1 en support/2.12 procede (los stubs cubren cargo y nextest)"
else
  fail "el control positivo falló (rc=$LAST_RC): $LAST_OUT"
fi
head_msg="$(git -C "$FX_OK" log -1 --format=%s)"
if [[ "$head_msg" == "chore: bump to v2.12.1" ]]; then
  pass "el commit creado es 'chore: bump to v2.12.1'"
else
  fail "HEAD no es el commit de bump: $head_msg"
fi
if [[ "$(grep -m1 '^version' "$FX_OK/Cargo.toml")" == 'version = "2.12.1"' ]]; then
  pass "Cargo.toml quedó en la versión nueva"
else
  fail "Cargo.toml no se bumpó: $(grep -m1 '^version' "$FX_OK/Cargo.toml")"
fi
if [[ "$(git -C "$FX_OK" rev-list --count HEAD)" -eq $((NOTAG_BEFORE + 1)) ]]; then
  pass "exactamente UN commit nuevo; la re-ejecución es el único camino a un segundo"
else
  fail "conteo de commits inesperado: $(git -C "$FX_OK" rev-list --count HEAD)"
fi
if grep -q '^## \[2\.12\.1\]' "$FX_OK/CHANGELOG.md"; then
  pass "CHANGELOG recibió la sección del patch"
else
  fail "CHANGELOG sin la sección del patch"
fi
if grep -qF 'stub:' "$WORK/shim.log" && grep -qF 'nextest' "$WORK/shim.log"; then
  pass "los shims interceptaron las invocaciones de cargo/nextest (sin build real)"
else
  fail "los shims no cubrieron las invocaciones: $(tr '\n' ' ' < "$WORK/shim.log")"
fi

echo "== idempotencia documentada: re-ejecución (== CURRENT) permitida =="
# The re-run path (same version) must hit the "nada nuevo que commitear" OK
# and add no commit — equality is allowed by the monotonicity check.
run_bump "$FX_OK" 2.12.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "re-ejecución tras el bump termina OK"
else
  fail "re-ejecución idempotente rechazada (rc=$LAST_RC): $LAST_OUT"
fi
if [[ "$(git -C "$FX_OK" rev-list --count HEAD)" -eq $((NOTAG_BEFORE + 1)) ]]; then
  pass "la re-ejecución NO añadió commit"
else
  fail "la re-ejecución añadió commit(s) extra"
fi

# ─── The release contract: the declaration follows the patch (ID-1) ─────────
# `latest` is a fact about the published tags, and the enforcing gate
# (check_support_drift.sh --scope=published, a REQUIRED check inside
# `Validate PR metadata`) compares it against them. Before this contract
# existed, the first patch published on a support line left the declaration
# stale and the gate RED for every later PR — with no sanctioned writer to
# repair it. The case proves BOTH directions: the gate turns GREEN after the
# bump, and the same fixture WITHOUT the bump stays RED.
echo "== el bump mantiene la declaración: el gate queda verde tras publicar el parche =="
FX_DECL="$(new_fx_decl declaration 2.12.0)"
# Control: before the bump, the declaration agrees with the published tags.
run_drift "$FX_DECL" --scope=published
if [[ "$LAST_DRIFT_RC" -eq 0 ]]; then
  pass "la declaración coherente pasa el gate enforcing antes del bump"
else
  fail "el fixture de declaración no pasa el gate antes del bump (rc=$LAST_DRIFT_RC): $LAST_DRIFT_OUT"
fi

run_bump "$FX_DECL" 2.12.1
if [[ "$LAST_RC" -eq 0 ]]; then
  pass "el bump con declaración procede"
else
  fail "el bump con declaración falló (rc=$LAST_RC): $LAST_OUT"
fi
declared_latest="$(jq -r '.lines[] | select(.minor=="2.12") | .latest' "$FX_DECL/.github/support.json")"
if [[ "$declared_latest" == "2.12.1" ]]; then
  pass "la declaración declara el parche que el bump produce (.latest = 2.12.1)"
else
  fail "la declaración quedó en '$declared_latest': el bump no actualizó .latest"
fi
if grep -qF '| 2.12 | STABLE | 2.12.1 |' "$FX_DECL/SUPPORT.md"; then
  pass "SUPPORT.md fue re-renderizado por el escritor único con la nueva última"
else
  fail "SUPPORT.md no refleja la nueva última: $(grep -m1 '^| 2.12' "$FX_DECL/SUPPORT.md")"
fi
bump_files="$(git -C "$FX_DECL" show --name-only --format= HEAD)"
if grep -qxF '.github/support.json' <<<"$bump_files" && grep -qxF 'SUPPORT.md' <<<"$bump_files"; then
  pass "el commit de bump lleva la declaración y su proyección"
else
  fail "el commit de bump no lleva support.json + SUPPORT.md: $(tr '\n' ' ' <<<"$bump_files")"
fi

# The tag is cut AFTER the bump merges (cut-patch-tag.yml refuses a branch
# whose Cargo.toml does not carry the version): the fixture simulates it.
git -C "$FX_DECL" tag v2.12.1
run_drift "$FX_DECL" --scope=published
if [[ "$LAST_DRIFT_RC" -eq 0 ]]; then
  pass "con el parche publicado, el gate enforcing queda VERDE para la línea bumpeada"
else
  fail "el gate sigue rojo tras el bump (rc=$LAST_DRIFT_RC): $LAST_DRIFT_OUT"
fi

# NEGATION: the SAME fixture without the bump must still be RED. Without the
# declaration update the gate turns red for every later PR, which is exactly
# the defect this case pins — a case that only ever saw GREEN would prove
# nothing.
git -C "$FX_DECL" show HEAD~1:.github/support.json > "$FX_DECL/.github/support.json"
run_drift "$FX_DECL" --scope=published
if [[ "$LAST_DRIFT_RC" -ne 0 ]]; then
  pass "sin el bump, el MISMO fixture sigue rojo (dirección NEGATION)"
else
  fail "el gate quedó verde sin el bump: la declaración no era lo que lo sostenía"
fi

echo "== el refresh es idempotente y falla cerrado =="
FX_IDEM="$(new_fx_decl idempotente 2.12.0)"
run_bump "$FX_IDEM" 2.12.1
idem_before="$(sha256sum "$FX_IDEM/.github/support.json" | cut -d' ' -f1)"
idem_commits="$(git -C "$FX_IDEM" rev-list --count HEAD)"
run_bump "$FX_IDEM" 2.12.1
idem_after="$(sha256sum "$FX_IDEM/.github/support.json" | cut -d' ' -f1)"
if [[ "$idem_before" == "$idem_after" && "$(git -C "$FX_IDEM" rev-list --count HEAD)" -eq "$idem_commits" ]]; then
  pass "re-ejecutar el bump no mueve la declaración ni añade commit (idempotente)"
else
  fail "la re-ejecución movió la declaración o añadió commits"
fi

# A declared line whose minor has NO published patch has no fact to point at.
# The refresh must refuse instead of writing a half-updated declaration, and
# the refusal must leave the tree exactly as it was — the refresh runs BEFORE
# the version mutation for that reason.
FX_FC="$(new_fx_decl fail_closed 2.12.0)"
jq '.lines += [{"minor":"2.11","state":"EOL","branch":null,"latest":"2.11.0","eol_reason":"fixture"}]' \
  "$FX_FC/.github/support.json" > "$FX_FC/.github/support.json.tmp"
mv "$FX_FC/.github/support.json.tmp" "$FX_FC/.github/support.json"
( cd "$FX_FC" && ./scripts/render-support-md.sh >/dev/null )
git -C "$FX_FC" add -A
git -C "$FX_FC" commit -q -m "linea sin parche publicado"
fc_decl="$(sha256sum "$FX_FC/.github/support.json" | cut -d' ' -f1)"
fc_cargo="$(sha256sum "$FX_FC/Cargo.toml" | cut -d' ' -f1)"
run_bump "$FX_FC" 2.12.1
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "el bump se niega cuando la declaración no puede calcularse (fail closed)"
else
  fail "el bump aceptó una declaración que no puede calcularse: $LAST_OUT"
fi
if [[ "$fc_decl" == "$(sha256sum "$FX_FC/.github/support.json" | cut -d' ' -f1)" ]]; then
  pass "la declaración quedó byte a byte intacta tras el rechazo"
else
  fail "el rechazo mutó la declaración a medias"
fi
if [[ "$fc_cargo" == "$(sha256sum "$FX_FC/Cargo.toml" | cut -d' ' -f1)" ]]; then
  pass "Cargo.toml quedó intacto: el refresh ocurre antes de la mutación"
else
  fail "el rechazo mutó Cargo.toml"
fi
if [[ -z "$(git -C "$FX_FC" status --porcelain)" ]]; then
  pass "el rechazo no dejó cambios sin commitear"
else
  fail "el rechazo dejó cambios: $(git -C "$FX_FC" status --porcelain | tr '\n' ' ')"
fi

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]
