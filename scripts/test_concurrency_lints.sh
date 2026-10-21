#!/usr/bin/env bash
# test_concurrency_lints.sh
#
# Semantics harness for scripts/check_concurrency_lints.sh (issue #1616,
# audit CC-L1 and CC-L2).
#
# The gate is a grep, and the repo's own standard for a grep gate is explicit:
# "Grepping for a retry loop proves none of those, and a gate that does not
# retry is worse than no gate — it buys false confidence." So this harness runs
# the gate against mktemp fixtures and proves, offline and with no repo state
# touched, that:
#
#   1. a clean tree PASSES;
#   2. a bare `acquire().await;` FAILS with the CC-L1 rule (not incidentally,
#      not on some other rule);
#   3. a non-literal `Semaphore::new(x)` FAILS with the CC-L2 rule, INCLUDING a
#      `NonZeroUsize::get()` argument — the gate is strict by design, so the
#      only way to pass a non-literal is a documented exemption;
#   4. `Semaphore::new(0)` FAILS with the CC-L2 rule;
#   5. the correct shapes PASS: a bound permit, a `?`-propagated permit, a
#      literal permit count, and a zero-permit semaphore inside `#[cfg(test)]`
#      (tests legitimately build one to exercise permit accounting);
#   6. a documented exemption silences exactly the site it names — the real
#      allowlist's row passes at its own path, and the same argument text in a
#      DIFFERENT file still fails;
#   7. a WILDCARD binding (`let _ = sem.acquire().await;`) is reported: it
#      drops the permit at the semicolon, exactly like the bare statement;
#   8. the test-module exemption ends where the test module ends: production
#      code AFTER a test module is scanned again.
#
# Case 6 is the one that matters most: an allowlist gate is only safe if it is
# keyed narrowly, and this proves the key includes the path.

set -uo pipefail

GATE="$(dirname "$0")/check_concurrency_lints.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

# Fixture layout: the gate scans $ROOT/crates/*/src/**/*.rs.
make_tree() {
  local root="$1"
  mkdir -p "$root/crates/webfang_core/src/mod_a" "$root/crates/webfang_mcp/src"
}

# assert_case <name> <expect PASS|FAIL> <expected-rule-or-empty> <<'RS' ... RS
assert_case() {
  local name="$1" expect="$2" rule="$3" root
  root="$WORK/$name"
  rm -rf "$root"
  make_tree "$root"
  cat > "$root/crates/webfang_core/src/mod_a/case.rs"
  local out rc
  out="$(CONCURRENCY_LINT_ROOT="$root" bash "$GATE" 2>&1)"
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

  if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "$rule"; then
    printf '  ok   %-46s FAIL (%s fired)\n' "$name" "$rule"
    passed=$((passed + 1))
  else
    printf '  FAIL %-46s expected FAIL via %s, got rc=%s\n%s\n' "$name" "$rule" "$rc" "$out"
    failed=$((failed + 1))
  fi
}

echo "check_concurrency_lints semantics harness"

# --- 1. clean tree ---------------------------------------------------------
assert_case clean_tree_passes PASS "" <<'RS'
use tokio::sync::Semaphore;

async fn good(sem: &Semaphore) {
    let _permit = sem.acquire().await?;
    let _literal = Semaphore::new(8);
    let _shifted = Semaphore::new(1 << 20);
}
RS

# A `NonZeroUsize` argument is provably non-zero, but the gate cannot see the
# type — it is strict on purpose: anything non-literal needs a documented
# exemption. This case pins that behaviour so a later loosening is a decision
# rather than a drift.
assert_case cc_l2_nonzero_get_needs_exemption FAILS "CC-L2" <<'RS'
use std::num::NonZeroUsize;
use tokio::sync::Semaphore;

fn undocumented() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(NonZeroUsize::new(4).unwrap().get()))
}
RS

