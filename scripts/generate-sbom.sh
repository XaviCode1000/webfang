#!/usr/bin/env bash
# generate-sbom.sh — deterministic SPDX 2.3 SBOM for everything this repository
# ships or builds (issue #1607, SC-05).
#
# WHY THIS EXISTS
# ===============
# The repository shipped no SBOM at all. Two consequences, both of which this
# script removes:
#   1. A consumer of a release binary had no machine-readable inventory of what
#      was linked into it, so "is this affected by advisory X" was answerable
#      only by reading a build log.
#   2. A published SBOM is itself an artifact that must be signed and attested,
#      so it is produced in the release pipeline (the `attest` job) and shipped
#      as a release asset rather than generated ad hoc.
#
# BOTH LOCKFILES — THE POINT OF SC-05
# ===================================
# `fuzz/` is listed in the workspace `exclude` array, so it is NOT a workspace
# member and NOTHING that walks the workspace graph can see it. An SBOM derived
# only from the root lockfile would silently omit the fuzz harness's entire
# dependency set — the same blind spot that made `fuzz/Cargo.lock` escape the
# advisory gate (SC-14, closed in scripts/check_advisory_policy.sh). So this
# script covers both:
#
#   * Cargo.lock      — via `cargo metadata --locked --all-features`.
#   * fuzz/Cargo.lock — read DIRECTLY, as a lockfile. See the measured reason in
#     the python core: `cargo metadata --locked --manifest-path fuzz/Cargo.toml`
#     exits 101 here ("the lock file … needs to be updated but --locked was
#     passed"), because the committed fuzz lockfile is STALE relative to
#     fuzz/Cargo.toml (it predates the current webfang_core graph — it does not
#     even contain `age`, which webfang_core has depended on since). An SBOM
#     must describe the lockfile that is actually locked, so it reads the
#     committed lock rather than a fresh re-resolution, and regenerating that
#     lockfile is a separate change with its own advisory-baseline impact
#     (reported as follow-up, not silently done here).
#
# DETERMINISM — WHY THE OUTPUT IS BYTE-STABLE
# ============================================
# The whole point of an SBOM you can re-generate is that a re-run is a NO-OP DIFF.
# A CI check like `git diff --exit-code` on a regenerated SBOM is only meaningful
# if the generator has no ambient inputs. Everything volatile is therefore
# removed or derived:
#   * NO `creationInfo.created = now()`. It is the validated commit's committer
#     date, so the same commit always yields the same document. (Overridable
#     with SOURCE_DATE_EPOCH, the same variable the archives use.)
#   * NO random UUID in `documentNamespace` — derived from name + version +
#     commit, which is what makes it unique per document AND stable per commit.
#   * NO dict-iteration order: `json.dump(..., sort_keys=True)` and every array is
#     explicitly `sort`ed.
#   * NO floating tool download and NO new dependency: the writer is python3's
#     standard library (present on every GitHub runner image), and the only
#     cargo invocation is the project's own `cargo metadata`.
#   * `LC_ALL=C`, so any residual collation is byte collation.
#
# SPDX 2.3 conformance notes
# ==========================
#   * Per-package `licenseDeclared` is `NOASSERTION` for packages that only
#     appear in `fuzz/Cargo.lock`: a lockfile records name/version/source/checksum
#     and carries NO license field, and inventing one would be worse than
#     declaring ignorance. Every such package is annotated with a `comment`
#     naming its source lockfile, so the gap is visible rather than silent.
#   * `licenseConcluded` is set equal to `licenseDeclared` because nothing here
#     has been through a legal review; claiming a concluded license would be a
#     stronger statement than this generator can support.
#   * `downloadLocation` carries the package URL (purl), and the same purl is
#     repeated as a PACKAGE-MANAGER `externalRef`, which is the machine-readable
#     form consumers query.
#
# USAGE
#   bash scripts/generate-sbom.sh <output.spdx.json>
#
# Fail-closed: a missing cargo, a missing python3, a `--locked` resolution that
# wants to change the lockfile, or a missing fuzz/Cargo.lock all FAIL. There is
# no partial-SBOM path — an SBOM that silently covers less than the build is
# worse than no SBOM, because it is believed.
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C
export PYTHONHASHSEED=0
export PYTHONDONTWRITEBYTECODE=1

