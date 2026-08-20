#!/usr/bin/env bash
# Ratchet gate for code duplication (issue #516).
# Migrated to jscpd-rs 0.1.12 (Rust, drop-in jscpd) — see migrate/jscpd-rs.
# Tool MSRV 1.93 > workspace 1.88: install as binary via
#   cargo install jscpd-rs --version 0.1.12 --locked
# (do NOT add to workspace Cargo.toml). Binary is `jscpd` (drop-in).
#
# Runs jscpd over crates/ and fails hard if the number of duplicated lines in
# Rust sources exceeds the committed baseline (scripts/quality-baselines.json).
# This is a DESCENDING ratchet: the baseline is only ever lowered as code is
# deduplicated; raising it requires explicitly bumping the JSON (and a reviewer).
#
# Usage: bash scripts/check_duplication.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE_FILE="$ROOT/scripts/quality-baselines.json"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

if [ ! -f "$BASELINE_FILE" ]; then
  echo "::error::Missing baseline file $BASELINE_FILE"
  exit 1
fi

BASELINE="$(python3 -c "import json;print(json.load(open('$BASELINE_FILE'))['duplicated_lines_rust'])")"

# jscpd-rs (Rust): binary `jscpd` must be pre-installed (no npx fallback).
if ! command -v jscpd >/dev/null 2>&1; then
  echo "::error::jscpd no encontrado. Instala con: cargo install jscpd-rs --version 0.1.12 --locked"
  exit 1
fi
JSCPD="jscpd"

echo "Running jscpd over crates/ (baseline duplicated-lines: $BASELINE)..."
# jscpd's json reporter writes to <output-dir>/jscpd-report.json (--output is a dir).
$JSCPD "$ROOT/crates/" --min-tokens 50 --silent --reporters json --output "$WORKDIR" 2>"$WORKDIR/jscpd.err" || true

# Parse the rust format's duplicated-lines count.
CURRENT="$(python3 - "$WORKDIR/jscpd-report.json" <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as fh:
        d = json.load(fh)
    rust = d.get("statistics", {}).get("formats", {}).get("rust", {})
    # jscpd (npm): rust.duplicatedLines | jscpd-rs: rust.total.duplicatedLines
    val = rust.get("duplicatedLines")
    if val is None:
        total = rust.get("total")
        if isinstance(total, dict):
            val = total.get("duplicatedLines", 0)
        else:
            val = 0
    print(int(val) if val is not None else 0)
except Exception:
    print("-1")
PY
)"

echo "Duplicated lines (rust): $CURRENT  (baseline: $BASELINE)"

if [ "$CURRENT" = "-1" ]; then
  echo "::error::jscpd failed to produce a report"
  sed -n '1,20p' "$WORKDIR/jscpd.err" 2>/dev/null || true
  exit 1
fi

if [ "$CURRENT" -gt "$BASELINE" ]; then
  echo "::error::Duplication INCREASED: $CURRENT > $BASELINE. Deduplicate before merging (see scripts/quality-baselines.json)."
  exit 1
fi

echo "OK: duplication at or below baseline."
