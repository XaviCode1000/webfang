#!/usr/bin/env bash
#
# test_check_target_isolation.sh — semantics harness for
# scripts/check_target_isolation.sh (#1679).
#
# Hermetic and cargo-free: the guard is driven against throwaway `git init`
# fixture repos (one initial commit, `git worktree add` siblings, per-tree
# .envrc written by the fixture), so every branch of the target policy is
# exercised in milliseconds without cargo, a network, or the real stores.
#
# Isolation guarantees:
#   - WEBFANG_SEEDS_ROOT / WEBFANG_QUARANTINE_ROOT always point into the
#     sandbox, so the real seed and quarantine stores are never touched;
#   - HOME is redirected into the sandbox, so the literal-$HOME expansion
#     case never reads or writes the caller's home;
#   - each case sets or unsets CARGO_TARGET_DIR explicitly, so the ambient
#     CARGO_TARGET_DIR of whoever runs the harness cannot leak in (a direnv'd
#     worktree shell would otherwise decide case outcomes silently);
#   - GIT_CONFIG_GLOBAL / GIT_CONFIG_SYSTEM point at /dev/null, so the
#     maintainer's git config cannot shape fixture behaviour.
#
# What it pins — the case matrix of #1679. The decision-tree ORDER is itself
# load-bearing, so the cases sit in the sequence the guard evaluates them:
#    1  unset target                         -> 2 target-unset
#    2  target inside the seed store         -> 2 seed-store-target
#    3  target inside the quarantine store   -> 2 quarantine-store-target
#    4  worktree env == main's declaration   -> 2 target-owned-by-other-worktree
#    5  main checkout not bootstrapped       -> 2 main-unbootstrapped
#    6  KEY: tree B env == tree A's target   -> 2 target-owned-by-other-worktree
#                                              EVEN with --allow-unregistered-target
#    7  env == own registered target         -> 0
#    8  unregistered, no flag                -> 2 unregistered-target
#    9  unregistered + --allow-unregistered  -> 0
#   10  extra worktree without .envrc        -> case 7 still 0 (tolerated non-owner)
#   11  sibling .envrc uses literal $HOME    -> collision still detected
#   12  env is a symlink to another tree's   -> 2 target-owned-by-other-worktree
#   13  two trees registered to ONE target   -> 2 (collision before own-registration)
#   14  run in main with main's own target   -> 0
#   15  prefix-sharing sibling of the store  -> NOT seed-store-target
#                                             -> 2 unregistered-target
#   16  target that cannot be canonicalised  -> 2 uncanonicalizable-target
#   17  git enumeration fails                -> 2 worktree-enumeration-failed
#                                              (fail-closed; case 16 uses a
#                                              set-but-empty target, the one
#                                              input coreutils realpath -m 9.10
#                                              refuses to canonicalise)
#
# Case 17 note: the worktree list is intercepted by a stub `git` that fails
# ONLY `worktree list` and execs the real git for everything else, so the
# fail-closed branch is proven without touching the fixture repos.
#
# Exit code: 0 when every case holds, 1 otherwise.
#
# Usage: bash scripts/tests/test_check_target_isolation.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ISOLATION_SCRIPT="$ROOT/scripts/check_target_isolation.sh"

FAILED=0
LAST_OUT=""

if [ ! -r "$ISOLATION_SCRIPT" ]; then
  echo "guard script not found: $ISOLATION_SCRIPT" >&2
  exit 1
fi

# --- sandbox -----------------------------------------------------------------
SANDBOX="$(mktemp -d)"
FIXTURES=()
ISO_PATH_PREPEND=""

