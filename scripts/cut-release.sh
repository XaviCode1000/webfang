#!/usr/bin/env bash
# Manual Release PR producer for `main`.
#
# This replaces the `release-plz-pr` CI job, which is structurally unsatisfiable
# for this workspace (webfang#1387): in git_only mode release-plz runs
# `cargo package` against a worktree checked out at the last `v*` tag, and the
# inter-crate deps are { path, version } on crates that are never published
# (binaries-only distribution), so crates.io resolution can never succeed. There
# is no config to skip that packaging. The job was last wired with
# `continue-on-error: true`, which reported success over a logged exit-1 — 73
# commits merged with no Release PR and no signal. Hence: deleted, not silenced.
#
# What still works and is deliberately untouched: `release-plz release` pushes
# the `vX.Y.Z` tag for an already-merged version bump, which triggers
# release.yml (4 binaries + SHA256SUMS + GitHub Release). Proven on v2.1.0 by
# the hand-cut Release PR #1339 — this script automates exactly that gesture.
#
# Runs INSIDE a `chore/release-X.Y.Z` worktree based on main and produces the
# single `chore: bump` commit: root `[workspace.package] version`, refreshed
# Cargo.lock, CHANGELOG entry rendered by git-cliff from cliff.toml, and the
# version snapshot update.
#
# Idempotent: re-running after accepting the snapshot proceeds to the commit
# instead of failing.
#
# Contract checks are mechanical. Version *class* is not: the semver hint below
# is a signal, the maintainer decides (see AGENTS.md -> "Advisory duty").
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

die() { echo "cut-release: $*" >&2; exit 1; }
info() { echo "cut-release: $*"; }

usage() {
  echo "usage: cut-release.sh X.Y.Z [--dry-run]" >&2
  echo "       (accepts X.Y.Z and X.Y.Z-rc.N; --dry-run verifies and previews, changes nothing)" >&2
}

# --- Arguments ---
NEW_VERSION="${1:-}"
DRY=0
case "${2:-}" in
  "") ;;
  --dry-run) DRY=1 ;;
  *) usage
    die "argumento desconocido: ${2}" ;;
esac
[[ -n "$NEW_VERSION" ]] || { usage; exit 1; }

