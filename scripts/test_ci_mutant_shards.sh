#!/usr/bin/env bash
#
# test_ci_mutant_shards.sh — semantics harness for scripts/ci_mutant_shards.sh
# (#2285: dynamic per-PR shard count).
#
# Offline: it builds a toy git+cargo fixture in a temp dir and runs REAL
# `cargo mutants --list --json` on it (--list parses sources and needs no
# compilation; over the whole webfang scope it is ~1 s). It exercises the
# helper's public interface only: prepare-diff, shard-count, shards.
#
# Regression for the "advanced base" bug (#2285): while a PR sits open the
# base branch advances (other PRs merge) and GitHub rebuilds the PR merge
# branch on the new tip. A diff taken against a STALE captured ref (the old
# three-dot merge-base behavior) includes the ADVANCEMENT hunks, so the
# mutant count would attribute other PRs' functions to this PR; the helper
# must diff against the checked-out head's first parent, which by definition
# holds only this PR's changes.

set -u
set -o pipefail

# Hermetic git: the developer global config signs commits and tags with an
# SSH key (commit.gpgsign / tag.gpgsign), which would block the fixture on a
# signer prompt. The fixture must use NO global identity or signing policy.
export GIT_CONFIG_GLOBAL=/dev/null

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="$SCRIPT_DIR/ci_mutant_shards.sh"

PASS=0
FAIL=0

