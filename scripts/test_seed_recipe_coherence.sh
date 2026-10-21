#!/usr/bin/env bash
set -uo pipefail
# test_seed_recipe_coherence.sh — the golden rule, made executable.
#
#     If two builds can produce different artifacts, they must not produce the
#     same SeedCompatibilityKey. And whoever holds a key must build exactly the
#     contract that key describes.
#
# This exists because the previous arrangement violated that rule in five ways
# at once, all shipped, all measured:
#
#     --target aarch64-…    key said aarch64, build ran `cargo build`
#     --profile release     key said release,    build ran the dev profile
#     --features ai         key said ai,         build had no --features
#     -- --release …        reached cargo,       key did not change
#     RUSTUP_TOOLCHAIN      key hashed nightly,  build unset it
#     CARGO_INCREMENTAL=1   key said on,         build forced 0
#     build-dir             consumer refused,    producer did not
#     config outside repo   build moved,         key did not
#
# The test does not assert that a key "changes when the environment changes" —
# that is the wrong property, and asserting it would have locked in the bug. The
# recipe deliberately PINS toolchain and incremental, so a stray
# RUSTUP_TOOLCHAIN in the caller's shell must change NEITHER the key NOR the
# build. What must hold is correspondence: whatever the key says, the build
# must do.
#
# So each case compares the key's own recorded fields against what cargo was
# actually invoked with, and separately checks that two recipes that differ in a
# contract field do not collide on one key.
#
# Manual: ~30 s. No real build: cargo is a stub that records its argv and the
# contract-relevant environment it was handed.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

# The REAL cargo, captured before anything shadows it. The lock-invariant case
# below needs a genuine cargo: a stub can only prove that the stub was called,
# never what cargo does with a lock that disagrees with the manifests.
#
# It must NOT be the mbx shim. In a worktree, `command -v cargo` resolves to
# ~/.local/share/mbx/bin/cargo — a CACHE WRAPPER whose whole purpose is to
# build. Asked for `cargo metadata`, it decided to build: that is how a
# harness that promises "no build" compiled the workspace (1.9 GB of artifacts,
# 6m33s of CPU, and a global OOM that took the session down with it). The
# wrapper is invisible in the argv and the error, so the only defence is to
# resolve the toolchain's cargo and to refuse the wrapper outright.
REAL_CARGO=""
if command -v rustup >/dev/null 2>&1; then
  REAL_CARGO="$(rustup which cargo 2>/dev/null || true)"
fi
[ -x "$REAL_CARGO" ] || REAL_CARGO="$HOME/.cargo/bin/cargo"
[ -x "$REAL_CARGO" ] || REAL_CARGO="$(command -v cargo 2>/dev/null || true)"
case "$REAL_CARGO" in
  '' | */mbx/bin/cargo)
    echo "coherence-harness: refusing to run — the resolved cargo is '${REAL_CARGO:-<none>}'," >&2
    echo "coherence-harness: the mbx shim is a build cache wrapper, and this harness must not build." >&2
    exit 2
    ;;
esac

SANDBOX="$(mktemp -d)"
# The sandbox holds PUBLISHED seeds, and a published seed is read-only by
# contract: the cleanup has to restore the write bits before it can remove
# them, or it leaves the tree behind and reports a failure that is not one.
trap 'chmod -R u+w "$SANDBOX" 2>/dev/null; rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/bin" "$SANDBOX/seeds" "$SANDBOX/safety"
# NOT named CARGO_*: the recipe deliberately unsets every CARGO_* for the build,
# and this test had its log file wiped by that policy — the policy was right
# and the name was wrong.
export SEED_TEST_LOG="$SANDBOX/cargo.log" SEED_REPO_ROOT="$REPO_ROOT"

# SAFETY NET: a cargo in front of the real one that REFUSES to build anything.
#
# The recording stub below is prepended to PATH per invocation, and every
# invocation that forgets to do that reaches the REAL cargo — and a real
# `cargo build --workspace` of this repository is the exact resource accident
# this suite must not cause (it compiles BoringSSL and links gigabytes).
# Measured: one such slip cost 1m15s wall, 6m33s CPU and 1.9 GB of artifacts.
#
# So the safety stub is on PATH for the WHOLE harness: a `cargo metadata` is
# delegated to the real cargo (the workspace-contract digest is computed from
# the real metadata, as it always was), and every other invocation is refused
# loudly. An accidental build is now impossible instead of merely unlikely.
cat >"$SANDBOX/safety/cargo" <<'SAFETY'
#!/usr/bin/env bash
if [ "${1:-}" = "metadata" ]; then
  exec "$WEBFANG_REAL_CARGO" "$@"
fi
echo "safety-cargo: refusing to run 'cargo $*' in the coherence harness" >&2
exit 1
SAFETY
chmod +x "$SANDBOX/safety/cargo"
export WEBFANG_REAL_CARGO="$REAL_CARGO"
export PATH="$SANDBOX/safety:$PATH"

cat >"$SANDBOX/bin/cargo" <<'STUB'
#!/usr/bin/env bash
# The recipe now calls `cargo metadata` to compute the workspace build contract,
# so the stub has to answer it — and must NOT log it as a build invocation, or
# every argv assertion below would read the metadata call instead.
if [ "${1:-}" = "metadata" ]; then
  printf '{"workspace_members":["webfang_core 0.1.0 (path+file:///repo/crates/webfang_core)"],"workspace_default_members":["webfang_core 0.1.0 (path+file:///repo/crates/webfang_core)"],"packages":[{"name":"webfang_core","id":"webfang_core 0.1.0 (path+file:///repo/crates/webfang_core)","dependencies":[],"features":{}}]}\n'
  exit 0
