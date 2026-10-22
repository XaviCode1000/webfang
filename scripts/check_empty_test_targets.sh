#!/usr/bin/env bash
set -uo pipefail
# check_empty_test_targets.sh — a declared test target that compiles to ZERO
# tests is a false green.
#
# THE DEFECT THIS GUARDS
# ======================
# Cargo DISCOVERS a file in `tests/` and BUILDS a target out of it. If the file
# carries a crate-level gate on a feature that is NOT in the package's default
# feature set, the target is built and it contains NO test at all: the run
# reports green, the file looks covered, and nobody notices. #1852 is exactly
# that: `cargo nextest run -p webfang_mcp` skipped 151 tests of 22 files
# because those files are gated on the `mcp` feature, which was not in the
# package's default.
#
# The sibling guard `scripts/check_dead_tests.sh` covers the OTHER half: a
# `.rs` file in `tests/` that cargo never discovers. This one covers a file
# cargo DOES discover and builds into an empty target.
#
# THE RULE
# ========
# For every integration-test file that is a real target (auto-discovered
# `tests/*.rs`, or an explicitly declared `[[test]]` path):
#
#   * no crate-level gate            → nothing to check (the target has tests)
#   * gate names feature F, F in the package's `default`
#                                   → nothing to check: the default build
#                                     really compiles those tests
#   * gate names feature F, F NOT in the default
#                                   → the default build compiles the target to
#                                     ZERO tests, so the package MUST declare
#                                     that target with `required-features = ["F"]`
#                                     (cargo's own declaration for a
#                                     conditional target). Without it → FAIL.
#
# A declared target that has `required-features` must cover every feature its
# gate names: `required-features` that does not cover the gate is the same
# silent emptiness, one layer deeper.
#
# The two ways to fix a failure (both are in the message):
#   1. declare the target with `required-features = ["<feature>"]`, or
#   2. put the feature in the package's `default` set, if the target SHOULD
#      run by default.
#
# OFFLINE / NO-CARGO
# ==================
# Pure text over the checked-out tree: the manifests and the test files. No
# cargo, no network, no build, no repo state touched. The workspace member
# list comes from the root manifest, not from `cargo metadata`, so the guard
# runs before anything is compiled.
#
# Usage: bash scripts/check_empty_test_targets.sh
# Env:   EMPTY_TEST_TARGETS_ROOT — point the guard at a fixture tree (the
#        harness of `scripts/test_empty_test_targets.sh` uses it).

REPO_ROOT="${EMPTY_TEST_TARGETS_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT" || { echo "check_empty_test_targets: no such root: $REPO_ROOT" >&2; exit 1; }

FAILURES=0
CHECKED=0
CANDIDATES=0

# ─────────────────────────────────────────────────────────────────────────────
# Parsing helpers (awk only: no cargo, no toml parser, no network)
# ─────────────────────────────────────────────────────────────────────────────

# normalize_manifest — a manifest whose multi-line lists are JOINED into one
# line each, so the parsing pass never has to know whether a list was
# written in one line or in five. A TOML list that spans lines is the normal
# way to write `members = [...]`, and a parser that only reads the first line
# silently sees an EMPTY list: that is the kind of parser that turns a guard
# into a green light wired to nothing.
normalize_manifest() {
  awk '
    {
      if (pending) {
        buf = buf " " $0
        ob = gsub(/\{/, "{", buf); cb = gsub(/\}/, "}", buf)
        os = gsub(/\[/, "[", buf); cs = gsub(/\]/, "]", buf)
        if (ob + os <= cb + cs) { print buf; pending = 0; buf = "" }
        next
      }
      line = $0
      ob = gsub(/\{/, "{", line); cb = gsub(/\}/, "}", line)
      os = gsub(/\[/, "[", line); cs = gsub(/\]/, "]", line)
      if (ob + os > cb + cs) { buf = line; pending = 1; next }
      print line
    }
  ' "$1"
}

