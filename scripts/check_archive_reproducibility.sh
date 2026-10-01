#!/usr/bin/env bash
# check_archive_reproducibility.sh — assert the normalization invariants on a
# produced release archive (issue #1607, SC-04).
#
# WHAT THIS IS FOR
# ================
# release.yml normalizes every archive it publishes: sorted entry order, uid/gid
# 0, an mtime equal to SOURCE_DATE_EPOCH, no owner-name fields, no pax extended
# headers, and a gzip member whose own header carries neither a timestamp nor a
# file name. Those are the properties that make two builds of the same source
# produce the same container. This script ASSERTS them, on the artifact that is
# about to be published, so a regression in the packaging steps fails the release
# instead of shipping an archive nobody can reproduce.
#
# Asserting is deliberately stronger than nothing and deliberately weaker than a
# full two-build comparison — see HONEST SCOPE below.
#
# WHAT IS ASSERTED
# ================
#   Common (tar.gz and zip):
#     1. The archive is readable and structurally sound (gzip CRC, zip CRCs).
#     2. Entry names are in strictly ascending byte order — sorting is what makes
#        the order independent of readdir, and a single duplicate or inversion
#        means the writer regressed.
#     3. Exactly one regular-file entry, no directories, no symlinks, no Apple
#        DoubleSidecar `._*` entries, no pax/zip64 extended records.
#   .tar.gz additionally:
#     4. uid == 0 and gid == 0 for every entry.
#     5. uname == "" and gname == "" — a recorded owner NAME is host-dependent
#        even when the numeric id is 0.
#     6. mtime == SOURCE_DATE_EPOCH for every entry.
#     7. The gzip member header stores mtime 0 and does not set FNAME/FCOMMENT.
#        Measured nuance, because the obvious explanation here is wrong: GNU
#        gzip already writes MTIME=0 and FLG=0 when it compresses a PIPE, which
#        is what `tar czf` does — so `tar czf` is NOT clock-stamping the member
#        and these assertions do not "catch tar czf". What they catch is
#        compressing a NAMED REGULAR FILE (`gzip -N some.tar`, MTIME and FNAME
#        both land in the header) and any compressor that stamps by default. The
#        release pipeline compresses through an explicit `gzip -n -9` anyway, so
#        the header bytes are a stated property of the build rather than an
#        accident of whichever gzip default the runner image ships.
#
# HONEST SCOPE — WHAT THIS DOES NOT PROVE
# =======================================
# This does NOT prove the archive is byte-reproducible across two builds, and it
# must not be reported as if it did. A genuine two-build byte comparison of
# `webfang-x86_64-pc-windows-msvc.zip` additionally requires a DETERMINISTIC PE
# TIMESTAMP FROM THE MSVC LINKER, and that is not reachable from this
# repository's inputs: the value is derived from the link-time system clock
# inside the linker, and the only knobs that influence it (linker flags, or
# post-processing the PE header) live outside the sanctioned scope of this
# change — no `Cargo.toml`, build-script, or Rust-source edit is allowed, and
# shipping a PE rewriter would be a much larger change than the defect it
# addresses. The honest formulation is therefore:
#
#   * CONTAINER determinism — every byte outside the enclosed binary — is
#     asserted here, one invariant at a time, and a regression fails the release.
#   * CONTENT determinism of the enclosed binary is NOT asserted, and cannot be,
#     for the Windows target. Reproducing that requires a linker-level change
#     that is out of scope here.
#
# Claiming otherwise would be the exact failure mode this file exists to
# prevent: a green gate that certifies something it never measured.
#
# USAGE
#   SOURCE_DATE_EPOCH=<unix-ts> bash scripts/check_archive_reproducibility.sh <archive> [archive ...]
#
# Fail-closed: a missing SOURCE_DATE_EPOCH, a missing python3, or an unreadable
# archive is a FAILURE. There is no "cannot tell, so skip" path.
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C
export PYTHONHASHSEED=0
export PYTHONDONTWRITEBYTECODE=1

