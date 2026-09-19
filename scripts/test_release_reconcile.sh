#!/usr/bin/env bash
#
# Semantics harness for the release reconciliation scripts (webfang#1484).
#
# Two properties here are behavioral and cannot be proven by grepping:
#   1. the dispatch is actually RETRIED on a transient failure — a retry that
#      does not retry is worse than no retry, because it buys false confidence;
#   2. the sweep never dispatches a HUMAN tag — v1.0.0 has no GitHub Release, and
#      release.yml checks out the tag it is handed, so dispatching it would
#      publish binaries built from unrelated code.
#
# Everything runs against mktemp fixtures: a synthetic git repo and a fake `gh`
# on PATH. No network, no real tags, no real repository state.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# `gh release view <tag>` succeeds iff $STATE/assets-<tag> exists, and prints its
# content as the asset list. `gh workflow run` records the dispatched tag and
# fails while $STATE/fail_remaining is above zero, which is what lets the harness
# distinguish "retried" from "gave up".
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"
case "${1:-} ${2:-}" in
  "release view")
    tag="${3:-}"
    if [[ -f "$state/assets-$tag" ]]; then
      cat "$state/assets-$tag"
      exit 0
    fi
    exit 1
    ;;
  "workflow run")
    tag=""
    seen_ref=0
    for arg in "$@"; do
      [[ "$arg" == "--ref" ]] && seen_ref=1
      [[ "$arg" == tag=* ]] && tag="${arg#tag=}"
    done
    # Mark the record if the dispatch pinned --ref. The sweep must run the CURRENT
    # release.yml from the default branch; pinning --ref would run the historical
    # definition bundled with an old tag, so the tests below can catch a regression
    # by reading this marker instead of trusting the comment.
    (( seen_ref )) && tag="REF:$tag"
    printf '%s\n' "$tag" >>"$state/dispatched"
    remaining=0
    [[ -f "$state/fail_remaining" ]] && remaining="$(cat "$state/fail_remaining")"
    if (( remaining > 0 )); then
      printf '%s' "$((remaining - 1))" >"$state/fail_remaining"
      exit 1
    fi
    exit 0
    ;;
  *)
    echo "fake gh: unhandled invocation: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

# ─── Fixture repo ────────────────────────────────────────────────────────────
FIXTURE="$WORK/repo"
git -C "$WORK" init -q repo
git -C "$FIXTURE" config user.email tester@example.com
git -C "$FIXTURE" config user.name tester
git -C "$FIXTURE" commit -q --allow-empty -m "chore: init"
# The trusted tag: annotated, tagged by the Actions bot, release-plz subject.
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v9.9.9 -m "chore: Release package webfang_core version 9.9.9"
# The human tag: same shape, wrong provenance. v1.0.0 has no GitHub Release.
git -C "$FIXTURE" tag -a v1.0.0 -m "v1.0.0"
mkdir -p "$FIXTURE/scripts"
cp "$REPO_ROOT/scripts/release-plz-tags.sh" \
   "$REPO_ROOT/scripts/ensure-release.sh" \
   "$REPO_ROOT/scripts/reconcile-releases.sh" "$FIXTURE/scripts/"

COMPLETE_ASSETS="$STATE/assets-v9.9.9"
cat >"$COMPLETE_ASSETS" <<'ASSETS'
webfang-x86_64-unknown-linux-gnu.tar.gz
webfang-aarch64-unknown-linux-gnu.tar.gz
webfang-aarch64-apple-darwin.tar.gz
webfang-x86_64-pc-windows-msvc.zip
SHA256SUMS.txt
ASSETS

# ─── Harness ─────────────────────────────────────────────────────────────────
PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"
    FAIL=$((FAIL + 1))
  fi
}

# Runs a fixture script the way CI does: fake gh first on PATH, backoff disabled
# so the retry cases do not spend real time sleeping.
run_fixture() {
  local script="$1"
  shift
  ( cd "$FIXTURE" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    GITHUB_REPOSITORY="owner/repo" \
    RELEASE_DISPATCH_BACKOFF_SECONDS=0 \
    bash "scripts/$script" "$@" )
}