fi
{
  printf 'ARGV\t%s\n' "$*"
  printf 'RUSTFLAGS\t%s\n' "${RUSTFLAGS-<unset>}"
  # The RUSTFLAGS are logged QUOTED: the value can contain newlines (a
  # multi-line RUSTFLAGS is legitimate), and a line-oriented log would turn
  # one value into several lines and hide exactly the defect the round-trip
  # case below looks for.
  printf 'RUSTFLAGS_Q\t%s\n' "$(printf '%q' "${RUSTFLAGS-<unset>}")"
  printf 'SCCACHE_DIR\t%s\n' "${SCCACHE_DIR-<unset>}"
  printf 'CARGO_INCREMENTAL\t%s\n' "${CARGO_INCREMENTAL-<unset>}"
  printf 'RUSTUP_TOOLCHAIN\t%s\n' "${RUSTUP_TOOLCHAIN-<unset>}"
  printf 'CARGO_TARGET_DIR\t%s\n' "${CARGO_TARGET_DIR-<unset>}"
  printf 'RUSTC_WRAPPER\t%s\n' "${RUSTC_WRAPPER-<unset>}"
    printf 'RUSTC\t%s\n' "${RUSTC-<unset>}"
} >> "$SEED_TEST_LOG"
# A failing build has to fail the PUBLICATION (S14.2): the stub fails the
# non-metadata call and prints a message, so the test can prove cargo's own
# error reaches the operator instead of being swallowed.
if [ -n "${SEED_TEST_FAIL_BUILD:-}" ]; then
  echo "stub-cargo: simulated build failure" >&2
  exit 1
fi
[ -n "${CARGO_METADATA:-}" ] && printf '{"packages":[{"name":"webfang_core","targets":[{"name":"webfang_core"}]}]}\n'
exit 0
STUB
chmod +x "$SANDBOX/bin/cargo"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }

# publish with a stub cargo; leaves the key on stdout and the invocation in $CARGO_LOG
pub() { : >"$SEED_TEST_LOG"
        PATH="$SANDBOX/bin:$PATH" bash "$SCRIPT_DIR/seed_publish.sh" \
          --seeds-root "$SANDBOX/seeds" "$@" 2>"$SANDBOX/err" \
          | sed -n 's/^==> key \(v[0-9]-[0-9a-f]*\).*/\1/p' | head -1; }
cargo_field() { sed -n "s/^$1\t//p" "$SEED_TEST_LOG" | head -1; }
manifest_field() {  # $1=field -> the value the key recorded
  local d="$SANDBOX/mf"; mkdir -p "$d"
  bash "$SCRIPT_DIR/seed_compat_key.sh" --features "" --profile dev \
    --recipe-schema 1 --wrapper-policy prescribed-empty --toolchain-id "" \
    --config-digest "x" --workspace-digest "y" --emit-manifest "$d" >/dev/null 2>&1
  sed -n "s/^$1 = \"\(.*\)\"/\1/p" "$d/manifest.toml" | head -1; }

echo "recipe coherence: the key must describe the build"

# --- 1. each named field reaches cargo AND the key --------------------------
# $1=label  $2=expected-in-argv  rest: the publish flags
check_field() {
  local label="$1" want="$2"; shift 2
  local key; key="$(pub "$@")"
  local argv; argv="$(cargo_field ARGV)"
  case "$argv" in *"$want"*) ok "$label: '$want' reached cargo" ;;
    *) bad "$label: '$want' did NOT reach cargo (argv: $argv)" ;; esac
  if [ -n "$key" ]; then ok "$label: a key was computed ($key)"; else bad "$label: no key"; fi
}

check_field "profile"  "--profile dev"    --profile dev
check_field "features" "--features chromium" --features chromium
check_field "target"   "--target aarch64-unknown-linux-gnu" --target aarch64-unknown-linux-gnu

# --- 2. two different contracts must not share a key -------------------------
K_DEV="$(pub --profile dev)"
K_REL="$(pub --profile release)"
K_FEAT="$(pub --features chromium)"
K_TGT="$(pub --target aarch64-unknown-linux-gnu)"
UNIQ="$(printf '%s\n' "$K_DEV" "$K_REL" "$K_FEAT" "$K_TGT" | sort -u | grep -c .)"
if [ "$UNIQ" -eq 4 ]; then ok "four different contracts produced four different keys"
else bad "four contracts collapsed onto $UNIQ key(s)"; fi

# --- 3. the ambient environment must not leak into either side ---------------
# The recipe PINS toolchain and incremental. A stray value in the caller's shell
# must change neither the key nor the build — previously it changed the key and
# not the build, which is the exact split this test exists to prevent.
K_BASE="$(pub)"
K_NIGHTLY="$(RUSTUP_TOOLCHAIN=nightly pub)"
K_INCR="$(CARGO_INCREMENTAL=1 pub)"
if [ "$K_NIGHTLY" = "$K_BASE" ]; then ok "a stray RUSTUP_TOOLCHAIN does not move the key"
else bad "RUSTUP_TOOLCHAIN moved the key ($K_BASE -> $K_NIGHTLY)"; fi
if [ "$K_INCR" = "$K_BASE" ]; then ok "a stray CARGO_INCREMENTAL does not move the key"
else bad "CARGO_INCREMENTAL moved the key ($K_BASE -> $K_INCR)"; fi

# And the build must have been pinned to the recipe's values, not the ambient ones.
pub >/dev/null
if [ "$(cargo_field RUSTC_WRAPPER)" = "<unset>" ]; then ok "the build ran without RUSTC_WRAPPER"
else bad "RUSTC_WRAPPER reached the build: $(cargo_field RUSTC_WRAPPER)"; fi
if [ "$(cargo_field CARGO_INCREMENTAL)" = "0" ]; then ok "the build ran with the recipe's CARGO_INCREMENTAL=0"
else bad "the build's CARGO_INCREMENTAL diverged: $(cargo_field CARGO_INCREMENTAL)"; fi

# --- 4. no unkeyed escape hatch ---------------------------------------------
if pub -- --release --all-features >/dev/null 2>&1; then
  bad "'-- <arbitrary cargo args>' was accepted"
else
  ok "'-- <arbitrary cargo args>' is refused rather than silently unkeyed"
fi

# --- 5. build-dir refused by BOTH sides, one implementation -----------------
mkdir -p "$SANDBOX/cargohome"
printf '[build]\nbuild-dir = "%s/bd"\n' "$SANDBOX" >"$SANDBOX/cargohome/config.toml"
if CARGO_HOME="$SANDBOX/cargohome" pub >/dev/null 2>&1; then
  bad "the PRODUCER published despite a configured build-dir"
else
  ok "the producer refuses a configured build-dir"
