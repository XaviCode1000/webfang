#!/bin/bash
set -euo pipefail

# Detect test files that aren't discovered by cargo (dead tests)
# With crate-local tests/ and auto-discovery, every .rs file in crates/*/tests/
# (excluding mod.rs, common/, fixtures/) is automatically a test target.
# This script verifies no .rs files exist in root tests/ (regression guard).

DEAD=0

# Check root tests/ doesn't have .rs files (they should all be crate-local now)
if [ -d "tests" ] && [ "$(find tests -name '*.rs' 2>/dev/null | wc -l)" -gt 0 ]; then
  echo "ERROR: Root tests/ still has .rs files — all tests must be crate-local"
  find tests -name '*.rs'
  DEAD=$((DEAD+1))
fi

# Check no orphan test files outside standard locations
for f in $(find . -name "*.rs" -path "*/tests/*" -not -path "./target/*" -not -path "*/common/*" -not -path "*/fixtures*" -not -path "*/snapshots/*" -not -name "mod.rs" 2>/dev/null); do
  # Verify the file is in a valid crate tests/ directory
  if ! echo "$f" | grep -qE '^\./crates/[^/]+/tests/'; then
    echo "WARNING: Test file outside crate-local tests/: $f"
  fi
done

if [ $DEAD -gt 0 ]; then
  echo "FAILED: $DEAD dead test issue(s) found"
  exit 1
fi

echo "OK: No dead tests detected"
exit 0
