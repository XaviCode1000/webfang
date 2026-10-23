#!/usr/bin/env bash
# Meta-guard: a wired guard without a negation harness is not a guard.
#
# Issue #2056. ODD `odd/tasks/scripts-fail-open-remediation.md`, spec S1.
#
# WHY THIS EXISTS. The audit AUDIT-SCRIPTS-FAIL-OPEN-20261018-222141 found 12
# CRITICAL defects, and every one of them lived in a branch no test exercised:
# a `|| true` over a producer whose output is parsed later, a regex
# over-escaped past recognition, a jq predicate that emitted zero outputs, a
# `comm` fed version-ordered input. The guards were present, ran in CI, and
# could not fail. Green CI proved nothing about any of them.
#
# Three shapes are enforced here, and each one shipped:
#
#   1. NO HARNESS. Every `scripts/check_*.sh` reachable from a workflow,
#      `justfile` or `mise.toml` must have a semantics harness. "Reachable"
#      includes one level of indirection: `scripts/ci_fast_gate.sh` is itself
#      a workflow step, so the guards it invokes are wired even when no
#      workflow names them directly.
#   2. HARNESS NOT WIRED. A harness that no CI job runs is the #1707 shape:
#      the assertions exist, the gate never executes them, and the fix is
#      revertible in silence. `scripts/tests/test_check_target_isolation.sh`
#      is invoked only from `scripts/ci_fast_gate.sh`, which no workflow step
#      runs, so its semantics are unenforced in CI.
#   3. HARNESS THAT CANNOT FAIL. The harness must go red when its guard is
#      deliberately broken. This is the check that closes the root cause: a
#      harness whose every case still passes against a guard replaced by
#      `exit 0` is decoration, not coverage. The mutation is applied in place
#      and restored from a byte-identical backup verified by checksum, so a
#      crash cannot leave the tree mutated.
#
# The baseline ratchets. Every entry names why it is still open, and an entry
# that stops reproducing is itself a failure: a baseline that never shrinks is
# the same fail-open shape one level up.
#
# Usage:
#   bash scripts/test_guard_coverage.sh               # full check (CI)
#   bash scripts/test_guard_coverage.sh --no-mutation # skip the mutation run
#   bash scripts/test_guard_coverage.sh --verbose     # list what was checked
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE="$ROOT/scripts/guard_coverage_baseline.txt"
MUTATION_TIMEOUT="${MUTATION_TIMEOUT:-120}"

RUN_MUTATION=1
VERBOSE=0
for arg in "$@"; do
  case "$arg" in
    --no-mutation) RUN_MUTATION=0 ;;
    --verbose) VERBOSE=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

VIOLATIONS=()
declare -A BASELINE_HIT=()
CHECKS_RUN=0

violation() { VIOLATIONS+=("$1"); }
note() { [ "$VERBOSE" -eq 1 ] && echo "  ok   $1"; return 0; }

# --- wiring enumeration ------------------------------------------------------
#
# A guard counts as wired when a workflow, `justfile` or `mise.toml` names it,
# or names a script that names it (one level deep, which covers the fast gate).

