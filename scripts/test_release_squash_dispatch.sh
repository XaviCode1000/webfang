#!/usr/bin/env bash
#
# Semantics harness for the squash-merge dispatch defect (webfang#1674).
#
# The defect cannot be proven by grepping: it is a property of WHICH COMMIT the
# tag resolves to relative to HEAD, and of what the run REPORTS when it finds
# nothing. So the fixture below reproduces the measured v2.4.0 shape exactly:
#
#   main            A ── B ── C   (C = the squash-merge commit, HEAD)
#                     \
#   release branch   D            (tag v9.9.9 points here, PRE-squash)
#
# `git tag --points-at HEAD` on that fixture returns nothing — which is the bug,
# asserted as a vacuity guard before every check that follows, so these tests
# cannot silently degrade into testing the happy fast-forward path.
#
# Properties pinned here:
#   1. a squash-merged Release PR still gets its tag dispatched, with
#      expected_sha = the PRE-SQUASH commit the tag actually points at (L1.1);
#   2. a human tag is still never dispatched (publishing unrelated binaries);
#   3. a run that dispatches nothing is a VISIBLE skip, not a silent success —
#      the `::notice::` marker and the job summary must both appear;
#   4. a release commit on HEAD with NO trusted tag anywhere is a hard error,
#      not a green no-op: that tag can never be dispatched by anyone later;
#   5. idempotency: a tag that already has a complete Release is left alone.
#
# Everything runs offline against mktemp fixtures and a fake `gh`. No network,
# no real tags, no real repository state.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

COMPLETE_ASSETS_LINES=(
  webfang-x86_64-unknown-linux-gnu.tar.gz
  webfang-aarch64-unknown-linux-gnu.tar.gz
  webfang-aarch64-apple-darwin.tar.gz
  webfang-x86_64-pc-windows-msvc.zip
  SHA256SUMS.txt
)

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# Same contract as the fake in test_release_reconcile.sh (and for the same
# measured reason, webfang#1535): existence is answered from the by-tag view,
# assets only ever from .../releases/<id>/assets.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"
case "${1:-}" in
  release)
    [[ "${2:-}" == "view" ]] || { echo "fake gh: unhandled invocation: $*" >&2; exit 99; }
    [[ -f "$state/assets-${3:-}" ]] || exit 1
    exit 0
    ;;
  api)
    path="${2:-}"
    jq_expr=""
    [[ "${3:-}" == "--jq" ]] && jq_expr="${4:-}"
    not_found() {
      printf '{"message":"Not Found","status":"404"}\n'
      echo "gh: Not Found (HTTP 404)" >&2
      exit 1
    }
    case "$path" in
      repos/*/releases/tags/*)
        tag="${path##*/}"
        [[ -f "$state/assets-$tag" ]] || not_found
        [[ "$jq_expr" == ".id" ]] && printf 'fixture-%s\n' "$tag"
        exit 0
        ;;
      repos/*/releases/*/assets)
        id="${path#*/releases/}"
        id="${id%/assets}"
        case "$id" in
          fixture-*)
            tag="${id#fixture-}"
            if [[ -f "$state/assets-$tag" ]]; then
              cat "$state/assets-$tag"
              exit 0
            fi
            ;;
        esac
        not_found
        ;;
      *)
        echo "fake gh: unhandled api endpoint: $path" >&2
        exit 99
        ;;
    esac
    ;;
  workflow)
    [[ "${2:-}" == "run" ]] || { echo "fake gh: unhandled invocation: $*" >&2; exit 99; }
    tag=""
    expected_sha=""
    seen_ref=0
    for arg in "$@"; do
      [[ "$arg" == "--ref" ]] && seen_ref=1
      [[ "$arg" == tag=* ]] && tag="${arg#tag=}"
      [[ "$arg" == expected_sha=* ]] && expected_sha="${arg#expected_sha=}"
    done
    (( seen_ref )) && tag="REF:$tag"
    printf '%s\n' "$tag" >>"$state/dispatched"
    if [[ -n "$expected_sha" ]]; then
      printf '%s\n' "$expected_sha" >>"$state/dispatched_sha"
    else
      printf 'MISSING\n' >>"$state/dispatched_sha"
    fi
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

