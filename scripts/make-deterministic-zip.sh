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
    # A stale target that survived a failed previous step would otherwise be
    # published as if it were this run's archive.
    echo "::error::'$OUT' ya existe: este script no sobrescribe un artefacto de una ejecución anterior (bórralo si el reintento es intencionado)."
    exit 1
fi

# The whole writer. Quoted heredoc: no shell expansion, so the Python is exactly
# the text below and never sees a repository-controlled value interpolated into
# source. Paths arrive as argv, not as text.
python3 - "$OUT" "$SOURCE_DATE_EPOCH" "$@" <<'PY'
import os
import stat
import sys
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

with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
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
PY

if [[ ! -s "$OUT" ]]; then
    echo "::error::'$OUT' salió vacío o inexistente: el escritor no produjo un archivo."
    exit 1
fi

echo "wrote $OUT ($(wc -c <"$OUT" | tr -d ' ') bytes, SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH, entries sorted, fixed timestamps and permissions)"