fi
OUT="$(CARGO_HOME="$SANDBOX/cargohome" PATH="$SANDBOX/bin:$PATH" \
       CARGO_TARGET_DIR="$SANDBOX/tgt" bash "$SCRIPT_DIR/seed_target.sh" 2>&1)"
case "$OUT" in
  *"reason=build-dir-configured"*) ok "the consumer refuses the same build-dir" ;;
  *) bad "the consumer did not refuse: $OUT" ;;
esac
if grep -q 'seed_recipe_has_build_dir' "$SCRIPT_DIR/seed_publish.sh" && grep -q 'seed_recipe_has_build_dir' "$SCRIPT_DIR/seed_target.sh"; then
  ok "both sides call the same shared implementation (they cannot drift)"
else
  bad "the build-dir check is not shared — it can drift again"
fi

# --- 6. config outside the repository moves the key -------------------------
# A $CARGO_HOME config is read by cargo, so it is part of the contract. This
# used to be invisible: the key hashed only the repo's .cargo/config.toml.
# A HARMLESS config, deliberately without build-dir. The first version of this
# case reused the build-dir fixture, and the test passed for the wrong reason: the
# producer refused before computing a key, so the two "keys" differed trivially
# and proved nothing about the digest. A test that goes green without exercising
# the thing it names is worse than no test.
mkdir -p "$SANDBOX/plainhome" "$SANDBOX/ancestor/.cargo"
printf '[build]\nrustflags = []\n' >"$SANDBOX/plainhome/config.toml"
printf '# harmless\n' >"$SANDBOX/ancestor/.cargo/config.toml"
K_NOCARGOHOME="$(CARGO_HOME="$SANDBOX/emptyhome" pub)"
K_CARGOHOME="$(CARGO_HOME="$SANDBOX/plainhome" pub)"
[ -n "$K_NOCARGOHOME" ] || { echo "  FAIL control: no key at all with an empty CARGO_HOME"; exit 1; }
[ -n "$K_CARGOHOME" ]   || { echo "  FAIL no key with a plain CARGO_HOME — refusing to assert a difference between nothing"; exit 1; }
if [ "$K_NOCARGOHOME" != "$K_CARGOHOME" ]; then ok "a change in \$CARGO_HOME moves the key"
else bad "\$CARGO_HOME config changed but the key did not"; fi

