#!/usr/bin/env bash
# scripts/test_artifact_notices.sh — semantics harness for
# scripts/generate-third-party-notices.sh (audit B26).
#
# The bundle is a LEGAL document: a consumer reads the licence expression off
# it and decides which licence they are complying with. The audit found the
# table truncating the expression silently — "MIT OR Apache-2.0 WITH
# LLVM-exception" was printed as "MIT OR Apache-2.0 WITH LLVM-except", which
# is not the licence, and it disagreed with the SBOM the bundle claims to
# mirror.
#
# The contract pinned here:
#   - the licence expression is printed IN FULL, byte for byte as the SBOM
#     carries it;
#   - a cell that IS width-limited says so: it ends with an ellipsis;
#   - the generator still fails closed (missing prerequisites, empty output,
#     a bundle without both licence texts).
#
# Style: user-facing messages in Spanish (repo convention for scripts/);
# internal output in English.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/generate-third-party-notices.sh"

# Scratch lives on a real disk, never on the tmpfs of /tmp.
SCRATCH="${WEBSITE_TEST_SCRATCH:-$HOME/.cache/tmp/webfang-artifact-tests}"
mkdir -p "$SCRATCH" || exit 2
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs | ramfs)
    echo "refusing tmpfs scratch: $SCRATCH" >&2
    exit 2
    ;;
esac
WORKDIR="$(mktemp -d "$SCRATCH/notices.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

pass=0
fail=0

ok() {
  echo "  PASS  $1"
  pass=$((pass + 1))
}

bad() {
  echo "  FAIL  $1"
  fail=$((fail + 1))
}

LONG_NAME="superlongcratename_exceeding_the_notice_table_width_for_sure"
FULL_LICENSE="MIT OR Apache-2.0 WITH LLVM-exception"

# A repository fixture with the two lockfiles, the two licence texts and a
# cargo stub that answers `cargo metadata` with a fixture inventory.
make_fixture() { # <dir>
  local d="$1"
  mkdir -p "$d/scripts" "$d/fuzz" "$d/stub"
  cp "$SCRIPT" "$d/scripts/generate-third-party-notices.sh"
  # The real texts are long; the guard only looks for the two markers, and
  # the fixture carries them so the bundle passes the fail-closed check.
  printf 'MIT License\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software (fixture).\n' >"$d/LICENSE"
  printf 'Apache License\n\nVersion 2.0, January 2004\n\nLicensed under the Apache License, Version 2.0 (fixture).\n' >"$d/LICENSE-APACHE"
  cat >"$d/Cargo.lock" <<'LOCKEOF'
version = 4

[[package]]
name = "superlongcratename_exceeding_the_notice_table_width_for_sure"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
LOCKEOF
  cat >"$d/fuzz/Cargo.lock" <<'LOCKEOF'
version = 4

[[package]]
name = "fuzz-only-crate"
version = "0.0.1"
LOCKEOF
  cat >"$d/metadata.json" <<METAEOF
{"packages":[
 {"name":"$LONG_NAME","version":"1.2.3","id":"reg#1","license":"$FULL_LICENSE","repository":"https://example.invalid/repo","license_file":null,"description":null,"source":"registry","dependencies":[],"targets":[],"features":{}},
 {"name":"fuzz-only-crate","version":"0.0.1","id":"reg#2","license":"MIT","repository":"https://example.invalid/f","license_file":null,"description":null,"source":"registry","dependencies":[],"targets":[],"features":{}}
],"workspace_members":["reg#1"],"root":"reg#1"}
METAEOF
  cat >"$d/stub/cargo" <<'STUBEOF'
#!/usr/bin/env bash
case " $* " in
  *' metadata '*) cat "${FIXTURE_METADATA:?}" ;;
  *) echo "stub cargo: unsupported: $*" >&2; exit 99 ;;
esac
STUBEOF
  chmod +x "$d/stub/cargo"
}

echo "generate-third-party-notices.sh"

# --- the licence expression is never shortened ------------------------------
# NEGATION: before the fix, the table printed "MIT OR Apache-2.0 WITH
# LLVM-except" — a licence expression that is not the licence, and that
# disagrees with the SBOM the bundle claims to mirror. This case accepts it.
fixture="$WORKDIR/notices"
make_fixture "$fixture"
out_file="$WORKDIR/notices.txt"
(cd "$fixture" && PATH="$fixture/stub:$PATH" FIXTURE_METADATA="$fixture/metadata.json" bash scripts/generate-third-party-notices.sh "$out_file" >/dev/null 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && [ -s "$out_file" ]; then
  ok "the bundle is generated (rc=$rc)"
else
  bad "the bundle is generated (rc=$rc)"
fi

row="$(grep -E '^superlongcratename.*1\.2\.3' "$out_file" | head -1)"
if [ -n "$row" ]; then
  ok "the inventory holds the crate row"
else
  bad "the inventory holds the crate row"
fi

if printf '%s' "$row" | grep -qF "$FULL_LICENSE"; then
  ok "the licence expression is printed in full"
else
  bad "the licence expression is printed in full"
  printf '        row: %s\n' "$row"
fi

if printf '%s' "$row" | grep -qF 'WITH LLVM-except '; then
  bad "the licence expression is not silently shortened"
else
  ok "the licence expression is not silently shortened"
fi

if printf '%s' "$row" | grep -qF '…'; then
  ok "a width-limited cell announces the truncation with an ellipsis"
else
  bad "a width-limited cell announces the truncation with an ellipsis"
  printf '        row: %s\n' "$row"
fi

if grep -qF "$FULL_LICENSE" "$out_file" && [ "$(grep -cF "$FULL_LICENSE" "$out_file")" -ge 1 ]; then
  ok "the full licence expression appears in the bundle"
else
  bad "the full licence expression appears in the bundle"
fi

# --- the bundle still fails closed ------------------------------------------
fixture="$WORKDIR/notices-missing-lock"
make_fixture "$fixture"
rm -f "$fixture/fuzz/Cargo.lock"
(cd "$fixture" && PATH="$fixture/stub:$PATH" FIXTURE_METADATA="$fixture/metadata.json" bash scripts/generate-third-party-notices.sh "$WORKDIR/missing.txt" >/dev/null 2>&1)
rc=$?
if [ "$rc" -ne 0 ] && [ ! -s "$WORKDIR/missing.txt" ]; then
  ok "a missing lockfile fails closed and writes no bundle (rc=$rc)"
else
  bad "a missing lockfile fails closed and writes no bundle (rc=$rc)"
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "generate-third-party-notices.sh: $pass passed, 0 failed"
  exit 0
fi
echo "generate-third-party-notices.sh: $pass passed, $fail failed"
exit 1
