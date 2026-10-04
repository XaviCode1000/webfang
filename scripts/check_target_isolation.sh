#!/usr/bin/env bash
# check_target_isolation.sh — build-cache target policy (#1679).
#
# Decides whether the ambient CARGO_TARGET_DIR of the tree at --root may be
# built into. Exit 0 = allow, exit 2 = reject. Invoked by ci_fast_gate.sh
# before any lane runs; also runnable standalone. Read-only git only, no
# destructive commands, no network.
#
# Policy — the order below is LOAD-BEARING (stores before registry, collision
# before own-registration, so no misordered branch can ever allow):
#   1. target unset                            -> reject target-unset
#   2. target cannot be canonicalised          -> reject uncanonicalizable-target
#   3. target inside the seed store            -> reject seed-store-target (no opt-out)
#      target inside the quarantine store      -> reject quarantine-store-target (no opt-out)
#   4. worktree + main checkout not bootstrapped -> reject main-unbootstrapped
#   5. enumerate live worktrees                -> fail-closed on git failure
#   6. read every tree's .envrc declaration    -> reject sibling-declaration-unparseable
#   7. target owned by ANOTHER live worktree   -> reject target-owned-by-other-worktree
#                                                 (--allow-unregistered-target does
#                                                 NOT bypass this: #1267)
#   8. target == own tree's declaration        -> allow (own registration)
#   9. target declared by nobody               -> reject unregistered-target unless
#                                                 --allow-unregistered-target
#
# The ownership registry is the set of live worktrees' .envrc files. Each
# contributes its LAST `export CARGO_TARGET_DIR=` line, with $HOME / ${HOME}
# and a leading ~/ expanded — sibling .envrc files write literal $HOME/...
# while main's uses an absolute path. A tree with no .envrc (tool-generated
# detached worktrees) declares nothing, is nobody's owner, and is skipped.
#
# Every comparison is by canonical path identity (`realpath -m`), never by a
# string prefix: a sibling directory named `seeds-webfang` shares a prefix
# with the seed store and is legal, while a symlink resolving into the store
# is not. `realpath -m` rather than `readlink -f` because it never falls back
# to returning its input on failure — a path that cannot be classified is
# exactly the input this guard must refuse.
#
# The opt-out --allow-unregistered-target means precisely "I accept this
# CARGO_TARGET_DIR is not part of the worktree registry", per invocation. It
# never means "I accept sharing a target with another worktree": the
# collision check (step 7) runs before own-registration (step 8) precisely so
# that two trees registered to the same target are both rejected, whichever
# of them runs the gate.
#
# Usage: scripts/check_target_isolation.sh [--root <path>] [--allow-unregistered-target]
# Env overrides (hermetic test harness only; production defaults are the
# documented store roots): WEBFANG_SEEDS_ROOT, WEBFANG_QUARANTINE_ROOT.

set -uo pipefail

ALLOW_UNREGISTERED_TARGET=false
ROOT_ARG=""