entry_sources() {
  local f
  for f in "$ROOT/.github/workflows"/*.yml "$ROOT/justfile" "$ROOT/mise.toml"; do
    [ -f "$f" ] && printf '%s\n' "$f"
  done
}

# Full-line comments are stripped before extracting names: a guard mentioned
# only in prose is not wired, and every one of these files explains itself at
# length. Inline trailing comments survive, which is deliberate — stripping
# them would need a parser, and a false "wired" claim is noise, not a hole.
guard_refs_in() { # file
  grep -v '^[[:space:]]*#' "$1" 2>/dev/null | grep -ohE 'check_[a-z0-9_]+\.sh' || true
}

script_refs_in() { # file
  grep -v '^[[:space:]]*#' "$1" 2>/dev/null | grep -ohE 'scripts/[a-z0-9_/]+\.sh' || true
}

wired_guards() {
  local src s
  # Direct references.
  entry_sources | while IFS= read -r src; do
    guard_refs_in "$src"
  done
  # One level of indirection through scripts the entry points run.
  entry_sources | while IFS= read -r src; do
    script_refs_in "$src"
  done | sort -u | while IFS= read -r s; do
    [ -f "$ROOT/$s" ] || continue
    guard_refs_in "$ROOT/$s"
  done
}

# --- baseline ----------------------------------------------------------------
#
# Only the categories below may be baselined. `no-mutation` and
# `broken-harness` are deliberately absent: those are the checks that make this
# guard worth running, and an escape hatch on them turns the whole file into
# decoration. A baseline line naming a forbidden category is a violation, not a
# silent exemption — otherwise a PR could add `no-mutation scripts/test_x.sh`
# and buy permanent immunity from the only check that matters.
BASELINABLE_CATEGORIES=" no-harness unwired-guard unwired-harness no-marker "

baselined() { # category subject
  [ -f "$BASELINE" ] || return 1
  awk -v c="$1" -v s="$2" '
    /^[[:space:]]*#/ { next }
    NF < 2 { next }
    $1 == c && $2 == s { found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$BASELINE"
}

# Records a baseline hit, or fails when the entry is absent or its category is
# not baselinable at all.
expect_baselined() { # category subject message
  case "$BASELINABLE_CATEGORIES" in
    *" $1 "*) ;;
    *)
      violation "$3"
      return 1
      ;;
  esac
  if baselined "$1" "$2"; then
    BASELINE_HIT["$1 $2"]=1
    note "$2 — baselined ($1)"
    return 0
  fi
  violation "$3"
  return 1
}

# --- harness discovery -------------------------------------------------------
#
# Two conventions are in use and both are legitimate:
#   scripts/test_<name>.sh              (most harnesses)
#   scripts/tests/test_check_<name>.sh  (the two that live in scripts/tests/)

find_harness() { # guard basename
  local name="${1#check_}"
  name="${name%.sh}"
  local c
  for c in "scripts/test_${name}.sh" "scripts/tests/test_check_${name}.sh"; do
    if [ -f "$ROOT/$c" ]; then
      printf '%s\n' "$c"
      return 0
    fi
  done
  return 1
}

harness_wired() { # repo-relative harness path
  local src
  while IFS= read -r src; do
    # A harness named only in a COMMENT is not wired: a step is a command
    # line, and the same convention the guard and script references already
    # follow applies here (full-line comments stripped, inline trailing
    # comments survive — stripping those would need a parser, and a false
    # "wired" claim is noise, not a hole). The reference is read through a
    # process substitution so an early-exit SIGPIPE under `set -o pipefail`
    # cannot turn a found reference into a missed one.
    grep -qF -- "$1" <(grep -v '^[[:space:]]*#' "$src" 2>/dev/null) && return 0
  done < <(entry_sources)
  return 1
}

# --- mutation ----------------------------------------------------------------
#
# The protocol is green-then-red, and the order matters: a harness that is
# broken for any unrelated reason (missing env, missing fixture, a syntax
# error) also exits non-zero against a broken guard. Without the intact run
# first, "it went red" proves nothing — it may have been red all along. So a
# harness must pass with the guard intact AND fail with the guard stubbed.

harness_passes_intact() { # harness-rel
  ( cd "$ROOT" && timeout "$MUTATION_TIMEOUT" bash "$1" ) >/dev/null 2>&1
}

# Replace the guard with an always-green stub, run its harness, restore. A
# harness that still exits 0 never proved the guard can fail.

# Restore state shared with the signal trap. A cancelled CI job or a Ctrl-C
# must not leave a guard replaced by `exit 0`, and it must not delete the only
# copy of the original bytes before the restore has been verified — so the
# backup is removed only after the checksum matches.
MUTATION_GUARD=""
MUTATION_BACKUP=""
MUTATION_SUM=""

restore_mutation() {
  [ -n "$MUTATION_GUARD" ] || return 0
  local ok=0

  cp "$MUTATION_BACKUP" "$MUTATION_GUARD" || ok=1
  if [ "$ok" -eq 0 ]; then
    [ "$(sha256sum "$MUTATION_GUARD" | cut -d' ' -f1)" = "$MUTATION_SUM" ] || ok=1
  fi

  if [ "$ok" -ne 0 ]; then
    # Keep the backup: a human can still recover the file by hand.
    echo "FATAL: could not restore $MUTATION_GUARD; original bytes kept at $MUTATION_BACKUP" >&2
    MUTATION_GUARD=""
    exit 3
  fi

  rm -f "$MUTATION_BACKUP"
  MUTATION_GUARD=""
  MUTATION_BACKUP=""
  MUTATION_SUM=""
}

mutation_caught() { # guard-rel harness-rel
  local guard="$ROOT/$1" harness="$2"
  local backup sum_before rc=0

  backup="$(mktemp)" || { violation "mutation: cannot create a backup file for $1"; return 1; }
  cp "$guard" "$backup" || {
    rm -f "$backup"
    violation "mutation: cannot back up $1"
    return 1
  }
  sum_before="$(sha256sum "$guard" | cut -d' ' -f1)"

  MUTATION_GUARD="$guard"
  MUTATION_BACKUP="$backup"
  MUTATION_SUM="$sum_before"
  # EXIT covers every normal path; INT and TERM restore and then TERMINATE.
  # A handler that only restores would leave a cancelled run executing the
  # remaining guards, so cancellation would stop cancelling anything.
  trap 'restore_mutation' EXIT
  trap 'restore_mutation; exit 130' INT
  trap 'restore_mutation; exit 143' TERM

  printf '#!/usr/bin/env bash\nexit 0\n' > "$guard"

  ( cd "$ROOT" && timeout "$MUTATION_TIMEOUT" bash "$harness" ) >/dev/null 2>&1 || rc=$?

  restore_mutation

  case "$rc" in
    0) return 1 ;;   # harness passed against a broken guard: no coverage
    124) return 1 ;; # hung on a broken guard: not a clean red either
    *) return 0 ;;
  esac
}

# --- main --------------------------------------------------------------------

# Only names that resolve to a real file count. Fixture literals inside a
# harness (`scripts/check_foo.sh`) are extracted by the same pattern and must
# not inflate the count.
mapfile -t GUARDS < <(
  wired_guards | sort -u | while IFS= read -r g; do
    [ -f "$ROOT/scripts/$g" ] && printf '%s\n' "$g"
  done
)

if [ "${#GUARDS[@]}" -eq 0 ]; then
  echo "FAIL: no wired check_*.sh found — the enumeration is broken, not the repo" >&2
  exit 1
fi

echo "guard coverage: ${#GUARDS[@]} wired guard(s)"

# Guards nothing reaches are outside S1's enforcement scope, but they must not
# vanish silently: a guard no CI job runs is the same #1707 shape one level up.
# `scripts/ci_fast_gate.sh` is a LOCAL tool — the only mention of it under
# .github/workflows is a comment — so the guards it alone invokes are unwired.
for f in "$ROOT"/scripts/check_*.sh; do
  [ -f "$f" ] || continue
  b="$(basename "$f")"
  case " ${GUARDS[*]} " in
    *" $b "*) continue ;;
  esac
  expect_baselined "unwired-guard" "$b" \
    "unwired-guard: $b is reachable from no workflow, justfile or mise.toml task" || true
done

for guard in "${GUARDS[@]}"; do
  [ -f "$ROOT/scripts/$guard" ] || continue
  CHECKS_RUN=$((CHECKS_RUN + 1))

  if ! harness="$(find_harness "$guard")"; then
    expect_baselined "no-harness" "$guard" \
      "no-harness: $guard is wired but has no scripts/test_*.sh or scripts/tests/test_check_*.sh" || true
    continue
  fi

  if ! harness_wired "$harness"; then
    expect_baselined "unwired-harness" "$harness" \
      "unwired-harness: $harness exists but no workflow step runs it (#1707 shape)" || true
  else
    # Deliberately records NOTHING. The ratchet fires on a baseline entry that
    # was never consumed as an exemption, so a fixed condition leaves its key
    # unset and the stale entry is reported. Writing a sentinel here would make
    # the ratchet blind for this category — the entry would outlive the defect.
    note "$harness is wired"
  fi

  if ! grep -qE '^[[:space:]]*# NEGATION:' "$ROOT/$harness"; then
    expect_baselined "no-marker" "$harness" \
      "no-marker: $harness carries no '# NEGATION:' case marker" || true
  else
    note "$harness carries a negation marker"
  fi

  if [ "$RUN_MUTATION" -eq 1 ]; then
    if ! harness_passes_intact "$harness"; then
      expect_baselined "broken-harness" "$harness" \
        "broken-harness: $harness fails on an intact tree, so its red proves nothing" || true
    elif mutation_caught "scripts/$guard" "$harness"; then
      note "$harness is green intact and red on a broken $guard"
    else
      expect_baselined "no-mutation" "$harness" \
        "no-mutation: $harness still passes when $guard is replaced by 'exit 0'" || true
    fi
  fi
done

# --- ratchet -----------------------------------------------------------------

if [ -f "$BASELINE" ]; then
  while read -r cat subject _reason; do
    case "$cat" in ''|'#'*) continue ;; esac
    [ -n "${subject:-}" ] || continue
    # A forbidden category is rejected even when the entry is otherwise
    # well-formed: this is the line a PR would add to buy immunity.
    case "$BASELINABLE_CATEGORIES" in
      *" $cat "*) ;;
      *)
        violation "forbidden-baseline: '$cat $subject' — that category can never be baselined"
        continue
        ;;
    esac
    key="$cat $subject"
    if [ "${BASELINE_HIT[$key]:-unset}" = "unset" ]; then
      violation "stale-baseline: '$cat $subject' no longer reproduces — remove it from scripts/guard_coverage_baseline.txt"
    fi
  done < "$BASELINE"
fi

# --- report ------------------------------------------------------------------

if [ "${#VIOLATIONS[@]}" -gt 0 ]; then
  echo ""
  echo "FAIL: ${#VIOLATIONS[@]} guard-coverage violation(s) across $CHECKS_RUN guard(s):"
  for v in "${VIOLATIONS[@]}"; do
    echo "  - $v"
  done
  exit 1
fi

echo "OK: $CHECKS_RUN wired guard(s) covered (mutation: $([ "$RUN_MUTATION" -eq 1 ] && echo on || echo off))"
exit 0