# --- 7. the key must not depend on WHERE the checkout is ---------------------
# The property the whole feature exists for: a seed is shared across worktrees.
#
# This was broken while the config digest was being added, and every case above
# missed it because they all ran from one checkout. The digest recorded the
# ABSOLUTE PATH of each config file it found, so the same commit hashed
# differently from two different directories — purely because the ancestor walk
# visited different paths. A suite that only ever asks from one place cannot see
# a property about asking from two places.
CK1="$SANDBOX/elsewhere/checkout-one"
CK2="$SANDBOX/deeper/nested/checkout-two"
mkdir -p "$CK1" "$CK2"
git -C "$REPO_ROOT" worktree add --detach "$CK1" HEAD >/dev/null 2>&1
git -C "$REPO_ROOT" worktree add --detach "$CK2" HEAD >/dev/null 2>&1
if [ -d "$CK1" ] && [ -d "$CK2" ]; then
  k_at() { SEED_REPO_ROOT="$1" bash -c '. "$2/seed_recipe.sh"
            seed_recipe_parse "$1" --features ""; seed_recipe_compute_key "$1"
            printf "%s" "$SEED_RECIPE_KEY"' _ "$1" "$SCRIPT_DIR"; }
  KA="$(k_at "$CK1")"; KB="$(k_at "$CK2")"
  if [ -n "$KA" ] && [ "$KA" = "$KB" ]; then
    ok "the same commit keys identically from two unrelated directories"
  else
    bad "the key depends on the checkout's LOCATION: '$KA' vs '$KB'"
  fi
  git -C "$REPO_ROOT" worktree remove --force "$CK1" >/dev/null 2>&1
  git -C "$REPO_ROOT" worktree remove --force "$CK2" >/dev/null 2>&1
  git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1
else
  bad "could not create the two checkouts; the location-independence case did not run"
fi

# --- 8. the ambient CARGO_* namespace must not reach the build ---------------
# A closed policy, checked by poisoning the whole family. Each of these was
# measured reaching cargo while the key stayed identical.
: >"$SEED_TEST_LOG"
export CARGO_BUILD_RUSTC=/opt/otro/rustc CARGO_BUILD_RUSTFLAGS="-C debuginfo=0" \
       CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu \
       CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER=/wrapper \
       CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_DEV_DEBUG=1 \
       CARGO_PROFILE_DEV_LTO=thin CARGO_PROFILE_RELEASE_LTO=true
pub >/dev/null 2>&1
unset CARGO_BUILD_RUSTC CARGO_BUILD_RUSTFLAGS CARGO_BUILD_TARGET \
      CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER CARGO_PROFILE_DEV_OPT_LEVEL \
      CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_DEV_LTO CARGO_PROFILE_RELEASE_LTO
# The build must actually have run, or "no variable leaked" is vacuous: an empty
# log reads as "nothing recorded", which the old version of this assertion
# counted as a leak and reported as one.
if [ -z "$(cargo_field ARGV)" ]; then
  bad "the build did not run, so a leak check would be vacuous: $(head -1 "$SANDBOX/err")"
else
  ok "the build ran under the poisoned environment"
fi
LEAKED=""
for v in CARGO_BUILD_RUSTC CARGO_BUILD_RUSTFLAGS CARGO_BUILD_TARGET \
         CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER CARGO_PROFILE_DEV_OPT_LEVEL \
         CARGO_PROFILE_DEV_DEBUG CARGO_PROFILE_DEV_LTO; do
  val="$(cargo_field "$v")"
  if [ -n "$val" ] && [ "$val" != "<unset>" ]; then LEAKED="$LEAKED $v=$val"; fi
done
if [ -z "$LEAKED" ]; then
  ok "no ambient CARGO_*/RUST* variable reaches the build (poisoned all of them)"
else
  bad "these contractual variables still reached the build:$LEAKED"
fi

# --- 9. the workspace build contract, and what must NOT be in it ------------
# A profile table or a dependency feature changes the artifacts while leaving
# Cargo.lock byte-identical. A comment changes nothing at all.
WD="$SANDBOX/contract"
mk_tree() { mkdir -p "$1"; cp -r "$REPO_ROOT/.cargo" "$REPO_ROOT/Cargo.toml" \
    "$REPO_ROOT/Cargo.lock" "$REPO_ROOT/crates" "$1/"; }
tree_key() { SEED_REPO_ROOT="$1" bash -c '. "$2/seed_recipe.sh"
    seed_recipe_parse "$1" --features ""; seed_recipe_compute_key "$1"
    printf "%s" "$SEED_RECIPE_KEY"' _ "$1" "$SCRIPT_DIR" 2>/dev/null; }
mk_tree "$WD/base"; mk_tree "$WD/prof"; mk_tree "$WD/dep"; mk_tree "$WD/cosmetic"
python3 - "$WD/prof" <<'PY2'
import pathlib, re, sys
p = pathlib.Path(sys.argv[1]) / "Cargo.toml"
p.write_text(re.sub(r"(?m)^opt-level = .*$", "opt-level = 3", p.read_text(), count=1))
PY2
python3 - "$WD/dep" <<'PY2'
import pathlib, re, sys
p = pathlib.Path(sys.argv[1]) / "crates/webfang_core/Cargo.toml"
s = p.read_text()
m = re.search(r"^(\w[\w-]* = \{ version = \"[^\"]+\", features = \[)([^\]]*)(\])", s, re.M)
if m:
    s = s[:m.start()] + m.group(1) + m.group(2).rstrip() + ', "coherence-probe"' + m.group(3) + s[m.end():]
    p.write_text(s)
PY2
printf '\n# a harmless comment\n' >> "$WD/cosmetic/Cargo.toml"
KB="$(tree_key "$WD/base")"; KP="$(tree_key "$WD/prof")"
KD="$(tree_key "$WD/dep")";   KC="$(tree_key "$WD/cosmetic")"
if [ -n "$KP" ] && [ "$KB" != "$KP" ]; then
  ok "a [profile.*] change moves the key, with Cargo.lock untouched"
else bad "a profile change did not move the key (KP=$KP)"; fi
if [ -n "$KD" ] && [ "$KB" != "$KD" ]; then
  ok "a dependency-feature change moves the key, with Cargo.lock untouched"
else bad "a dependency feature change did not move the key (KD=$KD)"; fi
if [ -n "$KC" ] && [ "$KB" = "$KC" ]; then
  ok "a comment in Cargo.toml does NOT move the key (reuse across commits survives)"
else bad "the workspace digest is too coarse: a comment re-keys it"; fi

# --- 10. config `include` is refused, not guessed at -------------------------
# A real copy of the project with an `include` in its own config, because the
# refusal has to be seen on the real path — pointing at a directory that is not a
# cargo project at all just fails for the wrong reason.
IC="$SANDBOX/inc"; mkdir -p "$IC/shared"
cp -r "$REPO_ROOT/.cargo" "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" \
      "$REPO_ROOT/crates" "$IC/" 2>/dev/null
printf '[profile.dev]\nopt-level = 1\n' > "$IC/shared/policy.toml"
printf '[build]\ninclude = "../shared/policy.toml"\n' >> "$IC/.cargo/config.toml"
if SEED_REPO_ROOT="$IC" bash -c '. "$1/seed_recipe.sh"
  seed_recipe_parse "$2" --features ""
  seed_recipe_has_config_include "$2"' _ "$SCRIPT_DIR" "$IC" >/dev/null 2>&1; then
  ok "a config with \`include\` is detected"
else
  bad "a config with include was not detected"
fi
SEED_REPO_ROOT="$IC" PATH="$SANDBOX/bin:$PATH" CARGO_TARGET_DIR="$SANDBOX/tgt" \
  bash "$SCRIPT_DIR/seed_publish.sh" --seeds-root "$SANDBOX/seeds" --features "" \
  >/dev/null 2>"$SANDBOX/err2"
if grep -q 'include' "$SANDBOX/err2"; then
  ok "the producer refuses it, naming include as the reason"
else
  bad "producer did not refuse on include grounds: $(head -1 "$SANDBOX/err2")"
fi

# --- 11. every declarative field that changes a unit is in the contract ------
# `foo = "1"` and `foo = { version = "1", default-features = false }` produce the
# same lock and the same explicit feature list, and cargo still compiles them with
# different feature sets. `uses_default_features` was missing from the normaliser
# and this was verified identical-key before it was added.
DF="$SANDBOX/deffeat"
mk_tree "$DF/base"; mk_tree "$DF/nodefault"
python3 - "$DF/nodefault/crates/webfang_core/Cargo.toml" <<'PY2'
import pathlib, re, sys
p = pathlib.Path(sys.argv[1]); s = p.read_text()
m = re.search(r'^(\w[\w-]* = \{ version = "[^"]+", features = \[[^\]]*\] \})$', s, re.M)
if m:
    p.write_text(s[:m.start()] + m.group(1)[:-2] + ', default-features = false }' + s[m.end():])
PY2
KF="$(tree_key "$DF/base")"; KN="$(tree_key "$DF/nodefault")"
if [ -n "$KN" ] && [ "$KF" != "$KN" ]; then
  ok "default-features=false moves the key, with Cargo.lock untouched"
else bad "uses_default_features is not in the workspace contract (KN='$KN')"; fi

# --- 12. the recipe pins which cargo and which rustc actually run -------------
pub >/dev/null 2>&1
# The recipe pins the compiler the key hashed. The previous version of this check
# accepted an ABSENT RUSTC as a pass, so it was satisfied by not pinning the
# compiler at all — green for the wrong reason, and the mutation that removed the
# pin did not turn it red.
PINNED_RUSTC="$( # shellcheck source=scripts/seed_recipe.sh
  . "$SCRIPT_DIR/seed_recipe.sh"; seed_recipe_parse "$REPO_ROOT" --features ""
                  printf '%s' "$SEED_RECIPE_RUSTC_BIN" 2>/dev/null)"
GOT_RUSTC="$(cargo_field RUSTC)"
if [ -n "$GOT_RUSTC" ] && [ "$GOT_RUSTC" != "<unset>" ]; then
  if [ "$GOT_RUSTC" = "$PINNED_RUSTC" ]; then
    ok "the build compiles with the same rustc the key hashed"
  else
    bad "the build's RUSTC is '$GOT_RUSTC', the recipe resolved '$PINNED_RUSTC'"
  fi
else
  bad "the recipe does not pin a compiler for the build (RUSTC=${GOT_RUSTC:-absent});"
  echo "       the key hashed a rustc the build was free to ignore"
fi
PINNED_HOME="$( # shellcheck source=scripts/seed_recipe.sh
  . "$SCRIPT_DIR/seed_recipe.sh"; seed_recipe_parse "$REPO_ROOT" --features ""
                  printf '%s' "$SEED_RECIPE_CARGO_HOME" 2>/dev/null)"