usage() {
  cat <<'EOF'
check_target_isolation.sh — build-cache target policy (#1679)

Usage: scripts/check_target_isolation.sh [--root <path>] [--allow-unregistered-target]

Decides whether the ambient CARGO_TARGET_DIR may be built into from the tree
at --root. Exit 0 = allow, exit 2 = reject (fail-closed; the full decision
tree is documented in the header comment of the script).

Options:
  --root <path>                tree under check (default: git toplevel of $PWD)
  --allow-unregistered-target  accept a target that no live worktree's .envrc
                               declares (per-invocation opt-out, never an env
                               var). It does NOT allow sharing another
                               worktree's target — that collision is always
                               rejected (#1267).
  -h, --help                   this text
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      if [[ $# -lt 2 || -z "${2:-}" ]]; then
        echo "error: --root requires a path argument" >&2
        exit 2
      fi
      ROOT_ARG="$2"
      shift 2
      ;;
    --allow-unregistered-target)
      ALLOW_UNREGISTERED_TARGET=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1' (see --help)" >&2
      exit 2
      ;;
  esac
done

# Temp file holding `git worktree list --porcelain -z` output. The list MUST
# go through a file, not a command substitution: bash strips NUL bytes from
# $(...) output, and the -z format is NUL-delimited by design.
WT_TMP=""

# shellcheck disable=SC2329 # invoked by the EXIT trap, not by name
cleanup() {
  if [[ -n "$WT_TMP" ]]; then
    rm -f "$WT_TMP" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# _canon <path> <reason-token> [context]: canonicalise with `realpath -m`.
# Refuse (print the reason token to stderr, return 2) rather than let an
# unresolvable path be compared as a plain string. The explicit empty check
# is defensive: some realpath builds accept "" where this one does not.
_canon() {
  local p="${1:-}" reason="${2:-uncanonicalizable-target}" ctx="${3:-}" out=""
  if [[ -z "$p" ]]; then
    out=""
  elif ! out="$(realpath -m -- "$p" 2>/dev/null)"; then
    out=""
  fi
  if [[ -z "$out" ]]; then
    {
      echo "error: could not canonicalise a path, so isolation cannot be proven."
      echo "  path: $p"
      if [[ -n "$ctx" ]]; then
        echo "  declared by: $ctx"
      fi
      echo "  refusing rather than compare an unresolved path against the"
      echo "  registry or a reserved store with a string match."
      echo "reason=$reason"
    } >&2
    return 2
  fi
  printf '%s' "$out"
}

# _canon_soft <path>: like _canon but for inputs whose failure means "not a
# match", not "refuse everything" (own tree's declaration, worktree paths).
_canon_soft() {
  local out=""
  if [[ -z "${1:-}" ]]; then
    return 1
  fi
  if ! out="$(realpath -m -- "$1" 2>/dev/null)" || [[ -z "$out" ]]; then
    return 1
  fi
  printf '%s' "$out"
}

# expand_home <value>: expand the .envrc declaration forms actually in use.
# Sibling .envrc files write literal $HOME/... (or ${HOME}/... or ~/...);
# main's writes an absolute path, which passes through untouched.
expand_home() {
  local v="$1"
  v="${v//\$\{HOME\}/$HOME}"
  v="${v//\$HOME/$HOME}"
  # Patterns compare a LITERAL leading tilde (no expansion inside [[ ]]);
  # escaped so shellcheck does not read it as an expansion attempt (SC2088).
  if [[ "$v" == \~/* ]]; then
    v="$HOME/${v:2}"
  elif [[ "$v" == \~ ]]; then
    v="$HOME"
  fi
  printf '%s' "$v"
}

# --- resolve the tree under check ---------------------------------------------
if [[ -n "$ROOT_ARG" ]]; then
  ROOT="$ROOT_ARG"
else
  # Fall back to $PWD rather than failing: every later git command re-fails
  # fail-closed if $PWD is not inside a repository.
  ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || ROOT="$PWD"
fi
ROOT_CANON="$(_canon "$ROOT" "uncanonicalizable-root")" || exit 2
cd "$ROOT_CANON" || exit 2

# --- 1. unset target ------------------------------------------------------------
# Strictly UNSET, not empty: a set-but-empty value is a value that cannot be
# canonicalised, and it is step 2's job to say so precisely.
if [[ -z "${CARGO_TARGET_DIR+x}" ]]; then
  {
    echo "error: CARGO_TARGET_DIR is not set — direnv is not loaded for this tree."
    echo "  fix: run 'direnv allow' once per worktree (see AGENTS.md § worktree bootstrap),"
    echo "  then re-run this gate from a direnv-loaded shell."
    echo "reason=target-unset"
  } >&2
  exit 2
fi

# --- 2. canonical target ----------------------------------------------------------
TGT_CANON="$(_canon "${CARGO_TARGET_DIR%/}" "uncanonicalizable-target")" || exit 2

# --- 3. reserved stores -------------------------------------------------------------
# Seeds are read-only REFERENCES, never build outputs; quarantine holds objects
# moved OUT of the build system because they cannot be trusted. Building into
# either puts exactly that back into a live path. Identity (canonical equality
# or a /* descendant), never a substring test — a legitimate
# ~/.cache/cargo-target/seeds-webfang merely shares a prefix and is allowed.
SEEDS_ROOT="${WEBFANG_SEEDS_ROOT:-$HOME/.cache/cargo-target/seeds}"
SEEDS_CANON="$(_canon "$SEEDS_ROOT" "uncanonicalizable-store-root")" || exit 2
if [[ "$TGT_CANON" == "$SEEDS_CANON" || "$TGT_CANON" == "$SEEDS_CANON"/* ]]; then
  {
    echo "error: CARGO_TARGET_DIR points into the seed store"
    echo "  CARGO_TARGET_DIR $TGT_CANON"
    echo "  seed store       $SEEDS_CANON"
    echo "  a seed is a read-only reference; cargo must never be allowed to"
    echo "  write into one, or this worktree's units become every future"
    echo "  worktree's seed."
    echo "  fix: give this worktree its own target dir, e.g."
    echo "        $HOME/.cache/cargo-target/$(basename "$ROOT_CANON")"
    echo "reason=seed-store-target"
  } >&2
  exit 2
fi

QUARANTINE_ROOT="${WEBFANG_QUARANTINE_ROOT:-$HOME/.cache/cargo-target/quarantine}"
QUARANTINE_CANON="$(_canon "$QUARANTINE_ROOT" "uncanonicalizable-store-root")" || exit 2
if [[ "$TGT_CANON" == "$QUARANTINE_CANON" || "$TGT_CANON" == "$QUARANTINE_CANON"/* ]]; then
  {
    echo "error: CARGO_TARGET_DIR points into the quarantine store"
    echo "  CARGO_TARGET_DIR $TGT_CANON"
    echo "  quarantine       $QUARANTINE_CANON"
    echo "  quarantined objects were moved out of the build system because they"
    echo "  cannot be trusted, and their removal is a separate authorized step"
    echo "  gated on scripts/quarantine_age.sh plus fresh ownership evidence."
    echo "  Building into one puts that state back into an active path."
    echo "  fix: give this worktree its own target dir, e.g."
    echo "        $HOME/.cache/cargo-target/$(basename "$ROOT_CANON")"
    echo "reason=quarantine-store-target"
  } >&2
  exit 2
fi

# --- 4. main bootstrap precondition -------------------------------------------------
# A worktree created without its own .envrc INHERITS CARGO_TARGET_DIR from the
# shell that launched it. Main's own .envrc is the declared policy for the main
# tree on this machine; when it cannot be read, this guard cannot prove that
# the target under check is not main's own (the registry of step 6 reads
# main's declaration like any other tree's, and an unreadable registry entry
# poisons the ownership proof). Refuse rather than assume. Fail-closed is the
# point: "worktree implies isolated target" has to be an invariant, not a
# heuristic that silently degrades depending on whether someone ran the
# bootstrap first.
MAIN_COMMON_GIT="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || {
  echo "error: cannot establish the repository context for $ROOT_CANON." >&2
  echo "reason=worktree-enumeration-failed" >&2
  exit 2
}
MAIN_ROOT_CANON="$(_canon "$(dirname "$MAIN_COMMON_GIT")" "uncanonicalizable-main-root")" || exit 2
if [[ "$ROOT_CANON" != "$MAIN_ROOT_CANON" ]]; then
  MAIN_ENVRC="$MAIN_ROOT_CANON/.envrc"
  MAIN_TARGET=""
  if [[ -f "$MAIN_ENVRC" ]]; then
    MAIN_TARGET="$(sed -n 's/^export CARGO_TARGET_DIR=//p' "$MAIN_ENVRC" 2>/dev/null | tail -1)"
  fi
  if [[ -z "$MAIN_TARGET" ]]; then
    if [[ -f "$MAIN_ENVRC" ]]; then
      WHY="$MAIN_ENVRC exists but declares no CARGO_TARGET_DIR"
    else
      WHY="$MAIN_ENVRC is missing"
    fi
    {
      echo "error: worktree build isolation cannot be verified."
      echo "  main checkout is not bootstrapped: $WHY."
      echo "  without it this gate cannot prove that"
      echo "    $TGT_CANON"
      echo "  is not main's own target dir, so it refuses rather than assume."
      echo "  Cargo itself is fine here; this is a precondition of the workflow."
      echo "  fix: bootstrap the main checkout, then re-run:"
      echo "        cd $MAIN_ROOT_CANON"
      echo "        # create .envrc per AGENTS.md § worktree bootstrap, then:"
      echo "        direnv allow"
      echo "reason=main-unbootstrapped"
    } >&2
    exit 2
  fi
fi

# --- 5. enumerate live worktrees ------------------------------------------------------
# `git worktree list --porcelain -z` is the stable structured interface for
# script enumeration: attributes are NUL-terminated and records are separated
# by an EMPTY token (a \0\0 sequence), so paths containing newlines survive.
# git 2.55.0 verified. Any git failure here is fail-closed: an unreadable
# registry cannot prove isolation.
if ! WT_TMP="$(mktemp)"; then
  echo "error: cannot create a temporary file for worktree enumeration." >&2
  echo "reason=worktree-enumeration-failed" >&2
  exit 2
fi
if ! git worktree list --porcelain -z >"$WT_TMP" 2>/dev/null; then
  echo "error: 'git worktree list --porcelain -z' failed; the live worktree" >&2
  echo "  registry cannot be read, so isolation cannot be proven. Refusing" >&2
  echo "  rather than assume." >&2
  echo "reason=worktree-enumeration-failed" >&2
  exit 2
fi
declare -a WT_TOKENS=()
mapfile -d '' -t WT_TOKENS < "$WT_TMP"
rm -f "$WT_TMP" 2>/dev/null || true
WT_TMP=""
if [[ ${#WT_TOKENS[@]} -eq 0 ]]; then
  echo "error: worktree enumeration returned nothing; a repository always has" >&2
  echo "  at least one worktree, so the registry is unreadable. Refusing." >&2
  echo "reason=worktree-enumeration-failed" >&2
  exit 2
fi

# --- 6. walk the registry --------------------------------------------------------------
# SELF (the tree under check) has its declaration recorded separately: it is
# the claim this tree makes for ITSELF (step 8), never an ownership claim
# against the target. Every OTHER live tree is a potential owner of the target
# (step 7). A missing .envrc, or one with no CARGO_TARGET_DIR line, means "no
# declaration" — the tree is not an owner and is skipped silently (this
# tolerates the tool-generated detached worktrees under .git/ that no
# bootstrap ever touches). A declaration that exists but cannot be
# canonicalised poisons the ownership proof and refuses (same precedent as
# main-unbootstrapped).
SELF_DECL_RAW=""
SELF_HAS_ENVRC=false
OWNER_TREE=""
OWNER_DECL=""
declare -a WT_PATHS=()
for tok in "${WT_TOKENS[@]}"; do
  [[ -n "$tok" ]] || continue # record boundary between porcelain records
  case "$tok" in
    "worktree "*) WT_PATHS+=("${tok#worktree }") ;;
    *) continue ;; # HEAD / branch / detached / bare attributes
  esac
done
if [[ ${#WT_PATHS[@]} -eq 0 ]]; then
  echo "error: worktree enumeration produced no worktree entries; the registry" >&2
  echo "  output is not readable as porcelain. Refusing." >&2
  echo "reason=worktree-enumeration-failed" >&2
  exit 2
fi

for wt in "${WT_PATHS[@]}"; do
  W_CANON="$(_canon_soft "$wt")" || continue # cannot canonicalise -> not an owner
  if [[ "$W_CANON" == "$ROOT_CANON" ]]; then
    SELF_HAS_ENVRC=false
    if [[ -f "$W_CANON/.envrc" ]]; then
      SELF_HAS_ENVRC=true
      SELF_DECL_RAW="$(sed -n 's/^export CARGO_TARGET_DIR=//p' "$W_CANON/.envrc" 2>/dev/null | tail -1)"
    fi
    continue
  fi
  W_ENVRC="$W_CANON/.envrc"
  [[ -f "$W_ENVRC" ]] || continue # no declaration -> not an owner
  W_RAW="$(sed -n 's/^export CARGO_TARGET_DIR=//p' "$W_ENVRC" 2>/dev/null | tail -1)"
  [[ -n "$W_RAW" ]] || continue # declares nothing -> not an owner
  W_DECL="$(_canon "$(expand_home "$W_RAW")" "sibling-declaration-unparseable" "$W_CANON")" || exit 2
  if [[ -z "$OWNER_TREE" && "$W_DECL" == "$TGT_CANON" ]]; then
    OWNER_TREE="$W_CANON"
    OWNER_DECL="$W_DECL"
  fi
done

# --- 7. collision: another live worktree owns this target -------------------------------
# MUST run before own-registration (step 8): if two trees were both
# bootstrapped onto the same target, a naive own-registration check would
# allow whichever of them ran the gate. Collision-first makes both orders
# reject. The opt-out flag deliberately does NOT reach this branch: sharing
# another worktree's target is an isolation violation (#1267), not an
# unregistered one — the first needs authorisation, the second is a bug.
if [[ -n "$OWNER_TREE" ]]; then
  {
    echo "error: this tree's CARGO_TARGET_DIR resolves to another live worktree's target dir" >&2
    echo "  tree:             $ROOT_CANON" >&2
    echo "  CARGO_TARGET_DIR  $TGT_CANON" >&2
    echo "  owned by          $OWNER_TREE" >&2
    echo "  owner's declared  $OWNER_DECL" >&2
    echo "  two trees building the same profile into one directory get the same" >&2
    echo "  output filenames (#1267): the last writer wins, stale links report" >&2
    echo "  as fresh, and an E2E run silently executes the other tree's binary." >&2
    echo "  --allow-unregistered-target does not excuse this." >&2
    echo "  fix: give this worktree its own target dir, then re-run — write" >&2
    echo "  .envrc per the worktree bootstrap documented in AGENTS.md" >&2
    echo "  (§ Git Worktree Isolation → Worktree lifecycle) for '$ROOT_CANON'," >&2
    echo "  then direnv allow." >&2
    echo "reason=target-owned-by-other-worktree" >&2
  } >&2
  exit 2
fi

# --- 8. own registration -------------------------------------------------------------------
SELF_DECL_CANON=""
if [[ -n "$SELF_DECL_RAW" ]]; then
  SELF_DECL_CANON="$(_canon_soft "$(expand_home "$SELF_DECL_RAW")")" || true
fi
if [[ -n "$SELF_DECL_CANON" && "$SELF_DECL_CANON" == "$TGT_CANON" ]]; then
  {
    echo "target: registered (own)"
    echo "  CARGO_TARGET_DIR $TGT_CANON"
    echo "  declared by      $ROOT_CANON/.envrc"
    echo "reason=own-registered-target"
  }
  exit 0
fi
if [[ "$SELF_HAS_ENVRC" == "true" && -n "$SELF_DECL_RAW" ]]; then
  {
    echo "note: this tree's .envrc declares a different CARGO_TARGET_DIR than the" >&2
    echo "  environment provides (stale direnv?). The environment is what cargo" >&2
    echo "  actually uses, so the check continues on the environment's value." >&2
  } >&2
fi

# --- 9. unregistered ---------------------------------------------------------------------------
if [[ "$ALLOW_UNREGISTERED_TARGET" == "true" ]]; then
  {
    echo "target: unregistered (accepted via --allow-unregistered-target)"
    echo "  CARGO_TARGET_DIR $TGT_CANON"
    echo "  no live worktree's .envrc declares this target; the per-invocation"
    echo "  opt-out was passed explicitly."
    echo "reason=unregistered-accepted-by-flag"
  }
  exit 0
fi
{
  echo "error: CARGO_TARGET_DIR is not registered by any live worktree's .envrc" >&2
  echo "  tree:            $ROOT_CANON" >&2
  echo "  CARGO_TARGET_DIR $TGT_CANON" >&2
  echo "  every tree must build into a target its own .envrc declares, so the" >&2
  echo "  registry can prove isolation. Fix by running the worktree bootstrap" >&2
  echo "  documented in AGENTS.md (§ Git Worktree Isolation → Worktree lifecycle)" >&2
  echo "  for '$ROOT_CANON':" >&2
  echo "    - write .envrc FROM THIS WORKTREE'S OWN NAME, containing" >&2
  echo "        export CARGO_TARGET_DIR=\$HOME/.cache/cargo-target/$(basename "$ROOT_CANON")" >&2
  echo "        export CARGO_INCREMENTAL=0" >&2
  echo "    - direnv allow" >&2
  echo "  (or, for this invocation only, re-run with --allow-unregistered-target" >&2
  echo "   to accept an unregistered target explicitly)" >&2
  echo "reason=unregistered-target" >&2
  echo "override=--allow-unregistered-target" >&2
} >&2
exit 2
