#!/usr/bin/env bash
# Semantics harness for the docs version-pin WRITER.
#
# Companion to scripts/test_docs_version_pins.sh, which pins the SEMANTICS of
# the READER (scripts/check_docs_version_pins.sh). This file pins the semantics
# of the WRITER (scripts/bump_docs_version_pins.sh), which is the piece that
# makes a Release PR executable without a human commit.
#
# Why this file exists: the gate compares and fails. It cannot repair, so every
# Release PR was structurally unexecutable until a human bumped the pins — and
# that human commit is ephemeral, because release-plz regenerates the Release
# PR branch on every push to `main`. The bump therefore belongs to the release
# flow, and the writer that applies it has to be trustworthy in the same way
# the gate is: it must rewrite exactly the stale pins, nothing else.
#
# The properties that cannot be proven by grepping the script:
#
#   1. it REWRITES a stale pin to the workspace version, in the README and in
#      docs/src/*.md — a writer that only reports drift leaves the Release PR
#      exactly as broken as it was;
#   2. it is the EXACT INVERSE of the gate: after it runs, the gate passes over
#      the same tree. That is the property that ties the two scripts together
#      (case 11), and it is the one that matters — the two must agree on the
#      marker, the file set and the version grammar, or the Release PR stays
#      red after the bump;
#   3. it RESPECTS the `<!-- version-stale:allow -->` marker, at line scope:
#      the marker on another line never excuses this one;
#   4. it is IDEMPOTENT: the release flow may run it on every push to `main`,
#      so a second run must change nothing and still exit 0;
#   5. it fails CLOSED on what it cannot decide: no manifest, no version, an
#      unexpected version format, no documents at all — and, above all, on an
#      AMBIGUOUS line (two different pins on one line), where rewriting would
#      mean inventing prose ("de v2.4.0 a v3.0.0" → "de v3.0.1 a v3.0.1").
#      A closed failure must leave the tree INTACT: half a rewrite is worse
#      than no rewrite, because the gate then reports drift nobody can explain.
#
# Everything runs against mktemp fixtures. No network, no mutation of the
# repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

WRITER="scripts/bump_docs_version_pins.sh"
READER="scripts/check_docs_version_pins.sh"

# A harness that cannot see its subject fails instead of passing: without this
# check the first `cp` below dies with a raw "No such file or directory" and
# the run says nothing about what was being tested.
if [[ ! -f "$REPO_ROOT/$WRITER" ]]; then
  echo "::error::el harness no tiene sujeto: falta $WRITER" >&2
  printf '\nharness: 0 ok, 1 fail\n'
  exit 1
fi

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fixture <name> <version>: a minimal repo with the writer under test AND
# the gate it must invert (case 11 runs the real gate over the same fixture).
new_fixture() {
  local dir="$WORK/$1" version="$2"
  mkdir -p "$dir/scripts" "$dir/docs/src"
  cp "$REPO_ROOT/$WRITER" "$dir/scripts/"
  cp "$REPO_ROOT/$READER" "$dir/scripts/"
  chmod +x "$dir/scripts/bump_docs_version_pins.sh" "$dir/scripts/check_docs_version_pins.sh"
  cat > "$dir/Cargo.toml" <<EOF
[workspace]
members = []

[workspace.package]
version = "$version"
edition = "2021"
EOF
  printf '%s\n' "$dir"
}

# run_writer <dir>: stdout and stderr are captured separately, because the
# success line is the only place the writer reports WHAT it changed.
run_writer() {
  local dir="$1"
  ( cd "$dir" && bash scripts/bump_docs_version_pins.sh >"$WORK/stdout" 2>"$WORK/stderr" )
}

# run_reader <dir>: the gate, over the same fixture the writer just touched.
run_reader() {
  local dir="$1"
  ( cd "$dir" && bash scripts/check_docs_version_pins.sh >"$WORK/reader_stdout" 2>"$WORK/reader_stderr" )
}

# ─── 1. stale pin in README.md is rewritten ──────────────────────────────────
# NEGATION: pre-fix the gate could only report the drift, so the Release PR
# needed a human commit — and release-plz deleted that commit on the next push
# to `main`, so the PR came back red every time.
D="$(new_fixture stale-readme 3.0.0)"
printf 'VERSION=v2.4.0\n' > "$D/README.md"
printf '# Capítulo\n\nPin: v3.0.0\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  if grep -q 'VERSION=v3.0.0' "$D/README.md"; then
    pass "el pin viejo de README.md se reescribe a la versión del workspace"
  else
    fail "el pin viejo de README.md se reescribe ($(cat "$D/README.md"))"
  fi
