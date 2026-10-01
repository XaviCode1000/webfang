#!/usr/bin/env bash
# generate-third-party-notices.sh — the third-party notice bundle shipped with
# every release (issue #1607, SC-12).
#
# WHAT THIS IS, AND WHY IT IS A SEPARATE ARTIFACT FROM THE SBOM
# =============================================================
# The SBOM (scripts/generate-sbom.sh) is the machine-readable inventory: SPDX
# 2.3 JSON, one entry per package, with checksums, for tooling. This is the
# human-readable counterpart: for every crate in the inventory, the four facts a
# redistribution notice has to carry — CRATE, VERSION, LICENSE EXPRESSION,
# REPOSITORY — followed by the licence texts of the workspace itself, verbatim.
# Two formats, one derivation, so they cannot disagree about what was shipped:
# a consumer diffing the notice against the SBOM finds the same set.
#
# SCOPE, STATED PLAINLY
# ====================
# The inventory covers every crate locked in EITHER lockfile (Cargo.lock and
# fuzz/Cargo.lock — see the SBOM script for why the second one is invisible to a
# workspace walk). That is a SUPERSET of what links into any one binary: the
# release builds `-p webfang_cli --features "ai mcp"`, while the full lock also
# contains crates behind other features and other workspace members. A superset
# is the right direction for a notice bundle — it cannot under-declare — and the
# exact per-target inventory is the SBOM's job.
#
# DETERMINISM
# ===========
# Entries are sorted with `LC_ALL=C` byte collation, and NO timestamp is emitted.
# The document identifies itself by the workspace version and the commit it was
# generated from, both of which are properties of the source, so re-running this
# over the same commit is a no-op diff. That is what makes it meaningful to
# diff the regenerated bundle in review and to sign it as a release asset.
#
# USAGE
#   bash scripts/generate-third-party-notices.sh <output.txt>
#
# Fail-closed: missing cargo, missing python3, a missing LICENSE / LICENSE-APACHE,
# or a `--locked` resolution that wants to change Cargo.lock all FAIL. A notice
# bundle missing the Apache-2.0 text would be worse than none, because the
# package advertises `MIT OR Apache-2.0` and a consumer picking the Apache
# option must be able to read it.
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C
export PYTHONHASHSEED=0
export PYTHONDONTWRITEBYTECODE=1

usage() {
    cat >&2 <<'EOF'
usage: bash scripts/generate-third-party-notices.sh <output.txt>

Writes the third-party notice bundle: every crate in the workspace lockfile and
in fuzz/Cargo.lock with its version, SPDX license expression and repository,
followed by this workspace's own MIT and Apache-2.0 texts verbatim.
EOF
}

# `--help` must print usage and exit 0, not be taken as the output path.
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
MIT_TEXT="$ROOT/LICENSE"
APACHE_TEXT="$ROOT/LICENSE-APACHE"

if ! command -v cargo >/dev/null 2>&1; then
    echo "::error::cargo no está disponible y este script NO se salta: sin resolución no hay inventario, y un notice bundle sin inventario parece conforme."
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "::error::python3 no está disponible y este script NO se salta: es el escritor determinista del bundle."
    exit 1
fi

if ! python3 -c 'import tomllib' >/dev/null 2>&1; then
    echo "::error::el intérprete de python3 no trae 'tomllib' (necesario para leer fuzz/Cargo.lock): requiere Python 3.11 o superior. Este script NO se salta: omitir el lockfile de fuzz omitiría su inventario (SC-05/SC-12)."
    exit 1
fi

for required in "$ROOT_LOCK" "$FUZZ_LOCK" "$MIT_TEXT" "$APACHE_TEXT"; do
    if [[ ! -f "$required" ]]; then
        echo "::error::'$required' no existe. El notice bundle necesita los dos lockfiles y los DOS textos de licencia del workspace: el paquete declara 'MIT OR Apache-2.0' en Cargo.toml, así que publicar solo uno deja al consumidor sin la opción que él eligió (SC-13)."
        exit 1
    fi
done

COMMIT_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! cargo metadata --locked --format-version 1 --all-features \
        --manifest-path "$ROOT/Cargo.toml" >"$WORK/metadata.json"; then
    echo "::error::'cargo metadata --locked' falló sobre el lockfile del workspace. Sin --locked habría reescrito Cargo.lock y el bundle describiría una resolución distinta de la confirmada."
    exit 1
fi

python3 - "$WORK/metadata.json" "$ROOT_LOCK" "$FUZZ_LOCK" "$MIT_TEXT" "$APACHE_TEXT" "$OUT" "$COMMIT_SHA" <<'PY'
import json
import sys
import tomllib

metadata_path, root_lock_path, fuzz_lock_path, mit_path, apache_path, out_path, commit_sha = sys.argv[1:8]

with open(metadata_path, "rb") as handle:
    metadata = json.load(handle)
with open(root_lock_path, "rb") as handle:
    root_lock = tomllib.load(handle)
with open(fuzz_lock_path, "rb") as handle:
    fuzz_lock = tomllib.load(handle)
with open(mit_path, encoding="utf-8") as handle:
    mit_text = handle.read()
with open(apache_path, encoding="utf-8") as handle:
    apache_text = handle.read()

RULE = "=" * 78
THIN = "-" * 78

version = ""
for member in metadata.get("workspace_members", []):
    for package in metadata["packages"]:
        if package["id"] == member:
            version = package["version"]
            break
    if version:
        break

root_license = "MIT OR Apache-2.0"
for package in metadata["packages"]:
    if package["id"] in metadata.get("workspace_members", []) and package.get("license"):
        root_license = package["license"]
        break

