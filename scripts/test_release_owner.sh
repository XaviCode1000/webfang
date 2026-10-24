#!/usr/bin/env bash
# Regression guard for #2281: the release automation must target the current
# repository, and the old owner must survive only in the historical allowlist,
# the changelog and historical ADR notes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OLD_OWNER="XaviCode""1000"
NEW_REPO="XaviCodeDev1000/webfang"
FAIL=0

fail() {
  echo "FAIL: $*" >&2
  FAIL=$((FAIL + 1))
}

pass() {
  echo "OK: $*"
}

contains() {
  local file="$1"
  local needle="$2"
  if grep -Fq -- "$needle" "$file"; then
    pass "$file contains $needle"
  else
    fail "$file is missing $needle"
  fi
}

not_contains() {
  local file="$1"
  local needle="$2"
  if grep -Fq -- "$needle" "$file"; then
    fail "$file still contains $needle"
  else
    pass "$file does not contain $needle"
  fi
}

contains "scripts/release-legacy-tags.tsv" "v5.0.0	${OLD_OWNER}/webfang"
not_contains "scripts/release-legacy-tags.tsv" "v5.0.1"

legacy_count=$(awk -F '\t' 'NF == 2 && $1 !~ /^#/ { count++ } END { print count + 0 }' scripts/release-legacy-tags.tsv)
if [[ "$legacy_count" -eq 38 ]]; then
  pass "legacy allowlist has 38 frozen tags"
else
  fail "legacy allowlist has $legacy_count tags, expected 38"
fi

contains ".github/workflows/release-plz.yml" "github.repository == '${NEW_REPO}'"
not_contains ".github/workflows/release-plz.yml" "$OLD_OWNER"
contains ".github/workflows/release-reconcile.yml" "github.repository == '${NEW_REPO}'"
not_contains ".github/workflows/release-reconcile.yml" "$OLD_OWNER"

contains ".github/workflows/release.yml" 'RELEASE_CERT_IDENTITY_REGEXP:'
contains ".github/workflows/release.yml" 'github.repository'
not_contains ".github/workflows/release.yml" "$OLD_OWNER"

contains "scripts/generate-sbom.sh" "$NEW_REPO"
contains "scripts/generate-sbom.sh" "Organization: XaviCodeDev1000"
not_contains "scripts/generate-sbom.sh" "$OLD_OWNER"

contains "scripts/verify-release-signatures.sh" "GITHUB_REPOSITORY"
contains "scripts/verify-release-signatures.sh" "release-legacy-tags.tsv"
not_contains "scripts/verify-release-signatures.sh" "$OLD_OWNER"

# Current-facing files must point at the new repository. CHANGELOG.md and
# docs/adr/ are historical records and are intentionally excluded.
mapfile -t stale < <(
  grep -RIn \
    --include='*.md' --include='*.yml' --include='*.yaml' \
    --include='*.sh' --include='*.toml' --include='*.json' \
    --exclude-dir='.git' --exclude-dir='target' --exclude-dir='adr' \
    --exclude-dir='evidence' --exclude-dir='odd' \
    --exclude='CHANGELOG.md' \
    -F -- "$OLD_OWNER" . 2>/dev/null || true
)
if [[ ${#stale[@]} -eq 0 ]]; then
  pass "no stale owner references outside the historical allowlist"
else
  fail "stale owner references remain"
  printf '%s\n' "${stale[@]}" >&2
fi

if [[ "$FAIL" -ne 0 ]]; then
  echo "release owner guard: $FAIL failure(s)" >&2
  exit 1
fi

echo "release owner guard: all checks passed"
