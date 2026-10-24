#!/usr/bin/env bash
# test_duplication.sh
#
# Semantics harness for scripts/check_duplication.sh (issue #1757, ODD
# `odd/tasks/duplication-arrange-helpers.md`, S4).
#
# The guard answers one question: is the tree's duplicated-lines count still at
# or below the committed ratchet? Before this harness, the guard could answer
# "no" to nothing: it translated "I did not measure" into "0 duplicated lines",
# and 0 is exactly the value that means "there is no duplication". A report
# without the rust section, a corpus with zero Rust files and a non-integer
# count all produced a GREEN gate over an unmeasured tree.
#
# WHAT THIS HARNESS PROVES, offline (mktemp sandboxes; no cargo, no network):
#
#   1. a report ABOVE the baseline FAILS, naming `Duplication INCREASED`, and
#      the very same report with the baseline RAISED PASSES (the positive
#      control: the guard decides, it does not always fail);
#   2. a report WITHOUT `statistics.formats.rust` FAILS (it used to exit 0);
#   3. a corpus with ZERO Rust files FAILS (it used to exit 0);
#   4. a detector that FAILS fails loudly and is NOT read as "0 duplicated";
#   5. a detector with the WRONG VERSION is refused;
#   6. a MISSING baseline fails;
#   7. a baseline WITHOUT the `duplicated_lines_rust` key — or that is not
#      JSON at all — fails with a message that names the cause (it used to
#      exit 1 through a Python traceback: right verdict, wrong diagnosis);
#   8. a corpus path that does not exist fails (already fail-closed; pinned).
#
# HERMETIC BY DESIGN. The detector is STUBBED on PATH: the CI job that runs
# this harness (`repo-guards`) does not install jscpd — it is installed in the
# `code-quality` job, which still runs the real ratchet against the real
# `crates/`. A harness that demanded the real binary would be red in the very
# job that is supposed to prove the guard can fail. The split is explicit:
# THIS harness proves the guard DECIDES; the ratchet in `code-quality` proves
# the real detector measures. A green harness is not a claim that the real
# detector finds duplicates.
#
# The guard is exercised as a COPY inside a sandbox (its ROOT is derived from
# its own location), so the harness never writes to this repository, and the
# meta-guard's mutation (the guard replaced by `exit 0`) reaches the copy: the
# harness goes RED on a broken guard.
#
# Usage: bash scripts/test_duplication.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check_duplication.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

