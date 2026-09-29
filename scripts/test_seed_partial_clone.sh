#!/usr/bin/env bash
set -uo pipefail
# test_seed_partial_clone.sh — a failed clone must leave NO target behind.
#
# The contract is:
#
#     reflink failure → remove the partial target → cold build from a clean dir
#
# and not:
#
#     reflink failure → cold build over a half-populated target
#
# The second one is the whole hazard this mechanism exists to avoid, re-entering
# through its own error path. Cargo decides freshness from what it finds on disk;
# a directory carrying a few copied files and a manifest is not a state any
# cleanup step or review would recognise as debris.
#
# The failure is simulated with a stub `cp` that writes some of the seed and then
# exits non-zero, which is the only honest way to reach that branch without
# needing a filesystem whose reflink fails halfway.
#
# Manual: ~2 s.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
SEEDER="$SCRIPT_DIR/seed_target.sh"
KEYSH="$SCRIPT_DIR/seed_compat_key.sh"

export SEED_REPO_ROOT="$REPO_ROOT"
# The key is obtained through the SAME recipe production uses, never by calling
# seed_compat_key.sh directly with a hand-built argument list. A test that
# assembles the key's inputs itself is a second opinion about what the key means,
# and a second opinion is what drifted in the first place.
# shellcheck source=scripts/seed_recipe.sh
. "$SCRIPT_DIR/seed_recipe.sh"
seed_recipe_parse "$REPO_ROOT" --features ''
seed_recipe_compute_key "$REPO_ROOT"
mapfile -t KEY_ARGS < <(seed_recipe_key_args)
KEY="$SEED_RECIPE_KEY" CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER RUSTUP_TOOLCHAIN

# On the real cache filesystem, not tmpfs: rename and unlink behaviour differs,
# and this test is specifically about whether a half-built directory can be
# removed at all.
SANDBOX="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}/.test-partial.$$"
SEEDS="$SANDBOX/seeds"
trap 'chmod -R u+w "$SANDBOX" 2>/dev/null; rm -rf "$SANDBOX" 2>/dev/null' EXIT
mkdir -p "$SANDBOX/bin" "$SEEDS"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }

# A minimal but valid seed, published by production so the key is a real one.
SEED="$SEEDS/$KEY"
mkdir -p "$SEED/debug/.fingerprint/dep-some-crate"
printf 'content\n' >"$SEED/debug/.fingerprint/dep-some-crate/lib-x"
# A hand-written manifest would not verify, and the consumer would then report
# `incompatible-manifest` and never reach the clone at all — the test would pass
# against a branch it never exercised. The manifest is emitted by production.
bash "$KEYSH" "${KEY_ARGS[@]}" --emit-manifest "$SEED" >/dev/null
bash "$KEYSH" "${KEY_ARGS[@]}" --verify "$SEED" >/dev/null \
  || { echo "FAIL: fixture seed does not verify; the test would measure nothing"; exit 1; }

# A `cp` that does half the job and fails, leaving a directory it also makes
# unremovable — the harder of the two cases, because cleanup can fail too.
cat >"$SANDBOX/bin/cp" <<'STUB'
#!/usr/bin/env bash
# args: -a --reflink=always --no-preserve=mode SRC DEST
src=""; dest=""
for a in "$@"; do case "$a" in -a|--*|"") ;; *) dest="$a";; esac; done
dest="${!#}"
mkdir -p "$dest/debug" 2>/dev/null
printf 'half-copied\n' >"$dest/manifest.toml" 2>/dev/null
printf 'debris\n'      >"$dest/debug/partial.o" 2>/dev/null
# A subdirectory with no write bit: rm -rf cannot recurse into it, cannot unlink
# its children, and cannot remove it, because unlinking needs write on ITS parent.
chmod 555 "$dest/debug" 2>/dev/null
exit 1
STUB
chmod +x "$SANDBOX/bin/cp"

echo "partial clone failure must not leave a target behind"

LEFT=0
OUT="$(PATH="$SANDBOX/bin:$PATH" bash "$SEEDER" --seeds-root "$SEEDS" --features '' \
        --target-dir "$SANDBOX/target" 2>&1)"; RC=$?
echo "      $OUT"

case "$OUT" in
  *"seed: cold"*) ok "the failure is reported as a cold build, not a seeded one" ;;
  *)             bad "no cold verdict in the output" ;;
esac

# The decisive assertion. "Clean" means either gone, or present and empty.
if [ ! -e "$SANDBOX/target" ]; then
  ok "the partial target was removed entirely (best case)"
else
  LEFT="$(find "$SANDBOX/target" -mindepth 1 2>/dev/null | wc -l)"
  if [ "$LEFT" -eq 0 ]; then
    ok "the partial target was emptied (nothing inside it)"
  else
    bad "$LEFT entr(y/ies) of a half-copied seed are still in the target; a cold"
    echo "        build would now start from debris the caller cannot see"
    find "$SANDBOX/target" -mindepth 1 2>/dev/null | head -4 | sed 's/^/          /'
  fi
fi

# And the caller must not be told to proceed when the state is not clean.
if [ "$LEFT" -gt 0 ] 2>/dev/null; then
  if [ "$RC" -eq 0 ]; then
    bad "exit 0 with debris still present: the caller is told to carry on and build over it"
  else
    ok "non-zero exit with debris present: the caller is stopped"
  fi
else
  ok "nothing to stop: the target is clean"
fi

# --- the branch where cleanup itself is impossible ---------------------------
# The previous case is cleaned by the second attempt. This one makes removal
# genuinely impossible — `rm` is stubbed to always fail — so the leftovers
# survive. That is the state where saying "cold, carry on" would be a lie, and
# the consumer must refuse instead of exit 0.
rm -f "$SANDBOX/bin/cp" "$SANDBOX/bin/rm"
cat >"$SANDBOX/bin/rm" <<'STUB'
#!/usr/bin/env bash
exit 1          # removal is impossible in this scenario
STUB
cat >"$SANDBOX/bin/cp" <<'STUB'
#!/usr/bin/env bash
dest="${!#}"
mkdir -p "$dest/debug" 2>/dev/null
printf 'debris\n' >"$dest/debug/partial.o" 2>/dev/null
printf 'half\n'    >"$dest/manifest.toml" 2>/dev/null
exit 1
STUB
chmod +x "$SANDBOX/bin/rm" "$SANDBOX/bin/cp"

OUT2="$(PATH="$SANDBOX/bin:$PATH" bash "$SEEDER" --seeds-root "$SEEDS" --features '' \
         --target-dir "$SANDBOX/unremovable" 2>&1)"; RC2=$?
# Match the VERDICT shape, not the word: a refusal message that explains WHY it
# is not a cold build will legitimately contain the word.
case "$OUT2" in
  *"seed: cold"*) bad "reported a cold build while the target is unremovable debris" ;;
  *)              ok "no cold verdict while the target cannot be cleaned" ;;
esac
if [ "$RC2" -eq 0 ]; then
  bad "exit 0 with unremovable debris: the caller proceeds and builds over it"
else
  ok "exits $RC2 (non-zero) so the caller is stopped before Cargo"
fi
case "$OUT2" in
  *"seed: refused"*) ok "emits a seed: refused verdict, greppable like the others" ;;
  *)                 bad "no refused verdict in the output" ;;
esac

# The operator needs a way out that actually works here.
chmod -R u+w "$SANDBOX/unremovable" 2>/dev/null || true
PATH="$SANDBOX/bin:$PATH" /bin/rm -rf "$SANDBOX/unremovable" 2>/dev/null || true

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
echo "a failed clone leaves either nothing or an empty dir — never debris for the next build"
