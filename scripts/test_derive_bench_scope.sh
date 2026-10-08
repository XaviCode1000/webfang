#!/usr/bin/env bash
set -euo pipefail
# derive-bench-scope.py — semantics harness. RED before GREEN.
#
# This script decides which crates the nightly `Benches` workflow compiles and
# measures. `cargo bench -p <pkg>` is an allow-list, not a filter, so anything
# it leaves out is neither built nor run, `cargo` still exits 0, and the nightly
# reports green with the coverage silently gone. That failure mode is invisible
# by construction, which is exactly why it needs pinned assertions.
#
# The behaviors pinned here, each of which was a real defect or a real claim:
#   1. the real repository derives a single line of `-p <pkg>` tokens, and the
#      same tree derives the SAME scope on every run;
#   2. a crate with an explicit `[[bench]]` table IS included;
#   3. a crate whose bench targets are AUTODISCOVERED (`benches/*.rs`, no
#      `[[bench]]` table) IS included — the version that parsed manifests for
#      `[[bench]]` silently skipped it;
#   4. a crate with `autobenches = false` is EXCLUDED, because cargo does not
#      bench it, so scoping it in would build nothing useful;
#   5. the emitted package name comes from cargo, not from a `[[bench]]` entry's
#      own `name` key, which names the TARGET and would be the wrong token;
#   6. a workspace member with NO bench target at all is EXCLUDED, so a helper
#      that emitted every package would be caught;
#   7. an empty scope FAILS CLOSED with exit 1 and EMPTY stdout, so the caller's
#      `scope="$(...)"` can never degrade into an unscoped full-workspace build;
#   8. an unresolvable workspace FAILS CLOSED with EMPTY stdout and an explained
#      message on stderr rather than a Python traceback, because the first hard
#      stop this step ever produces must be the diagnosable one.
#
# The assertions pin observable behavior — exit status, stdout, stderr
# emptiness — rather than the derive script's exact wording, so a reworded
# message does not read as a scope-derivation regression.
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures live in a mktemp tree and are dependency-free, so `cargo metadata`
# resolves them offline and no repository file is written. One check is the
# exception and says so where it runs: check 1 derives the REAL repository, so
# it reads the live workspace and touches whatever `cargo metadata` does there.
# It is run first, while nothing else has run, so any effect it has is isolated
# to that one step.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DERIVE="$SCRIPT_DIR/derive-bench-scope.py"

fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$DERIVE" ] || { echo "FAIL: derive script not found at $DERIVE"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Build a dependency-free workspace under $1 and print its root.
# Every crate gets a src/lib.rs so it is a valid package on its own.
make_workspace() {
  local root="$1"; shift
  mkdir -p "$root/scripts"
  cp "$DERIVE" "$root/scripts/"
  local members="" crate
  for crate in "$@"; do
    mkdir -p "$root/crates/$crate/src"
    printf 'pub fn placeholder() {}\n' > "$root/crates/$crate/src/lib.rs"
    # A valid, bench-free package by default; individual checks overwrite it.
    cat > "$root/crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"
EOF
    if [ -z "$members" ]; then
      members="\"crates/$crate\""
    else
      members="$members, \"crates/$crate\""
    fi
  done
  {
    echo '[workspace]'
    echo "members = [$members]"
    echo 'resolver = "2"'
  } > "$root/Cargo.toml"
  echo "$root"
}

# Run the derive script; stdout in $OUT, stderr in $ERR, status in $STATUS.
# `--offline` and `--locked` keep a fixture run from reaching the registry or
# repairing a lockfile. They are passed through to `cargo metadata`, which
# accepts both; check 1 runs in the real repository where the committed lock is
# authoritative, and the fixture runs need no network at all.
run_derive() {
  local root="$1"
  set +e
  OUT="$(cd "$root" && CARGO_NET_OFFLINE=true python3 scripts/derive-bench-scope.py \
    2>"$WORK/stderr")"
  STATUS=$?
  set -e
  ERR="$(cat "$WORK/stderr")"
}

# ---------------------------------------------------------------------------
# 1. the real repository (the one non-hermetic check; see the header)
# ---------------------------------------------------------------------------
echo "== real repository =="
run_derive "$REPO_ROOT"
if [ "$STATUS" -ne 0 ]; then
  bad "1. real repo exits 0 (got $STATUS: $ERR)"
elif [ "$(printf '%s\n' "$OUT" | wc -l)" -ne 1 ]; then
  bad "1. real repo emits EXACTLY ONE line (got: $OUT)"
elif ! printf '%s' "$OUT" | grep -Eq '^([[:space:]]*-p[[:space:]]+[A-Za-z0-9_-]+)+$'; then
  bad "1. real repo emits only '-p <pkg>' tokens (got: $OUT)"
elif ! printf '%s' "$OUT" | grep -q -- '-p webfang_core'; then
  bad "1. real repo includes webfang_core (got: $OUT)"
elif ! printf '%s' "$OUT" | grep -q -- '-p webfang_ai'; then
  bad "1. real repo includes webfang_ai (got: $OUT)"
else
  ok "1. real repo derives a single line of -p tokens"
fi

# Determinism: the same tree must derive the same scope every time, or the
# nightly's allow-list churns for no reason. Proved, not assumed.
run_derive "$REPO_ROOT"
FIRST="$OUT"
run_derive "$REPO_ROOT"
if [ "$OUT" != "$FIRST" ]; then
  bad "1b. derivation is deterministic across runs (first: $FIRST / now: $OUT)"
else
  ok "1b. derivation is deterministic across three runs"
fi

