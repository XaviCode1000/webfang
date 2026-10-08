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
#   1. the real repository derives a non-empty scope of `-p <pkg>` tokens;
#   2. a crate whose bench targets are AUTODISCOVERED (`benches/*.rs`, no
#      `[[bench]]` table) IS included — the version that parsed manifests for
#      `[[bench]]` silently skipped it;
#   3. a crate with `autobenches = false` is EXCLUDED, because cargo does not
#      bench it, so scoping it in would build nothing useful;
#   4. a crate with an explicit `[[bench]]` table IS included;
#   5. the emitted package name comes from cargo, not from a `[[bench]]` entry's
#      own `name` key, which names the TARGET and would be the wrong token;
#   6. an empty scope FAILS CLOSED with exit 1 and EMPTY stdout, so the caller's
#      `scope="$(...)"` can never degrade into an unscoped full-workspace build;
#   7. an unresolvable workspace FAILS CLOSED with an explained message on
#      stderr rather than a Python traceback, because the first hard stop this
#      step ever produces must be the diagnosable one.
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures live in a mktemp tree and are dependency-free, so `cargo metadata`
# resolves them offline and no repository state is ever read or written.

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
run_derive() {
  local root="$1"
  set +e
  OUT="$(cd "$root" && python3 scripts/derive-bench-scope.py 2>"$WORK/stderr")"
  STATUS=$?
  set -e
  ERR="$(cat "$WORK/stderr")"
}

# ---------------------------------------------------------------------------
# 1. the real repository
# ---------------------------------------------------------------------------
echo "== real repository =="
run_derive "$REPO_ROOT"
if [ "$STATUS" -ne 0 ]; then
  bad "1. real repo exits 0 (got $STATUS: $ERR)"
elif ! printf '%s' "$OUT" | grep -Eq '^([[:space:]]*-p[[:space:]]+[A-Za-z0-9_-]+)+$'; then
  bad "1. real repo emits only '-p <pkg>' tokens (got: $OUT)"
elif ! printf '%s' "$OUT" | grep -q -- '-p webfang_core'; then
  bad "1. real repo includes webfang_core (9 declared benches) (got: $OUT)"
elif ! printf '%s' "$OUT" | grep -q -- '-p webfang_ai'; then
  bad "1. real repo includes webfang_ai (1 declared bench) (got: $OUT)"
else
  ok "1. real repo derives a non-empty scope of -p tokens"
fi

# ---------------------------------------------------------------------------
# 2, 3, 4, 5. which crates belong in the scope
# ---------------------------------------------------------------------------
echo "== scope membership =="
ROOT="$(make_workspace "$WORK/members" declared autodiscovered opted_out)"

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

run_derive "$ROOT"
if [ "$STATUS" -ne 0 ]; then
  bad "2-5. membership fixture derives (got $STATUS: $ERR)"
else
  ok "2-5. membership fixture derives cleanly"
  if printf '%s' "$OUT" | grep -q -- '-p declared'; then
    ok "4. explicit [[bench]] crate is in scope"
  else
    bad "4. explicit [[bench]] crate is in scope (got: $OUT)"
  fi
  if printf '%s' "$OUT" | grep -q -- '-p autodiscovered'; then
    ok "2. AUTODISCOVERED crate is in scope (the manifest-parsing version missed this)"
  else
    bad "2. AUTODISCOVERED crate is in scope (got: $OUT)"
  fi
  if printf '%s' "$OUT" | grep -q -- '-p opted_out'; then
    bad "3. autobenches=false crate is EXCLUDED (it was in scope: $OUT)"
  else
    ok "3. autobenches=false crate is excluded"
  fi
  if printf '%s' "$OUT" | grep -q -- 'totally_different_target_name'; then
    bad "5. emits the PACKAGE name, not the [[bench]] target name (got: $OUT)"
  else
    ok "5. emits the package name, not the [[bench]] target name"
  fi
fi

# ---------------------------------------------------------------------------
# 6. empty scope fails closed, with EMPTY stdout
# ---------------------------------------------------------------------------
echo "== fail-closed: nothing to bench =="
ROOT="$(make_workspace "$WORK/empty" plain)"
run_derive "$ROOT"
if [ "$STATUS" -eq 0 ]; then
  bad "6. empty scope exits non-zero (got 0: $OUT)"
elif [ -n "$OUT" ]; then
  bad "6. empty scope writes NOTHING to stdout (got: $OUT)"
elif ! printf '%s' "$ERR" | grep -qi 'no workspace crate'; then
  bad "6. empty scope explains itself on stderr (got: $ERR)"
else
  ok "6. empty scope fails closed, empty stdout, explained"
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
  bad "7. unresolvable workspace exits non-zero (got 0)"
elif printf '%s' "$ERR" | grep -q 'Traceback'; then
  bad "7. unresolvable workspace reports explained, not a traceback (got: $ERR)"
elif ! printf '%s' "$ERR" | grep -qi 'cargo metadata'; then
  bad "7. unresolvable workspace names the failing step (got: $ERR)"
else
  ok "7. unresolvable workspace fails closed with an explained message"
fi

# ---------------------------------------------------------------------------
echo "----------------------------------------"
if [ "$fail" -ne 0 ]; then
  echo "derive-bench-scope harness: FAILED"
  exit 1
fi
echo "derive-bench-scope harness: all checks passed"
