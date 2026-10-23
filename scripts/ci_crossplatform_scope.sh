#!/usr/bin/env bash
set -euo pipefail
# ci_crossplatform_scope.sh — decide whether a diff needs the Windows/macOS lane.
#
# Usage:
#   ci_crossplatform_scope.sh --base <sha> --head <sha>
#
# Prints exactly `true` or `false` on stdout. The caller (scripts/ci_plan.sh)
# treats any failure as `true`: a platform regression must never be skipped by
# a detector that could not run. The detector is intentionally filename-first
# and content-bounded: it scans only changed `.rs` files for cfg/target markers
# with `git grep <head> -- <file>`, never the whole tree.
#
# A change without a cfg marker can still break Windows (for example a
# std::process::Command call). That residual risk is accepted by #2282 and
# falls to the scheduled (nightly) full-scope run of ci.yml.

usage() {
  cat <<'EOF'
usage: ci_crossplatform_scope.sh --base <sha> --head <sha>

Prints true when the diff needs the Windows/macOS test lane, false otherwise.
Fail-closed: exits 0 with `true` when the diff cannot be inspected.
EOF
}

BASE_REF=""
HEAD_REF=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base | --base-ref)
      BASE_REF="${2:?missing value for $1}"
      shift 2
      ;;
    --head | --head-ref)
      HEAD_REF="${2:?missing value for $1}"
      shift 2
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

fail_closed() {
  echo "crossplatform-scope: $1; failing closed (true)" >&2
  echo "true"
  exit 0
}

[[ -n "$BASE_REF" && -n "$HEAD_REF" ]] || fail_closed "missing --base/--head"
[[ "$BASE_REF" =~ ^[0-9a-f]{40}$ ]] || fail_closed "base is not a 40-hex sha"
[[ "$HEAD_REF" =~ ^[0-9a-f]{40}$ ]] || fail_closed "head is not a 40-hex sha"

# Three-dot matches the PR/merge_group intent (changes since the merge base).
# Push diffs may not share an ancestor after a force-push, so fall back to the
# two-dot form instead of widening every such push to the full lane.
if ! CHANGED="$(git diff --name-only "$BASE_REF...$HEAD_REF" -- 2>/dev/null)"; then
  CHANGED="$(git diff --name-only "$BASE_REF" "$HEAD_REF" -- 2>/dev/null)" ||
    fail_closed "could not diff $BASE_REF..$HEAD_REF"
fi

if [[ -z "$CHANGED" ]]; then
  echo "crossplatform-scope: empty diff -> false" >&2
  echo "false"
  exit 0
fi

needs_crossplatform() {
  local file="$1"

  # Workspace/input surface: dependencies, lockfile, build scripts, cargo
  # config and the toolchain pin can all change the platform surface without
  # touching a `.rs` file.
  case "$file" in
    Cargo.toml | */Cargo.toml | Cargo.lock | build.rs | */build.rs | .cargo/* | rust-toolchain.toml | */rust-toolchain.toml)
      return 0
      ;;
  esac

  [[ "$file" == *.rs ]] || return 1

  # Bounded content scan: only this changed file, at the reviewed head. `-I`
  # skips binary content; `-q` keeps the check cheap. Exit 1 means "no match";
  # anything above 1 means git could not inspect the file, which is a
  # fail-closed condition decided by the caller.
  local rc=0
  git grep -I -q -E \
    -e 'cfg\(' \
    -e 'cfg_attr' \
    -e 'cfg!' \
    -e 'target_family' \
    -e 'target_pointer_width' \
    -e 'target_arch' \
    -e 'std::os::windows' \
    -e 'windows[-_]sys' \
    -e 'winapi' \
    "$HEAD_REF" -- "$file" || rc=$?
  if [[ "$rc" -eq 0 ]]; then
    return 0
  fi
  if [[ "$rc" -gt 1 ]]; then
    fail_closed "git grep failed for $file"
  fi
  return 1
}

while IFS= read -r file; do
  [[ -n "$file" ]] || continue
  if needs_crossplatform "$file"; then
    echo "crossplatform-scope: $file -> true" >&2
    echo "true"
    exit 0
  fi
done <<< "$CHANGED"

CHANGED_COUNT="$(printf '%s\n' "$CHANGED" | grep -c . || true)"
echo "crossplatform-scope: no platform surface in ${CHANGED_COUNT} changed path(s) -> false" >&2
echo "false"
