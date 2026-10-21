#!/usr/bin/env bash
# make-deterministic-zip.sh — write a byte-reproducible ZIP (issue #1607, SC-04).
#
# WHY A SCRIPT AND NOT A `7z` LINE IN release.yml
# ================================================
# The Windows packaging step used to be `7z a ....zip webfang.exe`. 7z is not a
# build input of this repository: it is preinstalled on some runner images, it is
# not version-pinned anywhere, and it stamps each entry with the wall clock, the
# host uid/gid and the host permission bits. Two builds of byte-identical content
# therefore produced two different ZIPs, and nothing downstream could tell the
# difference. The replacement has to be a program whose output is a pure function
# of its input, so it lives here, under test, instead of inside a YAML `run:`
# block where a block-scalar nesting mistake is invisible until the job runs.
#
# WHAT IS PINNED
# ==============
# python3's `zipfile` is in the standard library of the interpreter every GitHub
# runner image ships, so this adds no dependency and downloads nothing. The
# writer fixes, for every entry:
#
#   * date_time        — derived from SOURCE_DATE_EPOCH (the validated commit's
#                        committer date), NOT from `time.time()`.
#   * create_system    — forced to 3 (Unix) on every host. Left to itself,
#                        `zipfile` writes 0 on Windows and 3 on Linux, so the
#                        same bytes would differ per runner OS.
#   * external_attr    — forced to S_IFREG|0755. That is what makes the extracted
#                        `webfang.exe` land executable in WSL/Git-Bash, and it
#                        stops the host's umask from leaking into the archive.
#   * flag_bits        — cleared, so no UTF-8-name flag (bit 11) and no
#                        streaming data descriptor (bit 3) whose offset depends
#                        on the writer's internal buffering.
#   * create_version /
#     extract_version  — pinned to 20 (deflate, no ZIP64), instead of derived
#                        from the input size.
#   * entry order      — sorted by name.
#   * compression      — deflate level 9, always, never `stored`.
#
# A ZIP cannot represent a timestamp before 1980-01-01 (the DOS epoch). A commit
# older than that is not something this repository can have, but the writer
# FAILS on it rather than silently clamping, because a clamped mtime is exactly
# the kind of quiet divergence this script exists to remove.
#
# HONEST SCOPE
# ============
# This makes the CONTAINER deterministic. It cannot make the enclosed binary
# byte-reproducible: on Windows the MSVC linker stamps a PE timestamp derived
# from the build clock, and that lives inside `webfang.exe`, not in the archive
# structure. See scripts/check_archive_reproducibility.sh for the same boundary
# stated from the assertion side.
#
# ATOMIC PUBLICATION
# ==================
# The archive is written to a staging file in the target's own directory and
# moved into place with `os.replace()` — an atomic rename inside one
# filesystem. A run killed mid-write therefore leaves NO partial at the target
# path: the staging file is removed and the target keeps whatever it held
# before. The writer used to write the archive IN PLACE, so a killed run left
# a truncated ZIP that looked like an artifact, and the "target already
# exists" refusal then demanded a manual deletion before the retry could run
# at all (audit B31).
#
# USAGE
#   bash scripts/make-deterministic-zip.sh <output.zip> <file> [file ...]
#
# Requires: SOURCE_DATE_EPOCH to be set to a UNIX timestamp (seconds).
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

# Byte collation, always. The entry order is a sort, and under a locale like
# es_ES.UTF-8 collation ignores '-' and '_' at the primary level, so the same
# input would produce a different archive on a different machine.
export LC_ALL=C

# Never let the environment reach the interpreter: PYTHONHASHSEED and friends
# are exactly the kind of ambient input that turns a "deterministic" writer into
# a variable one.
export PYTHONHASHSEED=0
export PYTHONDONTWRITEBYTECODE=1

usage() {
    echo "usage: SOURCE_DATE_EPOCH=<unix-ts> bash scripts/make-deterministic-zip.sh <output.zip> <file> [file ...]" >&2
}