# ─── Fixture repo: the squash-merge shape ────────────────────────────────────
FIXTURE="$WORK/repo"
git init -q -b main "$FIXTURE"
git -C "$FIXTURE" config user.email tester@example.com
git -C "$FIXTURE" config user.name tester
git -C "$FIXTURE" commit -q --allow-empty -m "chore: init"
git -C "$FIXTURE" commit -q --allow-empty -m "feat: something"
# The Release PR's head commit: this is where release-plz will tag. It is NOT
# reachable from main, exactly like a PR head at squash-merge time.
git -C "$FIXTURE" checkout -q -b release-branch
git -C "$FIXTURE" commit -q --allow-empty -m "chore: release v9.9.9"
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v9.9.9 -m "chore: Release package webfang_core version 9.9.9"
git -C "$FIXTURE" checkout -q main
# The squash merge: a NEW commit on main carrying the release subject.
git -C "$FIXTURE" commit -q --allow-empty -m "chore: release v9.9.9 (#1674)"
# A human tag with no Release, on an unrelated commit — v1.0.0's real shape.
# Deliberately NOT on HEAD: the vacuity guard below must show a completely
# empty `git tag --points-at HEAD`, which is what the incident looked like.
git -C "$FIXTURE" -c user.name='maintainer' tag -a v1.0.0 -m "v1.0.0" \
  "$(git -C "$FIXTURE" rev-list --max-parents=0 HEAD)"

mkdir -p "$FIXTURE/scripts"
cp "$REPO_ROOT/scripts/sweep-releases.sh" \
   "$REPO_ROOT/scripts/release-plz-tags.sh" \
   "$REPO_ROOT/scripts/release-tag-trust.sh" \
   "$REPO_ROOT/scripts/ensure-release.sh" \
   "$REPO_ROOT/scripts/reconcile-releases.sh" "$FIXTURE/scripts/"

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
check_contains() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] not found in output\n' "$label" "$needle"
    FAIL=$((FAIL + 1))
  fi
}
check_absent() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] unexpectedly present\n' "$label" "$needle"
    FAIL=$((FAIL + 1))
  fi
}

# Runs the sweep the way the dispatch job does: fake gh first on PATH, backoff
# disabled, and a step-summary file so the visible-skip contract is observable.
# Output lands in RUN_OUT and the script's exit status is RETURNED, so a failing
# run can be inspected without `set -e` killing the harness.
SUMMARY="$WORK/step_summary"
RUN_OUT=""
run_sweep() {
  local head_subject="${1:-}"
  : >"$SUMMARY"
  rm -f "$STATE/dispatched" "$STATE/dispatched_sha"
  RUN_OUT="$( cd "$FIXTURE" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    GITHUB_REPOSITORY="owner/repo" \
    GITHUB_STEP_SUMMARY="$SUMMARY" \
    RELEASE_DISPATCH_BACKOFF_SECONDS=0 \
    bash scripts/sweep-releases.sh --head-commit-subject "$head_subject" 2>&1 )"
}
run_reconciler() {
  : >"$SUMMARY"
  rm -f "$STATE/dispatched" "$STATE/dispatched_sha"
  RUN_OUT="$( cd "$FIXTURE" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    GITHUB_REPOSITORY="owner/repo" \
    GITHUB_STEP_SUMMARY="$SUMMARY" \
    RELEASE_DISPATCH_BACKOFF_SECONDS=0 \
    bash scripts/reconcile-releases.sh 2>&1 )"
}
mark_complete() { printf '%s\n' "${COMPLETE_ASSETS_LINES[@]}" >"$STATE/assets-$1"; }
dispatched() { [[ -f "$STATE/dispatched" ]] && cat "$STATE/dispatched" || true; }
dispatched_sha() { [[ -f "$STATE/dispatched_sha" ]] && cat "$STATE/dispatched_sha" || true; }
summary_text() { cat "$SUMMARY" 2>/dev/null || true; }

echo "test_release_squash_dispatch: behavioral checks for the squash-merge dispatch path (webfang#1674)"

# 1. VACUITY GUARD. The fixture must reproduce the incident: no trusted tag at
#    HEAD, because the tag sits on the pre-squash commit. If this ever passes
#    "v9.9.9", every check below is silently testing the happy fast-forward
#    path and the harness is worthless.
check "fixture reproduces the bug: no tag at HEAD" "" \
  "$( git -C "$FIXTURE" tag --points-at HEAD | tr '\n' ' ' | sed 's/ $//' )"
check "fixture: the trusted tag is NOT an ancestor of HEAD" "false" \
  "$( git -C "$FIXTURE" merge-base --is-ancestor 'refs/tags/v9.9.9^{commit}' HEAD && echo true || echo false )"

# 2. THE DEFECT. A squash-merged Release PR still gets dispatched, and the
#    dispatch carries the PRE-SQUASH commit the tag actually points at — that
#    is what release.yml's L1.1 preflight compares against (webfang#1540).
#    No Release exists yet, so the sweep must dispatch.
out=""
rc=0
run_sweep "chore: release v9.9.9 (#1674)" || rc=$?
out="$RUN_OUT"
check "squash-merged release -> tag dispatched" "v9.9.9" \
  "$(dispatched | tr '\n' ' ' | sed 's/ $//')"
check "squash-merged release -> expected_sha is the PRE-SQUASH commit" \
  "$(git -C "$FIXTURE" rev-parse 'refs/tags/v9.9.9^{commit}')" \
  "$(dispatched_sha | tr '\n' ' ' | sed 's/ $//')"
