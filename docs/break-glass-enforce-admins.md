# Break-glass: `enforce_admins` outage procedure

Emergency merge path when a required check (`CI Gate`, `Validate PR metadata`,
`cargo-mutants (PR diff)`) is blocked by infrastructure (runner outage, cache
outage, rate limit) rather than by code. This procedure opens **only** the admin
enforcement door and closes it again. It never touches `strict` or the required
checks list.

## When to use

- All three required checks are green-or-blocked-by-outage: the failure is in
  the runner/cache/network layer, not in fmt/clippy/tests/guards.
- `gh` API confirms the outage (e.g. jobs stuck `queued`, 5xx from Actions).
- Single maintainer (or on-call) decides the merge cannot wait for recovery.

## Procedure (with log)

```bash
set -euo pipefail
repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
ts="$(date -u +%Y%m%dT%H%M%SZ)"

# 1. Snapshot current protection (store this file in the incident issue).
gh api "repos/$repo/branches/main/protection" > "protection-backup-$ts.json"

# 2. Open the admin door only.
gh api -X DELETE "repos/$repo/branches/main/protection/enforce_admins"
echo "enforce_admins OFF at $ts — merge now, restore afterwards"
```

Merge the PR, then immediately:

```bash
# 3. Close the door again.
gh api -X POST "repos/$repo/branches/main/protection/enforce_admins"

# 4. Verify (must print true).
gh api "repos/$repo/branches/main/protection" --jq '.enforce_admins.enabled'
```

## Rules

- Attach `protection-backup-$ts.json` to the incident issue. Never commit it.
- `strict: true` and the three required contexts stay in force throughout;
  this procedure grants no bypass to anyone but the admin, and only while
  the door is open.
- If step 4 does not print `true`, repeat step 3 before doing anything else.
- Audit reference: Fase 1 decision `odd/tasks/ci-gate-fase1.md` (required-checks
  audit, 2026-09-29) — `enforce_admins: true` is the steady state.