# --- 2. CC-L1: the permit is discarded at the semicolon --------------------
assert_case cc_l1_bare_acquire FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

async fn bad(sem: &Semaphore) {
    sem.acquire().await;
}
RS

assert_case cc_l1_bare_acquire_owned FAILS "CC-L1" <<'RS'
async fn bad(arc: Arc<Semaphore>) {
    arc.acquire_owned().await?;
}
RS

# The wildcard binding. `let _ = sem.acquire().await;` DROPS the permit at the
# semicolon -- the permit is never bound to a name, so it is released
# immediately. It is the bare statement in disguise, and the gate's `let ` / `=`
# exclusions used to skip it.
assert_case cc_l1_wildcard_binding FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

async fn bad(sem: &Semaphore) {
    let _ = sem.acquire().await;
}
RS

assert_case cc_l1_wildcard_binding_question_mark FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

async fn bad(sem: &Semaphore) {
    let _ = sem.acquire().await?;
}
RS

# A NAMED binding keeps the permit alive until the binding dies, so it stays
# accepted -- the wildcard rule must not widen into "every `let` is a bug".
assert_case cc_l1_named_binding_passes PASS "" <<'RS'
use tokio::sync::Semaphore;

async fn good(sem: &Semaphore) {
    let _permit = sem.acquire().await;
    let permit = sem.acquire().await.expect("permit");
    drop(permit);
}
RS

# --- 3. CC-L2: runtime value, no visible non-zero guarantee ----------------
assert_case cc_l2_runtime_value FAILS "CC-L2" <<'RS'
use tokio::sync::Semaphore;

fn bad(from_config: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(from_config))
}
RS

# --- 4. CC-L2: the literal zero --------------------------------------------
assert_case cc_l2_literal_zero FAILS "CC-L2" <<'RS'
use tokio::sync::Semaphore;

fn bad() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(0))
}
RS

# --- 5. correct shapes must NOT be flagged ---------------------------------
assert_case cc_l1_bound_permit_passes PASS "" <<'RS'
async fn good(sem: &Semaphore) {
    let permit = sem.acquire().await.expect("permit");
    drop(permit);
    let _keep = sem.acquire().await?;
}
RS

assert_case cc_l2_literal_passes PASS "" <<'RS'
use tokio::sync::Semaphore;

fn good() {
    let _a = Semaphore::new(1);
    let _b = Semaphore::new(1 << 20);
    let _c = Semaphore::new(100);
}
RS

assert_case zero_permit_in_test_code_passes PASS "" <<'RS'
use tokio::sync::Semaphore;

fn build() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(64))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permits_are_returned_on_drop() {
        // Legitimate: a zero-permit semaphore is how this test observes that a
        // guard returns permits.
        let sem = Arc::new(Semaphore::new(0));
        let _g = PermitGuard::new(Arc::clone(&sem), 50);
        assert_eq!(sem.available_permits(), 50);
    }
}
RS

# The exemption covers the TEST MODULE, not the rest of the file. Production
# code after a test module used to be exempt for the rest of the file, so this
# bare acquire was never reported.
assert_case cc_l1_after_test_module_is_scanned FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

#[cfg(test)]
mod tests {
    #[test]
    fn permits_are_returned_on_drop() {
        let sem = Arc::new(Semaphore::new(0));
        assert_eq!(sem.available_permits(), 0);
    }
}

async fn production(sem: &Semaphore) {
    sem.acquire().await;
}
RS

# A `mod x;` declaration puts the test body in ANOTHER file, so nothing in this
# file is exempt and the production code after it is scanned.
assert_case cc_l1_after_external_test_module_is_scanned FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

#[cfg(test)]
mod tests;

async fn production(sem: &Semaphore) {
    sem.acquire().await;
}
RS

# The exemption still covers the body of an inline test module: a zero-permit
# semaphore built inside `mod tests { ... }` is legitimate, and the production
# code before the module is scanned as well.
assert_case cc_l1_before_test_module_is_scanned FAILS "CC-L1" <<'RS'
use tokio::sync::Semaphore;

