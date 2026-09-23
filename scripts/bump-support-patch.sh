#!/usr/bin/env bash
# Manual PATCH bump for support/X.Y lines. release-plz NEVER runs here: both
# `release-pr` and `update` fail on `cargo package` for the unpublished
# path+version inter-crate deps (same #1337 codepath, verified by pre-flight
# on a disposable branch — not an inference).
#
# Runs INSIDE the hotfix PR worktree (base support/X.Y) and commits: root
# Cargo.toml version, regenerated Cargo.lock, and CHANGELOG.md entry. The
# version test no longer needs a snapshot rewrite (webfang#1543): it derives
# expected output from CARGO_PKG_VERSION, so a re-run after a failed bump
# just re-checks and proceeds to the commit instead of failing on a .snap.
#
# HOTFIX CODE vs RELEASE PREPARATION are different concepts sharing one PR:
# the fix commit(s) come first (authored by the agent); this script enforces
# the release contract (tag uniqueness, line accepts, lock in sync, CHANGELOG
# entry, `test_version` green) and produces the single `chore: bump` commit.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

NEW_VERSION="${1:?usage: bump-support-patch.sh X.Y.Z [hotfix-pr-number]}"
PR_REF="${2:-}"
[[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "versión $NEW_VERSION no es X.Y.Z" >&2; exit 1; }

# --- Release contract (mechanical, no judgment) ---
# 1. Tag uniqueness.
git fetch --tags origin >/dev/null 2>&1 || true
git rev-parse "v${NEW_VERSION}" >/dev/null 2>&1 && { echo "tag v${NEW_VERSION} ya existe" >&2; exit 1; }
# 2. The X.Y line must exist and must accept changes (STABLE or MAINTENANCE).
MINOR="${NEW_VERSION%.*}"
LINE_JSON="$(git show origin/main:.github/support.json 2>/dev/null || cat .github/support.json 2>/dev/null || true)"
if [[ -n "$LINE_JSON" ]]; then
  STATE="$(printf '%s' "$LINE_JSON" | jq -r --arg m "$MINOR" '.lines[] | select(.minor==$m) | .state // empty')"
  [[ -z "$STATE" ]] && STATE="MISSING"
  case "$STATE" in
    STABLE|MAINTENANCE) ;;
    *) echo "línea $MINOR en estado $STATE: bump bloqueado (requiere excepción aprobada)" >&2; exit 1 ;;
  esac
else
  echo "tip: support.json no disponible, se omite el check de estado (revisar a mano)" >&2
fi

# --- Bump (root workspace only; crates inherit via version.workspace = true,
# --- and the inter-crate pins stay at ^old by #1339 precedent) ---
CURRENT="$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
if [[ "$CURRENT" != "$NEW_VERSION" ]]; then
  sed -i -E "s/^version = \".*\"/version = \"${NEW_VERSION}\"/" Cargo.toml
fi

# --- Lockfile (release builds use --locked: MUST be committed in sync) ---
cargo check --workspace >/dev/null 2>&1

# --- CHANGELOG entry (after ## [Unreleased], mirroring the #1339 layout) ---
if ! grep -q "## \\[${NEW_VERSION}\\]" CHANGELOG.md; then
  grep -q '^## \[Unreleased\]' CHANGELOG.md || { echo "CHANGELOG sin sección [Unreleased]" >&2; exit 1; }
  ENTRY="- Patch release de la línea ${MINOR}."
  [[ -n "$PR_REF" ]] && ENTRY="- Patch release de la línea ${MINOR} (hotfix #${PR_REF})."
  BLOCK="## [${NEW_VERSION}] - $(date +%Y-%m-%d)

### 🩹 Fixed

${ENTRY}"
  awk -v block="$BLOCK" '{print} /^## \[Unreleased\]$/ && !done {print ""; print block; done=1}' \
    CHANGELOG.md > CHANGELOG.md.tmp
  mv CHANGELOG.md.tmp CHANGELOG.md
fi

# --- Version assertion (no snapshot: test derives expected from CARGO_PKG_VERSION) ---
# A bump without rewriting a golden .snap used to leave red CI; the test no
# longer freezes the version string (webfang#1543), so a green run here only
# proves the binary still answers --version with the bumped package version.
if ! cargo nextest run -p webfang_core test_version >/dev/null 2>&1; then
  echo "STOP: test_version falló tras el bump." >&2
  exit 1
fi

# --- Commit (explicit stage; never -A inside an agent worktree) ---
git add Cargo.toml Cargo.lock CHANGELOG.md
if git diff --cached --quiet; then
  echo "OK: nada nuevo que commitear (re-ejecución tras accept)."
else
  git commit -m "chore: bump to v${NEW_VERSION}"
  echo "OK: bump a v${NEW_VERSION} commiteado."
fi
