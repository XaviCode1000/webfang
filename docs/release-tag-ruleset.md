# Release Tag Protection Ruleset

This document describes the GitHub Ruleset that protects release tags (`v*`) from mutation and deletion, and the `apply-tag-ruleset.sh` script that manages it.

## Ruleset Specification

**Name:** `release-tags-immutable`
**Target:** `tag`
**Enforcement:** `active`
**Conditions:**
```json
{
  "ref_name": {
    "include": ["v*"],
    "exclude": []
  }
}
```
**Rules:**
1. `update` — Restrict updates (force-push / retag)
2. `deletion` — Block deletion (prevent delete-and-recreate)

## Semantics

### What is Protected
- All tags matching the `v*` pattern (e.g., `v1.0.0`, `v2.1.0-rc.1`, `v3.0.0`)
- The `*` in fnmatch does not cross `/`, which is fine for tags since they have no path separator

### What is NOT Restricted
- **Creation** of new tags is deliberately NOT restricted. Blocking creation would break the release pipeline (release-plz and cut-patch-tag.yml must cut new tags).

### Who Can Bypass
- Users with **bypass permission** on the ruleset (typically repository admins and the `github-actions[bot]` for automated workflows)
- The ruleset applies to all actors without bypass permission

## Script: `apply-tag-ruleset.sh`

### Subcommands

#### `apply`
Idempotently creates or updates the ruleset.

```bash
bash scripts/apply-tag-ruleset.sh apply
```

**Behavior:**
- If a ruleset named `release-tags-immutable` already exists, it is updated (PUT)
- Otherwise, a new ruleset is created (POST)
- Never deletes a ruleset it did not create
- Requires a token with `administration:write` permission

**Exit codes:**
- `0` — Success (created or updated)
- `1` — API call failed

#### `check`
Verifies the ruleset exists and has the correct shape.

```bash
bash scripts/apply-tag-ruleset.sh check
```

**Behavior:**
- Queries the GitHub API for the ruleset
- Validates: target=tag, enforcement=active, include contains v*, rules contain both update and deletion
- Prints exactly what is missing if incomplete
- Distinguishes "not protected" (exit 1) from "cannot tell" (exit 2 — API failure or unparseable response)

**Exit codes:**
- `0` — Ruleset exists and is correct
- `1` — Ruleset absent or incomplete (prints remediation command)
- `2` — API failure or unparseable response (cannot determine status)

### Ordering: Apply → Merge

The ruleset **MUST be applied before** any release tag is pushed that depends on its protection. The correct ordering:

1. **Apply the ruleset** (one-time setup, or after ruleset changes):
   ```bash
   bash scripts/apply-tag-ruleset.sh apply
   ```

2. **Verify it is active** (optional but recommended):
   ```bash
   bash scripts/apply-tag-ruleset.sh check
   ```

3. **Cut release tags** (release-plz, cut-patch-tag.yml, etc.) — tags are now protected

**Why this ordering matters:**
- If a tag is pushed BEFORE the ruleset exists, that tag is NOT retroactively protected
- The ruleset only protects tags created AFTER it becomes active
- The L1 provenance gate (check 13 in `check_release_dispatch.sh`) verifies the ruleset is active as part of the release hand-off invariant

## Integration with L1 Provenance

The ruleset check is invoked as **L1.5** in `check_release_provenance.sh`:

```bash
bash scripts/check_release_provenance.sh ruleset
```

This is called from `release.yml`'s preflight job. The preflight job requires `administration:read` permission on the token to query the ruleset. If the token lacks this permission, the check fails closed (exit 2).

The `check_release_dispatch.sh` guard (check 13) also verifies that `cut-patch-tag.yml` creates annotated tags and dispatches release.yml without `--ref`, passing `expected_sha`.

## Remediation

If `check` reports the ruleset is missing or incomplete:

```bash
# Apply the correct ruleset (requires admin token with administration:write)
bash scripts/apply-tag-ruleset.sh apply

# Verify
bash scripts/apply-tag-ruleset.sh check
```

The `check` output includes the exact JSON payload and `gh api` command for manual application if the script fails.

## Testing

The behavioral test suite `test_release_provenance.sh` includes ruleset verification (rows I.12a–I.12c):

- **I.12a**: No ruleset → exit 1 (not protected)
- **I.12b**: Ruleset missing deletion rule → exit 1 (incomplete)
- **I.12c**: API transport failure → exit 2 (cannot tell)

Run locally:
```bash
bash scripts/test_release_provenance.sh
```

## Related Files

| File | Role |
|------|------|
| `scripts/apply-tag-ruleset.sh` | Apply/check the ruleset |
| `scripts/check_release_provenance.sh` | L1 provenance gate (includes ruleset as L1.5) |
| `scripts/check_release_dispatch.sh` | CI guard verifying the ruleset wiring |
| `scripts/test_release_provenance.sh` | Behavioral acceptance matrix |
| `.github/workflows/release.yml` | Preflight runs L1.5 ruleset check |
| `.github/workflows/cut-patch-tag.yml` | Creates annotated tags, dispatches with expected_sha |