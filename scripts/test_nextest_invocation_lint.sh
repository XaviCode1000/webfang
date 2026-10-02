#!/usr/bin/env bash
# test_nextest_invocation_lint.sh
#
# Semantics harness for scripts/check_nextest_invocation_lint.sh (issue
# #1784, follow-up to #1781 / #1782).
#
# The gate is a text scan, and the repo's own standard for a grep gate is
# explicit: "Grepping for a retry loop proves none of those, and a gate
# that does not retry is worse than no gate — it buys false confidence."
# So this harness runs the gate against mktemp fixtures and proves, offline
# and with no repo state touched, that:
#
#   1. a clean tree PASSES (plain `cargo nextest run` with `--retries`
#      only — retries are out of scope);
#   2. a spaced `--test-threads 4`, an `=`-joined `--test-threads=4`, a
#      glued `-j4`, a spaced `-j 4`, and a `--jobs 4` on a
#      `cargo nextest run` FAIL;
#   3. a flag on a `\`-continuation line FAILS — joining continuations
#      before matching is load-bearing, because the flag shares no
#      physical line with `nextest run`;
#   4. the same shapes inside a YAML `run: |` block FAIL (the workflows
#      dir is scanned, not just scripts/);
#   5. the correct shapes PASS: libtest/Miri/TSan `--test-threads=1`
#      (different runner, out of scope), a `-j` that belongs to ANOTHER
#      command after `&&` or `||` (split first, match per segment),
#      full-line `#` comments (including inside YAML `run: |` blocks),
#      and `--json`-like flags (full-token match, never a substring);
#   6. the zero-findings baseline over the REAL tree PASSES — the
#      baseline is the final case here, not a manual run, so it
#      re-verifies on every CI.
#
# Case 5 is the one that matters most: a lint that fires on the five
# legitimate libtest/Miri/TSan `=1` lines, or on another command's `-j`,
# would block the tree on day one.

set -uo pipefail

GATE="$(dirname "$0")/check_nextest_invocation_lint.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

# Fixture layout: the gate scans $ROOT/scripts/**/*.sh and
# $ROOT/.github/workflows/*.yml.
make_tree() {
  local root="$1"
  mkdir -p "$root/scripts" "$root/.github/workflows"
}

# assert_case <name> <expect PASS|FAIL> <repo-relative-path> <<'EOF' ... EOF
assert_case() {
  local name="$1" expect="$2" relpath="$3" root
  root="$WORK/$name"
  rm -rf "$root"
  make_tree "$root"
  mkdir -p "$root/$(dirname "$relpath")"
  cat > "$root/$relpath"
  local out rc
  out="$(NEXTEST_LINT_ROOT="$root" bash "$GATE" 2>&1)"
  rc=$?

  if [ "$expect" = "PASS" ]; then
    if [ "$rc" -eq 0 ]; then
      printf '  ok   %-46s PASS (gate accepted)\n' "$name"
      passed=$((passed + 1))
    else
      printf '  FAIL %-46s expected PASS, got rc=%s\n%s\n' "$name" "$rc" "$out"
      failed=$((failed + 1))
    fi
    return
  fi

  if [ "$rc" -ne 0 ]; then
    printf '  ok   %-46s FAIL (gate fired)\n' "$name"
    passed=$((passed + 1))
  else
    printf '  FAIL %-46s expected FAIL, got rc=%s\n%s\n' "$name" "$rc" "$out"
    failed=$((failed + 1))
  fi
}

echo "check_nextest_invocation_lint semantics harness"

# --- 1. clean tree ------------------------------------------------------
assert_case clean_tree_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
cargo nextest run --workspace --lib --retries 2
cargo nextest run --test '*' --retries 2
EOF

# --- 2. flag spellings that must FIRE ------------------------------------
assert_case spaced_test_threads FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --workspace --lib --test-threads 4
EOF

assert_case equals_test_threads FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib --test-threads=4
EOF

assert_case glued_short_j FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib -j4
EOF

assert_case spaced_short_j FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib -j 4
EOF

assert_case jobs_flag FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib --jobs 4
EOF

# --- 3. continuation-line flag (joining is load-bearing) ------------------
assert_case continuation_line_flag FAILS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --workspace --all-features \
  --test-threads 4
EOF

# --- 4. YAML run block (workflows dir is scanned too) ---------------------
assert_case yaml_run_block_flag FAILS ".github/workflows/ci.yml" <<'EOF'
jobs:
  test-core:
    steps:
      - name: Unit tests
        run: |
          cargo nextest run --lib --retries 2
          cargo nextest run --test '*' --test-threads 4
EOF

# --- 5. correct shapes must NOT be flagged --------------------------------
assert_case libtest_ignored_serial_passes PASS "scripts/run_ai.sh" <<'EOF'
#!/usr/bin/env bash
cargo test -p webfang_ai --features ai --test ai_integration -- \
  --ignored --test-threads=1
EOF

assert_case miri_serial_passes PASS ".github/workflows/sanitizers.yml" <<'EOF'
jobs:
  miri:
    steps:
      - run: |
          cargo +nightly miri test -p webfang_core -- \
            infrastructure::bridge infrastructure::network \
            --test-threads=1
EOF

assert_case tsan_serial_passes PASS ".github/workflows/sanitizers.yml" <<'EOF'
jobs:
  tsan:
    steps:
      - run: |
          RUSTFLAGS="-Zsanitizer=thread" \
          cargo +nightly test -Zbuild-std \
            -p webfang_core --lib \
            -- infrastructure::bridge \
            --test-threads=1 \
            --skip slow_test
EOF

# The `-j` belongs to another command: split first, match per segment.
assert_case cross_command_and_j_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --workspace --lib && cargo build -j 2
EOF

assert_case cross_command_or_j_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib || cargo build -j 2
EOF

assert_case semicolon_command_j_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib; cargo build --jobs 2
EOF

# Full-line comments are ignored, including inside YAML run blocks.
assert_case full_line_comment_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
# cargo nextest run --lib --test-threads 4
cargo nextest run --lib --retries 2
EOF

assert_case yaml_full_line_comment_passes PASS ".github/workflows/ci.yml" <<'EOF'
jobs:
  test-core:
    steps:
      - name: Unit tests
        run: |
          # cargo nextest run --lib --test-threads 4
          cargo nextest run --lib --retries 2
EOF

# `--json`-like flags share a prefix with `-j` but are not a token match.
assert_case json_like_flag_passes PASS "scripts/run_tests.sh" <<'EOF'
#!/usr/bin/env bash
cargo nextest run --lib --message-format=json --retries 2
EOF

# --- 6. baseline over the real tree (fixtures first, real tree last) ------
# The post-#1782 tree is clean: no `cargo nextest run` invocation under
# scripts/ + .github/workflows/ carries a thread-count flag, and the five
# libtest/Miri/TSan `=1` lines are a different runner.
GATE_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
out="$(NEXTEST_LINT_ROOT="$GATE_ROOT" bash "$GATE" 2>&1)"; rc=$?
if [ "$rc" -eq 0 ]; then
  printf '  ok   %-46s PASS (real-tree baseline clean)\n' "real_tree_baseline"
  passed=$((passed + 1))
else
  printf '  FAIL %-46s real-tree baseline fired (rc=%s)\n%s\n' \
    "real_tree_baseline" "$rc" "$out"
  failed=$((failed + 1))
fi

echo
echo "check_nextest_invocation_lint semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
