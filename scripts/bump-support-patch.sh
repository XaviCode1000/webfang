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

# ─── Base of a hotfix branch ────────────────────────────────────────────────
# Git does not record "the base of a branch" — that is a PR concept, and the
# authority for it is the topology gate (check-topology.sh) in CI. Locally
# there are three signals that DO exist, and every one of them is checked
# against the expected line:
#
#   1. the explicit declaration `git config branch.<name>.webfang-base`
#   2. the branch's own reflog: git records where a branch was born
#   3. the tracking ref, when it names a branch OTHER than the head itself
#
# A signal that names another base is a REFUSAL, never a fallback: a branch
# that says it comes from main does not get a second chance because another
# signal happens to agree. And when no signal exists at all the answer is a
# refusal too — never a guess.
normalize_base_ref() {
  local ref="$1"
  ref="${ref#refs/heads/}"
  local remote
  while IFS= read -r remote; do
    [[ -n "$remote" ]] || continue
    ref="${ref#"$remote/"}"
  done < <(git remote 2>/dev/null || true)
  printf '%s\n' "$ref"
}

# resolve_hotfix_base <branch> <expected-base>: exit 0 when the base of
# <branch> is provably <expected-base>, exit 1 (with a diagnostic that names
# the branch, the expected base and how to establish it) otherwise.
resolve_hotfix_base() {
  local branch="$1" expected="$2"
  local -a sig_labels=()
  local -a sig_values=()

  # 1. Explicit declaration.
  local declared
  declared="$(git config --get "branch.${branch}.webfang-base" 2>/dev/null || true)"
  if [[ -n "$declared" ]]; then
    sig_labels+=("declaración (git config branch.${branch}.webfang-base)")
    sig_values+=("$(normalize_base_ref "$declared")")
  fi

  # 2. The branch's own creation record.
  local created
  created="$(git reflog show --format='%H|%gs' "$branch" 2>/dev/null \
    | grep -m1 -E '\|branch: Created from ' || true)"
  if [[ -n "$created" ]]; then
    local origin_oid="${created%%|*}"
    local origin="${created#*|}"
    origin="${origin#branch: Created from }"
    if [[ "$origin" == "HEAD" || "$origin" =~ ^[0-9a-f]{40}$ ]]; then
      # "Created from HEAD" keeps the name of the branch it was born from
      # out of the record; what it does keep is the COMMIT. When that commit
      # is the tip of the expected line, the base is that line.
      local candidate
      for candidate in "refs/heads/$expected" "refs/remotes/origin/$expected"; do
        if [[ "$(git rev-parse --verify --quiet "$candidate" 2>/dev/null || true)" == "$origin_oid" ]]; then
          origin="$expected"
          break
        fi
      done
    fi
    sig_labels+=("origen de la rama (reflog)")
    sig_values+=("$(normalize_base_ref "$origin")")
  fi

  # 3. The tracking ref, when it names another branch (an upstream that
  # mirrors the head itself says nothing about the base).
  local upstream
  upstream="$(git rev-parse --abbrev-ref --symbolic-full-name "${branch}@{upstream}" 2>/dev/null || true)"
  if [[ -n "$upstream" && "$upstream" != "origin/${branch}" && "$upstream" != "${branch}" ]]; then
    sig_labels+=("tracking ref")
    sig_values+=("$(normalize_base_ref "$upstream")")
  fi

  if [[ ${#sig_values[@]} -eq 0 ]]; then
    echo "bump bloqueado: no se pudo establecer la base de la rama '$branch'; el patch de la línea ${expected#support/} se commitea sobre '$expected'." >&2
    echo "  cómo establecerla: creá la rama desde la línea de soporte ('git switch -c $branch $expected' o 'git worktree add -b $branch <ruta> $expected'), o declarala explícitamente ('git config branch.${branch}.webfang-base $expected')." >&2
    return 1
  fi

  local i
  for i in "${!sig_values[@]}"; do
    if [[ "${sig_values[$i]}" != "$expected" ]]; then
      echo "bump bloqueado: la base de la rama '$branch' no es '$expected' — ${sig_labels[$i]}: ${sig_values[$i]}" >&2
      echo "  el patch de la línea ${expected#support/} se commitea sobre '$expected' (un hotfix se abre con base support/X.Y)." >&2
      return 1
    fi
  done
  return 0
}

# --- B2 (#2060): the patch bump is bound to its support branch ---
# The bump lives on the branch whose minor line it serves; a bump from any
# other branch (or a detached HEAD, printed by git as "HEAD") would create a
# version that no support line captures. Refused before ANY mutation, before
# any cargo invocation.
#
# The sanctioned flow puts the bump inside the HOTFIX PR whose base is
# support/X.Y — the HEAD of that PR is `hotfix/*`, not `support/X.Y`, and
# check-topology.sh admits ONLY hotfix/* -> support/*. A guard that demands
# the head to BE the support branch therefore makes the sanctioned patch
# release unrunnable, so a hotfix branch is accepted when its base is
# PROVABLY the support line (resolve_hotfix_base below).
BRANCH_NOW="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH_NOW" != "support/${MINOR}" ]]; then
  if [[ "$BRANCH_NOW" == hotfix/* ]]; then
    resolve_hotfix_base "$BRANCH_NOW" "support/${MINOR}" || exit 1
  else
    echo "bump bloqueado: estás en la rama '$BRANCH_NOW' y el patch de la línea $MINOR se commitea en 'support/${MINOR}' (un checkout detached tampoco vale: git lo reporta como HEAD)" >&2
    exit 1
  fi
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
#    The tag set that decides uniqueness is the REMOTE's. The fetch that
#    brings it into the local view is NOT optional, and its failure is NOT
#    swallowed: a failed fetch leaves the local view as the only one
#    available, and a uniqueness check on a degraded view is not a
#    uniqueness check — it is a guard that read as a pass because it could
#    not see. So the failure is named and the bump refuses.
if ! git remote get-url origin >/dev/null 2>&1; then
  echo "bump bloqueado: no hay remote 'origin' — la unicidad del tag v${NEW_VERSION} no se puede establecer contra el remoto (fail closed)" >&2
  exit 1
fi
if ! FETCH_ERR="$(git fetch --tags origin 2>&1 >/dev/null)"; then
  echo "bump bloqueado: 'git fetch --tags origin' falló — la unicidad del tag v${NEW_VERSION} se comprobaría sobre una vista degradada (fail closed); restaurá la conectividad del remoto y reintentá." >&2
  [[ -n "$FETCH_ERR" ]] && echo "  git: $FETCH_ERR" >&2
  exit 1
fi
if git rev-parse "v${NEW_VERSION}" >/dev/null 2>&1; then
  echo "tag v${NEW_VERSION} ya existe" >&2
  exit 1
fi
# 2. The X.Y line must exist and must accept changes (STABLE or MAINTENANCE).
#    An UNAVAILABLE line state is not a passed gate. The state is a clause of
#    the release contract, so a state that could not be read leaves the
#    contract UNEVALUATED — and a contract that was never evaluated must
#    never be reported as a satisfied one. The refusal names the missing
#    state instead of degrading to a "tip".
LINE_JSON="$(git show "origin/main:${SUPPORT_JSON}" 2>/dev/null || cat "$SUPPORT_JSON" 2>/dev/null || true)"
if [[ -z "$LINE_JSON" ]]; then
  echo "bump bloqueado: no se pudo leer $SUPPORT_JSON (ni desde 'origin/main' ni desde el árbol de trabajo) — el estado de la línea $MINOR no se puede establecer y el contrato de release no se puede evaluar (fail closed)" >&2
  exit 1
fi
if ! STATE="$(printf '%s' "$LINE_JSON" | jq -r --arg m "$MINOR" '.lines[] | select(.minor==$m) | .state // empty' 2>/dev/null)"; then
  echo "bump bloqueado: $SUPPORT_JSON no se pudo interpretar (JSON inválido) — el estado de la línea $MINOR no se puede establecer (fail closed)" >&2
  exit 1
fi
[[ -z "$STATE" ]] && STATE="MISSING"
case "$STATE" in
  STABLE|MAINTENANCE) ;;
  *) echo "línea $MINOR en estado $STATE: bump bloqueado (requiere excepción aprobada)" >&2; exit 1 ;;
esac

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
  # The declaration follows the patch this bump produces; a bump that skips
  # the refresh leaves the declaration stale and turns the enforcing gate
  # red for EVERY later PR, with no sanctioned writer to repair it. So the
  # skip is a refusal, not a tip: the contract is not satisfied by a
  # declaration nobody updated.
  echo "bump bloqueado: $SUPPORT_JSON no existe en este árbol — la declaración de la línea $MINOR no puede seguir al parche que este bump produce (fail closed)" >&2
  exit 1
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