ok() {
  printf '  ok   %-58s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-58s %s\n' "$1" "$2"
  if [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# --- the detector stub -------------------------------------------------------
#
# A stub, not a detector: it replays a canned report. It reproduces the two
# behaviours of the real binary the guard depends on — the `--version` line and
# the `<output-dir>/jscpd-report.json` file — and the one failure the real
# binary produces when the corpus path does not exist.

STUB_DIR="$WORK/bin"
mkdir -p "$STUB_DIR"
cat >"$STUB_DIR/jscpd" <<'STUB'
#!/usr/bin/env bash
# Stub of kucherenko/jscpd v5 5.4.0 for the duplication ratchet harness.
# Env: JSCPD_STUB_VERSION, JSCPD_STUB_EXIT, JSCPD_STUB_REPORT.
set -uo pipefail

if [ "${1:-}" = "--version" ]; then
  echo "${JSCPD_STUB_VERSION:-jscpd 5.4.0}"
  exit 0
fi

corpus="${1:-}"
outdir=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output" ]; then
    outdir="$arg"
  fi
  prev="$arg"
done

if [ -n "${JSCPD_STUB_EXIT:-}" ]; then
  echo "jscpd-stub: forced failure" >&2
  exit "$JSCPD_STUB_EXIT"
fi

if [ ! -d "$corpus" ]; then
  echo "Error: path does not exist: $corpus" >&2
  exit 1
fi

if [ -z "$outdir" ]; then
  echo "jscpd-stub: no --output directory given" >&2
  exit 2
fi

if [ ! -f "${JSCPD_STUB_REPORT:-}" ]; then
  echo "jscpd-stub: no report to replay" >&2
  exit 2
fi

cp "${JSCPD_STUB_REPORT}" "$outdir/jscpd-report.json"
exit 0
STUB
chmod +x "$STUB_DIR/jscpd"

# --- the report fixtures -----------------------------------------------------
#
# Shapes taken from the real jscpd v5 report: `statistics.formats.rust` is a
# FLAT object (clones, duplicatedLines, sources, ...) and `statistics.total`
# carries the same numbers for the whole corpus.

REPORTS="$WORK/reports"
mkdir -p "$REPORTS"

# rust_report <file> <duplicatedLines> <sources>
rust_report() {
  cat >"$1" <<EOF
{
  "duplicates": [],
  "statistics": {
    "detectionDate": "2026-01-01T00:00:00.000Z",
    "formats": {
      "rust": {
        "clones": 3,
        "duplicatedLines": $2,
        "duplicatedTokens": 120,
        "lines": 900,
        "newClones": 0,
        "newDuplicatedLines": 0,
        "percentage": 1.5,
        "percentageTokens": 2.0,
        "sources": $3,
        "tokens": 4000
      }
    },
    "total": {
      "clones": 3,
      "duplicatedLines": $2,
      "duplicatedTokens": 120,
      "lines": 900,
      "newClones": 0,
      "newDuplicatedLines": 0,
      "percentage": 1.5,
      "percentageTokens": 2.0,
      "sources": $3,
      "tokens": 4000
    }
  }
}
EOF
}

# A report that measured files but says nothing about Rust.
no_rust_report() {
  cat >"$1" <<'EOF'
{
  "duplicates": [],
  "statistics": {
    "detectionDate": "2026-01-01T00:00:00.000Z",
    "formats": {},
    "total": {
      "clones": 0,
      "duplicatedLines": 0,
      "duplicatedTokens": 0,
      "lines": 300,
      "newClones": 0,
      "newDuplicatedLines": 0,
      "percentage": 0.0,
      "percentageTokens": 0.0,
      "sources": 12,
      "tokens": 900
    }
  }
}
EOF
}

# The exact shape jscpd v5 writes for a corpus with no files at all.
empty_corpus_report() {
  cat >"$1" <<'EOF'
{
  "duplicates": [],
  "statistics": {
    "detectionDate": "2026-01-01T00:00:00.000Z",
    "formats": {},
    "total": {
      "clones": 0,
      "duplicatedLines": 0,
      "duplicatedTokens": 0,
      "lines": 0,
      "newClones": 0,
      "newDuplicatedLines": 0,
      "percentage": 0.0,
      "percentageTokens": 0.0,
      "sources": 0,
      "tokens": 0
    }
  }
}
EOF
}

# A count that is not a number of lines.
non_integer_report() {
  cat >"$1" <<'EOF'
{
  "duplicates": [],
  "statistics": {
    "detectionDate": "2026-01-01T00:00:00.000Z",
    "formats": {
      "rust": {
        "clones": 3,
        "duplicatedLines": "200",
        "duplicatedTokens": 120,
        "lines": 900,
        "newClones": 0,
        "newDuplicatedLines": 0,
        "percentage": 1.5,
        "percentageTokens": 2.0,
        "sources": 12,
        "tokens": 4000
      }
    },
    "total": {
      "clones": 3,
      "duplicatedLines": 0,
      "duplicatedTokens": 0,
      "lines": 900,
      "newClones": 0,
      "newDuplicatedLines": 0,
      "percentage": 0.0,
      "percentageTokens": 0.0,
      "sources": 12,
      "tokens": 4000
    }
  }
}
EOF
}

rust_report "$REPORTS/rust_200.json" 200 12
rust_report "$REPORTS/rust_50.json" 50 12
rust_report "$REPORTS/rust_0_one_file.json" 0 1
rust_report "$REPORTS/rust_0_no_file.json" 0 0
no_rust_report "$REPORTS/no_rust_section.json"
empty_corpus_report "$REPORTS/empty_corpus.json"
non_integer_report "$REPORTS/non_integer.json"

# --- the sandbox -------------------------------------------------------------
#
# The guard derives its ROOT from its own location, so a COPY of it inside a
# sandbox reads the sandbox's baseline and measures the sandbox's corpus. The
# repository is never written to.

OUT=""
RC=0

new_sandbox() { # <name> [no-corpus]
  local name="$1"
  local dir="$WORK/$name"
  mkdir -p "$dir/scripts"
  if [ "${2:-with-corpus}" = "with-corpus" ]; then
    mkdir -p "$dir/crates"
  fi
  cp "$GUARD" "$dir/scripts/check_duplication.sh"
  printf '%s\n' "$dir"
}

write_baseline() { # <dir> <value>
  cat >"$1/scripts/quality-baselines.json" <<EOF
{
  "description": "fixture",
  "updated": "2026-01-01",
  "duplicated_lines_rust": $2
}
EOF
}

write_baseline_without_key() { # <dir>
  cat >"$1/scripts/quality-baselines.json" <<'EOF'
{
  "description": "fixture without the ratchet key",
  "updated": "2026-01-01"
}
EOF
}

write_baseline_not_json() { # <dir>
  printf 'this is not json\n' >"$1/scripts/quality-baselines.json"
}

run_guard() { # <dir>
  OUT="$(cd "$1" && PATH="$STUB_DIR:$PATH" bash scripts/check_duplication.sh 2>&1)"
  RC=$?
}

expect_pass() { # <label> <dir>
  run_guard "$2"
  if [ "$RC" -eq 0 ]; then
    ok "$1" "rc=0"
  else
    bad "$1" "expected PASS, got rc=$RC" "$OUT"
  fi
}

expect_fail() { # <label> <dir> <must-contain> [must-not-contain]
  run_guard "$2"
  if [ "$RC" -eq 0 ]; then
    bad "$1" "expected FAIL, got rc=0" "$OUT"
    return
  fi
  if ! printf '%s' "$OUT" | grep -qF -- "$3"; then
    bad "$1" "the message does not name the cause ('$3')" "$OUT"
    return
  fi
  if [ -n "${4:-}" ] && printf '%s' "$OUT" | grep -qF -- "$4"; then
    bad "$1" "the output must not contain '$4'" "$OUT"
    return
  fi
  ok "$1" "rc=$RC"
}

echo "check_duplication semantics harness"

if [ ! -f "$GUARD" ]; then
  bad "the guard exists" "missing $GUARD"
  echo
  echo "PASS=$passed FAIL=$failed"
  exit 1
fi

# --- 1. duplication above the baseline fails ---------------------------------
# NEGATION: the RED the guard exists for — a tree that duplicated MORE than
# the committed ratchet must not pass.
D="$(new_sandbox increased)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/rust_200.json"
export JSCPD_STUB_VERSION="jscpd 5.4.0"
unset JSCPD_STUB_EXIT
expect_fail "duplication above the baseline fails" "$D" "Duplication INCREASED"

# NEGATION: the positive control of the case above — the SAME report with the
# baseline raised must pass, otherwise the guard would be failing for a reason
# unrelated to duplication.
D="$(new_sandbox increased_control)"
write_baseline "$D" 99999
expect_pass "the same report passes with a raised baseline" "$D"

# --- 2. a report without the rust section fails ------------------------------
# NEGATION: the parser turned a missing `statistics.formats.rust` into 0, and
# 0 is exactly "no duplication": the guard passed over an unmeasured tree.
D="$(new_sandbox no_rust_section)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/no_rust_section.json"
expect_fail "a report without the rust section fails" "$D" "statistics.formats.rust"

# NEGATION: the positive control of the case above — the same corpus, WITH the
# rust section, must pass.
D="$(new_sandbox no_rust_section_control)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/rust_50.json"
expect_pass "the same corpus with a rust section passes" "$D"

# --- 3. a corpus with zero Rust files fails ----------------------------------
# NEGATION: a corpus with no Rust files reported 0 duplicated lines and the
# guard exited 0 — a green semaphore wired to nothing.
D="$(new_sandbox empty_corpus)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/empty_corpus.json"
expect_fail "a corpus with zero Rust files fails" "$D" "0 ficheros"

# NEGATION: the same vacuity through the other shape — a rust section that
# measured no file at all.
D="$(new_sandbox empty_corpus_rust_section)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/rust_0_no_file.json"
expect_fail "a rust section without files fails" "$D" "0 ficheros"

# NEGATION: the positive control of the case above — ONE file measured, zero
# duplication, is a real measurement and must pass.
D="$(new_sandbox empty_corpus_control)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/rust_0_one_file.json"
expect_pass "a corpus with one measured file passes" "$D"

# --- 4. a detector that fails fails loudly -----------------------------------
# NEGATION: a detector that exits non-zero must not be read as "0 duplicated
# lines"; the guard already failed here, so this case PINS that behaviour.
D="$(new_sandbox detector_fails)"
write_baseline "$D" 100
export JSCPD_STUB_REPORT="$REPORTS/rust_50.json"
export JSCPD_STUB_EXIT=3
expect_fail "a failing detector fails the gate" "$D" "jscpd exited with code 3" "Duplicated lines (rust): 0"

# NEGATION: the positive control of the case above — the same guard with a
# detector that succeeds must pass.
D="$(new_sandbox detector_fails_control)"
write_baseline "$D" 100
unset JSCPD_STUB_EXIT
expect_pass "a detector that succeeds passes" "$D"

# --- 5. a detector with the wrong version is refused -------------------------
# NEGATION: jscpd-rs and kucherenko/jscpd install the same binary name and
# measure different scales; measuring with the wrong one false-greens.
D="$(new_sandbox wrong_version)"
write_baseline "$D" 100
export JSCPD_STUB_VERSION="jscpd 0.1.12"
expect_fail "a detector with the wrong version is refused" "$D" "5.4.0"

# NEGATION: the positive control of the case above — the pinned version must
# be accepted.
D="$(new_sandbox wrong_version_control)"
write_baseline "$D" 100
export JSCPD_STUB_VERSION="jscpd 5.4.0"
expect_pass "the pinned detector version is accepted" "$D"

# --- 6. a missing baseline fails ---------------------------------------------
# NEGATION: without a baseline there is no ratchet to compare against, and a
# gate without one must not be green.
D="$(new_sandbox missing_baseline)"
export JSCPD_STUB_REPORT="$REPORTS/rust_50.json"
expect_fail "a missing baseline fails" "$D" "Missing baseline file"

# NEGATION: the positive control of the case above — with the baseline back,
# the same report passes.
D="$(new_sandbox missing_baseline_control)"
write_baseline "$D" 100
expect_pass "the same report passes with the baseline back" "$D"

# --- 7. a broken baseline fails, naming the cause ----------------------------
# NEGATION: a baseline without the ratchet key used to exit 1 through a Python
# traceback — the right verdict, but the operator could not tell which cause.
D="$(new_sandbox baseline_without_key)"
write_baseline_without_key "$D"
expect_fail "a baseline without the ratchet key fails" "$D" "Missing baseline key 'duplicated_lines_rust'"

# NEGATION: the same class of silence with a different cause — a baseline that
# is not JSON at all.
D="$(new_sandbox baseline_not_json)"
write_baseline_not_json "$D"
expect_fail "a baseline that is not JSON fails" "$D" "not valid JSON"

# NEGATION: the positive control of the two cases above.
D="$(new_sandbox baseline_control)"
write_baseline "$D" 100
expect_pass "a well-formed baseline passes" "$D"

# --- 8. a corpus that does not exist fails -----------------------------------
# NEGATION: the real detector exits 1 on a missing path; the guard must not
# turn that into a measurement.
D="$(new_sandbox missing_corpus no-corpus)"
write_baseline "$D" 100
expect_fail "a corpus that does not exist fails" "$D" "jscpd exited with code 1"

# NEGATION: the positive control of the case above.
D="$(new_sandbox missing_corpus_control)"
write_baseline "$D" 100
expect_pass "an existing corpus passes" "$D"

echo
echo "check_duplication semantics harness: $passed passed, $failed failed"
echo "PASS=$passed FAIL=$failed"
[ "$failed" -eq 0 ]
