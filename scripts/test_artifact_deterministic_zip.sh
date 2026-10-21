#!/usr/bin/env bash
# scripts/test_artifact_deterministic_zip.sh — semantics harness for
# scripts/make-deterministic-zip.sh (audit B31).
#
# The writer used to build the archive IN PLACE. A run killed mid-write left
# a truncated ZIP at the target path that looked like an artifact, and the
# "target already exists" refusal then demanded a manual deletion before the
# retry could run at all. The contract pinned here:
#
#   - the archive is written to a staging file and published with an atomic
#     rename, so a failed run leaves NO partial at the target path and no
#     staging file behind;
#   - a stale or partial target is replaced by the next run (no manual
#     deletion demanded);
#   - the archive stays byte-reproducible for the same inputs and the same
#     SOURCE_DATE_EPOCH;
#   - a missing or malformed SOURCE_DATE_EPOCH still fails closed.
#
# Style: user-facing messages in Spanish (repo convention for scripts/);
# internal output in English.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/make-deterministic-zip.sh"

# Scratch lives on a real disk, never on the tmpfs of /tmp.
SCRATCH="${WEBSITE_TEST_SCRATCH:-$HOME/.cache/tmp/webfang-artifact-tests}"
mkdir -p "$SCRATCH" || exit 2
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs | ramfs)
    echo "refusing tmpfs scratch: $SCRATCH" >&2
    exit 2
    ;;
esac
WORKDIR="$(mktemp -d "$SCRATCH/deterministic-zip.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

pass=0
fail=0
skip=0

ok() {
  echo "  PASS  $1"
  pass=$((pass + 1))
}

bad() {
  echo "  FAIL  $1"
  fail=$((fail + 1))
}

skip_case() {
  echo "  SKIP  $1"
  skip=$((skip + 1))
}

echo "make-deterministic-zip.sh"

fixture="$WORKDIR/zip"
mkdir -p "$fixture"
printf 'payload A\n' >"$fixture/a.txt"
printf 'payload B\n' >"$fixture/b.txt"

# --- the archive is produced and nothing else is left behind ----------------
out_zip="$fixture/out.zip"
(cd "$fixture" && SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" "$out_zip" "$fixture/a.txt" "$fixture/b.txt" >/dev/null 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && [ -s "$out_zip" ]; then
  ok "the archive is written (rc=$rc)"
else
  bad "the archive is written (rc=$rc)"
fi
if python3 -c "import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); assert z.namelist()==['a.txt','b.txt']; z.testzip()" "$out_zip" >/dev/null 2>&1; then
  ok "the archive is a valid zip with the expected entries"
else
  bad "the archive is a valid zip with the expected entries"
fi
leftovers="$(find "$fixture" -maxdepth 1 -name '.webfang-zip-staging-*' -o -maxdepth 1 -name '*.tmp' | tr '\n' ' ')"
if [ -z "$leftovers" ]; then
  ok "no staging file is left in the target directory"
else
  bad "no staging file is left in the target directory (found: $leftovers)"
fi

# --- a failed write leaves no partial at the target path --------------------
# NEGATION: before the fix, the archive was written IN PLACE: a killed run left
# a truncated ZIP at the target path, and the "target already exists" refusal
# then demanded a manual deletion before the retry could run at all. These
# cases accept that partial and that refusal.
# The injection: an input the upfront check accepts (it IS a regular file) but
# that cannot be read. The writer then dies inside the archive build, which is
# exactly the "killed mid-write" shape the audit describes.
if [ "$(id -u)" -eq 0 ]; then
  skip_case "a failed write leaves no partial at the target path (root reads everything)"
else
  fixture_fail="$WORKDIR/zip-fail"
  mkdir -p "$fixture_fail"
  printf 'payload\n' >"$fixture_fail/ok.txt"
  printf 'unreadable\n' >"$fixture_fail/locked.txt"
  chmod 000 "$fixture_fail/locked.txt"
  (cd "$fixture_fail" && SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" "$fixture_fail/out.zip" "$fixture_fail/ok.txt" "$fixture_fail/locked.txt" >/dev/null 2>&1)
  rc=$?
  if [ "$rc" -ne 0 ]; then
    ok "a failed write exits non-zero (rc=$rc)"
  else
    bad "a failed write exits non-zero (rc=$rc)"
  fi
  if [ ! -e "$fixture_fail/out.zip" ]; then
    ok "a failed write leaves no partial archive at the target path"
  else
    bad "a failed write leaves no partial archive at the target path"
  fi
  staged="$(find "$fixture_fail" -maxdepth 1 -name '.webfang-zip-staging-*' | tr '\n' ' ')"
  if [ -z "$staged" ]; then
    ok "a failed write removes the staging file"
  else
    bad "a failed write removes the staging file (found: $staged)"
  fi
  chmod 644 "$fixture_fail/locked.txt"
fi

# --- a stale or partial target is replaced, not refused ---------------------
# The audit's trap: a truncated archive left by a dead run demanded a manual
# deletion before the retry could run.
head -c 60 "$out_zip" >"$fixture/partial.zip" && mv "$fixture/partial.zip" "$out_zip"
(cd "$fixture" && SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" "$out_zip" "$fixture/a.txt" "$fixture/b.txt" >/dev/null 2>&1)
rc=$?
if [ "$rc" -eq 0 ]; then
  ok "a re-run replaces a stale partial target (rc=$rc)"
else
  bad "a re-run replaces a stale partial target (rc=$rc)"
fi
if python3 -c "import sys,zipfile; z=zipfile.ZipFile(sys.argv[1]); assert z.namelist()==['a.txt','b.txt']; z.testzip()" "$out_zip" >/dev/null 2>&1; then
  ok "the replaced archive is complete"
else
  bad "the replaced archive is complete"
fi

# --- the archive stays byte-reproducible ------------------------------------
digest_a="$(md5sum "$out_zip" | cut -d' ' -f1)"
(cd "$fixture" && SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" "$fixture/again.zip" "$fixture/a.txt" "$fixture/b.txt" >/dev/null 2>&1)
digest_b="$(md5sum "$fixture/again.zip" | cut -d' ' -f1)"
if [ -n "$digest_a" ] && [ "$digest_a" = "$digest_b" ]; then
  ok "two runs with the same SOURCE_DATE_EPOCH produce identical bytes"
else
  bad "two runs with the same SOURCE_DATE_EPOCH produce identical bytes"
fi

# --- a missing or malformed epoch still fails closed ------------------------
(cd "$fixture" && env -u SOURCE_DATE_EPOCH bash "$SCRIPT" "$fixture/no-epoch.zip" "$fixture/a.txt" >/dev/null 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && [ ! -e "$fixture/no-epoch.zip" ]; then
  ok "a missing SOURCE_DATE_EPOCH fails closed and writes no archive (rc=$rc)"
else
  bad "a missing SOURCE_DATE_EPOCH fails closed and writes no archive (rc=$rc)"
fi
(cd "$fixture" && SOURCE_DATE_EPOCH=not-a-number bash "$SCRIPT" "$fixture/bad-epoch.zip" "$fixture/a.txt" >/dev/null 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && [ ! -e "$fixture/bad-epoch.zip" ]; then
  ok "a malformed SOURCE_DATE_EPOCH fails closed and writes no archive (rc=$rc)"
else
  bad "a malformed SOURCE_DATE_EPOCH fails closed and writes no archive (rc=$rc)"
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "make-deterministic-zip.sh: $pass passed, 0 failed, $skip skipped"
  exit 0
fi
echo "make-deterministic-zip.sh: $pass passed, $fail failed, $skip skipped"
exit 1
