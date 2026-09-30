#!/usr/bin/env bash
# Quarantine age: record and evaluate the authoritative quarantine start.
#
# `quarantine_started_at` is the ONLY authority for how long an object has been in
# quarantine. It is persisted, because none of the filesystem attributes can carry
# that fact:
#
#   mtime   when the CONTENT last changed        — says nothing about quarantine
#   atime   when it was last read               — moves on any traversal
#   ctime   when the INODE last changed state   — moves on chmod, chown, link count,
#                                                 and every other inode mutation. It
#                                                 is not "created here" and it is not
#                                                 "renamed here".
#
# A rename does set ctime, so after quarantining something its ctime usually equals
# the quarantine moment. Usually is not a contract: a later chmod, chown or hardlink
# moves it, and a retention policy whose clock can be shifted by an unrelated
# permission fix is not a retention policy. That is why the timestamp is written down
# once, deliberately, and read from the file thereafter.
#
# Fail-closed: an entry with no metadata, or with an unparseable timestamp, is NOT
# eligible for deletion. If a process dies between the rename and the metadata write,
# nothing becomes eligible until someone completes the record explicitly.
#
# Usage:
#   quarantine_age.sh record <path> --source <src> [--kind <kind>] [--note <text>]
#   quarantine_age.sh record <path> --source <src> --started-at <UTC ts>   # backfill
#   quarantine_age.sh eligible <path> <minimum_age_seconds>
#
# --started-at completes a record for a rename that already happened, which is the
# window where a crash leaves an entry quarantined but unrecorded. It is only
# honoured when no metadata exists at all, so it can never reset a running clock;
# when the moment cannot be established, the honest backfill is the rename time as
# observed at the time, not "now" and not an inferred old mtime.
#
# Exit: record 0 ok / 1 already recorded / 2 usage
#        eligible 0 eligible / 1 not yet / 2 unknown (no or bad metadata — fail closed)

set -euo pipefail

META_NAME="quarantine.meta"
die() { echo "quarantine_age.sh: $*" >&2; exit 2; }

read_field() {  # $1 = file, $2 = key
  [ -f "$1" ] || return 1
  sed -n "s/^$2 = //p" "$1" | head -1
}

cmd_record() {
  [ $# -ge 1 ] || die "record needs a path"
  local path="$1"; shift
  [ -d "$path" ] || die "not a directory: $path"
  local src="" kind="target-dir" note="" started_at=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --source)     src="${2:-}"; shift 2 ;;
      --kind)       kind="${2:-}"; shift 2 ;;
      --note)       note="${2:-}"; shift 2 ;;
      --started-at) started_at="${2:-}"; shift 2 ;;
      *) die "unknown argument '$1'" ;;
    esac
  done
  [ -n "$src" ] || die "--source is required (where the object lived before quarantine)"

  local meta="$path/$META_NAME"
  if [ -f "$meta" ]; then
    echo "quarantine_age.sh: $meta already exists; refusing to overwrite the start time." >&2
    echo "  A recorded quarantine_started_at is the authority. Rewriting it would reset" >&2
    echo "  the retention clock, so it is never done implicitly. Delete the file" >&2
    echo "  deliberately if the recorded start is genuinely wrong." >&2
    exit 1
  fi

  # The metadata is written INSIDE the entry, after the rename, matching the order
  # the policy describes: rename, then record. A crash in between leaves the object
  # quarantined but unrecorded, which the eligible path treats as not eligible.
  cat > "$meta" <<EOF
quarantine_started_at = ${started_at:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}
source_path = $src
object_kind = $kind
$([ -n "$note" ] && echo "note = $note")
EOF
  echo "  recorded $meta"
  read_field "$meta" quarantine_started_at | sed 's/^/  quarantine_started_at = /'
}

cmd_eligible() {
  [ $# -ge 2 ] || die "eligible needs a path and a minimum age in seconds"
  local path="$1" need="$2"
  local meta="$path/$META_NAME"
  local stamp
  if ! stamp="$(read_field "$meta" quarantine_started_at)" || [ -z "$stamp" ]; then
    echo "UNKNOWN  $path  (no quarantine.meta, or no quarantine_started_at in it)" >&2
    echo "  fail-closed: not eligible. Record it deliberately before considering deletion." >&2
    exit 2
  fi
  local start now age
  start="$(date -u -d "$stamp" +%s 2>/dev/null)" || {
    echo "UNKNOWN  $path  (unparseable quarantine_started_at: '$stamp')" >&2
    exit 2
  }
  now=$(date -u +%s); age=$(( now - start ))
  if [ "$age" -lt "$need" ]; then
    echo "NOT YET  $path  age=$(( age / 3600 ))h of $(( need / 3600 ))h"
    exit 1
  fi
  echo "ELIGIBLE  $path  age=$(( age / 3600 ))h of $(( need / 3600 ))h"
  echo "  Age is a gate, not evidence. Fresh ownership and use evidence is still required."
  exit 0
}

case "${1:-}" in
  record)   shift; cmd_record "$@" ;;
  eligible) shift; cmd_eligible "$@" ;;
  *) die "usage: quarantine_age.sh {record <path> --source <src>|eligible <path> <seconds>}" ;;
esac