async fn production(sem: &Semaphore) {
    sem.acquire().await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn permits_are_returned_on_drop() {
        let sem = Arc::new(Semaphore::new(0));
        assert_eq!(sem.available_permits(), 0);
    }
}
RS

assert_case zero_permit_in_inline_test_module_passes PASS "" <<'RS'
use tokio::sync::Semaphore;

#[cfg(test)]
mod tests {
    #[test]
    fn permits_are_returned_on_drop() {
        let sem = Arc::new(Semaphore::new(0));
        assert_eq!(sem.available_permits(), 0);
    }
}
RS

# --- 6. an exemption is keyed on the PATH, not the argument text -----------
# Uses the REAL allowlist that ships with the gate, so the rows are exercised
# exactly as they will run in CI.
GATE_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CONCURRENCY_LINT_ALLOWLIST="$GATE_ROOT/scripts/concurrency_lint_allowlist.txt"

# 6a. The real exempt site stays exempt. The fixture must sit at the SAME
# repo-relative path as the allowlist row, because the row is keyed on path.
mkdir -p "$WORK/exempt_site_passes/crates/webfang_core/src/infrastructure/downloader"
cat > "$WORK/exempt_site_passes/crates/webfang_core/src/infrastructure/downloader/resource_governor.rs" <<'RS'
use tokio::sync::Semaphore;

fn documented() {
    let _x = Semaphore::new(max_permits);
}
RS
out="$(CONCURRENCY_LINT_ROOT="$WORK/exempt_site_passes" bash "$GATE" 2>&1)"; rc=$?
if [ "$rc" -eq 0 ]; then
  printf '  ok   %-46s PASS (gate accepted)\n' "documented_exemption_passes"
  passed=$((passed + 1))
else
  printf '  FAIL %-46s expected PASS for a documented exemption, got rc=%s\n%s\n' \
    "documented_exemption_passes" "$rc" "$out"
  failed=$((failed + 1))
fi

# 6b. The SAME argument text in a different file is NOT exempt.
mkdir -p "$WORK/exempt_is_path_scoped/crates/webfang_core/src/infrastructure/downloader"
cat > "$WORK/exempt_is_path_scoped/crates/webfang_core/src/infrastructure/downloader/other_governor.rs" <<'RS'
use tokio::sync::Semaphore;

fn undocumented() {
    let _x = Semaphore::new(max_permits);
}
RS
out="$(CONCURRENCY_LINT_ROOT="$WORK/exempt_is_path_scoped" bash "$GATE" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "CC-L2"; then
  printf '  ok   %-46s FAIL (CC-L2 fired)\n' "exemption_is_path_scoped"
  passed=$((passed + 1))
else
  printf '  FAIL %-46s a CC-L2 exemption leaked to another file (rc=%s)\n%s\n' \
    "exemption_is_path_scoped" "$rc" "$out"
  failed=$((failed + 1))
fi

# 6c. An UNREADABLE allowlist must fail the gate, not silently disable every
# exemption. Without this the gate fails open the moment the path is wrong.
out="$(CONCURRENCY_LINT_ROOT="$WORK/exempt_site_passes" \
  CONCURRENCY_LINT_ALLOWLIST="$WORK/does-not-exist.txt" bash "$GATE" 2>&1)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "allowlist"; then
  printf '  ok   %-46s FAIL (unreadable allowlist reported)\n' "unreadable_allowlist_fails_closed"
  passed=$((passed + 1))
else
  printf '  FAIL %-46s gate failed OPEN on an unreadable allowlist (rc=%s)\n%s\n' \
    "unreadable_allowlist_fails_closed" "$rc" "$out"
  failed=$((failed + 1))
fi

echo
echo "check_concurrency_lints semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
