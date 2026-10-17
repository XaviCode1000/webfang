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

run_gate() {
  local dir="$1"
  ( cd "$dir" && bash scripts/check_docs_version_pins.sh >/dev/null 2>"$WORK/stderr" )
}

# ─── 1. consistent tree passes ───────────────────────────────────────────────
D="$(new_fixture clean 2.11.0)"
printf 'VERSION=v2.11.0\n' > "$D/README.md"
printf '# Chapter\n\nPin: v2.11.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  pass "árbol consistente pasa"
else
  fail "árbol consistente pasa ($(cat "$WORK/stderr"))"
fi

# ─── 2. stale pin in README fails ───────────────────────────────────────────
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
D="$(new_fixture marked 2.11.0)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\nObserved on v2.4.0. <!-- version-stale:allow -->\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  pass "historia intencional marcada pasa"
else
  fail "historia intencional marcada pasa ($(cat "$WORK/stderr"))"
fi

# ─── 5. marker on a DIFFERENT line does not excuse the pin ──────────────────
D="$(new_fixture marker-scope 2.11.0)"
printf 'ok\n' > "$D/README.md"
printf '# Chapter\n\n<!-- version-stale:allow -->\nVERSION=v2.4.0\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "el marker no excusa otra línea"
else
  pass "el marker no excusa otra línea"
fi

# ─── 6. missing version in the manifest fails closed ────────────────────────
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
D="$(new_fixture no-docs 2.11.0)"
rm -rf "$D/docs" "$D/README.md"
if run_gate "$D"; then
  fail "sin documentos falla cerrado"
else
  pass "sin documentos falla cerrado"
fi

# ─── 8. an unparseable version fails closed ─────────────────────────────────
D="$(new_fixture bad-version "two.eleven")"
printf 'ok\n' > "$D/README.md"
printf 'ok\n' > "$D/docs/src/a.md"
if run_gate "$D"; then
  fail "versión con formato inesperado falla cerrado"
else
  pass "versión con formato inesperado falla cerrado"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