usage() {
    cat >&2 <<'EOF'
usage: bash scripts/generate-sbom.sh <output.spdx.json>

Writes a deterministic SPDX 2.3 document covering the workspace lockfile AND
fuzz/Cargo.lock (fuzz/ is not a workspace member, so nothing else describes it).
Re-running with the same tree is byte-identical, so it is safe in CI as a diff.
EOF
}

# `--help` must print usage and exit 0, not be taken as the output path. Without
# this, `generate-sbom.sh --help` silently created a file literally named
# `--help` in the working directory and reported success.
case "${1:-}" in
    -h | --help)
        usage
        exit 0
        ;;
esac

if [[ $# -ne 1 ]]; then
    usage
    exit 2
fi

OUT="$1"

if [[ -z "$OUT" || "$OUT" == -* ]]; then
    echo "::error::el destino '$OUT' no es un nombre de fichero válido; pásalo como argumento posicional." >&2
    usage
    exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT_LOCK="$ROOT/Cargo.lock"
FUZZ_LOCK="$ROOT/fuzz/Cargo.lock"
FUZZ_MANIFEST="$ROOT/fuzz/Cargo.toml"

if ! command -v cargo >/dev/null 2>&1; then
    echo "::error::cargo no está disponible y este script NO se salta: sin la resolución del workspace, el SBOM no puede existir y una release sin SBOM parecería completo."
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 no está disponible y este script NO se salta: es el escritor determinista del SPDX y no hay sustituto declarado."
    exit 1
fi

if ! python3 -c 'import tomllib' >/dev/null 2>&1; then
    echo "::error::el intérprete de python3 no trae 'tomllib' (necesario para leer fuzz/Cargo.lock): requiere Python 3.11 o superior. Este script NO se salta: un SBOM que omitiera el lockfile de fuzz sería exactamente el defecto SC-05."
    exit 1
fi

for required in "$ROOT_LOCK" "$FUZZ_LOCK" "$FUZZ_MANIFEST"; do
    if [[ ! -f "$required" ]]; then
        echo "::error::'$required' no existe. El SBOM debe cubrir los DOS lockfiles (SC-05); si fuzz/ se eliminó a propósito, bórralo en el mismo commit que este hallazgo."
        exit 1
    fi
done

# `created` / `documentNamespace` need the commit this build corresponds to.
# SOURCE_DATE_EPOCH wins when the caller already derived it (the release
# pipeline does, from the validated commit); otherwise the committer date of
# HEAD is used. `date -u -d @<ts>` is GNU-only, so the formatting is done in
# python where it is portable.
CREATED_EPOCH="${SOURCE_DATE_EPOCH:-}"
COMMIT_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo "unknown")"
if [[ -z "$CREATED_EPOCH" ]]; then
    CREATED_EPOCH="$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || echo "")"
fi
if [[ ! "$CREATED_EPOCH" =~ ^[0-9]+$ ]]; then
    # Last resort, and it is deterministic WITHIN a run but not across days.
    # Reached only outside a git checkout, which is not the release path.
    CREATED_EPOCH=0
    echo "warning: no commit date available; creationInfo.created falls back to the UNIX epoch" >&2
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --locked is load-bearing: it makes cargo fail rather than silently update
# Cargo.lock, so the SBOM can never describe a graph other than the committed
# one. Measured on the ROOT lock; fuzz/ is read directly, see the header.
if ! cargo metadata --locked --format-version 1 --all-features \
        --manifest-path "$ROOT/Cargo.toml" >"$WORK/metadata.json"; then
    echo "::error::'cargo metadata --locked' falló sobre el lockfile del workspace. Sin --locked habría reescrito Cargo.lock y el SBOM describiría una resolución distinta de la confirmada."
    exit 1
fi

python3 - "$WORK/metadata.json" "$ROOT_LOCK" "$FUZZ_LOCK" "$OUT" "$CREATED_EPOCH" "$COMMIT_SHA" <<'PY'
import json
import os
import re
import sys
import time
import tomllib

metadata_path, root_lock_path, fuzz_lock_path, out_path, epoch_s, commit_sha = sys.argv[1:7]
epoch = int(epoch_s)

with open(metadata_path, "rb") as handle:
    metadata = json.load(handle)

with open(root_lock_path, "rb") as handle:
    root_lock = tomllib.load(handle)

with open(fuzz_lock_path, "rb") as handle:
    fuzz_lock = tomllib.load(handle)

# SPDX idrefs may only contain letters, digits, "." and "-". Crate names use
# "_", so it is normalized; a collision after normalization is disambiguated by
# appending the version, which is already part of the id.
def spdx_id(name, version):
    return "SPDXRef-Package-{}-{}".format(
        re.sub(r"[^a-zA-Z0-9.\-]+", "-", name), re.sub(r"[^a-zA-Z0-9.\-]+", "-", version)
    )


def purl(name, version):
    # crates.io purl: the namespace is omitted and the version is percent-safe
    # as-is for semver strings.
    return "pkg:cargo/{}@{}".format(name, version)


def normalize_license(value):
    if not value:
        return "NOASSERTION"
    return value.strip() or "NOASSERTION"


# packages[(name, version)] -> entry fields.
# `licenses` keeps the real license/repo when any source knows it; `found_in`
# lists every lockfile the package appears in, so a union entry says so.
packages = {}


def record(name, version, source, checksum, license_value, repository, lockfile):
    key = (name, version)
    entry = packages.get(key)
    if entry is None:
        entry = {
            "name": name,
            "version": version,
            "source": source,
            "checksum": checksum,
            "license": normalize_license(license_value),
            "repository": repository or "",
            "found_in": set(),
        }
        packages[key] = entry
    entry["found_in"].add(lockfile)
    # First non-empty value wins. `cargo metadata` is consulted before the raw
    # lockfiles precisely because it is the only one that carries license data,
    # and no lockfile entry can improve on it.
    if entry["license"] == "NOASSERTION" and license_value:
        entry["license"] = normalize_license(license_value)
    if not entry["repository"] and repository:
        entry["repository"] = repository
    if not entry["checksum"] and checksum:
        entry["checksum"] = checksum


# 1. cargo metadata (the root lockfile, resolved with every feature).
#    The workspace's own path members come along here too, with
#    source_path set instead of a registry source.
for package in metadata["packages"]:
    record(
        package["name"],
        package["version"],
        "registry" if package.get("source") else "local-workspace",
        "",
        package.get("license"),
        package.get("repository"),
        "Cargo.lock",
    )

# 2. The raw root lockfile: its `checksum` field is the content hash cargo
#    verified, which `cargo metadata` does not surface. Filling it in here is
#    what lets a consumer tie an SBOM entry to a specific .crate file.
for package in root_lock.get("package", []):
    if not package.get("source"):
        # Path member: already recorded by cargo metadata, no checksum exists.
        continue
    key = (package["name"], package["version"])
    if key in packages:
        packages[key]["checksum"] = package.get("checksum", "")

# 3. fuzz/Cargo.lock. Read directly — see the header for the measured reason
#    `cargo metadata --locked` cannot be used here.
for package in fuzz_lock.get("package", []):
    record(
        package["name"],
        package["version"],
        "registry" if package.get("source") else "local-workspace",
        package.get("checksum", ""),
        None,  # a lockfile has no license field
        None,
        "fuzz/Cargo.lock",
    )

# The workspace root itself, as the described package.
workspace_version = ""
for member in metadata.get("workspace_members", []):
    for package in metadata["packages"]:
        if package["id"] == member:
            workspace_version = package["version"]
            break
    if workspace_version:
        break

root_license = "MIT OR Apache-2.0"
for package in metadata["packages"]:
    if package["id"] in metadata.get("workspace_members", []) and package.get("license"):
        root_license = package["license"]
        break

document = {
    "spdxVersion": "SPDX-2.3",
    "dataLicense": "CC0-1.0",
    "SPDXID": "SPDXRef-DOCUMENT",
    "name": "webfang-{}".format(workspace_version or "workspace"),
    # Deterministic AND unique-per-document: the same (name, version, commit)
    # always yields the same namespace, and two different commits never collide.
    "documentNamespace": "https://github.com/XaviCode1000/webfang/spdx/webfang-{}-{}".format(
        workspace_version or "workspace", commit_sha
    ),
    "creationInfo": {
        # NOT `now()`: a re-run of this script over the same commit must be a
        # no-op diff, and a timestamp is the one field that could never be.
        "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(epoch)),
        "creators": [
            "Tool: generate-sbom.sh",
            "Organization: XaviCode1000",
        ],
    },
}

described_id = "SPDXRef-Package-webfang-workspace"
packages_out = [
    {
        "SPDXID": described_id,
        "name": "webfang",
        "versionInfo": workspace_version or "0.0.0",
        "downloadLocation": "NOASSERTION",
        "licenseConcluded": root_license,
        "licenseDeclared": root_license,
        "copyrightText": "Copyright (c) 2025 GazaDev",
        "comment": "The webfang workspace itself; the licence texts ship as LICENSE and LICENSE-APACHE.",
        "externalRefs": [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": "pkg:cargo/webfang@{}".format(workspace_version or "0.0.0"),
            }
        ],
    }
]