case "$PINNED_HOME" in
  /*) ok "CARGO_HOME is pinned to an absolute path" ;;
  *)  bad "CARGO_HOME is pinned to a relative path: '$PINNED_HOME'" ;;
esac
# a relative CARGO_HOME must be canonicalised, not carried through: the same
# string reads different files from different working directories.
REL="$( # shellcheck source=scripts/seed_recipe.sh
  . "$SCRIPT_DIR/seed_recipe.sh"
        CARGO_HOME=relhome seed_recipe_parse "$REPO_ROOT" --features ""
        printf '%s' "$SEED_RECIPE_CARGO_HOME" 2>/dev/null)"
case "$REL" in
  /*relhome) ok "a relative CARGO_HOME is canonicalised before being pinned" ;;
  *)        bad "a relative CARGO_HOME was carried through as '$REL'" ;;
esac

# --- 13. the installation path is not the artifact identity -------------------
# The design promises: the same compiler installed in two places is the same
# compiler, so the key must NOT move when only the installation path moves. That
# promise was documented but never proven — the coverage so far proved absolute
# paths and CARGO_HOME, not two distinct rustup homes exposing one toolchain.
#
# Tested at the layer the claim lives on. Driving it through a fake RUSTUP_HOME
# would be useless: `cargo` is a rustup shim, so pointing RUSTUP_HOME at a
# directory with no installation stops cargo itself from running, and the run
# would fail for a reason that has nothing to do with the key.
for n in one two; do
  T="$SANDBOX/rustup-$n/toolchains/1.88.0-x86_64-unknown-linux-gnu/bin"
  mkdir -p "$T"
  cat > "$T/rustc" <<'RSH'
#!/usr/bin/env bash
case "${1:-}" in
  -V)  echo "rustc 1.88.0 (03f2f8c7e 2025-04-28)" ;;
  -vV) printf 'rustc 1.88.0 (03f2f8c7e 2025-04-28)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.88.0\n' ;;
  *)   exit 0 ;;
esac
RSH
  chmod +x "$T/rustc"
done
key_at() { SEED_REPO_ROOT="$REPO_ROOT" bash "$SCRIPT_DIR/seed_compat_key.sh" \
             --features "" --profile dev --target "" \
             --config-digest "fixed" --workspace-digest "fixed" \
             --recipe-schema 3 --wrapper-policy none --toolchain-id "1.88.0" \
             --rustc-bin "$SANDBOX/rustup-$1/toolchains/1.88.0-x86_64-unknown-linux-gnu/bin/rustc" \
             2>/dev/null | tail -1; }
K1="$(key_at one)"; K2="$(key_at two)"
if [ -n "$K1" ] && [ "$K1" = "$K2" ]; then
  ok "two installations of the same toolchain produce the same key"
else
  bad "the key moved with the installation path (one='$K1' two='$K2'); the promise"
  echo "       that an installation path is not an artifact identity does not hold"
fi
# and the converse still holds: a DIFFERENT compiler must move it, or the case
# above would pass for the degenerate reason that nothing reaches the key.
cat > "$SANDBOX/rustup-two/toolchains/1.88.0-x86_64-unknown-linux-gnu/bin/rustc" <<'RSH'
#!/usr/bin/env bash
case "${1:-}" in
  -V)  echo "rustc 1.88.0 (deadbeefff 2026-01-01)" ;;
  -vV) printf 'rustc 1.88.0 (deadbeefff 2026-01-01)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.88.0\n' ;;
  *)   exit 0 ;;
esac
RSH
chmod +x "$SANDBOX/rustup-two/toolchains/1.88.0-x86_64-unknown-linux-gnu/bin/rustc"
K3="$(key_at two)"
if [ -n "$K3" ] && [ "$K3" != "$K1" ]; then
  ok "a different compiler does move the key, so the case above is not vacuous"
else
  bad "a different rustc did not move the key; the equality above proves nothing"
fi

amb_bad=0
# The DIRECT call, not the recipe path. seed_target.sh and seed_publish.sh both
# invoke seed_compat_key.sh themselves with the recipe's arguments, in their own
# shell, where the caller's ambient is still intact. An earlier version of this
# regression went through the recipe and was green against the defect: the recipe
# path happened to be immune, so the probe could not see the fork. The defect lived
# one layer down, in the function the two scripts call directly.
amb_direct() {
  if [ -z "${1:-}" ]; then set --; fi
  # shellcheck disable=SC2016  # $0/$1/$2 and ${A[@]} are resolved by the bash -c at runtime
  env "$@" SEED_REPO_ROOT="$REPO_ROOT" bash -c '
    . "$0"; seed_recipe_parse "$1" --features "" >/dev/null 2>&1
    mapfile -t A < <(seed_recipe_key_args)
    exec bash "$2" "${A[@]}" 2>/dev/null' \
    "$SCRIPT_DIR/seed_recipe.sh" "$REPO_ROOT" "$SCRIPT_DIR/seed_compat_key.sh"
}
amb_key() {
  if [ -z "${1:-}" ]; then set --; fi
  # shellcheck disable=SC2016  # $0/$1 are resolved by the bash -c at runtime
  env "$@" bash -c '. "$0"; seed_recipe_parse "$1" --features ""; seed_recipe_compute_key "$1"; printf "%s" "$SEED_RECIPE_KEY"' \
      "$SCRIPT_DIR/seed_recipe.sh" "$REPO_ROOT" 2>/dev/null
}

# --- 14. a differing ambient does not fork the key ----------------------------
# This is the regression for the defect that shipped in #1721: CARGO_INCREMENTAL
# was read from the ambient and written into the [key] block, so main (which sets
# 1) and every worktree (which sets 0) computed different keys. The primary
# workflow — publish from main, consume from a worktree — failed with
# `incompatible-manifest` against a seed that was sitting there under a matching
# name. Every test passed because publisher and consumer always ran with the same
# ambient, so the fork was invisible to all of them.
AMBIENT_BASE="$(amb_key)"
DIRECT_BASE="$(amb_direct)"
DIRECT_BAD=0
for spec in "CARGO_INCREMENTAL=0" "CARGO_INCREMENTAL=1" "CARGO_INCREMENTAL=true"; do
  DK="$(amb_direct "$spec")"
  if [ -z "$DK" ] || [ "$DK" != "$DIRECT_BASE" ]; then
    bad "seed_compat_key.sh called directly: $spec forks the key ($DIRECT_BASE -> $DK)"
    DIRECT_BAD=1
  fi
done
if [ -n "$DIRECT_BASE" ] && [ "$DIRECT_BAD" -eq 0 ]; then
  ok "the direct key call is identical across CARGO_INCREMENTAL values"
else
  bad "the direct key call forks on CARGO_INCREMENTAL"
fi
for spec in "CARGO_INCREMENTAL=0" "CARGO_INCREMENTAL=1" "CARGO_INCREMENTAL=true" \
            "CARGO_BUILD_RUSTC=/opt/otro/rustc" "CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu" \
            "CARGO_PROFILE_DEV_OPT_LEVEL=3" "CARGO_TARGET_DIR=/tmp/otro" "RUSTC=/opt/otro/rustc"; do
  AK="$(amb_key "$spec")"
  if [ -z "$AK" ]; then bad "amb_key produced nothing for $spec";
  elif [ "$AK" = "$AMBIENT_BASE" ]; then :; else
    bad "$spec forks the key: $AMBIENT_BASE -> $AK"
  fi
done
if [ -n "$AMBIENT_BASE" ] && [ "$amb_bad" -eq 0 ]; then
  ok "the key is identical across differing CARGO_*/RUST* ambients (8 probes)"