# shellcheck disable=SC2329 # invoked by the EXIT trap, not by name
cleanup() {
  local fx wt
  # .envrc files are untracked, so plain `worktree remove` may refuse;
  # remove each fixture worktree properly, then reclaim the sandbox.
  for fx in ${FIXTURES[@]+"${FIXTURES[@]}"}; do
    for wt in "$fx"/wt-*; do
      [ -d "$wt" ] || continue
      git -C "$fx" worktree remove --force "$wt" >/dev/null 2>&1 || true
    done
  done
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null

# --- fixture builders ----------------------------------------------------------
# new_fixture: one throwaway repo (git init + one commit), tracked for cleanup.
new_fixture() {
  local fx
  fx="$(mktemp -d "$SANDBOX/fx.XXXXXX")" || exit 1
  FIXTURES+=("$fx")
  git init -q -b main "$fx" || exit 1
  git -C "$fx" config user.email harness@example.invalid
  git -C "$fx" config user.name "Target Isolation Harness"
  echo "fixture" >"$fx/README.md"
  git -C "$fx" add README.md
  git -C "$fx" commit -qm init || exit 1
  printf '%s' "$fx"
}

# add_worktree <fixture> <name>: sibling worktree on its own branch.
add_worktree() {
  git -C "$1" worktree add -q -b "branch-$2" "$1/$2" >/dev/null 2>&1 || {
    echo "harness error: git worktree add failed for $1/$2" >&2
    exit 1
  }
  printf '%s' "$1/$2"
}

# write_envrc <tree> <line>...: per-tree declaration, exactly the shapes the
# real bootstrap produces (absolute path, or literal $HOME/...).
write_envrc() {
  local tree="$1"
  shift
  printf '%s\n' "$@" >"$tree/.envrc"
}

# --- case runner -----------------------------------------------------------------
# invoke_iso <root> <target|__UNSET__> [guard args...]
# Runs the guard with --root (the gate's calling convention), never relying on
# the caller's CWD: the invocation happens from the neutral sandbox root, so a
# bug that resolves git state from CWD instead of --root cannot pass unnoticed.
# `__UNSET__` is the sentinel for a strictly-unset CARGO_TARGET_DIR; any other
# value — including the empty string — is exported verbatim.
invoke_iso() {
  local root="$1" target="$2"
  shift 2
  (
    export HOME="$SANDBOX/home"
    export WEBFANG_SEEDS_ROOT="$SEEDS_FIXTURE"
    export WEBFANG_QUARANTINE_ROOT="$QUARANTINE_FIXTURE"
    export PATH="${ISO_PATH_PREPEND:+$ISO_PATH_PREPEND:}$PATH"
    if [ "$target" = "__UNSET__" ]; then
      unset CARGO_TARGET_DIR
    else
      export CARGO_TARGET_DIR="$target"
    fi
    cd "$SANDBOX" || exit 97
    bash "$ISOLATION_SCRIPT" --root "$root" "$@" 2>&1
  )
}

# run_case <name> <want-rc> <want-reason> <root> <target|__UNSET__> [args...]
# One verdict line per case; captured output printed only on failure. The
# combined output is kept in LAST_OUT for follow-up assertions.
run_case() {
  local name="$1" want_rc="$2" want_reason="$3" root="$4" target="$5"
  shift 5
  local out rc ok=1
  out="$(invoke_iso "$root" "$target" "$@")"
  rc=$?
  LAST_OUT="$out"
  [ "$rc" = "$want_rc" ] || ok=0
  case "$out" in *"reason=$want_reason"*) ;; *) ok=0 ;; esac
  if [ "$ok" = 1 ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: want exit=$want_rc reason=$want_reason, got exit=$rc" >&2
    echo "--- captured output ---" >&2
    printf '%s\n' "$out" >&2
    echo "--- end captured output ---" >&2
    FAILED=1
  fi
}

assert_contains() {
  local label="$1" haystack="$2" needle="$3"
  case "$haystack" in
    *"$needle"*) echo "  ok   $label contains '$needle'" ;;
    *) echo "  FAIL $label: '$needle' not found in output" >&2
       printf '%s\n' "$haystack" >&2
       FAILED=1 ;;
  esac
}

assert_not_contains() {
  local label="$1" haystack="$2" needle="$3"
  case "$haystack" in
    *"$needle"*) echo "  FAIL $label: '$needle' unexpectedly present" >&2
                 printf '%s\n' "$haystack" >&2
                 FAILED=1 ;;
    *) echo "  ok   $label without '$needle'" ;;
  esac
}

# --- fixtures ---------------------------------------------------------------------
STO="$SANDBOX/stores"
SEEDS_FIXTURE="$STO/seeds"
QUARANTINE_FIXTURE="$STO/quarantine"
mkdir -p "$SEEDS_FIXTURE" "$QUARANTINE_FIXTURE" "$SANDBOX/home"

