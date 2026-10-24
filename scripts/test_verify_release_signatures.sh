#!/usr/bin/env bash
# Behavioral harness for the release-signature identity rules (#2281).
#
# Proves, without network or a real cosign:
#   - the default identity is the current repository, derived from
#     GITHUB_REPOSITORY when present;
#   - --tag accepts only frozen pre-migration tags from the checked-in
#     allowlist, and refuses every new tag;
#   - --tag cannot be combined with an explicit identity override;
#   - a missing bundle, a missing cosign and an empty asset list fail closed;
#   - the identity regexp actually reaches cosign, not just the log line.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/verify-release-signatures.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

LOG="$WORK/cosign.log"
FAKE_COSIGN="$WORK/cosign"
ASSET="$WORK/webfang-test.tar.gz"
NO_BUNDLE="$WORK/webfang-no-bundle.tar.gz"
FAIL=0
OLD_OWNER="XaviCode""1000"

cat > "$FAKE_COSIGN" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "version" ]]; then
  exit 0
fi
if [[ "${1:-}" == "verify-blob" ]]; then
  printf '%s\n' "$*" >> "$FAKE_COSIGN_LOG"
  exit 0
fi
exit 1
FAKE
chmod +x "$FAKE_COSIGN"

printf 'payload\n' > "$ASSET"
printf '{}\n' > "$ASSET.sigstore.json"
printf 'payload\n' > "$NO_BUNDLE"
: > "$LOG"

fail() {
  echo "FAIL: $*" >&2
  FAIL=$((FAIL + 1))
}

pass() {
  echo "OK: $*"
}

run_strict() {
  env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
    COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" \
    bash "$SCRIPT" "$@"
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    pass "output contains $needle"
  else
    fail "output does not contain $needle"
  fi
}

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    fail "output unexpectedly contains $needle"
  else
    pass "output does not contain $needle"
  fi
}

assert_fails() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    fail "$label unexpectedly succeeded"
  else
    pass "$label fails closed"
  fi
}

# 1. Strict default: current repository, no legacy identity.
out="$(run_strict "$ASSET" 2>&1)"
assert_contains "$out" "XaviCodeDev1000/webfang"
assert_not_contains "$out" "$OLD_OWNER/webfang"
assert_contains "$(cat "$LOG")" "XaviCodeDev1000/webfang"

# 2. A fork verifies its own releases without editing the script.
: > "$LOG"
out="$(env -u RELEASE_CERT_IDENTITY_REGEXP GITHUB_REPOSITORY='fork-owner/fork-repo' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" "$ASSET" 2>&1)"
assert_contains "$out" "fork-owner/fork-repo"

# 3. A frozen historical tag uses the allowlisted old identity.
: > "$LOG"
out="$(run_strict --tag v5.0.0 "$ASSET" 2>&1)"
assert_contains "$out" "historical tag allowlist: v5.0.0"
assert_contains "$out" "$OLD_OWNER/webfang"
assert_contains "$(cat "$LOG")" "$OLD_OWNER/webfang"

# 4. A new tag is not allowed to borrow the old identity.
assert_fails "tag v5.0.1 outside the allowlist" \
  run_strict --tag v5.0.1 "$ASSET"

# 5. --tag and an explicit override are ambiguous and must not be combined.
assert_fails "--tag with --identity-regexp" \
  run_strict --tag v5.0.0 --identity-regexp '^https://example\.com/release$' "$ASSET"

# 6. --tag and RELEASE_CERT_IDENTITY_REGEXP are ambiguous too.
assert_fails "--tag with RELEASE_CERT_IDENTITY_REGEXP" \
  env -u GITHUB_REPOSITORY RELEASE_CERT_IDENTITY_REGEXP='^https://example\.com/release$' \
  COSIGN="$FAKE_COSIGN" FAKE_COSIGN_LOG="$LOG" bash "$SCRIPT" --tag v5.0.0 "$ASSET"

# 7. Explicit override still works and reaches cosign.
: > "$LOG"
out="$(run_strict --identity-regexp '^https://example\.com/release$' "$ASSET" 2>&1)"
assert_contains "$out" "certificate identity regexp: ^https://example\\.com/release$"
assert_contains "$(cat "$LOG")" "--certificate-identity-regexp ^https://example\.com/release$"

# 8. Missing bundle and empty asset list fail closed.
assert_fails "asset without a bundle" run_strict "$NO_BUNDLE"
assert_fails "empty asset list" run_strict

# 9. A missing verifier fails closed.
assert_fails "missing cosign" \
  env -u GITHUB_REPOSITORY -u RELEASE_CERT_IDENTITY_REGEXP \
  COSIGN="$WORK/does-not-exist" bash "$SCRIPT" "$ASSET"

if [[ "$FAIL" -ne 0 ]]; then
  echo "release signature identity harness: $FAIL failure(s)" >&2
  exit 1
fi

echo "release signature identity harness: all checks passed"