# ---------------------------------------------------------------------------
# 2, 3, 4, 5. which crates belong in the scope
# ---------------------------------------------------------------------------
echo "== scope membership =="
ROOT="$(make_workspace "$WORK/members" declared autodiscovered opted_out benchless)"

# declared: an explicit [[bench]] table. Its bench `name` deliberately differs
# from the package name, so check 5 has something to catch.
#
# The explicit `path` matters: without it cargo infers `benches/<name>.rs`, finds
# no such file, and falls back to autodiscovery — which silently renames the
# target after the FILE. Verified: with `path` omitted, cargo reports the bench
# target as "declared", the same as the package, and check 5 becomes unpinnable.
mkdir -p "$ROOT/crates/declared/benches"
cat > "$ROOT/crates/declared/Cargo.toml" <<'EOF'
[package]
name = "declared"
version = "0.1.0"
edition = "2021"

[[bench]]
name = "totally_different_target_name"
path = "benches/declared.rs"
harness = false
EOF
printf 'fn main() {}\n' > "$ROOT/crates/declared/benches/declared.rs"

# autodiscovered: benches/*.rs and NO [[bench]] table.
mkdir -p "$ROOT/crates/autodiscovered/benches"
cat > "$ROOT/crates/autodiscovered/Cargo.toml" <<'EOF'
[package]
name = "autodiscovered"
version = "0.1.0"
edition = "2021"
EOF
printf 'fn main() {}\n' > "$ROOT/crates/autodiscovered/benches/picked_up.rs"

# opted_out: autobenches = false, so cargo does not bench it.
mkdir -p "$ROOT/crates/opted_out/benches"
cat > "$ROOT/crates/opted_out/Cargo.toml" <<'EOF'
[package]
name = "opted_out"
version = "0.1.0"
edition = "2021"
autobenches = false
EOF
printf 'fn main() {}\n' > "$ROOT/crates/opted_out/benches/suppressed.rs"

# benchless: a valid workspace member with no bench target at all. Without it,
# nothing here would catch an implementation that emitted EVERY package, which
# would inflate the nightly scope with crates that can never be measured.
run_derive "$ROOT"
if [ "$STATUS" -ne 0 ]; then
  bad "2-5. membership fixture derives (got $STATUS: $ERR)"
else
  ok "2-6. membership fixture derives cleanly"
  if printf '%s' "$OUT" | grep -q -- '-p declared'; then
    ok "2. explicit [[bench]] crate is in scope"
  else
    bad "2. explicit [[bench]] crate is in scope (got: $OUT)"
  fi
  if printf '%s' "$OUT" | grep -q -- '-p autodiscovered'; then
    ok "3. AUTODISCOVERED crate is in scope (the manifest-parsing version missed this)"
  else
    bad "3. AUTODISCOVERED crate is in scope (got: $OUT)"
  fi
  if printf '%s' "$OUT" | grep -q -- '-p opted_out'; then
    bad "4. autobenches=false crate leaked into scope (scope: $OUT)"
  else
    ok "4. autobenches=false crate stays out of scope"
  fi
  if printf '%s' "$OUT" | grep -q -- 'totally_different_target_name'; then
    bad "5. emitted a [[bench]] TARGET name instead of a package name (scope: $OUT)"
  else
    ok "5. emits the package name, not the [[bench]] target name"
  fi
  if printf '%s' "$OUT" | grep -q -- '-p benchless'; then
    bad "6. a crate with no bench targets leaked into scope (scope: $OUT)"
  else
    ok "6. a crate with no bench targets stays out of scope"
  fi
fi

# ---------------------------------------------------------------------------
# 6. empty scope fails closed, with EMPTY stdout
# ---------------------------------------------------------------------------
echo "== fail-closed: nothing to bench =="
ROOT="$(make_workspace "$WORK/empty" plain)"
run_derive "$ROOT"
if [ "$STATUS" -eq 0 ]; then
  bad "7. empty scope exits non-zero (got 0: $OUT)"
elif [ -n "$OUT" ]; then
  bad "7. empty scope writes NOTHING to stdout (got: $OUT)"
elif [ -z "$ERR" ]; then
  bad "7. empty scope explains itself on stderr (stderr was empty)"
else
  ok "7. empty scope fails closed, empty stdout, explained"
fi

# ---------------------------------------------------------------------------
# 7. an unresolvable workspace explains itself instead of tracebacking
# ---------------------------------------------------------------------------
echo "== fail-closed: unresolvable workspace =="
ROOT="$(make_workspace "$WORK/broken" broken)"
# Remove the only lib target, leaving a package with no targets at all.
# `cargo metadata` rejects that, which is the failure this check pins: the step
# must explain itself rather than surface a Python traceback.
rm -f "$ROOT/crates/broken/src/lib.rs"
run_derive "$ROOT"
if [ "$STATUS" -eq 0 ]; then
  bad "8. unresolvable workspace exits non-zero (got 0)"
elif printf '%s' "$ERR" | grep -q 'Traceback'; then
  bad "8. unresolvable workspace reports explained, not a traceback (got: $ERR)"
elif [ -n "$OUT" ]; then
  # The same caller-side hazard check 6 pins: a partial token list plus a
  # failure would still let `scope="$(...)"` reach cargo bench.
  bad "8. unresolvable workspace writes NOTHING to stdout (got: $OUT)"
else
  ok "8. unresolvable workspace fails closed, empty stdout, explained"
fi

# ---------------------------------------------------------------------------
echo "----------------------------------------"
if [ "$fail" -ne 0 ]; then
  echo "derive-bench-scope harness: FAILED"
  exit 1
fi
echo "derive-bench-scope harness: all checks passed"
