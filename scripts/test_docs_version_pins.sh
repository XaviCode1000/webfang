#!/usr/bin/env bash
# Semantics harness for the docs version-pin gate.
#
# Three properties are behavioral and cannot be proven by grepping the script:
#
#   1. it FAILS on the drift it exists to catch — a pin that contradicts the
#      workspace version. A drift check that passes on drifted docs is
#      indistinguishable from no check, and "no check" is the state that let
#      nine `v2.4.0` pins survive four minor releases;
#   2. it PASSES on a consistent tree, and on intentional history that says so
#      with the marker — so the gate demands correctness, not a hand-edited
#      allowlist;
#   3. it fails CLOSED on what it cannot read: no manifest, no version, no
#      documents. A guard that degrades to "skip" when blind reports safety it
#      never established.
#
# Everything runs against mktemp fixtures. No network, no mutation of the
# repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fixture <name> <version>: a minimal repo with the script under test.
new_fixture() {
  local dir="$WORK/$1" version="$2"
  mkdir -p "$dir/scripts" "$dir/docs/src"
  cp "$REPO_ROOT/scripts/check_docs_version_pins.sh" "$dir/scripts/"
  chmod +x "$dir/scripts/check_docs_version_pins.sh"
  cat > "$dir/Cargo.toml" <<EOF
[workspace]
members = []

[workspace.package]
version = "$version"
edition = "2021"
EOF
  printf '%s\n' "$dir"
}

# stdout is captured too: the success line is the only place the gate reports
# WHAT it checked, and case 11 asserts on it.
run_gate() {
  local dir="$1"
  ( cd "$dir" && bash scripts/check_docs_version_pins.sh >"$WORK/stdout" 2>"$WORK/stderr" )
}

# ─── 1. consistent tree passes ───────────────────────────────────────────────
# NEGATION: pre-fix there was no enforceable check at all — the pins were prose
# nothing read, and nine `v2.4.0` pins survived four minor releases.
D="$(new_fixture clean 2.11.0)"
printf 'VERSION=v2.11.0\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  pass "árbol consistente pasa"
else
  fail "árbol consistente pasa ($(cat "$WORK/stderr"))"
fi

# ─── 2. stale pin in README fails ───────────────────────────────────────────
# NEGATION: pre-fix a drifted pin in the narrative was read by nothing; the
# README could claim a version the workspace had long left behind.
D="$(new_fixture stale-readme 2.11.0)"
printf 'VERSION=v2.4.0\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "pin viejo en README falla"
else
  if grep -q 'README.md' "$WORK/stderr"; then
    pass "pin viejo en README falla y nombra el archivo"
  else
    fail "pin viejo en README nombra el archivo"
  fi
fi

# ─── 3. stale pin inside docs/src fails ─────────────────────────────────────
# NEGATION: pre-fix a drifted pin inside a chapter was read by nothing; the
# docs tree could carry a version the workspace had long left behind.
D="$(new_fixture stale-chapter 2.11.0)"
printf 'ok\n' > "$D/README.md"
cat > "$D/docs/src/a.md" <<'MD'
# Chapter

```bash
VERSION=v2.4.0
```
MD
if run_gate "$D"; then
  fail "pin viejo en docs/src falla"
else
  if grep -q 'docs/src/a.md' "$WORK/stderr"; then
    pass "pin viejo en docs/src falla y nombra el archivo"
  else
    fail "pin viejo en docs/src nombra el archivo"
  fi
fi

# ─── 4. intentional history with the marker passes ──────────────────────────
# NEGATION: pre-fix a stale pin had no sanctioned way out, so the only way to
# keep one was to switch the check off or hand-edit an allowlist; the marker
# keeps the gate demanding correctness instead.
D="$(new_fixture marked 2.11.0)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\nObserved on v2.4.0. <!-- version-stale:allow -->\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  pass "historia intencional marcada pasa"
else
  fail "historia intencional marcada pasa ($(cat "$WORK/stderr"))"
fi