# FX1: main(.envrc -> M1) + wt-a(.envrc -> A1) + wt-b (deliberately NO .envrc).
FX1="$(new_fixture)"
T1="$SANDBOX/fx1-targets"
mkdir -p "$T1"
M1="$T1/main-target"
A1="$T1/wt-a-target"
WT_A="$(add_worktree "$FX1" wt-a)"
add_worktree "$FX1" wt-b >/dev/null
write_envrc "$FX1" "export CARGO_TARGET_DIR=$M1" "export CARGO_INCREMENTAL=1"
write_envrc "$WT_A" "export CARGO_TARGET_DIR=$A1" "export CARGO_INCREMENTAL=0" "unset RUSTC_WRAPPER"

# FX5: main has NO .envrc at all; wt-a declares A5.
FX5="$(new_fixture)"
T5="$SANDBOX/fx5-targets"
mkdir -p "$T5"
A5="$T5/wt-a-target"
WT5A="$(add_worktree "$FX5" wt-a)"
write_envrc "$WT5A" "export CARGO_TARGET_DIR=$A5" "export CARGO_INCREMENTAL=0"

# FX3: main(.envrc -> M3) + wt-a(.envrc -> A3) + wt-b(.envrc -> B3).
FX3="$(new_fixture)"
T3="$SANDBOX/fx3-targets"
mkdir -p "$T3"
M3="$T3/main-target"
A3="$T3/wt-a-target"
B3="$T3/wt-b-target"
WT3A="$(add_worktree "$FX3" wt-a)"
WT3B="$(add_worktree "$FX3" wt-b)"
write_envrc "$FX3" "export CARGO_TARGET_DIR=$M3" "export CARGO_INCREMENTAL=1"
write_envrc "$WT3A" "export CARGO_TARGET_DIR=$A3" "export CARGO_INCREMENTAL=0"
write_envrc "$WT3B" "export CARGO_TARGET_DIR=$B3" "export CARGO_INCREMENTAL=0"

# FX4: two worktrees registered to the SAME target S4 (the issue's agent A/B).
FX4="$(new_fixture)"
T4="$SANDBOX/fx4-targets"
mkdir -p "$T4"
M4="$T4/main-target"
S4="$T4/shared-target"
WT4A="$(add_worktree "$FX4" wt-a)"
WT4B="$(add_worktree "$FX4" wt-b)"
write_envrc "$FX4" "export CARGO_TARGET_DIR=$M4" "export CARGO_INCREMENTAL=1"
write_envrc "$WT4A" "export CARGO_TARGET_DIR=$S4"
write_envrc "$WT4B" "export CARGO_TARGET_DIR=$S4"

# FX6: sibling declaration written the way real sibling .envrc files are —
# literal $HOME/... — pointing under the sandbox HOME.
FX6="$(new_fixture)"
T6="$SANDBOX/fx6-targets"
H6="$SANDBOX/home/targets"
mkdir -p "$T6" "$H6"
M6="$T6/main-target"
B6="$T6/wt-b-target"
A6="$H6/wt-a-target"
WT6A="$(add_worktree "$FX6" wt-a)"
WT6B="$(add_worktree "$FX6" wt-b)"
write_envrc "$FX6" "export CARGO_TARGET_DIR=$M6" "export CARGO_INCREMENTAL=1"
# shellcheck disable=SC2016 # the literal $HOME is the point of case 11
write_envrc "$WT6A" 'export CARGO_TARGET_DIR=$HOME/targets/wt-a-target' "export CARGO_INCREMENTAL=0"
write_envrc "$WT6B" "export CARGO_TARGET_DIR=$B6" "export CARGO_INCREMENTAL=0"

# --- section A: unset and canonicalisation ------------------------------------------
echo "A: unset and canonicalisation"
run_case "case 1: unset CARGO_TARGET_DIR is rejected" \
  2 target-unset "$FX1" "__UNSET__"
run_case "case 16: target that cannot be canonicalised is rejected" \
  2 uncanonicalizable-target "$FX1" ""

# --- section B: reserved stores -------------------------------------------------------
echo "B: reserved stores (identity, never prefix)"
run_case "case 2: target inside the seed store is rejected" \
  2 seed-store-target "$FX1" "$SEEDS_FIXTURE/key"