else
  fail "el pin viejo de README.md se reescribe ($(cat "$WORK/stderr"))"
fi

# ─── 1b. a MISSING docs tree is not a tree with no pins (audit B12) ───────────
# NEGATION: the writer's discovery was `find docs/src ... 2>/dev/null`: a
# missing tree yielded an empty list with no error, the non-empty guard still
# passed on README.md alone, and the writer rewrote ONLY README.md and
# reported success — every docs version pin left stale while the writer said
# it had done its job. This writer runs FIRST in the Release PR flow, so its
# blind spot hides the reader's too.
D="$(new_fixture missing-docs 3.0.0)"
rm -rf "$D/docs/src"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
if run_writer "$D"; then
  fail "el escritor se niega cuando falta el árbol docs/src (reportó éxito: $(cat "$WORK/stderr"))"
elif grep -q 'docs/src' "$WORK/stderr"; then
  pass "el escritor se niega cuando falta el árbol docs/src nombrando la ruta"
else
  fail "el escritor se niega cuando falta el árbol docs/src nombrando la ruta ($(cat "$WORK/stderr"))"
fi

# ─── 1c. a docs tree with no document is not a clean tree (audit B12) ────────
D="$(new_fixture empty-docs 3.0.0)"
rm -f "$D"/docs/src/*.md
printf 'VERSION=v3.0.0\n' > "$D/README.md"
if run_writer "$D"; then
  fail "el escritor se niega cuando el árbol docs/src no tiene ningún .md (reportó éxito: $(cat "$WORK/stderr"))"
elif grep -q 'docs/src' "$WORK/stderr"; then
  pass "el escritor se niega cuando el árbol docs/src no tiene ningún .md nombrando la ruta"
else
  fail "el escritor se niega cuando el árbol docs/src no tiene ningún .md nombrando la ruta ($(cat "$WORK/stderr"))"
fi

# ─── 2. stale pin inside docs/src/*.md is rewritten ──────────────────────────
# NEGATION: pre-fix the drift in the docs tree could not be repaired by
# anything: the narrative is the copy-paste surface, so a stale recipe there
# installs an old binary.
D="$(new_fixture stale-chapter 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
cat > "$D/docs/src/a.md" <<'MD'
# Capítulo

```bash
VERSION=v2.4.0
```
MD
if run_writer "$D"; then
  if grep -q 'VERSION=v3.0.0' "$D/docs/src/a.md"; then
    pass "el pin viejo de docs/src/*.md se reescribe"
  else
    fail "el pin viejo de docs/src/*.md se reescribe ($(cat "$D/docs/src/a.md"))"
  fi
else
  fail "el pin viejo de docs/src/*.md se reescribe ($(cat "$WORK/stderr"))"
fi

# ─── 3. a marked line is left alone ──────────────────────────────────────────
# NEGATION: pre-fix a marked line had no writer-side counterpart at all; a
# writer that rewrote history would destroy the very thing the marker exists
# to preserve ("observed on v2.11.0").
D="$(new_fixture marked 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
cat > "$D/docs/src/a.md" <<'MD'
# Capítulo

Salida observada para `v2.11.0`: <!-- version-stale:allow -->
MD
if run_writer "$D"; then
  if grep -q 'v2.11.0' "$D/docs/src/a.md"; then
    pass "una línea marcada con version-stale:allow no se toca"
  else
    fail "una línea marcada con version-stale:allow no se toca ($(cat "$D/docs/src/a.md"))"
  fi
else
  fail "una línea marcada con version-stale:allow no se toca ($(cat "$WORK/stderr"))"
fi

# ─── 4. the marker does not excuse ANOTHER line ──────────────────────────────
# NEGATION: pre-fix the marker had no defined scope on the writer side; a
# writer that treated it as a file-wide switch would rewrite the history it
# was supposed to protect.
D="$(new_fixture marker-scope 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
printf '# Capítulo\n\n<!-- version-stale:allow -->\nVERSION=v2.4.0\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  if grep -q 'VERSION=v3.0.0' "$D/docs/src/a.md" && grep -q 'version-stale:allow' "$D/docs/src/a.md"; then
    pass "el marker de otra línea no exime a la línea sin marker"
  else
    fail "el marker de otra línea no exime a la línea sin marker ($(cat "$D/docs/src/a.md"))"
  fi
else
  fail "el marker de otra línea no exime a la línea sin marker ($(cat "$WORK/stderr"))"
fi

# ─── 5. idempotence: the second run changes nothing ─────────────────────────
# The release flow re-runs the writer on every push to `main` (release-plz
# regenerates the Release PR branch each time), so the second run MUST be a
# no-op that still exits 0 — otherwise the flow would commit an empty change
# on every push.
# NEGATION: pre-fix the bump was a human commit, and the bot's regeneration
# deleted it; there was no idempotent writer to reapply it.
D="$(new_fixture idempotent 3.0.0)"
printf 'VERSION=v2.4.0\n' > "$D/README.md"
printf '# Capítulo\n\nPin: v2.4.0\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  cp "$D/README.md" "$WORK/readme-after-first"
  cp "$D/docs/src/a.md" "$WORK/chapter-after-first"
  if run_writer "$D"; then
    if cmp -s "$D/README.md" "$WORK/readme-after-first" \
      && cmp -s "$D/docs/src/a.md" "$WORK/chapter-after-first" \
      && grep -q 'sin cambios' "$WORK/stdout"; then
      pass "la segunda corrida no cambia nada y sale 0"
    else
      fail "la segunda corrida no cambia nada ($(cat "$WORK/stdout"))"
    fi
  else
    fail "la segunda corrida no cambia nada y sale 0 ($(cat "$WORK/stderr"))"
  fi
else
  fail "la segunda corrida no cambia nada y sale 0 ($(cat "$WORK/stderr"))"
fi

# ─── 6. a consistent tree is a no-op ─────────────────────────────────────────
# NEGATION: pre-fix nothing could tell "nothing to do" from "nothing done" —
# the Release PR would get an empty commit on every push.
D="$(new_fixture consistent 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
printf '# Capítulo\n\nPin: v3.0.0\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  if grep -q 'sin cambios' "$WORK/stdout" && grep -q 'VERSION=v3.0.0' "$D/README.md"; then
    pass "un árbol ya consistente no cambia nada"
  else
    fail "un árbol ya consistente no cambia nada ($(cat "$WORK/stdout"))"
  fi
else
  fail "un árbol ya consistente no cambia nada ($(cat "$WORK/stderr"))"
fi

# ─── 7. an ambiguous line fails closed and leaves the tree INTACT ───────────
# Two different pins on one line cannot be rewritten without inventing prose
# ("de v2.4.0 a v3.0.0" → "de v3.0.1 a v3.0.1"). The gate demands that such a
# line be marked as history; the writer refuses to guess, and says where.
# NEGATION: pre-fix a rewrite pass over such a line would corrupt the prose
# silently; failing closed with the tree intact is the only honest outcome.
D="$(new_fixture ambiguous 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
printf '# Capítulo\n\nDe v2.4.0 a v3.0.0 se migró la receta.\n' > "$D/docs/src/a.md"
cp "$D/docs/src/a.md" "$WORK/ambiguous-before"
if run_writer "$D"; then
  fail "una línea ambigua falla cerrado"
else
  if cmp -s "$D/docs/src/a.md" "$WORK/ambiguous-before" \
    && grep -q 'docs/src/a.md' "$WORK/stderr"; then
    pass "una línea ambigua falla cerrado y el árbol queda intacto"
  else
    fail "una línea ambigua falla cerrado y el árbol queda intacto ($(cat "$WORK/stderr"))"
  fi
fi

# ─── 8. unreadable inputs fail closed ────────────────────────────────────────
# NEGATION: pre-fix a writer that degraded to "skip" when it could not read
# the version would report a green bump over a tree it never checked — the
# Release PR would stay red with a green flow.
D="$(new_fixture no-manifest 3.0.0)"
rm -f "$D/Cargo.toml"
printf 'ok\n' > "$D/README.md"
if run_writer "$D"; then
  fail "sin manifest falla cerrado"
else
  pass "sin manifest falla cerrado"
fi

D="$(new_fixture no-version 3.0.0)"
printf '[workspace]\nmembers = []\n' > "$D/Cargo.toml"
printf 'ok\n' > "$D/README.md"
if run_writer "$D"; then
  fail "sin versión en el manifest falla cerrado"
else
  pass "sin versión en el manifest falla cerrado"
fi

D="$(new_fixture bad-version "three.zero.zero")"
printf 'ok\n' > "$D/README.md"
printf 'ok\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  fail "versión con formato inesperado falla cerrado"
else
  pass "versión con formato inesperado falla cerrado"
fi

# ─── 9. no documents at all fails closed ─────────────────────────────────────
# NEGATION: pre-fix a writer with nothing to rewrite reported success over a
# tree it never read.
D="$(new_fixture no-docs 3.0.0)"
rm -rf "$D/docs" "$D/README.md"
if run_writer "$D"; then
  fail "sin documentos falla cerrado"
else
  pass "sin documentos falla cerrado"
fi

# ─── 10. a prerelease workspace rewrites to the prerelease pin ───────────────
# release.yml's preflight requires the tag to equal the root Cargo.toml
# version, so a release-candidate window puts `-rc.N` in the manifest by
# construction. The writer must carry the suffix intact: dropping it would
# claim a stable release that does not exist yet.
# NEGATION: pre-fix a writer that dropped the suffix would install a stable
# pin during a release-candidate window — the exact claim the gate exists to
# prevent.
D="$(new_fixture rc-workspace 3.0.1-rc.1)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
printf '# Capítulo\n\nPin: v3.0.0\n' > "$D/docs/src/a.md"
if run_writer "$D"; then
  if grep -q 'VERSION=v3.0.1-rc.1' "$D/README.md"; then
    pass "un workspace X.Y.Z-rc.N reescribe a vX.Y.Z-rc.N (sufijo intacto)"
  else
    fail "un workspace X.Y.Z-rc.N reescribe a vX.Y.Z-rc.N ($(cat "$D/README.md"))"
  fi
else
  fail "un workspace X.Y.Z-rc.N reescribe a vX.Y.Z-rc.N ($(cat "$WORK/stderr"))"
fi

# ─── 11. THE INVERSE PROPERTY: after the writer, the gate passes ────────────
# This is the case that ties the two scripts together. The writer and the
# gate must agree on the file set, the marker and the version grammar; if they
# disagree, the Release PR is still red after the bump, and the flow would
# push a commit that does not fix anything.
D="$(new_fixture inverse 3.0.0)"
printf 'VERSION=v2.4.0\n' > "$D/README.md"
cat > "$D/docs/src/a.md" <<'MD'
# Capítulo

Pin: v2.4.0
Salida observada para `v2.11.0`: <!-- version-stale:allow -->
MD
if run_writer "$D"; then
  if run_reader "$D"; then
    pass "tras el escritor, el gate pasa sobre el mismo árbol"
  else
    fail "tras el escritor, el gate pasa sobre el mismo árbol ($(cat "$WORK/reader_stderr"))"
  fi
else
  fail "tras el escritor, el gate pasa sobre el mismo árbol ($(cat "$WORK/stderr"))"
fi

# ─── 12. the writer touches nothing outside README.md + docs/src/*.md ────────
# CHANGELOG.md is written by release-plz alone (AGENTS.md § "CHANGELOG
# policy"); a writer that touched it would race the single writer of that
# file on every release.
# NEGATION: pre-fix nothing constrained the writer's file set, so a rewrite
# pass over "the documentation" could reach CHANGELOG.md and fight release-plz
# for the same file.
D="$(new_fixture foreign-file 3.0.0)"
printf 'VERSION=v3.0.0\n' > "$D/README.md"
printf 'ok\n' > "$D/docs/src/a.md"
printf '## [Unreleased]\n\n- fijó v2.4.0\n' > "$D/CHANGELOG.md"
cp "$D/CHANGELOG.md" "$WORK/changelog-before"
if run_writer "$D"; then
  if cmp -s "$D/CHANGELOG.md" "$WORK/changelog-before"; then
    pass "el escritor no toca un archivo ajeno (CHANGELOG.md queda intacto)"
  else
    fail "el escritor no toca un archivo ajeno ($(cat "$D/CHANGELOG.md"))"
  fi
else
  fail "el escritor no toca un archivo ajeno ($(cat "$WORK/stderr"))"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