usage() {
    cat <<'USAGE'
check_archive_reproducibility.sh — assert the normalization invariants on a
produced release archive (sorted entries, uid/gid 0, no owner-name fields, mtime
== SOURCE_DATE_EPOCH, gzip member header with MTIME 0 and no FNAME).

usage:
  bash scripts/check_archive_reproducibility.sh --help
  SOURCE_DATE_EPOCH=<unix-ts> bash scripts/check_archive_reproducibility.sh <archive.tar.gz|archive.zip> [archive ...]

Fail-closed: a missing SOURCE_DATE_EPOCH, a missing python3, or an unreadable
archive is a FAILURE, never a skip.

WHAT THIS DOES NOT PROVE: byte-identical rebuilds of the enclosed binary. On the
Windows target that additionally needs a deterministic PE timestamp from the MSVC
linker, which is not reachable from this repository's inputs. See the HONEST
SCOPE section of this file.
USAGE
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
esac

if [[ $# -lt 1 ]]; then
    usage
    exit 2
fi

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    echo "::error::SOURCE_DATE_EPOCH no está definido: sin él, 'mtime == SOURCE_DATE_EPOCH' no es una aserción sino una coincidencia, y este guard NO se salta."
    exit 1
fi

if ! [[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]]; then
    echo "::error::SOURCE_DATE_EPOCH='$SOURCE_DATE_EPOCH' no es un timestamp UNIX entero (segundos desde la época)."
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 no está disponible y este guard NO se salta: sin él, las invariantes de normalización no se pueden comprobar y el release se publicaría sin verificarlas."
    exit 1
fi

for archive in "$@"; do
    if [[ ! -f "$archive" ]]; then
        echo "::error::'$archive' no existe: no hay nada que comprobar."
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# One python3 invocation for every archive: a batch, so the summary below is a
# single ordered report and the exit code covers the whole set.
# ---------------------------------------------------------------------------
python3 - "$SOURCE_DATE_EPOCH" "$@" <<'PY'
import os
import sys
import tarfile
import time
import zipfile

epoch = int(sys.argv[1])
archives = sys.argv[2:]

DOS_EPOCH = 315532800
failures = []


def fail(archive, message):
    failures.append((archive, message))


def check_common_shape(archive, names):
    """Ordering + single regular-file entry, shared by both formats."""
    if names != sorted(names):
        fail(archive, "entries are not in ascending byte order: {}".format(names))
    if len(names) != len(set(names)):
        fail(archive, "duplicate entry names: {}".format(names))
    if len(names) != 1:
        fail(archive, "expected exactly 1 entry, found {}: {}".format(len(names), names))
    for name in names:
        if os.path.basename(name) != name:
            fail(archive, "entry is not at the archive root (a directory prefix "
                          "makes extraction paths host-dependent): {}".format(name))
        if name.startswith("._") or name == ".DS_Store" or "__MACOSX" in name:
            fail(archive, "host metadata entry present (AppleDouble / .DS_Store): "
                          "{}".format(name))


def check_tar(archive):
    # Read the gzip member header separately: tarfile validates the payload but
    # says nothing about the 10-byte gzip header, which is where `tar czf`
    # hides the build clock.
    with open(archive, "rb") as handle:
        head = handle.read(10)
    if len(head) < 10 or head[0] != 0x1F or head[1] != 0x8B:
        fail(archive, "not a gzip member (bad magic); is the extension a lie?")
        return
    # 10-byte gzip header: [0]=ID1 [1]=ID2 [2]=CM [3]=FLG [4..8]=MTIME
    # [8]=XFL [9]=OS. The method byte is index 2; index 3 is the flag byte.
    if head[2] != 0x08:
        fail(archive, "gzip compression method is {}, expected deflate(8)".format(head[2]))
    flags = head[3]
    mtime = int.from_bytes(head[4:8], "little")
    if mtime != 0:
        fail(archive, "gzip header stores mtime={} (expected 0): the member was "
                      "compressed from a NAMED regular file, so the compressor "
                      "recorded its timestamp. Note this is NOT what `tar czf` "
                      "does (a pipe already yields MTIME=0); build with "
                      "`tar … -cf - | gzip -n -9`.".format(mtime))
    if flags & 0x08:
        fail(archive, "gzip header has FNAME set (flag bit 3): the original file "
                      "name is recorded, which is a path-dependent byte")
    if flags & 0x10:
        fail(archive, "gzip header has FCOMMENT set (flag bit 4)")
    if flags & 0x02:
        fail(archive, "gzip header has FHCRC set (flag bit 2)")

    try:
        with tarfile.open(archive, "r:gz") as tar:
            members = tar.getmembers()
            names = [m.name for m in members]
            check_common_shape(archive, names)
            for member in members:
                if not member.isreg():
                    kind = "dir" if member.isdir() else ("symlink" if member.issym() else str(member.type))
                    fail(archive, "entry {} is a {}; only a regular file may ship".format(member.name, kind))
                if member.uid != 0 or member.gid != 0:
                    fail(archive, "entry {} has uid={} gid={} (expected 0/0)".format(
                        member.name, member.uid, member.gid))
                if member.uname or member.gname:
                    fail(archive, "entry {} records owner names uname={!r} gname={!r}; "
                                  "a name is host-dependent even when the id is 0".format(
                                      member.name, member.uname, member.gname))
                if member.mtime != epoch:
                    fail(archive, "entry {} has mtime={} (expected SOURCE_DATE_EPOCH={})".format(
                        member.name, member.mtime, epoch))
                if member.pax_headers:
                    fail(archive, "entry {} carries pax extended headers {}; the GNU "
                                  "format has no such records and they embed "
                                  "host-dependent nanosecond fields".format(
                                      member.name, sorted(member.pax_headers)))
    except (tarfile.TarError, OSError) as error:
        fail(archive, "unreadable as a gzip tar: {}".format(error))


def check_zip(archive):
    try:
        with zipfile.ZipFile(archive) as zf:
            broken = zf.testzip()
            if broken is not None:
                fail(archive, "CRC mismatch on entry {}".format(broken))
            infos = zf.infolist()
            names = [i.filename for i in infos]
            check_common_shape(archive, names)
            expected = None
            if epoch >= DOS_EPOCH:
                st = time.gmtime(epoch)
                expected = (st.tm_year, st.tm_mon, st.tm_mday, st.tm_hour, st.tm_min, st.tm_sec)
            for info in infos:
                if info.is_dir():
                    fail(archive, "entry {} is a directory; only a regular file may ship".format(info.filename))
                if expected is not None and tuple(info.date_time) != expected:
                    fail(archive, "entry {} has date_time={} (expected {})".format(
                        info.filename, tuple(info.date_time), expected))
                mode = (info.external_attr >> 16) & 0xFFFF
                if mode != 0o100755:
                    fail(archive, "entry {} has mode={} (expected 0o100755: a host umask "
                                  "must never decide whether the binary is executable)".format(
                                      info.filename, oct(mode)))
                if info.create_system != 3:
                    fail(archive, "entry {} has create_system={} (expected 3/Unix, so the "
                                  "same commit yields the same archive on every runner OS)".format(
                                          info.filename, info.create_system))
                if info.flag_bits != 0:
                    fail(archive, "entry {} has flag_bits={:#x} (expected 0)".format(
                        info.filename, info.flag_bits))
                if info.compress_type != zipfile.ZIP_DEFLATED:
                    fail(archive, "entry {} uses compression {} (expected deflate)".format(
                        info.filename, info.compress_type))
    except (zipfile.BadZipFile, OSError) as error:
        fail(archive, "unreadable as a zip: {}".format(error))

for path in archives:
    if path.endswith(".zip"):
        check_zip(path)
    elif path.endswith((".tar.gz", ".tgz")):
        check_tar(path)
    else:
        fail(path, "unrecognised archive extension; this guard knows .tar.gz/.tgz and .zip only")

if failures:
    for archive, message in failures:
        print("::error::{}: {}".format(archive, message))
    print("::error::{} archive(s) failed the reproducibility invariants; the release must not publish them.".format(len(failures)))
    sys.exit(1)

for path in archives:
    # Report the invariants that actually apply to the container. A zip has no
    # uid/gid, no owner names and no gzip header; saying "gzip mtime 0" about a
    # .zip would be a false claim in a gate whose entire job is to state facts.
    if path.endswith(".zip"):
        detail = "sorted entries, mtime={}, mode 0755, create_system=Unix, deflate".format(epoch)
    else:
        detail = "sorted entries, uid/gid 0, no owner names, mtime={}, gzip mtime 0".format(epoch)
    print("reproducibility invariants OK: {} ({})".format(path, detail))
PY