check_absent "squash-merged release -> human tag never dispatched" "v1.0.0" "$(dispatched)"
check_absent "squash-merged release -> dispatch does not pin --ref" "REF:" "$(dispatched)"

# 3. THE DAILY BACKSTOP MUST AGREE. The immediate path and the reconciler
#    resolve the same candidates through the same shared core, so the two
#    cannot disagree about what still needs a Release (acceptance criterion 2).
rc=0
run_reconciler || rc=$?
out="$RUN_OUT"
check "reconciler dispatches the same squash-merged tag" "v9.9.9" \
  "$(dispatched | tr '\n' ' ' | sed 's/ $//')"

# 4. THE FALSE GREEN IS CLOSED (a). When nothing needs dispatching, the run
#    must SAY SO: a ::notice:: annotation plus a job summary. The old code
#    printed one unremarkable line and exited 0 — a 0-second success that read
#    as "binaries were built".
mark_complete v9.9.9
rc=0
run_sweep "chore: release v9.9.9 (#1674)" || rc=$?
out="$RUN_OUT"
check "complete release -> exit 0" "0" "$rc"
check "complete release -> nothing dispatched" "" "$(dispatched | tr '\n' ' ' | sed 's/ $//')"
check_contains "no-op run -> emits a ::notice:: skip" "::notice title=Release dispatch" "$out"
check_contains "no-op run -> job summary records the skip" "explicit skip" "$(summary_text)"
check_absent "no-op run -> no longer prints the old silent green line" \
  "No release-plz tag points at HEAD" "$out"

# 5. THE FALSE GREEN IS CLOSED (b). HEAD is a release commit but history holds
#    no trusted tag at all: the tag was never created, so no dispatch is
#    possible now and no later sweep can invent one. That must be RED, not a
#    quiet success. Fixture: delete the only trusted tag.
git -C "$FIXTURE" tag -d v9.9.9 >/dev/null
rc=0
run_sweep "chore: release v9.9.9 (#1674)" || rc=$?
out="$RUN_OUT"
check "release commit with NO trusted tag -> exit 1 (not a false green)" "1" "$rc"
check_contains "release commit with NO tag -> names the cause" "::error::" "$out"
check_contains "release commit with NO tag -> tells the operator what to do" "gh workflow run release.yml" "$out"
check_contains "release commit with NO tag -> job summary says FAILED" "FAILED" "$(summary_text)"

# 6. NOT a release commit and no tags at all is the ordinary non-release push:
#    an explicit, visible skip, never an error. Red there would be noise.
rc=0
run_sweep "feat: unrelated change" || rc=$?
out="$RUN_OUT"
check "non-release push, no tags -> exit 0 (visible skip, not an error)" "0" "$rc"
check_contains "non-release push -> ::notice:: skip emitted" "::notice title=Release dispatch" "$out"

# 7. IDEMPOTENCY REGRESSION (acceptance criterion 3). Restore the trusted tag
#    and give it a complete Release: a sweep must dispatch nothing.
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v9.9.9 -m "chore: Release package webfang_core version 9.9.9" \
  'release-branch^{commit}'
rc=0
run_sweep "chore: release v9.9.9 (#1674)" || rc=$?
out="$RUN_OUT"
check "restored complete release -> exit 0" "0" "$rc"
check "restored complete release -> no dispatch (idempotent)" "" \
  "$(dispatched | tr '\n' ' ' | sed 's/ $//')"

# 8. A dispatch that cannot be retried must be RED here too — the backstop
#    staying bounded depends on the immediate path being loud when it fails.
rm -f "$STATE/assets-v9.9.9"
printf '99' >"$STATE/fail_remaining"
rc=0
run_sweep "chore: release v9.9.9 (#1674)" || rc=$?
out="$RUN_OUT"
check "terminal dispatch failure -> exit 1" "1" "$rc"
check_contains "terminal dispatch failure -> names the tag" "v9.9.9" "$out"

# 9. A dead trust predicate must not read as "nothing to sweep" (the #1476
#    shape), on the immediate path too — that is how a green would be earned
#    over a dead filter.
BROKEN="$WORK/repo-broken"
cp -r "$FIXTURE" "$BROKEN"
rm -f "$BROKEN/scripts/release-tag-trust.sh"
printf '0' >"$STATE/fail_remaining"
out="$( cd "$BROKEN" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
        GITHUB_REPOSITORY="owner/repo" \
        bash scripts/sweep-releases.sh --head-commit-subject "feat: x" 2>&1 )" && rc=0 || rc=$?
check "dead trust predicate -> sweep does NOT exit 0" "1" "$rc"
check_absent "dead predicate -> no visible 'nothing to publish' skip" \
  "::notice title=Release dispatch" "$out"

echo "test_release_squash_dispatch: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: a squash-merged Release PR is dispatched, and a run with nothing to do is visibly a skip."
