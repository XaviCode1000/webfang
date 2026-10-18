#!/usr/bin/env bash
set -uo pipefail
# test_seed_target_refusal.sh — the consumer must never build into the seed store.
#
# scripts/check_target_isolation.sh (the fast gate) already has its own harness;
# this one pins scripts/seed_target.sh's own D6 refusal, because a direct
# invocation must not be able to bypass the gate.
#
# Policy (A6, #2058): TARGET_DIR equal to SEEDS_ROOT is refused with exit 2,
# exactly like every descendant of the store. The predicate in
# check_target_isolation.sh is the reference shape (== || == /*). The refusal
# must fire BEFORE any key computation or store read, and the fix collapses the
# duplicated guard block into one — pinned structurally by the last case.
#
# Sandbox: an empty store is enough. The refusal is identity-based and runs
# before anything can seed or read it, so no compatible seed is ever needed and
# nothing in ~/.cache/cargo-target is touched.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONSUMER="$SCRIPT_DIR/seed_target.sh"

SBX="$(mktemp -d)"
trap 'chmod -R u+w "$SBX" 2>/dev/null; rm -rf "$SBX"' EXIT
STORES="$SBX/seeds"
mkdir -p "$STORES"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }

expect_refuse() {
  local label="$1" target="$2" out rc
  out="$(bash "$CONSUMER" --seeds-root "$STORES" --target-dir "$target" 2>&1)"; rc=$?
  if [ "$rc" -ne 2 ]; then
    bad "$label — expected exit 2, got $rc ($(printf '%s' "$out" | head -2 | tr '\n' ' '))"; return
  fi
  case "$out" in
    *"refusing to use a seed directory as a build target"*)
      ok "$label → exit 2, it says why" ;;
    *)
      bad "$label — exit 2 but no refusal message" ;;
  esac
}

expect_allow() {
  local label="$1" target="$2" out rc
  out="$(bash "$CONSUMER" --seeds-root "$STORES" --target-dir "$target" 2>&1)"; rc=$?
  if [ "$rc" -ne 0 ]; then
    bad "$label — expected a cold allow (exit 0), got $rc: $(printf '%s' "$out" | head -2 | tr '\n' ' ')"; return
  fi
  case "$out" in
    *"refusing to use a seed directory"*)
      bad "$label — wrongly refused a legitimate target"; return ;;
    *)
      ok "$label → allowed, cold path taken" ;;
  esac
}

# NEGATION: 1. the store root itself — the equality arm the predicate lost (A6).
expect_refuse "1. target = the store root (equality arm)" "$STORES"
# Controls: descendants were already refused before the fix — they must stay green.
expect_refuse "2. target = a child of the store"          "$STORES/child"
expect_refuse "3. target = not-yet-existing under store"  "$STORES/child/deep/future"
expect_allow "4. target = tree-named dir elsewhere"       "$SBX/elsewhere/webfang"

# NEGATION: 5. the duplication collapsed — the guard block appears exactly once.
# One site keeps the earliest position; two copies drift (this is measured A6).
n_blocks="$(grep -c 'refusing to use a seed directory as a build target' "$CONSUMER")"
if [ "$n_blocks" -eq 1 ]; then
  ok "5. refusal block appears exactly once (duplication collapsed)"
else
  bad "5. refusal block appears $n_blocks times (want 1)"
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
