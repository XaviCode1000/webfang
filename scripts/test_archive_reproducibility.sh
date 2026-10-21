#!/usr/bin/env bash
# test_archive_reproducibility.sh
#
# Semantics harness for scripts/check_archive_reproducibility.sh (issue #1607,
# SC-04). The guard asserts the NORMALIZATION invariants of a release archive
# (sorted entries, uid/gid 0, no owner names, mtime == SOURCE_DATE_EPOCH, a
# gzip member header with MTIME 0 and no FNAME) on the artifact that is about
# to be published.
#
# What this harness proves, offline (mktemp fixtures built with python3; no
# network, no cargo):
#
#   1. a well-formed tar.gz and a well-formed zip PASS;
#   2. an archive whose mtime does not match SOURCE_DATE_EPOCH FAILS, for both
#      formats;
#   3. a zip built for a SOURCE_DATE_EPOCH below the DOS epoch (1980-01-01)
#      FAILS with an explanation. The zip format cannot represent that mtime,
#      so the assertion is unsatisfiable; the guard used to skip it silently
#      and still print a success line claiming `mtime~0` — an assertion that
#      never ran, reported as verified;
#   4. a missing SOURCE_DATE_EPOCH, a missing archive and an unrecognised
#      extension all FAIL (there is no "cannot tell, so skip" path).
#
# Usage: bash scripts/test_archive_reproducibility.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_archive_reproducibility.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! command -v python3 >/dev/null 2>&1; then
  echo "test_archive_reproducibility.sh: python3 not available; this harness needs it to build fixtures" >&2
  exit 2
fi

passed=0
failed=0

ok() {
  printf '  ok   %-52s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-52s %s\n' "$1" "$2"
  if [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# --- fixtures ----------------------------------------------------------------
# build_zip <path> <y,m,d,h,m,s> — a single-entry zip with the unix mode,
# the unix create_system and deflate, i.e. everything except the mtime.
# build_tar <path> <mtime> — a single-entry tar.gz with uid/gid 0, no owner
# names and a gzip header that carries neither a timestamp nor a file name.
python3 - "$WORK" <<'PY'
import gzip
import io
import sys
import tarfile
import zipfile

work = sys.argv[1]


def build_zip(path, date_time):
    zi = zipfile.ZipInfo("webfang-linux-x86_64", date_time)
    zi.create_system = 3
    zi.compress_type = zipfile.ZIP_DEFLATED
    zi.external_attr = 0o100755 << 16
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr(zi, b"MZ")


def build_tar(path, mtime):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tf:
        info = tarfile.TarInfo("webfang-linux-x86_64")
        payload = b"MZ"
        info.size = len(payload)
        info.mtime = mtime
        info.uid = 0
        info.gid = 0
        info.uname = ""
        info.gname = ""
        tf.addfile(info, io.BytesIO(payload))
    with open(path, "wb") as handle:
        gz = gzip.GzipFile(filename="", mode="wb", fileobj=handle, mtime=0)
        gz.write(buf.getvalue())
        gz.close()


build_zip(f"{work}/good.zip", (2020, 1, 1, 0, 0, 2))
build_zip(f"{work}/stale.zip", (2020, 1, 1, 0, 0, 0))
build_zip(f"{work}/pre_dos.zip", (2020, 1, 1, 0, 0, 0))
build_tar(f"{work}/good.tar.gz", 1577836800)
build_tar(f"{work}/stale.tar.gz", 0)
PY

OUT=""
RC=0
run_guard() { # <epoch> <archive...>
  local epoch="$1"
  shift
  OUT="$(SOURCE_DATE_EPOCH="$epoch" bash "$GATE" "$@" 2>&1)"
  RC=$?
}

expect_pass() { # <label> <epoch> <archive>
  run_guard "$2" "$3"
  if [ "$RC" -eq 0 ]; then
    ok "$1" "rc=0"
  else
    bad "$1" "expected PASS, got rc=$RC" "$OUT"
  fi
}

expect_fail() { # <label> <epoch> <archive> [needle]
  run_guard "$2" "$3"
  if [ "$RC" -eq 0 ]; then
    bad "$1" "expected FAIL, got rc=0" "$OUT"
    return
  fi
  if [ -n "${4:-}" ] && ! printf '%s' "$OUT" | grep -qiF "$4"; then
    bad "$1" "expected FAIL mentioning '$4'" "$OUT"
    return
  fi
  ok "$1" "rc=$RC"
}

echo "check_archive_reproducibility semantics harness"

# --- 1. well-formed archives pass -------------------------------------------
# NEGATION: a guard that rejects everything is not a gate either; these are
# the negative controls for every FAIL case below.
expect_pass "well-formed tar.gz passes" 1577836800 "$WORK/good.tar.gz"
expect_pass "well-formed zip passes" 1577836802 "$WORK/good.zip"

# --- 2. a wrong mtime fails, for both formats -------------------------------
# NEGATION: the mtime is the invariant the whole guard exists for; an archive
# that does not carry SOURCE_DATE_EPOCH must never be published.
expect_fail "tar.gz with a stale mtime fails" 1577836800 "$WORK/stale.tar.gz" "mtime"
expect_fail "zip with a stale mtime fails" 1577836802 "$WORK/stale.zip" "date_time"

# --- 3. a pre-DOS epoch cannot be asserted, so it fails with the reason -----
# NEGATION: `expected` stayed None below the DOS epoch, so the date_time
# assertion never ran while the success line still printed `mtime~0` — an
# assertion that never ran, reported as verified.
expect_fail "zip with a pre-DOS SOURCE_DATE_EPOCH fails" 0 "$WORK/pre_dos.zip" "DOS"

# --- 4. the fail-closed paths ------------------------------------------------
# NEGATION: there is no "cannot tell, so skip" path in this guard.
OUT="$(bash "$GATE" "$WORK/good.zip" 2>&1)"
RC=$?
if [ "$RC" -ne 0 ]; then
  ok "a missing SOURCE_DATE_EPOCH fails" "rc=$RC"
else
  bad "a missing SOURCE_DATE_EPOCH fails" "expected FAIL, got rc=0" "$OUT"
fi

expect_fail "a missing archive fails" 1577836802 "$WORK/does-not-exist.zip" "no existe"

printf 'not an archive\n' >"$WORK/unknown.bin"
expect_fail "an unrecognised extension fails" 1577836802 "$WORK/unknown.bin" "unrecognised"

echo
echo "check_archive_reproducibility semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