for (name, version) in sorted(packages):
    entry = packages[(name, version)]
    identifier = spdx_id(name, version)
    comment = "locked in: {}".format(", ".join(sorted(entry["found_in"])))
    if entry["license"] == "NOASSERTION":
        comment += (
            "; licenseDeclared is NOASSERTION because the only source that lists this "
            "package is a lockfile, which carries no licence field"
        )
    package = {
        "SPDXID": identifier,
        "name": name,
        "versionInfo": version,
        "downloadLocation": purl(name, version),
        "licenseConcluded": entry["license"],
        "licenseDeclared": entry["license"],
        "copyrightText": "NOASSERTION",
        "comment": comment,
        "externalRefs": [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": purl(name, version),
            }
        ],
    }
    if entry["repository"]:
        package["homepage"] = entry["repository"]
    if entry["checksum"]:
        package["checksums"] = [
            {
                "algorithm": "SHA256",
                "checksumValue": entry["checksum"],
            }
        ]
    packages_out.append(package)

document["packages"] = packages_out
document["relationships"] = [
    {
        "spdxElementId": "SPDXRef-DOCUMENT",
        "relationshipType": "DESCRIBES",
        "relatedSpdxElement": described_id,
    }
]

with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
    json.dump(document, handle, indent=2, sort_keys=True, ensure_ascii=True)
    handle.write("\n")

noassertion = sum(1 for p in packages_out if p["licenseDeclared"] == "NOASSERTION")
sys.stderr.write(
    "sbom: {} packages ({} from fuzz/Cargo.lock included, {} with NOASSERTION license)\n".format(
        len(packages_out),
        sum(1 for p in packages_out if "fuzz/Cargo.lock" in p.get("comment", "")),
        noassertion,
    )
)
PY

if [[ ! -s "$OUT" ]]; then
    echo "::error::'$OUT' salió vacío o inexistente: el SBOM no se generó."
    exit 1
fi

# Parse the result with a real JSON parser before declaring success. A file that
# is not valid JSON is not an SBOM, and shipping it would look like one.
if ! python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("packages") else 1)' "$OUT"; then
    echo "::error::'$OUT' no es un JSON válido con 'packages': el SBOM generado es inservible."
    exit 1
fi

echo "wrote $OUT ($(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["packages"]))' "$OUT") packages, SPDX 2.3, deterministic)"