[[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] \
  || die "versión «${NEW_VERSION}» no es X.Y.Z (ni X.Y.Z-rc.N)"

SNAP_DIR="crates/webfang_core/tests/snapshots"
SNAP="${SNAP_DIR}/cli_binary_test__test_version.snap"
ROOT_TOML="Cargo.toml"
NEW_MAJOR="${NEW_VERSION%%.*}"

# --- Release contract (mechanical, no judgment) ---
# 1. Never on a support line: release-plz must not run there, and patches there
#    have their own script with a different contract (support.json line state).
BRANCH="$(git branch --show-current)"
[[ -n "$BRANCH" ]] || die "HEAD detached: ejecutá desde una rama chore/release-*"
case "$BRANCH" in
  support/*) die "estás en «${BRANCH}»: para un patch de soporte usá scripts/bump-support-patch.sh" ;;
  main) info "aviso: estás en main; lo normal es cortar desde un worktree chore/release-*" ;;
esac

# 2. Clean tree. release-plz.toml sets allow_dirty = false for the same reason
#    #1197 showed: a dirty tree commits its junk straight into the Release PR.
#    One exemption: the version snapshot this script itself makes stale. A rerun
#    after `cargo insta accept` is the documented happy path, and at that point
#    the snapshot IS modified — refusing it would break the loop this script
#    depends on. Anything else dirty is still a refusal.
DIRTY="$(git status --porcelain | grep -vF "$SNAP" || true)"
[[ -z "$DIRTY" ]] || die "árbol sucio, no corto un release encima:
${DIRTY}"

# 3. Tag uniqueness — release.yml triggers on v*, a colliding tag is a silent
#    no-op release.
git fetch --tags origin >/dev/null 2>&1 || true
if git rev-parse -q --verify "refs/tags/v${NEW_VERSION}" >/dev/null; then
  die "el tag v${NEW_VERSION} ya existe; elegí otra versión"
fi

# 4. Monotonic against the newest release tag. RC tags are excluded from the
#    baseline on purpose: v2.2.0 must compare against v2.1.0, not against
#    v2.2.0-rc.2. The bump itself is compared on its numeric core, so an RC is
#    only legal for a version that has not shipped — `sort -V` is not semver-aware
#    and would rank 2.2.0-rc.1 ABOVE 2.2.0, so never compare suffixes with it.
LAST_TAG="$(git tag --list 'v[0-9]*' | grep -v -- '-' | sort -V | tail -1)"
[[ -n "$LAST_TAG" ]] || die "no encuentro ningún tag v* local ni en origin"
LATEST="${LAST_TAG#v}"
NEW_BASE="${NEW_VERSION%%-*}"
if [[ "$NEW_BASE" == "$LATEST" ]]; then
  die "${NEW_VERSION} parte de la base ${LATEST}, que ya está releaseada (${LAST_TAG})"
fi
if [[ "$(printf '%s\n%s\n' "$LATEST" "$NEW_BASE" | sort -V | head -1)" != "$LATEST" ]]; then
  die "${NEW_VERSION} no es mayor que ${LATEST} (${LAST_TAG} es el último release)"
fi

# 5. Major-series guard. The inter-crate pins are path+version (`^old`, by #1339
#    precedent), so bumping the workspace into a new major series stops the whole
#    workspace from resolving — measured, not inferred: version=3.0.0 with pins at
#    2.0.0 dies with `failed to select a version for the requirement
#    webfang_core = "^2.0.0"`. Fail here with the fix named instead of leaving a
#    branch that no cargo command can build.
PIN_LINES="$(grep -nE '^\s*webfang_(core|ai|mcp)\s*=\s*\{[^}]*version\s*=' "$ROOT_TOML" || true)"
[[ -n "$PIN_LINES" ]] || die "no encuentro los pins inter-crate versionados en ${ROOT_TOML}"
PIN_COUNT="$(printf '%s\n' "$PIN_LINES" | wc -l | tr -d ' ')"
PIN_VERSION="$(printf '%s\n' "$PIN_LINES" | sed -nE 's/.*version *= *"([^"]*)".*/\1/p' | head -1)"
[[ -n "$PIN_VERSION" ]] || die "los pins inter-crate no tienen un version = \"...\" legible"
if [[ "${PIN_VERSION%%.*}" != "$NEW_MAJOR" ]]; then
  die "un bump a ${NEW_VERSION} rompe la resolución del workspace: los ${PIN_COUNT} pins inter-crate están en ^${PIN_VERSION} y no admiten la serie ${NEW_MAJOR}. Actualizá esos pins a \"${NEW_MAJOR}.0.0\" primero (líneas: $(printf '%s' "$PIN_LINES" | cut -d: -f1 | paste -sd, -)), después cortá el mayor."
fi

# 6. Exactly one workspace version to move. The bump below is a global sed on
#    `^version = `; a second such line in the root manifest would be clobbered.
VERSION_LINES="$(grep -cE '^version = ' "$ROOT_TOML" || true)"
[[ "$VERSION_LINES" == "1" ]] \
  || die "esperaba exactamente 1 línea «^version =» en ${ROOT_TOML}, encontré ${VERSION_LINES}: revisá a mano antes de bumpiar"

CURRENT="$(grep -m1 -E '^version = ' "$ROOT_TOML" | sed -E 's/^version = "(.*)"/\1/')"
[[ "$CURRENT" != "$NEW_VERSION" ]] || die "el workspace ya está en ${NEW_VERSION}"

# 7. Semver signal, never a gate (the categories that force a minor/major live in
#    AGENTS.md -> "Cutting a release"; only the maintainer applies release:cut).
SINCE="$(git rev-list --count "${LAST_TAG}..HEAD")"
FEATS="$(git log "${LAST_TAG}..HEAD" --pretty=%s | grep -cE '^feat[(a-z)!]*:' || true)"
BREAKING="$(git log "${LAST_TAG}..HEAD" --pretty=%B | grep -c '^BREAKING CHANGE:' || true)"
CLASS="patch"
[[ "$FEATS" -gt 0 ]] && CLASS="minor"
[[ "$BREAKING" -gt 0 ]] && CLASS="major"
info "${SINCE} commits desde ${LAST_TAG} · ${FEATS} feat · ${BREAKING} breaking → sugiere **${CLASS}** (señal, no gate)"
if [[ "$CLASS" != "major" && "$NEW_MAJOR" != "${CURRENT%%.*}" ]]; then
  info "ojo: la serie mayor cambia de ${CURRENT%%.*} a ${NEW_MAJOR} pero el historial no marca breaking"
fi

# --- CHANGELOG section, rendered by git-cliff (the same cliff.toml release-plz used) ---
command -v git-cliff >/dev/null 2>&1 \
  || die "git-cliff no está en PATH (probá: mise install, o instalá git-cliff)"
[[ -f cliff.toml ]] || die "falta cliff.toml"
grep -q '^## \[Unreleased\]' CHANGELOG.md || die "CHANGELOG.md no tiene la sección [Unreleased]"

SECTION="$(mktemp)"
trap 'rm -f "$SECTION"' EXIT
git-cliff --config cliff.toml --unreleased --tag "v${NEW_VERSION}" -s header -o "$SECTION" 2>/dev/null
grep -qE '^## \[' "$SECTION" || die "git-cliff no generó entrada: no hay commits releasables desde ${LAST_TAG}"
info "CHANGELOG: $(grep -cE '^### ' "$SECTION") secciones, $(grep -cE '^- ' "$SECTION") entradas para ${NEW_VERSION}"

# Preview only from here on when --dry-run.
if [[ "$DRY" == "1" ]]; then
  info "--dry-run: checklist verde, no toco nada. Primeras entradas que se escribirían:"
  head -14 "$SECTION"
  echo
  info "siguiente paso real: cut-release.sh ${NEW_VERSION}"
  exit 0
fi

# Layout is spliced by hand rather than with `git-cliff --prepend`, because
# --prepend puts the new release ABOVE `## [Unreleased]` (and, with -s header,
# buries the file header mid-document — both measured). The repo convention from
# #1339 is: header, [Unreleased], newest release, history.
OUT="$(mktemp)"
awk 'NR==FNR { sec[++n] = $0; next }
     { print }
     /^## \[Unreleased\]$/ && !done { for (i = 1; i <= n; i++) print sec[i]; done = 1 }' \
  "$SECTION" CHANGELOG.md >"$OUT"
mv "$OUT" CHANGELOG.md

# --- Bump root workspace version (crates inherit via version.workspace = true) ---
sed -i -E "s/^version = \".*\"/version = \"${NEW_VERSION}\"/" "$ROOT_TOML"

# --- Lockfile, in sync (release builds use --locked) ---
# `cargo metadata` refreshes the lock without compiling it: measured <1s for the
# 6 workspace entries, exit 0. It also re-checks resolution, so an illegal bump
# fails loudly here. `cargo check --workspace` (what bump-support-patch.sh uses)
# is correct but pays a full build for a lock rewrite.
env -u RUSTC_WRAPPER cargo metadata --offline --format-version 1 >/dev/null 2>&1 \
  || die "cargo metadata no pudo resolver con ${NEW_VERSION}: el árbol quedó restaurable con git checkout -- ${ROOT_TOML} Cargo.lock"

# --- Version snapshot (a bump without its .snap is red CI) ---
env -u RUSTC_WRAPPER cargo nextest run -p webfang_core test_version >/dev/null 2>&1 || true
if compgen -G "${SNAP_DIR}/*.snap.new" >/dev/null; then
  git checkout -- "$ROOT_TOML" Cargo.lock CHANGELOG.md
  die "hay .snap.new pendiente (probablemente ${SNAP#*/}).
  Revisá el diff:  cargo insta review
  Aceptalo:        cargo insta accept
  Y re-ejecutá:    scripts/cut-release.sh ${NEW_VERSION}
(los manifests quedaron restaurados; el árbol está limpio)"
fi

# --- Commit (explicit stage; never -A inside an agent worktree) ---
git add "$ROOT_TOML" Cargo.lock CHANGELOG.md
[[ -f "$SNAP" ]] && git add "$SNAP"
if git diff --cached --quiet; then
  info "nada nuevo que commitear (re-ejecución tras accept)."
else
  git commit -m "chore: bump to v${NEW_VERSION}"
  info "bump a v${NEW_VERSION} commiteado."
fi

# --- Named continuation: every refusal and every exit point says what to run.
# Labels mirror what the deleted automated Release PR carried (release-plz.toml
# pr_labels), so a hand-cut release is indistinguishable from an automated one:
# `release` for display, and exactly one `type:*` for pr-validation.yml.
info "siguiente paso:"
info "  git push -u origin ${BRANCH} && gh pr create --base main --head ${BRANCH} \\"
info "    --label release --label type:chore --title 'chore: release v${NEW_VERSION}' \\"
info "    --body 'Closes #1387'"
info "al mergearse, el job «Release-plz release» de release-plz.yml pushea v${NEW_VERSION} y release.yml publica los binarios."