else
  bad "at least one ambient forked the key"
fi
# RUSTFLAGS is the deliberate counter-example and must stay: the recipe captures
# it and passes it to the build, so key and build agree and a flags mismatch has
# to reject the seed. If this ever stops holding, the recipe is lying again.
RF="$(amb_key "RUSTFLAGS=-Cdebuginfo=0")"
if [ -n "$RF" ] && [ "$RF" != "$AMBIENT_BASE" ]; then
  ok "RUSTFLAGS still moves the key, as the contract intends"
else
  bad "RUSTFLAGS no longer moves the key; either the recipe stopped feeding it to"
  echo "       the build, or it became a constant that silently forks nothing"
fi

# --- 15. the reference build runs with --locked (B17) --------------------------
# The key digests Cargo.lock, so the build has to use THAT lock. Without
# --locked, cargo may rewrite a lock that no longer matches the manifests —
# during the build, after the key already hashed the old bytes. The published
# seed then describes a resolution the key never recorded, and the consumer
# that verifies the key reports "reproducible" for a build that was not.
#
# The flag belongs to the RECIPE, next to --profile and --target, for the same
# reason they do: the recipe is the single description of the build and both
# sides read it.
# NEGATION: pre-fix the recipe's cargo args carried no --locked, so the very
# build that publishes the seed was free to rewrite the lock the key hashed.
pub >/dev/null 2>&1
ARGV_LOCKED="$(cargo_field ARGV)"
case "$ARGV_LOCKED" in
  *--locked*) ok "the reference build runs with --locked" ;;
  *) bad "the reference build does NOT carry --locked (argv: $ARGV_LOCKED)" ;;
esac

# --- 16. the lock the key hashed is the lock the build leaves behind ----------
# The invariant S14.3 names: the digest of the lock the key recorded is the
# digest of the lock the build left on disk.
#
# The fixture is two crates with only PATH dependencies — no registry, so
# --offline is safe and nothing is ever downloaded — and it is exercised with
# `cargo metadata`, which performs the SAME resolution (and the same lock
# rewrite) as a build while compiling NOTHING. Building the real workspace
# here would be the resource accident this suite must not be.
#
# `--no-deps` is deliberately NOT used: with it cargo skips the resolve and
# therefore never checks the lock at all (measured: exit 0 on a stale lock),
# so it cannot witness this invariant.
#
# The lock goes stale by adding a second path dependency: the manifest moves,
# the lock does not. That is exactly the state the key can hash and the build
# can silently rewrite.
LOCKFX="$SANDBOX/lockfx"
mkdir -p "$LOCKFX/app/src" "$LOCKFX/dep/src" "$LOCKFX/dep2/src"
printf '[package]\nname = "coherence-dep"\nversion = "0.1.0"\nedition = "2021"\n' \
  > "$LOCKFX/dep/Cargo.toml"
printf 'pub fn dep() -> u8 { 1 }\n' > "$LOCKFX/dep/src/lib.rs"
printf '[package]\nname = "coherence-dep2"\nversion = "0.1.0"\nedition = "2021"\n' \
  > "$LOCKFX/dep2/Cargo.toml"
printf 'pub fn dep2() -> u8 { 2 }\n' > "$LOCKFX/dep2/src/lib.rs"
printf '[package]\nname = "coherence-app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ncoherence-dep = { path = "../dep" }\n' \
  > "$LOCKFX/app/Cargo.toml"
printf 'pub fn app() -> u8 { coherence_dep::dep() }\n' > "$LOCKFX/app/src/lib.rs"

# lock_meta <locked|free>: resolve the fixture, optionally with --locked.
# Returns cargo's exit code; the stderr lands in $SANDBOX/meta_err.
lock_meta() {
  local mode="$1"
  local -a margs=(metadata --offline --format-version 1)
  [ "$mode" = "locked" ] && margs=(metadata --locked --offline --format-version 1)
  ( cd "$LOCKFX/app" \
    && CARGO_TARGET_DIR="$LOCKFX/tgt" CARGO_BUILD_JOBS=1 \
       env -u RUSTC_WRAPPER "$REAL_CARGO" "${margs[@]}" \
  ) >/dev/null 2>"$SANDBOX/meta_err"
}