if [[ $# -lt 2 ]]; then
    usage
    exit 2
fi

OUT="$1"
shift

if [[ -z "${SOURCE_DATE_EPOCH:-}" ]]; then
    echo "::error::SOURCE_DATE_EPOCH no está definido: un ZIP determinista necesita una marca de tiempo explícita, nunca la hora del reloj."
    exit 1
fi

if ! [[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]]; then
    echo "::error::SOURCE_DATE_EPOCH='$SOURCE_DATE_EPOCH' no es un timestamp UNIX entero (segundos desde la época)."
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 no está disponible y este script NO se salta: sin un escritor determinista, el ZIP lleva la hora del reloj y dos builds del mismo contenido producen dos archivos distintos."
    exit 1
fi

for f in "$@"; do
    if [[ ! -f "$f" ]]; then
        echo "::error::'$f' no existe o no es un fichero regular: no se puede empaquetar."
        exit 1
    fi
done

if [[ -e "$OUT" ]]; then
    # A stale target is NOT refused: the writer below stages the archive in a
    # temporary file and moves it into place with an atomic rename, so a
    # target at OUT is always a COMPLETE archive — this writer can no longer
    # leave a partial one. The old refusal only manufactured the trap the
    # audit names (B31): a partial left behind by a killed run had to be
    # deleted BY HAND before the retry could run at all, and the deterministic
    # writer reproduces the same bytes for the same inputs anyway.
    echo "nota: '$OUT' ya existe; esta ejecución lo sustituye de forma atómica." >&2
fi

# The whole writer. Quoted heredoc: no shell expansion, so the Python is exactly
# the text below and never sees a repository-controlled value interpolated into
# source. Paths arrive as argv, not as text.
#
# The archive is written to a STAGING file in the same directory as the
# target and moved into place with os.replace() — an atomic rename inside one
# filesystem. A run killed mid-write therefore leaves no partial at OUT: the
# staging file is removed and OUT keeps whatever it held before (audit B31).
python3 - "$OUT" "$SOURCE_DATE_EPOCH" "$@" <<'PY'
import os
import stat
import sys
import tempfile
import time
import zipfile

# The DOS epoch. A ZIP stores a (year, month, day, hour, minute, second) tuple
# with a 1980 floor, so an earlier timestamp is not representable.
DOS_EPOCH = 315532800  # 1980-01-01T00:00:00Z

out = sys.argv[1]
epoch = int(sys.argv[2])
sources = sys.argv[3:]

if epoch < DOS_EPOCH:
    sys.stderr.write(
        "::error::SOURCE_DATE_EPOCH={} es anterior a 1980-01-01 y no cabe en un ZIP "
        "(la época DOS). No se ajusta en silencio: un mtime recortado es "
        "precisamente la divergencia que este script existe para eliminar.\n".format(epoch)
    )
    sys.exit(1)

# gmtime, not localtime: the commit date is an absolute instant, and a runner in
# a non-UTC timezone would otherwise write a different ZIP for the same commit.
stamped = time.gmtime(epoch)
date_time = (
    stamped.tm_year,
    stamped.tm_mon,
    stamped.tm_mday,
    stamped.tm_hour,
    stamped.tm_min,
    stamped.tm_sec,
)

# Unix permissions: regular file, 0755. Fixed, so the host umask never reaches
# the archive and the extracted binary is executable under WSL / Git-Bash.
external_attr = (stat.S_IFREG | 0o755) << 16

# The staging file lives in the target's own directory: os.replace() is only
# atomic within one filesystem, and a staging file on another device would turn
# the publish step back into a copy. It is hidden (leading dot) so a glob like
# "webfang-*.zip" cannot pick it up as a release artifact.
out_dir = os.path.dirname(os.path.abspath(out))
handle_fd, staged = tempfile.mkstemp(prefix=".webfang-zip-staging-", suffix=".tmp", dir=out_dir)
os.close(handle_fd)

try:
    with zipfile.ZipFile(staged, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in sorted(sources):
            name = os.path.basename(path)
            with open(path, "rb") as handle:
                payload = handle.read()
            info = zipfile.ZipInfo(name, date_time=date_time)
            info.compress_type = zipfile.ZIP_DEFLATED
            # 3 = Unix. zipfile derives this from os.name, which would make the same
            # commit produce a different archive on a Windows and a Linux runner.
            info.create_system = 3
            # 2.0 = deflate, no ZIP64 / no data-descriptor machinery. Derived
            # explicitly so the "version needed to extract" field cannot drift with
            # the input size.
            info.create_version = 20
            info.extract_version = 20
            info.external_attr = external_attr
            info.internal_attr = 0
            # Bit 3 (streaming data descriptor) and bit 11 (UTF-8 name) are both
            # cleared: the first records a data offset that depends on the writer's
            # buffering, the second changes the local-header bytes.
            info.flag_bits = 0
            archive.writestr(info, payload)
            sys.stderr.write("zip entry: {} ({} bytes)\n".format(name, len(payload)))

    # The archive is complete: publish it with an atomic rename. os.replace()
    # is atomic within a filesystem, and the staging file lives in the target's
    # own directory, so this is a rename and never a copy.
    os.chmod(staged, 0o644)
    os.replace(staged, out)
except BaseException:
    # A killed or failed run leaves NO partial behind: the staging file goes
    # away and the target keeps whatever it held before.
    if os.path.exists(staged):
        os.unlink(staged)
    raise
PY

if [[ ! -s "$OUT" ]]; then
    echo "::error::'$OUT' salió vacío o inexistente: el escritor no produjo un archivo."
    exit 1
fi

echo "wrote $OUT ($(wc -c <"$OUT" | tr -d ' ') bytes, SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH, entries sorted, fixed timestamps and permissions)"
