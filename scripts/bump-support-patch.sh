#!/usr/bin/env bash
# Manual PATCH bump for support/X.Y lines. release-plz NEVER runs here: both
# `release-pr` and `update` fail on `cargo package` for the unpublished
# path+version inter-crate deps (same #1337 codepath, verified by pre-flight
# on a disposable branch — not an inference).
#
# Runs INSIDE the hotfix PR worktree (base support/X.Y) and commits: root
# Cargo.toml version, regenerated Cargo.lock, the CHANGELOG.md entry, and the
# support declaration (`.github/support.json` + the rendered `SUPPORT.md`).
# The version test no longer needs a snapshot rewrite (webfang#1543): it
# derives expected output from CARGO_PKG_VERSION, so a re-run after a failed
# bump just re-checks and proceeds to the commit instead of failing on a .snap.
#
# HOTFIX CODE vs RELEASE PREPARATION are different concepts sharing one PR:
# the fix commit(s) come first (authored by the agent); this script enforces
# the release contract (tag uniqueness, line accepts, lock in sync, CHANGELOG
# entry, the support declaration following the patch, `test_version` green)
# and produces the single `chore: bump` commit.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

SUPPORT_JSON=".github/support.json"

NEW_VERSION="${1:?usage: bump-support-patch.sh X.Y.Z [hotfix-pr-number]}"
PR_REF="${2:-}"
[[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "versión $NEW_VERSION no es X.Y.Z" >&2; exit 1; }
MINOR="${NEW_VERSION%.*}"

# --- B2 (#2060): the patch bump is bound to its support branch ---
# The bump lives on the branch whose minor line it serves; a bump from any
# other branch (or a detached HEAD, printed by git as "HEAD") would create a
# version that no support line captures. Refused before ANY mutation, before
# any cargo invocation.
BRANCH_NOW="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH_NOW" != "support/${MINOR}" ]]; then
  echo "bump bloqueado: estás en la rama '$BRANCH_NOW' y el patch de la línea $MINOR se commitea en 'support/${MINOR}' (un checkout detached tampoco vale: git lo reporta como HEAD)" >&2
  exit 1
fi

# --- B2 (#2060): semver monotonicity ---
# The requested version is never BELOW the version already in the root
# Cargo.toml: a patch release never walks the workspace backwards. EQUALITY
# is the documented idempotent re-run path and must keep working. CURRENT is
# read here, BEFORE the bump block, so the check does not depend on the
# mutation's ordering.
CURRENT="$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')"
if [[ ! "$CURRENT" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "no se pudo leer una versión X.Y.Z del Cargo.toml raíz (leído: '$CURRENT')" >&2
  exit 1
fi
IFS='.' read -r -a NEW_SEM <<< "$NEW_VERSION"
IFS='.' read -r -a CUR_SEM <<< "$CURRENT"
if (( NEW_SEM[0] < CUR_SEM[0] )) \
   || (( NEW_SEM[0] == CUR_SEM[0] && NEW_SEM[1] < CUR_SEM[1] )) \
   || (( NEW_SEM[0] == CUR_SEM[0] && NEW_SEM[1] == CUR_SEM[1] && NEW_SEM[2] < CUR_SEM[2] )); then
  echo "bump bloqueado: v${NEW_VERSION} es anterior a la versión actual del árbol (v${CURRENT}); un patch release nunca retrocede" >&2
  exit 1
fi

# --- Release contract (mechanical, no judgment) ---
# 1. Tag uniqueness.
git fetch --tags origin >/dev/null 2>&1 || true
git rev-parse "v${NEW_VERSION}" >/dev/null 2>&1 && { echo "tag v${NEW_VERSION} ya existe" >&2; exit 1; }
# 2. The X.Y line must exist and must accept changes (STABLE or MAINTENANCE).
LINE_JSON="$(git show "origin/main:${SUPPORT_JSON}" 2>/dev/null || cat "$SUPPORT_JSON" 2>/dev/null || true)"
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

# --- ID-1: the declaration of the line follows the patch this bump produces ---
# `latest` is a FACT about the published tags, and the enforcing gate
# (check_support_drift.sh --scope=published, a REQUIRED check inside
# `Validate PR metadata`) fails as soon as the declaration lags behind them.
# The patch this bump produces IS the new `latest` of its line, and nothing
# else in the release flow writes it: release-plz never touches support.json,
# and rotate-stable.sh writes the `latest` of the minor it creates and of no
# other. So the refresh is part of the release contract, not an afterthought.
#
# It runs BEFORE the version mutation: a refusal must leave the tree exactly
# as it was, and the refresh is the only new step that can refuse.
if [[ -f "$SUPPORT_JSON" ]]; then
  "$REPO_ROOT/scripts/refresh_support_latest.sh" --set "${MINOR}=${NEW_VERSION}"
else
  echo "tip: $SUPPORT_JSON no disponible, se omite el refresh de la declaración de soporte" >&2
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
# The declaration and its rendered projection travel with the bump: the gate
# reads both, and a commit that carried one without the other would leave the
# repository inconsistent by construction.
STAGE=(Cargo.toml Cargo.lock CHANGELOG.md)
if [[ -f "$SUPPORT_JSON" ]]; then
  STAGE+=("$SUPPORT_JSON" "SUPPORT.md")
fi
git add "${STAGE[@]}"
if git diff --cached --quiet; then
  echo "OK: nada nuevo que commitear (re-ejecución tras accept)."
else
  git commit -m "chore: bump to v${NEW_VERSION}"
  echo "OK: bump a v${NEW_VERSION} commiteado."
fi