# (name, version) -> {"license": str, "repository": str, "found_in": set}
inventory = {}


def record(name, ver, license_value, repository, lockfile):
    entry = inventory.setdefault(
        (name, ver),
        {"license": "", "repository": "", "found_in": set()},
    )
    entry["found_in"].add(lockfile)
    # `cargo metadata` is consulted first and is the only source with a licence
    # field, so first-non-empty is the right precedence.
    if not entry["license"] and license_value:
        entry["license"] = license_value.strip()
    if not entry["repository"] and repository:
        entry["repository"] = repository.strip()


for package in metadata["packages"]:
    record(
        package["name"],
        package["version"],
        package.get("license"),
        package.get("repository"),
        "Cargo.lock",
    )

# A lockfile entry whose package `cargo metadata` did not return must still be
# listed, or the bundle would under-declare relative to the SBOM. Two explicit
# loops rather than a concatenation with a membership test: `package in <list>`
# on a list of dicts is an O(n*m) equality comparison that would silently
# mis-attribute a lockfile label if two entries ever compared equal.
for package in root_lock.get("package", []):
    record(package["name"], package["version"], None, None, "Cargo.lock")
for package in fuzz_lock.get("package", []):
    record(package["name"], package["version"], None, None, "fuzz/Cargo.lock")

lines = []
append = lines.append

append(RULE)
append("WebFang — Third-Party Notices")
append(RULE)
append("")
append("webfang {}".format(version or "workspace"))
append("commit {}".format(commit_sha))
append("generated by scripts/generate-third-party-notices.sh")
append("")
append("THIS BUNDLE COVERS")
append(THIN)
append("Every crate locked in EITHER lockfile of this repository:")
append("  * Cargo.lock      — the workspace, resolved with all features.")
append("  * fuzz/Cargo.lock — the fuzz harness, which the workspace `exclude`")
append("    array keeps out of every workspace walk, so nothing else sees it.")
append("That is a superset of what links into any single binary. The release")
append("builds `-p webfang_cli --features \"ai mcp\"`; the full lock also contains")
append("crates behind other features and other workspace members. The exact,")
append("machine-readable per-package inventory — with checksums — is the")
append("accompanying SBOM (webfang.spdx.json, SPDX 2.3).")
append("")
append("The legally operative obligations (which of these licences require")
append("notice retention, and the static-link obligations of the C/C++ code")
append("compiled into the binaries) are stated in the repository's NOTICE file,")
append("which is shipped alongside this bundle.")
append("")
append("")
append(RULE)
append("1. LICENCES OF THE WORKSPACE ITSELF")
append(RULE)
append("")
append("webfang is distributed under the terms of: {}".format(root_license))
append("You may choose either. The full texts follow, verbatim.")
append("")
append("")
append(RULE)
append("1a. MIT License")
append(RULE)
append("")
append(mit_text.rstrip("\n"))
append("")
append("")
append(RULE)
append("1b. Apache License 2.0")
append(RULE)
append("")
append(apache_text.rstrip("\n"))
append("")
append("")
append(RULE)
append("2. THIRD-PARTY CRATES")
append(RULE)
append("")
append("{} distinct name/version pairs.".format(len(inventory)))
append("")
append(
    "{:<38} {:<20} {:<34} {}".format("CRATE", "VERSION", "LICENSE", "REPOSITORY")
)
append(THIN)

missing_license = 0
for name, ver in sorted(inventory):
    entry = inventory[(name, ver)]
    license_value = entry["license"] or "NOASSERTION"
    if not entry["license"]:
        missing_license += 1
    repository = entry["repository"] or "(not declared upstream)"
    append(
        "{:<38} {:<20} {:<34} {}".format(
            name[:38], ver[:20], license_value[:34], repository
        ).rstrip()
    )

append(THIN)
append("")
if missing_license:
    append(
        "{} of {} entries carry NOASSERTION: they appear only in a lockfile, which".format(
            missing_license, len(inventory)
        )
    )
    append("records name, version, source and checksum but no licence field. The")
    append("authoritative licence for those crates is the `license` field in their")
    append("published .crate manifest on crates.io. They are listed rather than")
    append("omitted: an inventory that quietly drops entries is an inventory that")
    append("cannot be compared against anything.")
    append("")
append("Reproduce this bundle with:")
append("  bash scripts/generate-third-party-notices.sh <output.txt>")
append("The output is a pure function of the commit: no timestamp, byte-collation")
append("sorting, no network access, and `--locked` on every cargo invocation.")
append("")

with open(out_path, "w", encoding="utf-8", newline="\n") as handle:
    handle.write("\n".join(lines))

sys.stderr.write(
    "notices: {} crate entries ({} with NOASSERTION)\n".format(len(inventory), missing_license)
)
PY

if [[ ! -s "$OUT" ]]; then
    echo "::error::'$OUT' salió vacío o inexistente: el notice bundle no se generó."
    exit 1
fi

# The bundle must actually CARRY both licence texts. A generator that silently
# skipped Apache-2.0 would produce a plausible-looking file that drops the option
# a consumer is entitled to.
for marker in "MIT License" "Apache License"; do
    if ! grep -qF "$marker" "$OUT"; then
        echo "::error::'$OUT' no contiene el texto de '$marker': el paquete declara 'MIT OR Apache-2.0' y ambos textos deben viajar con el binario (SC-13)."
        exit 1
    fi
done

echo "wrote $OUT ($(wc -l <"$OUT" | tr -d ' ') lines, MIT + Apache-2.0 texts verbatim, deterministic)"