run_case "case 3: target inside the quarantine store is rejected" \
  2 quarantine-store-target "$FX1" "$QUARANTINE_FIXTURE/key"
run_case "case 15: prefix-sharing sibling of the seed store is not the store" \
  2 unregistered-target "$FX1" "${SEEDS_FIXTURE}-webfang"
assert_not_contains "case 15: no seed-store verdict for the sibling" \
  "$LAST_OUT" "reason=seed-store-target"

# --- section C: main bootstrap precondition ------------------------------------------
echo "C: main bootstrap precondition"
run_case "case 5: unbootstrapped main refuses the worktree" \
  2 main-unbootstrapped "$WT5A" "$A5"
assert_contains "case 5: preserved message text" \
  "$LAST_OUT" "main checkout is not bootstrapped"

# --- section D: registry ownership and collisions --------------------------------------
echo "D: registry ownership and collisions"
run_case "case 4: worktree env == main's declared target" \
  2 target-owned-by-other-worktree "$WT3A" "$M3"
assert_contains "case 4: owning tree is named" "$LAST_OUT" "$FX3"

LINK3="$T3/link-to-a"
ln -s "$A3" "$LINK3"
run_case "case 12: symlink resolving to another tree's target" \
  2 target-owned-by-other-worktree "$WT3B" "$LINK3"

run_case "case 6: KEY collision rejects even with --allow-unregistered-target" \
  2 target-owned-by-other-worktree "$WT3B" "$A3" --allow-unregistered-target
assert_not_contains "case 6: opt-out did not bypass the collision" \
  "$LAST_OUT" "reason=unregistered-accepted-by-flag"

run_case "case 13: collision wins over own registration (two trees, one target)" \
  2 target-owned-by-other-worktree "$WT4B" "$S4"
assert_contains "case 13: owner is the OTHER tree" "$LAST_OUT" "$WT4A"

run_case "case 11: literal \$HOME sibling declaration still collides" \
  2 target-owned-by-other-worktree "$WT6B" "$A6"

# --- section E: registration and opt-out -------------------------------------------------
echo "E: registration and opt-out"
run_case "case 7: own registered target is allowed" \
  0 own-registered-target "$WT_A" "$A1"

add_worktree "$FX1" wt-c >/dev/null # third tree, NO .envrc: tolerated non-owner
run_case "case 10: extra .envrc-less worktree tolerated in enumeration" \
  0 own-registered-target "$WT_A" "$A1"

run_case "case 14: main with its own declared target is allowed" \
  0 own-registered-target "$FX1" "$M1"

UNREG="$SANDBOX/unregistered-target"
run_case "case 8: unregistered target rejected without the flag" \
  2 unregistered-target "$WT_A" "$UNREG"
assert_contains "case 8: names the override" \
  "$LAST_OUT" "override=--allow-unregistered-target"

run_case "case 9: unregistered target accepted with the flag" \
  0 unregistered-accepted-by-flag "$WT_A" "$UNREG" --allow-unregistered-target

# --- section F: fail-closed enumeration ----------------------------------------------------
echo "F: fail-closed enumeration"
STUBDIR="$SANDBOX/stubbin"
mkdir -p "$STUBDIR"
REAL_GIT="$(command -v git)"
{
  echo '#!/usr/bin/env bash'
  # shellcheck disable=SC2016 # "${1:-}/${2:-}" belongs to the GENERATED stub
  echo 'if [ "${1:-} ${2:-}" = "worktree list" ]; then exit 1; fi'
  # shellcheck disable=SC2016 # "$@" belongs to the GENERATED stub, not to this shell
  printf 'exec %q "$@"\n' "$REAL_GIT"
} >"$STUBDIR/git"
chmod +x "$STUBDIR/git"
ISO_PATH_PREPEND="$STUBDIR"
run_case "case 17: git enumeration failure is fail-closed" \
  2 worktree-enumeration-failed "$WT_A" "$A1"
ISO_PATH_PREPEND=""

# --- summary ---------------------------------------------------------------------------------
if [ "$FAILED" -ne 0 ]; then
  echo "test_check_target_isolation: FAIL"
  exit 1
fi
echo "test_check_target_isolation: PASS"
exit 0
