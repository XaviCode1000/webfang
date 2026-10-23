#!/usr/bin/env bash
# Ratchet gate for code duplication (issue #516).
# Uses kucherenko/jscpd v5 5.4.0 (pinned) — see issue #1867.
# Tool MSRV 1.96 > workspace 1.88: install as binary via
#   cargo install jscpd --version 5.4.0 --locked
# (do NOT add to workspace Cargo.toml; binary-only forever). Binary is `jscpd`.
# NOTE: both jscpd-rs and kucherenko/jscpd install a binary named `jscpd`:
# CI must `cargo uninstall jscpd-rs || true` before installing v5.
#
# Runs jscpd over crates/ and fails hard if the number of duplicated lines in
# Rust sources exceeds the committed baseline (scripts/quality-baselines.json).
# This is a DESCENDING ratchet: the baseline is only ever lowered as code is
# deduplicated; raising it requires explicitly bumping the JSON (and a reviewer).
#
# FAIL-CLOSED READING (issue #1757, ODD `odd/tasks/duplication-arrange-helpers.md`,
# S4). This guard used to translate "I did not measure" into "0 duplicated
# lines", and 0 is exactly the value that means "there is no duplication":
#
#   - a report without the `statistics.formats.rust` section read as 0;
#   - a corpus with zero Rust files read as 0;
#   - a non-integer count was truncated to an integer.
#
# Each of those is a green gate over an unmeasured tree. Every one of them now
# FAILS with a message that names its cause, and the three causes stay
# distinguishable from "the detector failed" (a non-zero jscpd exit), which was
# already fail-closed.
#
# Usage: bash scripts/check_duplication.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE_FILE="$ROOT/scripts/quality-baselines.json"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

# --- the baseline ------------------------------------------------------------
#
# The baseline is a hand-written number, so reading it can fail in three
# distinguishable ways: the file is not there, the file is not JSON, the file
# is JSON without the key. The last two used to surface as a Python
# traceback; the verdict was right (exit 1) but the diagnosis was not. Every
# branch below names its cause.
read_baseline() {
  python3 - "$BASELINE_FILE" <<'PY'
import json, sys

path = sys.argv[1]


def emit(status, value=""):
    print("%s\t%s" % (status, value))


try:
    with open(path) as fh:
        raw = fh.read()
except FileNotFoundError:
    emit("MISSING_FILE", path)
    sys.exit(0)
except OSError as exc:
    emit("UNREADABLE", str(exc))
    sys.exit(0)

try:
    data = json.loads(raw)
except ValueError as exc:
    emit("BAD_JSON", str(exc))
    sys.exit(0)

if not isinstance(data, dict) or "duplicated_lines_rust" not in data:
    emit("MISSING_KEY", "duplicated_lines_rust")
    sys.exit(0)

value = data["duplicated_lines_rust"]
if isinstance(value, bool) or not isinstance(value, int):
    emit("NOT_AN_INTEGER", repr(value))
    sys.exit(0)

emit("OK", str(value))
PY
}

BASELINE_RAW="$(read_baseline)"
BASELINE_STATUS="${BASELINE_RAW%%$'\t'*}"
BASELINE_VALUE="${BASELINE_RAW#*$'\t'}"
case "$BASELINE_STATUS" in
  OK)
    BASELINE="$BASELINE_VALUE"
    ;;
  MISSING_FILE)
    echo "::error::Missing baseline file $BASELINE_FILE"
    exit 1
    ;;
  UNREADABLE)
    echo "::error::Cannot read baseline file $BASELINE_FILE: $BASELINE_VALUE"
    exit 1
    ;;
  BAD_JSON)
    echo "::error::Baseline file $BASELINE_FILE is not valid JSON: $BASELINE_VALUE"
    exit 1
    ;;
  MISSING_KEY)
    echo "::error::Missing baseline key 'duplicated_lines_rust' in $BASELINE_FILE"
    exit 1
    ;;
  NOT_AN_INTEGER)
    echo "::error::Baseline key 'duplicated_lines_rust' is not an integer in $BASELINE_FILE: $BASELINE_VALUE"
    exit 1
    ;;
  *)
    echo "::error::Cannot parse baseline file $BASELINE_FILE"
    exit 1
    ;;
esac

# kucherenko/jscpd v5: binary `jscpd` must be pre-installed (no npx fallback).
if ! command -v jscpd >/dev/null 2>&1; then
  echo "::error::jscpd no encontrado. Instala con: cargo install jscpd --version 5.4.0 --locked"
  exit 1
fi
JSCPD="jscpd"

# Detector identity pin (#1893): jscpd-rs and kucherenko/jscpd install the
# same binary name, and measuring with the wrong detector false-greens
# (jscpd-rs reports ~7.6k lines against a v5 ~13k baseline, always under it).
# Refuse loudly instead. Exact match is deliberate: any detector bump is a
# conscious PR, not silent drift.
JSCPD_EXPECTED_VERSION="jscpd 5.4.0"
JSCPD_VERSION="$($JSCPD --version 2>/dev/null | head -1 || true)"
if [ "$JSCPD_VERSION" != "$JSCPD_EXPECTED_VERSION" ]; then
  echo "::error::jscpd es '$JSCPD_VERSION', se esperaba '$JSCPD_EXPECTED_VERSION' (kucherenko/jscpd v5). Instala con: cargo +stable install jscpd --version 5.4.0 --locked (toolchain >= 1.96; el workspace sigue en 1.88, binario solamente)"
  exit 1
fi