if [ -z "$REAL_CARGO" ]; then
  bad "no real cargo on PATH: the lock-invariant case did not run"
elif ! lock_meta free || [ ! -f "$LOCKFX/app/Cargo.lock" ]; then
  bad "the fixture produced no Cargo.lock: the lock-invariant case did not run"
else
  # Make the lock stale: a second path dependency the lock does not know.
  printf 'coherence-dep2 = { path = "../dep2" }\n' >> "$LOCKFX/app/Cargo.toml"
  LOCK_BEFORE="$(sha256sum "$LOCKFX/app/Cargo.lock" | cut -d' ' -f1)"
  lock_meta locked
  RC_LOCKED=$?
  LOCK_AFTER="$(sha256sum "$LOCKFX/app/Cargo.lock" | cut -d' ' -f1)"
  if [ "$RC_LOCKED" -ne 0 ]; then
    ok "with --locked a stale lock FAILS the build (cargo exit $RC_LOCKED)"
  else
    bad "--locked did NOT fail on a stale lock (cargo exit 0)"
  fi
  if [ "$LOCK_BEFORE" = "$LOCK_AFTER" ]; then
    ok "with --locked the on-disk lock is byte-identical (the key's digest still describes it)"
  else
    bad "--locked rewrote the lock anyway ($LOCK_BEFORE -> $LOCK_AFTER)"
  fi
  lock_meta free
  RC_FREE=$?
  LOCK_FREE="$(sha256sum "$LOCKFX/app/Cargo.lock" | cut -d' ' -f1)"
  if [ "$RC_FREE" -eq 0 ] && [ "$LOCK_FREE" != "$LOCK_BEFORE" ]; then
    ok "without --locked the same stale lock is rewritten (cargo exit 0)"
  else
    bad "without --locked the lock was NOT rewritten (exit $RC_FREE, digest $LOCK_FREE)"
  fi
fi

# --- 17. a failed reference build fails the publication ------------------------
# S14.2: the failure has to be fail-closed AND visible — cargo's own message
# has to reach the operator. A `|| true`, or a subshell that swallows the
# status, would publish a seed that no successful build ever produced.
#
# The stub has to be on PATH for THIS invocation too: the publisher resolves
# the cargo binary it will run, and without the stub it would resolve the real
# one — and a real `cargo build --workspace` of this repository is the resource
# accident this suite must not cause. Measured cost of that slip: 1m15s wall,
# 6m33s CPU, 1.9 GB of artifacts, for a test that was supposed to compile
# nothing.
# NEGATION: pre-fix nothing pinned this, so a swallowed status would have
# published on a target dir that came from a failed build.
PATH="$SANDBOX/bin:$PATH" SEED_TEST_FAIL_BUILD=1 bash "$SCRIPT_DIR/seed_publish.sh" \
  --seeds-root "$SANDBOX/seeds" --features "" >"$SANDBOX/out17" 2>"$SANDBOX/err17"
RC17=$?
if [ "$RC17" -ne 0 ] \
  && grep -q 'reference build failed' "$SANDBOX/err17" \
  && grep -q 'simulated build failure' "$SANDBOX/err17"; then
  ok "a failed reference build fails the publication and surfaces cargo's message"
else
  bad "the publication did not fail closed on a failed build (exit $RC17)"
fi

# --- 18. the workspace build contract is REQUIRED by the key ------------------
# The digest was optional, so omitting it emitted `workspace_contract = ""`:
# two workspaces that shared a rustc, a configuration and a lock — but not
# the same manifests, profiles or dependency features — hashed to the SAME
# key, and a seed built under one build contract was offered to a consumer
# compiling under another.
#
# NEGATION: with a digest the key is computed, so the refusal below is a
# real refusal and not a script that never works.
KEY_ARGS_BASE=(--features "" --profile dev --recipe-schema 1
               --wrapper-policy prescribed-empty --toolchain-id ""
               --config-digest "sha256:deadbeef")
for variant in "omitted" "empty"; do
  if [ "$variant" = "omitted" ]; then
    OUT18="$(bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS_BASE[@]}" 2>&1)"; RC18=$?
  else
    OUT18="$(bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS_BASE[@]}" \
              --workspace-digest "" 2>&1)"; RC18=$?
  fi
  if [ "$RC18" -ne 0 ]; then
    ok "the key refuses a $variant --workspace-digest (exit $RC18)"
  else
    bad "the key was computed with a $variant workspace digest: $OUT18"
  fi
done
K_WITH_DIGEST="$(bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS_BASE[@]}" \
                 --workspace-digest "sha256:abc" 2>/dev/null | tail -1)"
if [ -n "$K_WITH_DIGEST" ]; then
  ok "the key is computed when the workspace digest IS present"
else
  bad "the key refuses a present workspace digest too; the case above proves nothing"
fi

# --- 19. the env-strip policy is LIVE, and it is HASHED ----------------------
# The policy listed RUSTC_WRAPPER, SCCACHE_DIR and SCCACHE_BASEDIRS, and the
# real strip was by PREFIX (CARGO_*/RUST*), so the two SCCACHE_* variables
# reached the build while the policy claimed they had been removed. The list
# was also a variable nobody read, and the key hashed the literal
# "prescribed-empty", so editing the policy re-keyed nothing.
: >"$SEED_TEST_LOG"
SCCACHE_DIR=/probe SCCACHE_BASEDIRS=/probe2 pub >/dev/null 2>&1
if [ "$(cargo_field SCCACHE_DIR)" = "<unset>" ]; then
  ok "the build ran without the policy-listed SCCACHE_DIR"
else
  bad "SCCACHE_DIR reached the build: $(cargo_field SCCACHE_DIR)"
fi
# And the policy is part of the contract, so changing it changes the key.
STRIP_SANDBOX="$SANDBOX/strip"
mkdir -p "$STRIP_SANDBOX/scripts"
cp "$SCRIPT_DIR/seed_recipe.sh" "$SCRIPT_DIR/seed_compat_key.sh" \
   "$SCRIPT_DIR/seed_workspace_contract.py" "$STRIP_SANDBOX/scripts/"