# manifest_default_features <manifest> — the package's `default` feature list.
manifest_default_features() {
  normalize_manifest "$1" | awk '
    /^\[features\]/ { in_features = 1; next }
    /^\[/ { in_features = 0 }
    in_features && /^[[:space:]]*default[[:space:]]*=/ {
      line = $0
      sub(/^[^=]*=[[:space:]]*/, "", line)
      gsub(/["\047]/, "", line)
      gsub(/[\[\],]/, " ", line)
      n = split(line, parts, /[[:space:]]+/)
      for (i = 1; i <= n; i++) if (parts[i] != "") print parts[i]
    }
  '
}

# manifest_test_targets <manifest> — one record per declared [[test]]:
#   <name>|<path>|<required-features, comma separated>
manifest_test_targets() {
  normalize_manifest "$1" | awk '
    function unquote(text) {
      gsub(/["\047]/, "", text)
      gsub(/[\[\]]/, "", text)
      return text
    }
    function flush() {
      if (in_test) printf "%s|%s|%s\n", name, path, req
      name = ""; path = ""; req = ""; in_test = 0
    }
    /^\[\[test\]\]/ { flush(); in_test = 1; next }
    /^\[/ { flush(); next }
    in_test && /^[[:space:]]*name[[:space:]]*=/ {
      line = $0; sub(/^[^=]*=[[:space:]]*/, "", line); name = unquote(line)
    }
    in_test && /^[[:space:]]*path[[:space:]]*=/ {
      line = $0; sub(/^[^=]*=[[:space:]]*/, "", line); path = unquote(line)
    }
    in_test && /^[[:space:]]*required-features[[:space:]]*=/ {
      line = $0; sub(/^[^=]*=[[:space:]]*/, "", line); req = unquote(line)
    }
    END { flush() }
  '
}

# gate_features <test file> — the cargo FEATURES named in the file's
# crate-level gate (`#![cfg(...)]`). A `cfg` that is not a feature (for
# example `tokio_unstable`) is not a feature and is ignored: cargo cannot
# enable it with a feature, so it is not what `required-features` can name.
#
# The WHOLE file is scanned, not its first N lines: a crate-level attribute
# can sit below a long doc comment, and a guard that stopped looking before
# the gate would call a gated target "ungated" — a false PASS, which is the
# worst failure mode a guard has.
gate_features() {
  grep -m1 '^#!\[cfg(' "$1" | grep -o 'feature[[:space:]]*=[[:space:]]*"[^"]*"' \
    | sed 's/.*=[[:space:]]*"\([^"]*\)".*/\1/'
}

# in_list <needle> <space separated haystack> → 0 when present.
in_list() {
  local needle="$1" hay="$2" item
  for item in $hay; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

# ─────────────────────────────────────────────────────────────────────────────
# The workspace members (root manifest, explicit list)
# ─────────────────────────────────────────────────────────────────────────────

members="$(normalize_manifest Cargo.toml | awk '
  /^\[workspace\]/ { in_ws = 1; next }
  /^\[/ { in_ws = 0 }
  in_ws && /^[[:space:]]*members[[:space:]]*=/ {
    line = $0
    sub(/^[^=]*=[[:space:]]*/, "", line)
    gsub(/["\047]/, "", line)
    gsub(/[\[\],]/, " ", line)
    n = split(line, parts, /[[:space:]]+/)
    for (i = 1; i <= n; i++) if (parts[i] != "") print parts[i]
  }
')"

# ─────────────────────────────────────────────────────────────────────────────
# The check
# ─────────────────────────────────────────────────────────────────────────────

for member in $members; do
  manifest="$member/Cargo.toml"
  [ -f "$manifest" ] || continue
  [ -d "$member/tests" ] || continue

  default_features="$(manifest_default_features "$manifest" | tr '\n' ' ')"

  # Declared targets: path → required-features.
  declared_paths=""
  declared_req=""
  while IFS='|' read -r t_name t_path t_req; do
    [ -n "$t_path" ] || continue
    declared_paths="$declared_paths$t_path"$'\n'
    declared_req="$declared_req$t_name=$t_req"$'\n'
  done < <(manifest_test_targets "$manifest")

  while IFS= read -r file; do
    rel="${file#"$member"/}"          # path relative to the package root
    is_declared=0
    req=""
    while IFS= read -r d_path; do
      [ "$d_path" = "$rel" ] && { is_declared=1; break; }
    done <<< "$declared_paths"
    if [ "$is_declared" = "1" ]; then
      while IFS='=' read -r t_name t_req; do
        [ -n "$t_name" ] || continue
        if [ "$t_name" = "$(basename "$rel" .rs)" ]; then req="$t_req"; break; fi
      done <<< "$declared_req"
    else
      # Only an auto-discovered top-level file is a target by itself.
      case "$rel" in
        tests/*.rs) ;;
        *) continue ;;
      esac
    fi

    CANDIDATES=$((CANDIDATES + 1))
    features="$(gate_features "$file")"
    [ -n "$features" ] || continue

    CHECKED=$((CHECKED + 1))
    for feature in $features; do
      in_list "$feature" "$default_features" && continue
      in_list "$feature" "$req" && continue
      FAILURES=$((FAILURES + 1))
      printf 'FAIL: check_empty_test_targets: %s compiles to ZERO tests in the default build: its gate names the feature "%s", which is not in the package default feature set%s\n' \
        "$rel" "$feature" \
        "$([ "$is_declared" = "1" ] && printf ' and is not covered by the required-features of its [[test]] declaration' || printf ' and the target is not declared with required-features')"
      printf '      fix 1: declare the target with required-features = ["%s"] in %s\n' "$feature" "$manifest"
      printf '      fix 2: put "%s" in the default feature set of %s if the target should run by default\n' "$feature" "$manifest"
    done
  done < <(find "$member/tests" -name '*.rs' -type f | sort)
done

if [ "$CANDIDATES" -eq 0 ]; then
  # A guard that inspected NOTHING is a green light wired to nothing: the
  # member list, the test tree or the discovery rule changed and the guard
  # would keep saying OK without looking at a single target.
  printf 'FAIL: check_empty_test_targets: the guard inspected NO test target at all (0 candidates): the member list, the tests tree or the discovery rule changed\n'
  exit 1
fi

if [ "$FAILURES" -eq 0 ]; then
  printf 'OK: check_empty_test_targets: no declared test target compiles to zero tests in the default build (%d gated target(s) checked)\n' "$CHECKED"
  exit 0
fi
printf 'FAIL: check_empty_test_targets: %d target(s) compile to zero tests in the default build\n' "$FAILURES"
exit 1
