#!/usr/bin/env bash
#
# test_target_isolation_envrc.sh — .envrc declaration parse shapes for
# scripts/check_target_isolation.sh (S10 / B32, issue #2059).
#
# The ownership registry of the guard is "each live worktree's LAST
# `export CARGO_TARGET_DIR=` line in .envrc". Before the S10 fix that line was
# parsed with the rigid `sed -n 's/^export CARGO_TARGET_DIR=//p'`:
# matching quotes survived, flexible whitespace did not, `${VAR:-default}`
# stayed literal, and a sibling's RELATIVE declaration was resolved against
# the cwd of the guard instead of the declaring tree's root. The concrete
# failure (B32): `export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/shared"`
# in a sibling was never recognized as the owner of that same target, so two
# worktrees could register the SAME target dir and the guard let whichever
# ran exit 0 — the exact #1267 collision the guard exists to prevent.
#
# Hermetic and cargo-free, following the conventions of
# scripts/tests/test_check_target_isolation.sh: throwaway `git init` fixtures,
# one worktree sibling each, .envrc written by the fixture, redirected HOME,
# store roots pointed into the sandbox, neutral cwd with --root, and
# per-invocation CARGO_TARGET_DIR so nothing ambient can leak in. No case
# reads or writes ~/.cache/cargo-target: every directory involved lives under
# the harness's own sandbox (created under /tmp/opencode when available).
#
# Case matrix:
#   A. the shapes S10 names — each misparsed pre-fix (RED rc=0 allow in 1–3,
#      wrong-reason unregistered-target in 4, rc=2 unregistered in 5):
#      1  sibling declares the quoted form  "...\"$HOME/...\""  -> collision FIRES
#      2  sibling declares ${HOME:-default}                    -> collision FIRES
#      3  sibling declares with indented / extra whitespace    -> collision FIRES
#      4  sibling declares a RELATIVE dir -> owned by THAT tree's root
#      5  the checked tree ITSELF declares the quoted form -> own registration
#   B. PRESERVE — behaviour that must NOT change with the liberalized parse:
#      6  own relative declaration against own root  -> allowed (pre-existing)
#      7  sibling quoted declaration of a DIFFERENT target -> NOT an owner
#      8  sibling "declares nothing" (bare empty value)     -> skipped
#      9  sibling without .envrc                            -> skipped
#     10  liberal-shape sibling declarations cannot rescue a seed-store target
#     11  ... nor a quarantine target
#     12  pre-existing ${HOME}/... sibling form            -> still collides
#     13  pre-existing ~/... sibling form                  -> still collides
#
# Every case carries a `# NEGATION:` line where the pre-fix verdict would have
# passed or misidentified; markers follow the convention used by
# scripts/test_seed_target_refusal.sh.
#
# Exit code: 0 when every case holds, 1 otherwise.
#
# Usage: bash scripts/test_target_isolation_envrc.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISOLATION_SCRIPT="$ROOT/scripts/check_target_isolation.sh"

FAILED=0
LAST_OUT=""

if [ ! -r "$ISOLATION_SCRIPT" ]; then
  echo "guard script not found: $ISOLATION_SCRIPT" >&2
  exit 1
fi

# --- sandbox -----------------------------------------------------------------
# Prefer the delegated scratch root /tmp/opencode; fall back to the ambient
# TMPDIR if it does not exist. Either way nothing lands in the repo or in
# ~/.cache/cargo-target.
SANDBOX="$(mktemp -d /tmp/opencode/t5-iso-envrc.XXXXXXXX 2>/dev/null || mktemp -d)"
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
  chmod -R u+w "$SANDBOX" 2>/dev/null || true
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null

# --- fixture builders ----------------------------------------------------------
# new_pair_fixture: one throwaway repo (git init + one commit) plus ONE
# worktree sibling wt-b, printed as "<main> <wt-b>".
new_pair_fixture() {
  local fx
  fx="$(mktemp -d "$SANDBOX/fx.XXXXXX")" || exit 1
  FIXTURES+=("$fx")
  git init -q -b main "$fx" || exit 1
  git -C "$fx" config user.email harness@example.invalid
  git -C "$fx" config user.name "Target Isolation EnvRc Harness"
  echo "fixture" >"$fx/README.md"
  git -C "$fx" add README.md
  git -C "$fx" commit -qm init || exit 1
  git -C "$fx" worktree add -q -b branch-wt-b "$fx/wt-b" >/dev/null 2>&1 || {
    echo "harness error: git worktree add failed for $fx/wt-b" >&2
    exit 1
  }
  printf '%s\n%s\n' "$fx" "$fx/wt-b"
}