key_with_strip_list() {  # $1 = the scripts dir -> the key it computes
  SEED_REPO_ROOT="$REPO_ROOT" bash -c '. "$1/seed_recipe.sh"
    seed_recipe_parse "$2" --features "" >/dev/null 2>&1
    seed_recipe_compute_key "$2"
    printf "%s" "$SEED_RECIPE_KEY"' _ "$1" "$REPO_ROOT"
}
K_STRIP_A="$(key_with_strip_list "$STRIP_SANDBOX/scripts")"
sed -i 's/SEED_RECIPE_STRIPPED_ENV="RUSTC_WRAPPER SCCACHE_DIR SCCACHE_BASEDIRS"/SEED_RECIPE_STRIPPED_ENV="RUSTC_WRAPPER SCCACHE_DIR SCCACHE_BASEDIRS COHERENCE_PROBE_VAR"/' \
  "$STRIP_SANDBOX/scripts/seed_recipe.sh"
K_STRIP_B="$(key_with_strip_list "$STRIP_SANDBOX/scripts")"
if [ -n "$K_STRIP_A" ] && [ "$K_STRIP_A" != "$K_STRIP_B" ]; then
  ok "editing the strip list re-keys the seed ($K_STRIP_A -> $K_STRIP_B)"
else
  bad "the strip policy is not hashed into the key ($K_STRIP_A -> $K_STRIP_B);"
  echo "       the policy can change without the artifacts changing"
fi

# --- 20. a multi-line RUSTFLAGS survives the env handshake -------------------
# The build environment travels as a printed list, one argument per line, so
# a value containing a newline was SPLIT into several arguments — and `env`
# reads the tail of a split value as a COMMAND, not as an assignment. The
# value has to reach cargo exactly as the recipe captured it.
MULTI_RUSTFLAGS=$'-A\ndead_code\n-C debuginfo=0'
: >"$SEED_TEST_LOG"
RUSTFLAGS="$MULTI_RUSTFLAGS" pub >/dev/null 2>&1
GOT_RF="$(cargo_field RUSTFLAGS_Q)"
WANT_RF="$(printf '%q' "$MULTI_RUSTFLAGS")"
if [ -n "$GOT_RF" ] && [ "$GOT_RF" = "$WANT_RF" ]; then
  ok "a multi-line RUSTFLAGS reaches the build byte-exact"
else
  bad "the multi-line RUSTFLAGS was altered on the way to the build:"
  echo "       got:  $GOT_RF"
  echo "       want: $WANT_RF"
fi

# --- 21. a release and a cross-target build can be PUBLISHED ------------------
# The prune hardcoded `debug`, so a complete release build and a complete
# cross-target build were both refused ("no debug/.fingerprint"): those two
# recipes could never produce a seed at all, however complete the build was.
#
# The stub below creates the layout cargo creates — the profile directory
# inside the target directory, under the target's own prefix — so the
# publish is exercised against a real tree instead of a reimplementation.
mkdir -p "$SANDBOX/layout/bin"
cat >"$SANDBOX/layout/bin/cargo" <<'STUB'
#!/usr/bin/env bash
if [ "${1:-}" = "metadata" ]; then exec "$WEBFANG_SAFETY_CARGO" "$@"; fi
case " $* " in
  *" build "*)
    prof=dev; tgt=""
    argv=("$@"); i=0
    while (( i < ${#argv[@]} )); do
      case "${argv[$i]}" in
        --profile) prof="${argv[$((i+1))]}"; ((i+=2)) ;;
        --target)  tgt="${argv[$((i+1))]}";  ((i+=2)) ;;
        *) ((i+=1)) ;;
      esac
    done
    dir="$CARGO_TARGET_DIR"
    [ -n "$tgt" ] && dir="$dir/$tgt"
    case "$prof" in
      dev|test)      dir="$dir/debug" ;;
      release|bench) dir="$dir/release" ;;
      *)             dir="$dir/$prof" ;;
    esac
    mkdir -p "$dir/.fingerprint/third-fake-0beef00d"
    printf 'registry-path\n' >"$dir/.fingerprint/third-fake-0beef00d/output"
    exit 0 ;;
esac
echo "layout-cargo: unsupported invocation: $*" >&2
exit 1
STUB
chmod +x "$SANDBOX/layout/bin/cargo"
export WEBFANG_SAFETY_CARGO="$SANDBOX/safety/cargo"
LAYOUT_SEEDS="$SANDBOX/layout-seeds"
layout_publish() {  # $@ = the recipe flags -> prints the exit code
  chmod -R u+w "$LAYOUT_SEEDS" 2>/dev/null || true
  rm -rf "$LAYOUT_SEEDS"; mkdir -p "$LAYOUT_SEEDS"
  PATH="$SANDBOX/layout/bin:$PATH" bash "$SCRIPT_DIR/seed_publish.sh" \
    --seeds-root "$LAYOUT_SEEDS" "$@" >"$SANDBOX/layout.out" 2>&1
  echo $?
}
RC_REL="$(layout_publish --profile release)"
K_REL="$(sed -n 's/^==> key \(v[0-9]-[0-9a-f]*\).*/\1/p' "$SANDBOX/layout.out" | head -1)"
if [ "$RC_REL" -eq 0 ] && [ -d "$LAYOUT_SEEDS/$K_REL/release/.fingerprint" ]; then
  ok "a complete RELEASE build publishes, and the seed carries the release profile"
else
  bad "the release publish failed (exit $RC_REL): $(tail -3 "$SANDBOX/layout.out")"
fi
RC_TGT="$(layout_publish --target aarch64-unknown-linux-gnu)"
K_TGT="$(sed -n 's/^==> key \(v[0-9]-[0-9a-f]*\).*/\1/p' "$SANDBOX/layout.out" | head -1)"
if [ "$RC_TGT" -eq 0 ] && [ -d "$LAYOUT_SEEDS/$K_TGT/aarch64-unknown-linux-gnu/debug/.fingerprint" ]; then
  ok "a complete CROSS-TARGET build publishes, and the seed carries the target prefix"
else
  bad "the cross-target publish failed (exit $RC_TGT): $(tail -3 "$SANDBOX/layout.out")"
fi

printf '\nharness: %d ok, %d fail\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
echo "the key describes the build: one recipe, one contract, no unkeyed door"
