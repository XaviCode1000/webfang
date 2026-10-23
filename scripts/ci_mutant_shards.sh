#!/usr/bin/env bash
#
# ci_mutant_shards.sh — helper behind the mutants.yml PR gate (#2285).
#
# Three commands, each with a single observable contract:
#
#   ci_mutant_shards.sh prepare-diff
#       Builds git.diff for `cargo mutants --in-diff` from EXPLICIT base and
#       head commits. The head is the checked-out sha (a GitHub merge branch:
#       refs/pull/N/merge for pull_request, the merge-group branch for
#       merge_group). The base is, by the definition of a merge branch, the
#       head's FIRST PARENT — an immutable commit that holds exactly this
#       PR's changes and none of the advancement that happened while the PR
#       sat open. The event-delivered sha (EVENT_BASE_SHA:
#       github.event.pull_request.base.sha or merge_group.base_sha) is used
#       as the intent and cross-checked: when it disagrees with HEAD^1 the
#       helper warns and falls back to HEAD^1 (the ambiguity of when GitHub
#       refreshes base.sha must never decide the diff). Machine lines on
#       stdout: base_sha=, head_sha=. git.diff is written to the CWD.
#
#   ci_mutant_shards.sh shard-count <mutant_count>
#       Pure formula (#2285 calibration, provisional): shards of ~8 mutants
#       capped at 16 (the #1436 wall was ~20 mutants/shard against a
#       90-minute timeout with a ~10-15 min BoringSSL base build; 8 mutants
#       with that base build stays well under the new 45-minute shard
#       timeout). Calibration knobs: MUTANT_SHARD_SIZE, MUTANT_SHARD_CAP.
#
#   ci_mutant_shards.sh shards <git.diff> [extra cargo-mutants args...]
#       Counts the diff's mutants with `cargo mutants --list --json
#       --in-diff` (a --list run needs no compilation) and emits the matrix
#       sizing. Fail-closed (#2285 docs contract): a nonzero --list exit
#       code never becomes a count of 0 or a fallback matrix — the job
#       fails with the code. Exit code 5 means the diff's new side does not
#       match the checked-out tree (an advanced-base
#       mismatch or a foreign diff), exit 6 means the diff is not a diff.
#
# stdout lines are key=value / shards=[...] for GITHUB_OUTPUT consumption;
# diagnostics go to stderr. Offline by construction: prepare-diff and shards
# only run git and cargo-mutants --list.
#
set -u
set -o pipefail

SHARD_SIZE="${MUTANT_SHARD_SIZE:-8}"
SHARD_CAP="${MUTANT_SHARD_CAP:-16}"

die() {
  echo "error: $*" >&2
  exit 1
}

emit_matrix() {
  local count="$1"
  local shard_count
  shard_count=$(( (count + SHARD_SIZE - 1) / SHARD_SIZE ))
  if (( shard_count > SHARD_CAP )); then
    shard_count="$SHARD_CAP"
  fi
  local list
  if (( count <= 0 )); then
    list=""
  else
    if ! list="$(seq -s, 1 "$shard_count" 2>/dev/null)"; then
      die "cannot build the shard list for shard_count=$shard_count"
    fi
  fi
  printf 'shard_count=%s\n' "$shard_count"
  printf 'shards=[%s]\n' "$list"
}

cmd_shard_count() {
  local count="${1:-}"
  [[ "$count" =~ ^[0-9]+$ ]] || die "shard-count expects a non-negative integer, got '${count}'"
  printf 'mutant_count=%s\n' "$count"
  emit_matrix "$count"
}

cmd_prepare_diff() {
  local head_sha="${HEAD_SHA:-$(git rev-parse HEAD 2>/dev/null)}"
  [[ -n "$head_sha" ]] || die "prepare-diff requires a git repo"
  local head_parent=""
  head_parent="$(git rev-parse "$head_sha^1" 2>/dev/null)" \
    || die "head sha $head_sha has no parent commit (did checkout use fetch-depth: 0?)"
  if [[ -n "${EVENT_BASE_SHA:-}" && "${EVENT_BASE_SHA}" != "$head_parent" ]]; then
    # The event's captured base sha disagrees with the checked-out tree: the
    # base advanced and the merge branch was rebuilt. Never merge two views:
    # the first parent IS the witness base of this PR's changes.
    echo "warning: event base sha ${EVENT_BASE_SHA} is not the head's first parent" \
      "(${head_parent}); the base advanced — diffing against the first parent" >&2
  fi
  local diff_base="$head_parent"
  if ! git diff "${diff_base}..${head_sha}" > git.diff; then
    die "git diff ${diff_base}..${head_sha} failed"
  fi
  printf 'base_sha=%s\n' "$diff_base"
  printf 'head_sha=%s\n' "$head_sha"
}

cmd_shards() {
  local diff_file="${1:-}"
  [[ -n "$diff_file" && -f "$diff_file" ]] \
    || die "shards expects an existing coverage-ready diff file, got '${diff_file}'"
  shift
  local list_json="" mutants_stderr=""
  # Temp side channel for the tool's stderr: in a command substitution stdout
  # and stderr need separate handles. If mktemp fails, degrade to dropping the
  # detail (never to failing the count path).
  mutants_stderr="$(mktemp "${TMPDIR:-/tmp}/mutants-list-stderr.XXXXXX" 2>/dev/null)" || true
  [[ -n "$mutants_stderr" ]] || mutants_stderr=/dev/null
  # `|| { }` (not `if !`): inside a negated condition bash overwrites $? with the
  # negation's result (0) — the same envelope-capture lesson as gh's stdout.
  list_json="$(cargo mutants --list --json --in-diff "$diff_file" "$@" 2>"$mutants_stderr")" || {
    # #2285/docs: exit 5 = the diff's new side does not match this tree
    # (advanced-base mismatch, foreign tree); exit 6 = not a diff at all.
    echo "error: cargo mutants --list --in-diff exited with code $? — " \
      "refusing to size the matrix from it (#2285: an --in-diff mismatch is fail-closed)" >&2
    # Relay the binary's own diagnostics (#2295 native review, R3-1/R4): the
    # WARN/ERROR detail naming the mismatching hunk/file is what triage in CI
    # needs; a bare "exited with code 5" would force a manual rerun.
    if [[ "$mutants_stderr" != /dev/null && -s "$mutants_stderr" ]]; then
      sed 's/^/  | /' "$mutants_stderr" >&2
    fi
    [[ "$mutants_stderr" != /dev/null ]] && rm -f "$mutants_stderr"
    exit 1
  }
  [[ "$mutants_stderr" != /dev/null ]] && rm -f "$mutants_stderr"
  local count
  count="$(printf '%s' "$list_json" | jq 'length' 2>/dev/null)" \
    || die "cannot parse the mutant list JSON emitted by cargo mutants"
  # A diff touching no Rust file yields rc 0 with NO stdout at all (INFO
  # "Diff changes no Rust source files" goes to stderr): that is zero
  # mutants, not a parse failure.
  if [[ -z "$count" ]]; then
    count=0
  fi
  printf 'mutant_count=%s\n' "$count"
  emit_matrix "$count"
}

# --- dispatch -----------------------------------------------------------------
case "${1:-}" in
  prepare-diff)
    shift
    cmd_prepare_diff "$@"
    ;;
  shard-count)
    shift
    cmd_shard_count "$@"
    ;;
  shards)
    shift
    cmd_shards "$@"
    ;;
  *)
    die "unknown command: ${1:-<empty>} (expected: prepare-diff | shard-count <n> | shards <diff> [args...])"
    ;;
esac
