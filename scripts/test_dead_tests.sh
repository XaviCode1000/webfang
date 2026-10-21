#!/usr/bin/env bash
# test_dead_tests.sh
#
# Semantics harness for scripts/check_dead_tests.sh (issue #1008/#1014).
#
# The guard answers one question: is every .rs file under a crate's tests/
# tree actually compiled by cargo? Cargo tells us exactly which test targets
# it builds and where each entry file lives, so the guard walks the `mod`
# declarations from those entries and reports whatever is left over.
#
# The walk has to follow RUST's module rules, not a guess:
#
#   * a crate root (any cargo target entry point) keeps its children in the
#     file's own directory;
#   * `foo.rs` — and `foo/mod.rs` — is the module `foo`, whose children live
#     in `foo/`.
#
# The guard used to resolve every `mod foo;` against the declaring file's own
# directory, so a `mod leaf;` inside `tests/sub.rs` looked for `tests/leaf.rs`
# and never for `tests/sub/leaf.rs` — the file Rust actually compiles. Live
# files were reported dead.
#
# What this harness proves, offline (a mktemp crate; `cargo metadata
# --no-deps` only, no build, no network):
#
#   1. a `mod` chain that Rust resolves through a NON-ROOT module
#      (`tests/sub.rs` -> `tests/sub/leaf.rs`) is reachable, not dead;
#   2. the same chain through a `mod.rs` is reachable too;
#   3. a file nothing references IS dead and is reported;
#   4. a file cargo auto-discovers as its own test target is not dead.
#
# Usage: bash scripts/test_dead_tests.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_dead_tests.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

ok() {
  printf '  ok   %-52s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-52s %s\n' "$1" "$2"
  if [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# --- cargo resolution --------------------------------------------------------
# The guard shells out to `cargo metadata`. The mbx shim must never be used for
# a build, and this harness does not build anything; resolve the real cargo
# explicitly so the run is reproducible on a machine where `cargo` is a
# wrapper.
CARGO_BIN_DIR=""
if command -v rustup >/dev/null 2>&1; then
  CARGO_REAL="$(rustup which cargo 2>/dev/null || true)"
  if [ -n "$CARGO_REAL" ] && [ ! -e "$CARGO_REAL" ]; then
    :
  fi
  if [ -n "$CARGO_REAL" ]; then
    case "$(readlink -f "$CARGO_REAL" 2>/dev/null || echo "$CARGO_REAL")" in
      *mbx*)
        echo "test_dead_tests.sh: refusing to run through the mbx shim ($CARGO_REAL)" >&2
        exit 2
        ;;
    esac
    CARGO_BIN_DIR="$(dirname "$CARGO_REAL")"
  fi
fi
if [ -z "$CARGO_BIN_DIR" ]; then
  CARGO_BIN_DIR="$(dirname "$(command -v cargo 2>/dev/null || echo /nonexistent/cargo)")"
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "test_dead_tests.sh: cargo not available; this harness needs it for 'cargo metadata'" >&2
  exit 2
fi

# --- fixture crate -----------------------------------------------------------
# new_crate <name> — a one-crate workspace with a test target `entry` whose
# entry file is `tests/entry.rs`.
new_crate() {
  local dir="$WORK/$1"
  mkdir -p "$dir/crates/fault/src" "$dir/crates/fault/tests"
  cat >"$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/fault"]
EOF
  cat >"$dir/crates/fault/Cargo.toml" <<'EOF'
[package]
name = "fault"
version = "0.0.0"
edition = "2021"

[lib]

[[test]]
name = "entry"
path = "tests/entry.rs"
EOF
  printf 'pub fn f() -> u8 { 0 }\n' >"$dir/crates/fault/src/lib.rs"
  printf 'mod sub;\n' >"$dir/crates/fault/tests/entry.rs"
  printf '%s' "$dir"
}

OUT=""
RC=0
run_gate() { # <dir>
  OUT="$(cd "$1" && env PATH="$CARGO_BIN_DIR:$PATH" bash "$GATE" 2>&1)"
  RC=$?
}

expect_pass() { # <label> <dir>
  run_gate "$2"
  if [ "$RC" -eq 0 ]; then
    ok "$1" "rc=0"
  else
    bad "$1" "expected PASS, got rc=$RC" "$OUT"
  fi
}

expect_fail_naming() { # <label> <dir> <needle>
  run_gate "$2"
  if [ "$RC" -ne 0 ] && printf '%s' "$OUT" | grep -qF "$3"; then
    ok "$1" "rc=$RC"
  else
    bad "$1" "expected FAIL naming '$3', got rc=$RC" "$OUT"
  fi
}

echo "check_dead_tests semantics harness"

# --- 1. a non-root module chain is reachable --------------------------------
# NEGATION: `tests/sub.rs` declares `mod leaf;` and Rust looks for
# `tests/sub/leaf.rs`. The old resolution looked for `tests/leaf.rs`, so the
# live file was reported dead.
D="$(new_crate nonroot_module)"
mkdir -p "$D/crates/fault/tests/sub"
printf 'mod leaf;\n' >"$D/crates/fault/tests/sub.rs"
printf '#[test]\nfn leaf_ok() {}\n' >"$D/crates/fault/tests/sub/leaf.rs"
expect_pass "non-root module chain is reachable" "$D"

# --- 2. the same chain through a mod.rs is reachable ------------------------
# NEGATION: the mirror shape must not be the only one that works, or a
# refactor that moves a file to `mod.rs` form changes the verdict.
D="$(new_crate mod_rs_module)"
mkdir -p "$D/crates/fault/tests/sub"
printf 'mod leaf;\n' >"$D/crates/fault/tests/sub/mod.rs"
printf '#[test]\nfn leaf_ok() {}\n' >"$D/crates/fault/tests/sub/leaf.rs"
expect_pass "mod.rs module chain is reachable" "$D"

# --- 3. a file nothing references is dead -----------------------------------
# NEGATION: the guard must not have been defanged into "everything is
# reachable" — the whole point is that an unreferenced file is reported.
D="$(new_crate orphan_file)"
mkdir -p "$D/crates/fault/tests/sub"
printf '#[test]\nfn orphan() {}\n' >"$D/crates/fault/tests/sub/orphan.rs"
expect_fail_naming "unreferenced file is reported dead" "$D" \
  "crates/fault/tests/sub/orphan.rs"

# --- 4. a cargo-discovered test target is not dead --------------------------
# NEGATION: `tests/foo.rs` is auto-discovered by cargo as its own test
# target, so it IS compiled and must not be reported.
D="$(new_crate discovered_target)"
printf '#[test]\nfn standalone() {}\n' >"$D/crates/fault/tests/standalone.rs"
expect_pass "cargo-discovered test target is reachable" "$D"

echo
echo "check_dead_tests semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