# write_envrc <tree> <line>...: write the .envrc exactly as given.
write_envrc() {
  local tree="$1"
  shift
  printf '%s\n' "$@" >"$tree/.envrc"
}

# --- case runner -----------------------------------------------------------------
# invoke_iso <root> <target> [guard args...]: the guard is invoked from the
# neutral sandbox root with --root, never relying on the caller's CWD.
invoke_iso() {
  local root="$1" target="$2"
  shift 2
  (
    export HOME="$SANDBOX/home"
    export WEBFANG_SEEDS_ROOT="$SEEDS_FIXTURE"
    export WEBFANG_QUARANTINE_ROOT="$QUARANTINE_FIXTURE"
    export PATH="${ISO_PATH_PREPEND:+$ISO_PATH_PREPEND:}$PATH"
    export CARGO_TARGET_DIR="$target"
    cd "$SANDBOX" || exit 97
    bash "$ISOLATION_SCRIPT" --root "$root" "$@" 2>&1
  )
}

# run_case <name> <want-rc> <want-reason> <root> <target> [args...]
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
mkdir -p "$SEEDS_FIXTURE" "$QUARANTINE_FIXTURE" "$SANDBOX/home/.cache/cargo-target"

# Common target dirs. The shared ones sit under the REDIRECTED HOME, in the
# "$HOME/.cache/cargo-target/..." shape S10's example names.
SHARED="$SANDBOX/home/.cache/cargo-target/shared"

# FX1: main + wt-b. wt-b's declaration is the exact S10 example: quoted,
# literal $HOME. Main's is the same path, unquoted — the two shapes must
# resolve to the SAME target.
FX1="$(new_pair_fixture)"
FX1_MAIN="${FX1%%$'\n'*}"
FX1_B="${FX1##*$'\n'}"
write_envrc "$FX1_MAIN" "export CARGO_TARGET_DIR=$SHARED" "export CARGO_INCREMENTAL=0"
# shellcheck disable=SC2016 # the literal $HOME is the point of case 1
write_envrc "$FX1_B" 'export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/shared"' "export CARGO_INCREMENTAL=0"

# FX2: wt-b declares with a ${VAR:-default} indirection; main unquoted, same target.
FX2="$(new_pair_fixture)"
FX2_MAIN="${FX2%%$'\n'*}"
FX2_B="${FX2##*$'\n'}"
write_envrc "$FX2_MAIN" "export CARGO_TARGET_DIR=$SHARED" "export CARGO_INCREMENTAL=0"
# shellcheck disable=SC2016 # the literal ${HOME:-...} is the point of case 2
write_envrc "$FX2_B" 'export CARGO_TARGET_DIR="${HOME:-/fallback}/.cache/cargo-target/shared"' "export CARGO_INCREMENTAL=0"

# FX3: wt-b's declaration line written with a leading indent and extra
# whitespace between `export` and the variable — shapes bash/accept in .envrc.
FX3="$(new_pair_fixture)"
FX3_MAIN="${FX3%%$'\n'*}"
FX3_B="${FX3##*$'\n'}"
write_envrc "$FX3_MAIN" "export CARGO_TARGET_DIR=$SHARED" "export CARGO_INCREMENTAL=0"
printf '  export \t CARGO_TARGET_DIR=%s\n' "$SHARED" >"$FX3_B/.envrc"

# FX4: wt-b declares a RELATIVE dir. Relative to wt-b's own root (S10), i.e.
# "$FX4/wt-b/shared-targets" — NOT relative to whatever cwd the guard runs in.
FX4="$(new_pair_fixture)"
FX4_MAIN="${FX4%%$'\n'*}"
FX4_B="${FX4##*$'\n'}"
write_envrc "$FX4_MAIN" "export CARGO_TARGET_DIR=$SANDBOX/fx4-main-target" "export CARGO_INCREMENTAL=0"
write_envrc "$FX4_B" "export CARGO_TARGET_DIR=shared-targets" "export CARGO_INCREMENTAL=0"