echo "Running jscpd over crates/ (baseline duplicated-lines: $BASELINE)..."
# jscpd's json reporter writes to <output-dir>/jscpd-report.json (--output is a dir).
# A non-zero jscpd exit fails the gate (no `|| true` swallow).
JSCPD_EXIT=0
$JSCPD "$ROOT/crates/" --min-tokens 50 --format rust --mode mild --silent --reporters json --output "$WORKDIR" 2>"$WORKDIR/jscpd.err" || JSCPD_EXIT=$?
if [ "$JSCPD_EXIT" -ne 0 ]; then
  echo "::error::jscpd exited with code $JSCPD_EXIT"
  sed -n '1,20p' "$WORKDIR/jscpd.err" 2>/dev/null || true
  exit 1
fi

# --- the measurement ---------------------------------------------------------
#
# The report is the only evidence this gate has, so "the report does not carry
# the measurement" is a FAILURE, never a zero. The reader distinguishes:
#
#   EMPTY_CORPUS    the detector measured zero files (nothing to decide about)
#   NO_RUST_SECTION the detector reported, but not about Rust
#   MISSING_COUNT   the Rust section exists but carries no count
#   NOT_AN_INTEGER  the count is not a number of lines
#   NO_REPORT/BAD_REPORT/NO_STATISTICS  the report itself is unusable
#
# Only OK carries a value; every other status exits non-zero with its own
# message, so a failing cause is never reported as another one.
read_measurement() {
  python3 - "$WORKDIR/jscpd-report.json" <<'PY'
import json, sys

path = sys.argv[1]


def emit(status, value=""):
    print("%s\t%s" % (status, value))


def as_int(value):
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    return value


try:
    with open(path) as fh:
        data = json.load(fh)
except FileNotFoundError:
    emit("NO_REPORT", path)
    sys.exit(0)
except ValueError as exc:
    emit("BAD_REPORT", str(exc))
    sys.exit(0)
except OSError as exc:
    emit("UNREADABLE_REPORT", str(exc))
    sys.exit(0)

if not isinstance(data, dict):
    emit("BAD_REPORT", "the report is not a JSON object")
    sys.exit(0)

statistics = data.get("statistics")
if not isinstance(statistics, dict):
    emit("NO_STATISTICS", "")
    sys.exit(0)

total = statistics.get("total")
total = total if isinstance(total, dict) else {}
total_sources = as_int(total.get("sources")) or 0

formats = statistics.get("formats")
if not isinstance(formats, dict) or not isinstance(formats.get("rust"), dict):
    # Nothing was measured at all, or the detector reported about another
    # format: both are "no measurement", and the two are told apart so the
    # operator knows which one happened.
    if total_sources == 0:
        emit("EMPTY_CORPUS", "0")
    else:
        emit("NO_RUST_SECTION", "")
    sys.exit(0)

rust = formats["rust"]
rust_sources = as_int(rust.get("sources"))
if rust_sources is None:
    rust_sources = total_sources

# A Rust section that measured zero files is an empty corpus, not "no
# duplication": the detector never looked at any Rust source.
if rust_sources == 0:
    emit("EMPTY_CORPUS", "0")
    sys.exit(0)

if "duplicatedLines" not in rust:
    emit("MISSING_COUNT", "")
    sys.exit(0)

value = rust["duplicatedLines"]
if isinstance(value, bool) or not isinstance(value, int):
    emit("NOT_AN_INTEGER", repr(value))
    sys.exit(0)

emit("OK", str(value))
PY
}

CURRENT_RAW="$(read_measurement)"
CURRENT_STATUS="${CURRENT_RAW%%$'\t'*}"
CURRENT_VALUE="${CURRENT_RAW#*$'\t'}"
case "$CURRENT_STATUS" in
  OK)
    CURRENT="$CURRENT_VALUE"
    ;;
  NO_REPORT)
    echo "::error::jscpd no produjo ningún informe ($CURRENT_VALUE): no hay medición sobre la que decidir."
    exit 1
    ;;
  BAD_REPORT)
    echo "::error::El informe de jscpd no es un informe válido: $CURRENT_VALUE"
    exit 1
    ;;
  UNREADABLE_REPORT)
    echo "::error::No se puede leer el informe de jscpd: $CURRENT_VALUE"
    exit 1
    ;;
  NO_STATISTICS)
    echo "::error::El informe de jscpd no trae la sección 'statistics': el detector no produjo un informe de Rust."
    exit 1
    ;;
  NO_RUST_SECTION)
    echo "::error::El informe de jscpd no trae la sección 'statistics.formats.rust': el detector no produjo un informe de Rust (medición no realizada)."
    exit 1
    ;;
  EMPTY_CORPUS)
    echo "::error::jscpd no midió ningún fichero Rust (corpus con 0 ficheros): no hay medición sobre la que decidir."
    exit 1
    ;;
  MISSING_COUNT)
    echo "::error::El informe de jscpd no trae 'statistics.formats.rust.duplicatedLines': no hay número que comparar con el baseline."
    exit 1
    ;;
  NOT_AN_INTEGER)
    echo "::error::'statistics.formats.rust.duplicatedLines' no es un número entero de líneas ($CURRENT_VALUE): la medición es inválida."
    exit 1
    ;;
  *)
    echo "::error::No se puede leer el informe de jscpd"
    exit 1
    ;;
esac

echo "Duplicated lines (rust): $CURRENT  (baseline: $BASELINE)"

if [ "$CURRENT" -gt "$BASELINE" ]; then
  echo "::error::Duplication INCREASED: $CURRENT > $BASELINE. Deduplicate before merging (see scripts/quality-baselines.json)."
  exit 1
fi

echo "OK: duplication at or below baseline."
