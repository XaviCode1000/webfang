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
# RED discipline: the refusal cases below are marked NEGATION with the
# pre-fix verdict recorded — before the guards existed the script mutated
# Cargo.toml, CHANGELOG.md and HEAD with exit 0 from any branch.
#
# Everything runs in mktemp fixture repos with a stub `cargo` on PATH (the
# same fake-returning-0 shim style as test_seed_contamination.sh): no build,
# no network, no mutation of the repository this harness runs from. The git
# settings that would otherwise leak from the developer's global config are
# pinned per fixture, and no fixture carries .github/support.json on
# purpose: the line-state check's own "tip, omitted" fallback path is used
# so these cases isolate the NEW guards (the state check is a pre-existing
# guard, not this task's scope).
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
  cp "$REPO_ROOT/scripts/bump-support-patch.sh" "$dir/scripts/"
  chmod +x "$dir/scripts/bump-support-patch.sh"
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
  # The working scenario is jam inside the hotfix worktree: HEAD IS the
  # support branch. Rename so every case below runs on support/2.12 (no
  # checkout/switch needed).
  git -C "$dir" branch -m main support/2.12
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

echo "== el bump nunca retrocede sobre la versión del árbol (semver) =="
FX_BACK="$(new_fx backwards 2.12.0)"
# NEGATION: pre-fix a request BELOW CURRENT (2.11.0 < 2.12.0) mutated the
# workspace version backwards and committed it — a release act in the wrong
# nothing to stand on. The line-state gate passes here (2.11 would be
# MAINTENANCE in a real-world rotation), so pre-fix the only thing that could
# stop the request was the now-added monotonicity check.
snap_files "$FX_BACK"
run_bump "$FX_BACK" 2.11.0
if [[ "$LAST_RC" -ne 0 ]]; then
  pass "se niega a bajar 2.12.0 -> 2.11.0"
else
  fail "aceptó un patch DOWNWARD (2.11.0 < CURRENT 2.12.0) — out: $LAST_OUT"
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

echo
echo "harness: $PASSED ok, $FAILED fail"
[[ "$FAILED" -eq 0 ]]