reset_state() {
  rm -f "$STATE/dispatched"
  printf '0' >"$STATE/fail_remaining"
}
dispatched() { [[ -f "$STATE/dispatched" ]] && cat "$STATE/dispatched" || true; }

echo "test_release_reconcile: behavioral checks for the reconciliation path"

# 1. The trust predicate over HISTORY must exclude the human tag. This is the
#    property that decides whether the sweep can publish wrong binaries.
check "--all lists only the trusted tag" \
  "v9.9.9" \
  "$( cd "$FIXTURE" && bash scripts/release-plz-tags.sh --all | tr '\n' ' ' | sed 's/ $//' )"

# 2. A complete Release is a no-op: nothing to dispatch.
reset_state
if run_fixture ensure-release.sh v9.9.9 >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "complete Release -> exit 0" "0" "$rc"
check "complete Release -> no dispatch" "" "$(dispatched | tr '\n' ' ' | sed 's/ $//')"

# 3. An incomplete Release with two transient failures must still end green after
#    the retry — and the retry must be observable as three attempts.
reset_state
printf '%s\n' "webfang-x86_64-unknown-linux-gnu.tar.gz" >"$COMPLETE_ASSETS"
printf '2' >"$STATE/fail_remaining"
if run_fixture ensure-release.sh v9.9.9 >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "transient failures -> retried to success (exit 0)" "0" "$rc"
check "transient failures -> 3 attempts recorded" "3" "$(dispatched | grep -c . || true)"

# 4. A terminal failure must be loud and name the tag, never exit 0.
reset_state
printf '99' >"$STATE/fail_remaining"
if err="$(run_fixture ensure-release.sh v9.9.9 2>&1 >/dev/null)"; then rc=0; else rc=$?; fi
check "terminal failure -> exit 1" "1" "$rc"
case "$err" in
  *v9.9.9*) check "terminal failure -> names the tag" "yes" "yes" ;;
  *) check "terminal failure -> names the tag" "yes" "no" ;;
esac

# 5. The sweep must dispatch the trusted tag ONLY. If this ever passes with
#    v1.0.0 dispatched, the sweep can publish binaries built from unrelated code.
reset_state
rm -f "$COMPLETE_ASSETS" # neither tag has a Release now
if run_fixture reconcile-releases.sh >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "sweep -> exit 0" "0" "$rc"
check "sweep -> dispatched exactly once" "v9.9.9" "$(dispatched | tr '\n' ' ' | sed 's/ $//')"
if dispatched | grep -qxF 'v1.0.0'; then
  check "sweep -> never dispatches the human tag" "yes" "no"
else
  check "sweep -> never dispatches the human tag" "yes" "yes"
fi
if dispatched | grep -q '^REF:'; then
  check "sweep -> dispatch does not pin --ref" "current release.yml" "historical release.yml"
else
  check "sweep -> dispatch does not pin --ref" "current release.yml" "current release.yml"
fi

# 6. The sweep must be idempotent: a second pass over a now-complete Release
#    dispatches nothing.
reset_state
printf '%s\n' "webfang-x86_64-unknown-linux-gnu.tar.gz" >"$COMPLETE_ASSETS"
run_fixture reconcile-releases.sh >/dev/null 2>&1 || true
before="$(dispatched | grep -c . || true)"
cat >"$COMPLETE_ASSETS" <<'ASSETS'
webfang-x86_64-unknown-linux-gnu.tar.gz
webfang-aarch64-unknown-linux-gnu.tar.gz
webfang-aarch64-apple-darwin.tar.gz
webfang-x86_64-pc-windows-msvc.zip
SHA256SUMS.txt
ASSETS
run_fixture reconcile-releases.sh >/dev/null 2>&1 || true
after="$(dispatched | grep -c . || true)"
check "sweep is idempotent once complete (no new dispatch)" "$before" "$after"

echo "test_release_reconcile: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: retry and sweep tag-selection behave as specified."
