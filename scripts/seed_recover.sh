#!/usr/bin/env bash
set -uo pipefail
# seed_recover.sh — restore a seed that a crash left retired but unpublished.
#
# The refresh path is: validate the new seed completely, THEN retire the old one,
# THEN publish the new one. That ordering is what keeps a valid seed available
# throughout — but it opens one window the caller cannot close, because the two
# final steps are separate operations:
#
#     mv seeds/<key>  ->  seeds/.retired.<key>.<pid>      <- retired
#     mv .staging/…   ->  seeds/<key>                    <- published
#
# A SIGKILL or power loss between them leaves the key ABSENT. The consumer then
# correctly reports `cold reason=no-seed` — a safe outcome, never a wrong build —
# but without recovery the key stays absent indefinitely, and the backup sits
# there under a name that does not say which key it belongs to.
#
# This is why the retired name carries the key. Recovery is then a rename, not an
# investigation. Deliberately explicit rather than automatic: silently resurrecting
# a retired seed from the consumer path would make a read operation mutate the
# store, which is the opposite of D3.
#
# Usage: seed_recover.sh [--seeds-root <dir>] [--dry-run]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SEEDS_ROOT="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}"
DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --seeds-root) SEEDS_ROOT="${2:-}"; shift 2 ;;
    --dry-run)    DRY=1; shift ;;
    -h|--help)    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 0 ;;
    *) echo "seed_recover.sh: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

[ -d "$SEEDS_ROOT" ] || { echo "seed_recover.sh: no seeds root at $SEEDS_ROOT"; exit 0; }
RESTORED=0; STALE=0; STAGING=0
DRYNOTE=""; [ "$DRY" -eq 1 ] && DRYNOTE=" (dry run)"

shopt -s nullglob
for retired in "$SEEDS_ROOT"/.retired.*; do
  base="$(basename "$retired")"
  key="${base#.retired.}"; key="${key%.*}"      # .retired.<key>.<pid> -> <key>
  if [ -z "$key" ]; then
    echo "  ?  $base: cannot tell which key it belongs to — left in place"
    continue
  fi
  if [ -d "$SEEDS_ROOT/$key" ]; then
    echo "  ·  $base: seeds/$key exists, so this backup is stale$DRYNOTE"
    STALE=$((STALE+1))
    continue
  fi
  if [ "$DRY" -eq 1 ]; then
    echo "  →  would restore $key from $base"
  else
    chmod -R u+w "$retired" 2>/dev/null || true
    if mv "$retired" "$SEEDS_ROOT/$key"; then
      echo "  ✓  restored $key from $base"
    else
      echo "  ✗  could not restore $key from $base — left in place" >&2
      continue
    fi
  fi
  RESTORED=$((RESTORED+1))
done

# Orphaned staging trees are NOT restored: a half-built reference is not a seed.
# They are only reported, so that removing them stays a deliberate act.
for stage in "$SEEDS_ROOT"/.staging.*; do
  echo "  !  $(basename "$stage"): orphaned staging tree, $(du -sh --apparent-size "$stage" 2>/dev/null | cut -f1) apparent — not a seed, safe to remove by hand"
  STAGING=$((STAGING+1))
done

echo "restored=$RESTORED stale=$STALE orphaned-staging=$STAGING$DRYNOTE"
[ "$STALE" -eq 0 ] || echo "note: stale backups are left in place; a seed present at the same key was published after them"
exit 0