# ─── 5. marker on a DIFFERENT line does not excuse the pin ──────────────────
# NEGATION: pre-fix no marker existed, so a stale pin could not be excused at
# all; the marker is scoped to its own line, and one sitting on another line
# must not excuse this pin.
D="$(new_fixture marker-scope 2.11.0)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\n<!-- version-stale:allow -->\nVERSION=v2.4.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "el marker no excusa otra línea"
else
  pass "el marker no excusa otra línea"
fi

# ─── 6. missing version in the manifest fails closed ────────────────────────
# NEGATION: pre-fix the guard degraded to "skip" when it could not read the
# version, reporting safety it never established.
D="$(new_fixture no-version 2.11.0)"
printf '[workspace]\nmembers = []\n' > "$D/Cargo.toml"
printf 'ok\n' > "$D/README.md"
printf 'ok\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "sin versión en el manifest falla cerrado"
else
  pass "sin versión en el manifest falla cerrado"
fi

# ─── 7. no documents at all fails closed ────────────────────────────────────
# NEGATION: pre-fix the guard degraded to "skip" when it found no document to
# read, reporting safety it never established.
D="$(new_fixture no-docs 2.11.0)"
rm -rf "$D/docs" "$D/README.md"
if run_gate "$D"; then
  fail "sin documentos falla cerrado"
else
  pass "sin documentos falla cerrado"
fi

# ─── 8. an unparseable version fails closed ─────────────────────────────────
# NEGATION: pre-fix the guard degraded to "skip" on a version it could not
# parse, reporting safety it never established.
D="$(new_fixture bad-version "two.eleven")"
printf 'ok\n' > "$D/README.md"
printf 'ok\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "versión con formato inesperado falla cerrado"
else
  pass "versión con formato inesperado falla cerrado"
fi

# ─── 9. a prerelease pin does not pass against a stable workspace ───────────
# The doc literal used to be extracted with a pattern that peeled the suffix,
# so `v2.11.0-rc.1` was compared as `v2.11.0` and matched a stable workspace.
# That is a false negative in the exact comparison this gate exists to make:
# the recipe would install a release candidate while claiming to be current.
# NEGATION: pre-fix the extracted pin lost its `-rc.N` suffix, so a release
# candidate was compared as a stable version and passed against a stable
# workspace.
D="$(new_fixture rc-pin-stable 2.11.0)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0-rc.1\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "pin prerelease contra workspace stable falla"
else
  pass "pin prerelease contra workspace stable falla"
fi

# ─── 10. a workspace on a prerelease is checkable at all ────────────────────
# During a stabilization window the manifest carries `-rc.N`: release.yml's
# preflight requires the tag to equal the root Cargo.toml version, so an RC tag
# cannot exist without one. The gate used to abort with "formato inesperado"
# and left the whole tree unchecked for the window it is most needed in.
# NEGATION: pre-fix a `-rc.N` manifest aborted the gate with "formato
# inesperado", so the whole tree went unchecked in the very window the gate
# exists for.
D="$(new_fixture rc-workspace 2.11.0-rc.1)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0-rc.1\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  pass "workspace prerelease es chequeable"
else
  fail "workspace prerelease es chequeable ($(cat "$WORK/stderr"))"
fi

# ─── 11. the success line reports the pins it read, not the file count ──────
# It printed the number of documents while asserting what they declare, and the
# counter that actually counted the pins was never read.
# NEGATION: pre-fix the success line reported the document count instead of
# the pins it had read, and the counter that counted the pins was never read.
D="$(new_fixture success-count 2.11.0)"
printf 'VERSION=v2.11.0\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0\nOtro pin: v2.11.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  if grep -q '^OK: 3 líneas' "$WORK/stdout"; then
    pass "la línea de éxito cuenta líneas con pines (3), no documentos (2)"
  else
    fail "la línea de éxito cuenta líneas con pines ($(cat "$WORK/stdout"))"
  fi
else
  fail "árbol consistente con tres pines pasa"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
