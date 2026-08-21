#!/usr/bin/env bash
set -euo pipefail
# Sprint 0 Gate 0 — freeze threat-matrix harness (RED before GREEN)
# Validates fail-closed behavior of pr-validation.yml Gate 0.
# Exit 0 = all checks pass, Exit 1 = fail.

FILE=".github/workflows/pr-validation.yml"
fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

# 1. FREEZE_FEATURES toggle exists
if grep -q 'FREEZE_FEATURES' "$FILE"; then ok "FREEZE_FEATURES env present"; else bad "FREEZE_FEATURES missing"; fi
# 2. Gate 0 step exists with label value check (type:feature)
if grep -q 'Gate 0' "$FILE" && grep -q 'type:feature' "$FILE"; then ok "Gate 0 label value check present"; else bad "Gate 0 label check missing"; fi
# 3. freeze-exception + CODEOWNER via gh api
if grep -q 'freeze-exception' "$FILE" && grep -q 'gh api' "$FILE"; then ok "freeze-exception + gh api CODEOWNER check present"; else bad "freeze-exception/gh api missing"; fi
# 4. Error message contains SDD link
if grep -q 'sdd/stabilization-sprint0-baseline' "$FILE"; then ok "SDD link in error message"; else bad "SDD link missing"; fi
# 5. enforce_admins documented (branch protection note in code or docs)
if grep -q 'enforce_admins' "$FILE" || grep -q 'enforce_admins' "AGENTS.md"; then ok "enforce_admins documented"; else bad "enforce_admins not documented"; fi
# 6. Fail-closed: gh empty → must not silently pass (check script exits 1 when gh returns empty)
# We verify the workflow step uses 'set -euo pipefail' and fails closed on unresolved PR
if grep -q 'set -euo pipefail' "$FILE"; then ok "fail-closed pipefail present"; else bad "pipefail missing"; fi

if [ "$fail" -ne 0 ]; then
  echo "Freeze gate harness: FAIL (expected before Gate0)"
  exit 1
fi
echo "Freeze gate harness: PASS"