pass() { printf '  OK   %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL %s — %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }

if [[ ! -x "$HELPER" ]]; then
  echo "test_ci_mutant_shards: helper not found at $HELPER" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Fixture: toy crate with an advanced base.
#
#   X   — src/lib.rs has base_fn                (1 mutant in base_fn)
#   Y   — src/other.rs added on top of X        (the "advancement": another
#                                              PR merged its function)
#   PR  — src/pr_file.rs added on top of Y      (THE PR's own change)
#
# The PR's merge branch (refs/pull/N/merge) is rebuilt on Y, so its first
# parent is Y, not X. The stale snapshot of the original PR base is kept in
# refs/stale-pr-base@X — exactly what an event-captured base sha still holds.
# ---------------------------------------------------------------------------
build_fixture() {
  local fix="$1"
  (
    cd "$fix" || exit 1
    git init -q
    git config user.email fixture@invalid
    git config user.name fixture
    mkdir -p src
    cat > Cargo.toml <<'EOF'
[package]
name = "toy"
version = "0.1.0"
edition = "2021"
EOF
    cat > src/lib.rs <<'EOF'
pub fn base_fn(a: i32) -> i32 {
    a.saturating_add(1)
}
EOF
    git add -A
    git commit -qm "X: base commit"
    git tag X-tag
    # The stale captured ref: what an 'opened'-event base sha still holds
    # after the base advanced.
    git update-ref refs/stale-pr-base "$(git rev-parse HEAD)"

    # The base branch advances (another PR merged its function).
    cat > src/other.rs <<'EOF'
pub fn advanced_fn(a: i32) -> i32 {
    a.saturating_mul(2)
}
EOF
    cat > src/lib.rs <<'EOF'
pub mod other;

pub fn base_fn(a: i32) -> i32 {
    a.saturating_add(1)
}
EOF
    git add -A
    git commit -qm "Y: base advanced by another PR"

    # The PR's own change, committed on top of the advanced base — this IS
    # the rebuilt merge branch: first parent = Y.
    cat > src/pr_file.rs <<'EOF'
pub fn pr_fn(a: i32) -> i32 {
    a.wrapping_sub(1)
}
EOF
    cat > src/lib.rs <<'EOF'
pub mod other;
pub mod pr_file;

pub fn base_fn(a: i32) -> i32 {
    a.saturating_add(1)
}
EOF
    git add -A
    git commit -qm "PR: add pr_fn"
  ) >/dev/null 2>&1
}

mutant_files_of_diff() {
  local workrepo="$1" reldiff="$2"
  (cd "$workrepo" && cargo mutants --list --json --in-diff "$reldiff" 2>/dev/null \
    | jq -r '.[].file' | sort -u)
}

FIXTURE="$(mktemp -d /tmp/opencode/mutant-fixture.XXXXXX)"
trap '[[ -n ${KEEP_FIXTURE:-} ]] || rm -rf "$FIXTURE"' EXIT
build_fixture "$FIXTURE"

# ---------------------------------------------------------------------------
# 1. shard-count formula (#2285 S1/S2, provisional calibration): 8 per shard,
#    cap 16, 0 mutants -> no shards.
# ---------------------------------------------------------------------------
count_shards() {
  bash "$HELPER" shard-count "$1" 2>/dev/null | awk '/^shard_count=/{print}' | cut -d= -f2
}

expect_shards() { # <count> <expected-shard_count>
  local got
  got="$(count_shards "$1")"
  if [[ "$got" == "$2" ]]; then
    pass "shard-count $1 -> $2"
  else
    fail "shard-count $1" "expected [$2], got [$got]"
  fi
}
expect_shards 0 0
expect_shards 1 1
expect_shards 8 1
expect_shards 9 2
expect_shards 16 2
expect_shards 17 3
expect_shards 24 3
expect_shards 25 4
expect_shards 128 16
# Cap: anything past 128 mutants must NOT produce more than 16 shards.
expect_shards 129 16
expect_shards 100000 16

# The shard list agrees with the count (parseable JSON for the matrix).
shards_line="$(bash "$HELPER" shard-count 10 2>/dev/null | awk '/^shards=/{print}')"
if [[ "$shards_line" == "shards=[1,2]" ]]; then
  pass "shards list for 10 mutants -> [1,2]"
else
  fail "shards list for 10 mutants" "expected [shards=[1,2]], got [$shards_line]"
fi

# ---------------------------------------------------------------------------
# 2. Advanced-base regression (#2285 S3/S4).
# ---------------------------------------------------------------------------
STALE_BASE="$(git -C "$FIXTURE" rev-parse refs/stale-pr-base)"
mkdir -p "$FIXTURE/work"
git clone -q "$FIXTURE" "$FIXTURE/work"
(cd "$FIXTURE/work" && git config user.email fixture@invalid && git config user.name fixture)

# OLD behavior (the defect): three dots against a stale captured ref.
(cd "$FIXTURE/work" && git diff "${STALE_BASE}...HEAD" > old.diff) 2>/dev/null
# NEW helper behavior: explicit base/head from the event, cross-checked.
prepare_out="$(cd "$FIXTURE/work" && EVENT_BASE_SHA="$STALE_BASE" bash "$HELPER" prepare-diff)"

# The helper warned when the event sha disagreed with the checkout's parent.
if (cd "$FIXTURE/work" && EVENT_BASE_SHA="$STALE_BASE" bash "$HELPER" prepare-diff 2>&1 >/dev/null) \
    | grep -q 'warning:'; then
  pass "prepare-diff warns when the event base sha disagrees with HEAD^1"
else
  fail "prepare-diff warning" "no warning found for a disagreeing event base sha"
fi

if [[ -s "$FIXTURE/work/git.diff" ]]; then
  pass "prepare-diff wrote git.diff"
else
  fail "prepare-diff wrote git.diff" "empty or absent"
fi

old_files="$(mutant_files_of_diff "$FIXTURE/work" old.diff | tr '\n' ' ')"
new_files="$(mutant_files_of_diff "$FIXTURE/work" git.diff | tr '\n' ' ')"

# The advancement (src/other.rs) DOES appear under the old listing...
if [[ "$old_files" == *"other.rs"* ]]; then
  pass "old three-dot listing includes the advancement hunks (regression reproduced)"
else
  fail "old three-dot listing" "expected src/other.rs mutants, got [$old_files]"
fi
# ...and must NOT appear under the helper's explicit-base listing.
if [[ "$new_files" != *"other.rs"* ]]; then
  pass "explicit base keeps the in-diff listing to the PR's own file"
else
  fail "explicit-base listing" "src/other.rs leaked into the PR-only diff, got [$new_files]"
fi
# And the PR's own functions are still there.
if [[ "$new_files" == *"pr_file.rs"* ]]; then
  pass "explicit base still captures the PR's own functions"
else
  fail "explicit-base listing" "pr_file.rs missing, got [$new_files]"
fi

# The helper really used the head's first parent (Y), not the stale sha.
used_base="$(printf '%s\n' "$prepare_out" | awk '/^base_sha=/{print}' | cut -d= -f2)"
if [[ "$used_base" == "$(git -C "$FIXTURE/work" rev-parse HEAD^1)" ]]; then
  pass "prepare-diff used the head's first parent as the diff base"
else
  fail "prepare-diff base" "expected HEAD^1 ($(git -C "$FIXTURE/work" rev-parse HEAD^1)), got [$used_base]"
fi

# ---------------------------------------------------------------------------
# 3. shards from a diff (#2285 S1/S5): agrees with a direct --list run of the
#    same command, and the count is the PR's OWN mutants only.
# ---------------------------------------------------------------------------
PR_FILES="$(mutant_files_of_diff "$FIXTURE/work" git.diff | tr '\n' ' ')"
if [[ "$PR_FILES" == *"pr_file.rs"* && "$PR_FILES" != *"other.rs"* ]]; then
  pass "in-diff listing holds the PR's own file (and nothing from the advancement)"
else
  fail "PR-diff mutant file count" "listing [$PR_FILES]"
fi
raw_count="$(cd "$FIXTURE/work" \
  && cargo mutants --list --json --in-diff git.diff 2>/dev/null | jq 'length')"
sh_out="$(cd "$FIXTURE/work" && bash "$HELPER" shards git.diff 2>/dev/null)"
helper_count="$(printf '%s' "$sh_out" | awk '/^mutant_count=/{print}' | cut -d= -f2)"
if [[ -n "$raw_count" && "$helper_count" == "$raw_count" ]]; then
  pass "shards count ($helper_count) agrees with a direct --list run ($raw_count)"
else
  fail "shards count" "raw=[$raw_count] helper=[$helper_count] out=[$sh_out]"
fi
if printf '%s' "$sh_out" | grep -q '^shard_count=1$' \
  && printf '%s' "$sh_out" | grep -q '^shards=\[1\]$'; then
  pass "shards sizes a single shard for the PR's small diff"
else
  fail "shards sizing" "expected shard_count=1 and shards=[1], got [$sh_out]"
fi

# ---------------------------------------------------------------------------
# 4. No-mutant diff (#2285 S5): a README-only diff yields zero mutants and an
#    empty matrix. Add ONLY the README: `git add -A` would stage the fixture's
#    own diff-artifact files into the commit and poison every later view.
# ---------------------------------------------------------------------------
(cd "$FIXTURE/work" \
  && printf 'doc\n' > README.md \
  && git add README.md \
  && git commit -qm "README" >/dev/null 2>&1) || true
(cd "$FIXTURE/work" && git diff 'HEAD^..HEAD' > readme.diff) || true
readme_hunks="$(grep -c 'src/' "$FIXTURE/work/readme.diff" || true)"
if [[ "$readme_hunks" == "0" ]]; then
  pass "the README-only diff contains no Rust hunks"
else
  fail "README-only diff sanity" "readme.diff touches Rust: readme.diff is not isolated, hunks=[$readme_hunks]"
fi
no_out="$(cd "$FIXTURE/work" && bash "$HELPER" shards readme.diff 2>/dev/null)"
if printf '%s' "$no_out" | grep -q '^mutant_count=0$' \
  && printf '%s' "$no_out" | grep -q '^shards=\[\]$'; then
  pass "no-mutant diff: count 0 and empty matrix"
else
  fail "no-mutant diff" "expected mutant_count=0 and shards=[], got [$no_out]"
fi

# ---------------------------------------------------------------------------
# 5. Fail-closed on a diff whose new side does not match the tree (#2285 S7):
#    cargo-mutants exit 5 — the exact advanced-base / foreign-tree signature.
#    A plain garbage file is NOT a valid fail-closed probe: cargo-mutants
#    treats it as an empty diff (rc 0, INFO "Diff file is empty"). Craft the
#    mismatch from the PR diff (stable across the README commit): swap the
#    new-side text so it no longer matches the source tree.
# ---------------------------------------------------------------------------
(cd "$FIXTURE/work" \
  && sed 's/+    a.wrapping_sub(1)/+    a.wrapping_sub(9)/' git.diff > wrong.diff) || true
wrong_rc=0
(cd "$FIXTURE/work" && bash "$HELPER" shards wrong.diff >/dev/null 2>"$FIXTURE/wrong.err") || wrong_rc=$?
if [[ $wrong_rc -ne 0 ]] && grep -qi 'exited with code' "$FIXTURE/wrong.err" 2>/dev/null; then
  pass "diff whose new side mismatches the tree fails closed with the exit code"
else
  fail "mismatched-new-side diff fail-closed" "rc=$wrong_rc err=[$(cat "$FIXTURE/wrong.err" 2>/dev/null)]"
fi

printf 'test_ci_mutant_shards: PASS=%d FAIL=%d\n' "$PASS" "$FAIL"
if [[ "$FAIL" -gt 0 ]]; then
  echo "test_ci_mutant_shards: FAILING" >&2
  exit 1
fi
echo "test_ci_mutant_shards: all dynamic-shard contract checks passed."