# FX5/6: a tree whose OWN .envrc (the tree under check) uses the quoted / the
# relative form; the environment (direnv) resolved what that .envrc literally
# means. No sibling involved — this exercises the SELF branch of the parse.
FX5="$(mktemp -d "$SANDBOX/fx.XXXXXX")" || exit 1
FIXTURES+=("$FX5")
git init -q -b main "$FX5" || exit 1
git -C "$FX5" config user.email harness@example.invalid
git -C "$FX5" config user.name "Target Isolation EnvRc Harness"
echo "fixture" >"$FX5/README.md"
git -C "$FX5" add README.md
git -C "$FX5" commit -qm init || exit 1
# shellcheck disable=SC2016 # the literal quoted $HOME is the point of case 5
write_envrc "$FX5" 'export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/own-target"'
FX5_TARGET="$SANDBOX/home/.cache/cargo-target/own-target"

FX6="$(mktemp -d "$SANDBOX/fx.XXXXXX")" || exit 1
FIXTURES+=("$FX6")
git init -q -b main "$FX6" || exit 1
git -C "$FX6" config user.email harness@example.invalid
git -C "$FX6" config user.name "Target Isolation EnvRc Harness"
echo "fixture" >"$FX6/README.md"
git -C "$FX6" add README.md
git -C "$FX6" commit -qm init || exit 1
FX6_TARGET="$FX6/own-target"
write_envrc "$FX6" "export CARGO_TARGET_DIR=own-target"
mkdir -p "$FX6_TARGET"

# FX7..13: preserved verdicts.
# FX7: wt-b quoted but a DIFFERENT target than the one under check.
FX7="$(new_pair_fixture)"
FX7_MAIN="${FX7%%$'\n'*}"
FX7_B="${FX7##*$'\n'}"
write_envrc "$FX7_MAIN" "export CARGO_TARGET_DIR=$SANDBOX/fx7-main-target" "export CARGO_INCREMENTAL=0"
# shellcheck disable=SC2016 # the literal quoted $HOME is the point of case 7
write_envrc "$FX7_B" 'export CARGO_TARGET_DIR="$HOME/.cache/cargo-target/b-target"' "export CARGO_INCREMENTAL=0"
mkdir -p "$SANDBOX/home/.cache/cargo-target/b-target"

# FX8: wt-b "declares nothing" — a bare empty value.
FX8="$(new_pair_fixture)"
FX8_MAIN="${FX8%%$'\n'*}"
FX8_B="${FX8##*$'\n'}"
write_envrc "$FX8_MAIN" "export CARGO_TARGET_DIR=$SANDBOX/fx8-main-target" "export CARGO_INCREMENTAL=0"
write_envrc "$FX8_B" "export CARGO_TARGET_DIR=" "export CARGO_INCREMENTAL=0"

# FX9: wt-b with NO .envrc at all.
FX9="$(new_pair_fixture)"
FX9_MAIN="${FX9%%$'\n'*}"
# FX12/13: the PRE-EXISTING expansion forms (literal ${HOME} and a leading
# ~/) in a sibling must still collide exactly as before.
FX12="$(new_pair_fixture)"
FX12_MAIN="${FX12%%$'\n'*}"
FX12_B="${FX12##*$'\n'}"
write_envrc "$FX12_MAIN" "export CARGO_TARGET_DIR=$SHARED" "export CARGO_INCREMENTAL=0"
# shellcheck disable=SC2016 # the literal ${HOME} is the point of case 12
write_envrc "$FX12_B" 'export CARGO_TARGET_DIR=${HOME}/.cache/cargo-target/shared' "export CARGO_INCREMENTAL=0"

FX13="$(new_pair_fixture)"
FX13_MAIN="${FX13%%$'\n'*}"
FX13_B="${FX13##*$'\n'}"
write_envrc "$FX13_MAIN" "export CARGO_TARGET_DIR=$SHARED" "export CARGO_INCREMENTAL=0"
# shellcheck disable=SC2016 # the literal ~ is the point of case 13
write_envrc "$FX13_B" 'export CARGO_TARGET_DIR=~/.cache/cargo-target/shared' "export CARGO_INCREMENTAL=0"

# --- section A: the S10 parse shapes ------------------------------------------------
echo "A: the .envrc declaration shapes S10 names"

# NEGATION: pre-fix this ran own-registered-target, exit 0 — wt-b's quoted
# declaration was never resolved to a path, so the #1267 collision the
# guard exists to prevent was invisible.
run_case "case 1: quoted sibling decl collides" \
  2 target-owned-by-other-worktree "$FX1_MAIN" "$SHARED"
assert_contains "case 1: owning tree is named" "$LAST_OUT" "$FX1_B"

# NEGATION: pre-fix this ran own-registered-target, exit 0 — ${VAR:-default}
# stayed literal text and realpath -m resolved it as a relative path.
run_case "case 2: \${VAR:-default} sibling decl collides" \
  2 target-owned-by-other-worktree "$FX2_MAIN" "$SHARED"
assert_contains "case 2: owning tree is named" "$LAST_OUT" "$FX2_B"

# NEGATION: pre-fix the rigid '^export CARGO_TARGET_DIR=' pattern missed the
# indented line entirely and wt-b declared nothing.
run_case "case 3: flexibly-whitespaced sibling decl collides" \
  2 target-owned-by-other-worktree "$FX3_MAIN" "$SHARED"
assert_contains "case 3: owning tree is named" "$LAST_OUT" "$FX3_B"

# NEGATION: pre-fix the relative value resolved against the GUARD's cwd, so
# the verdict was unregistered-target with the owner invisible.
run_case "case 4: sibling RELATIVE declaration resolves against ITS OWN root" \
  2 target-owned-by-other-worktree "$FX4_MAIN" "$FX4_B/shared-targets"
assert_contains "case 4: owner is wt-b" "$LAST_OUT" "$FX4_B"
assert_contains "case 4: owner's declaration shown rooted at wt-b" \
  "$LAST_OUT" "$FX4_B/shared-targets"

# NEGATION: pre-fix the SELF branch kept the quotes and could never match,
# so the tree under check was told to re-bootstrap it already had.
run_case "case 5: quoted SELF declaration registers the tree" \
  0 own-registered-target "$FX5" "$FX5_TARGET"

# --- section B: preserved verdicts -----------------------------------------------------
echo "B: preserved refusals and ownership rules"

run_case "case 6: own RELATIVE declaration resolves against its own root" \
  0 own-registered-target "$FX6" "$FX6_TARGET"

run_case "case 7: quoted sibling of a DIFFERENT target is not an owner" \
  2 unregistered-target "$FX7_MAIN" "$SHARED"
assert_not_contains "case 7: no phantom ownership" \
  "$LAST_OUT" "reason=target-owned-by-other-worktree"

run_case "case 8: sibling with a bare empty declaration is skipped" \
  2 unregistered-target "$FX8_MAIN" "$SHARED"
assert_not_contains "case 8: empty decl is not ownership" \
  "$LAST_OUT" "reason=target-owned-by-other-worktree"

run_case "case 9: sibling without .envrc is skipped" \
  2 unregistered-target "$FX9_MAIN" "$SHARED"
assert_not_contains "case 9: envrc-less sibling is not ownership" \
  "$LAST_OUT" "reason=target-owned-by-other-worktree"

run_case "case 10: liberal sibling shapes cannot rescue a seed-store target" \
  2 seed-store-target "$FX1_MAIN" "$SEEDS_FIXTURE/key"

run_case "case 11: nor a quarantine target" \
  2 quarantine-store-target "$FX1_MAIN" "$QUARANTINE_FIXTURE/key"

run_case "case 12: pre-existing literal \${HOME} sibling form still collides" \
  2 target-owned-by-other-worktree "$FX12_MAIN" "$SHARED"
assert_contains "case 12: owning tree is named" "$LAST_OUT" "$FX12_B"

run_case "case 13: pre-existing leading ~ sibling form still collides" \
  2 target-owned-by-other-worktree "$FX13_MAIN" "$SHARED"
assert_contains "case 13: owning tree is named" "$LAST_OUT" "$FX13_B"

# --- summary ---------------------------------------------------------------------------------
if [ "$FAILED" -ne 0 ]; then
  echo "test_target_isolation_envrc: FAIL"
  exit 1
fi
echo "test_target_isolation_envrc: PASS"
exit 0
