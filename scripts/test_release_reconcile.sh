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
# Four commands are modelled, each anchored to a measured fact (webfang#1535):
#
#   gh release view <tag>
#       The by-name asset view. Measured EMPTY for a COMPLETE release (id
#       394502300 / v2.3.0) while .../releases/<id>/assets returned all 5
#       (control v2.2.0: 5 everywhere). The fixture therefore answers success
#       with an EMPTY list whenever the release exists: a completeness check
#       that still reads assets from this path reports a complete release as
#       incomplete — exactly the spurious dispatch of run 35844323775.
#
#   gh api repos/<repo>/releases/tags/<tag>
#       `--jq '.id'` returns a valid id EVEN WHEN the by-tag object's embedded
#       .assets are empty (measured on 394502300: id fine, assets empty). Any
#       other projection of the by-tag object (notably .assets) returns EMPTY
#       for the same reason. Existence may be answered from here; assets may not.
#       A missing release 404s the way real gh does (body on stdout,
#       "gh: Not Found (HTTP 404)" on stderr, exit 1).
#
#   gh api repos/<repo>/releases/<id>/assets
#       The one endpoint measured to return the FULL asset list (5 for
#       394502300). Serves the $STATE/assets-<tag> fixture file. The fixture
#       id embeds the tag (`fixture-<tag>`) so this handler can reverse it
#       without a global table — the id is opaque to the caller.
#
#   gh workflow run
#       Records the dispatched tag and fails while $STATE/fail_remaining is
#       above zero, which is what lets the harness distinguish "retried" from
#       "gave up".
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"
case "${1:-}" in
  release)
    [[ "${2:-}" == "view" ]] || { echo "fake gh: unhandled invocation: $*" >&2; exit 99; }
    tag="${3:-}"
    if [[ -f "$state/assets-$tag" ]]; then
      # Release exists — but its embedded .assets are EMPTY (measured on
      # release 394502300 / v2.3.0; see the fixture header). Exit 0 with no
      # asset lines: that is the incident-time shape of this view.
      exit 0
    fi
    exit 1
    ;;
  api)
    path="${2:-}"
    jq_expr=""
    [[ "${3:-}" == "--jq" ]] && jq_expr="${4:-}"
    not_found() {
      # Faithful to real gh (measured): body on stdout, error on stderr, rc=1.
      printf '{"message":"Not Found","status":"404"}\n'
      echo "gh: Not Found (HTTP 404)" >&2
      exit 1
    }
    case "$path" in
      repos/*/releases/tags/*)
        tag="${path##*/}"
        [[ -f "$state/assets-$tag" ]] || not_found
        case "$jq_expr" in
          .id)
            # The id resolves even when embedded assets are empty — that is
            # precisely why existence may be answered from the by-tag view.
            printf 'fixture-%s\n' "$tag"
            ;;
          *)
            # Any other projection — notably .assets — is EMPTY at incident
            # time (release 394502300): a completeness check that reads
            # assets from here sees zero and dispatches spuriously.
            ;;
        esac
        exit 0
        ;;
      repos/*/releases/*/assets)
        idpart="${path#*/releases/}" # <id>/assets
        id="${idpart%/assets}"
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
# release-tag-trust.sh travels with release-plz-tags.sh: the latter SOURCES it for
# the shared trust predicate. Copying only the caller makes the `source` fail inside
# the fixture, `mapfile` swallows the error, and the sweep silently reports "no
# trusted tags" — a red that looks like a missing tag, not a missing file.
cp "$REPO_ROOT/scripts/release-plz-tags.sh" \
   "$REPO_ROOT/scripts/release-tag-trust.sh" \
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

# 7. A BROKEN TRUST PREDICATE MUST NEVER READ AS "nothing to reconcile".
#    This is the regression that made the library split dangerous: release-plz-tags.sh
#    sources release-tag-trust.sh, and a failed `source` exits non-zero with EMPTY
#    stdout. `mapfile` discards that status, so the sweep used to print "No release-plz
#    tags in history; nothing to reconcile." and exit 0 - a green safety net sitting on
#    top of a dead filter (the webfang#1476 shape: failure signal removed, green kept).
#    The fixture is copied into a scratch dir so this check never mutates the real one.
BROKEN="$WORK/repo-broken"
cp -r "$FIXTURE" "$BROKEN"
rm -f "$BROKEN/scripts/release-tag-trust.sh"
if err="$( cd "$BROKEN" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
            GITHUB_REPOSITORY="owner/repo" \
            bash scripts/reconcile-releases.sh 2>&1 )"; then rc=0; else rc=$?; fi
check "dead trust predicate -> sweep does NOT exit 0" "1" "$rc"
case "$err" in
  *"nothing to reconcile"*) check "dead predicate -> no false 'nothing to reconcile'" "absent" "present" ;;
  *) check "dead predicate -> no false 'nothing to reconcile'" "absent" "absent" ;;
esac
# The producer itself must be loud, not empty-and-nonzero.
rm -f "$FIXTURE/scripts/release-tag-trust.sh"
if err="$( cd "$FIXTURE" && bash scripts/release-plz-tags.sh --all 2>&1 >/dev/null )"; then rc=0; else rc=$?; fi
check "producer with missing library -> non-zero" "1" "$(( rc > 0 ? 1 : 0 ))"
case "$err" in
  *"missing trust predicate library"*) check "producer -> names the missing library" "yes" "yes" ;;
  *) check "producer -> names the missing library" "yes" "no" ;;
esac
# Restore the fixture for any later check; the trap cleans the whole tree anyway.
cp "$REPO_ROOT/scripts/release-tag-trust.sh" "$FIXTURE/scripts/"

# 8. Bug B (webfang#1535): a COMPLETE release whose by-tag view shows EMPTY
#    assets must NOT be dispatched. Measured on release 394502300 / tag
#    v2.3.0: the by-tag/by-name .assets array came back empty while
#    .../releases/<id>/assets returned all 5, so the completeness check called
#    a complete release incomplete and dispatched spuriously (run 35844323775).
#    Two vacuity guards first: the pin below only means something while the
#    fake really models the incident-time asymmetry. Then the behavioural pin:
#    by-tag empty + by-id complete -> exit 0 and NO dispatch.
reset_state
cat >"$COMPLETE_ASSETS" <<'ASSETS'
webfang-x86_64-unknown-linux-gnu.tar.gz
webfang-aarch64-unknown-linux-gnu.tar.gz
webfang-aarch64-apple-darwin.tar.gz
webfang-x86_64-pc-windows-msvc.zip
SHA256SUMS.txt
ASSETS
bytag_view="$(PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
  gh api repos/owner/repo/releases/tags/v9.9.9 --jq '.assets[].name')"
check "fixture: by-tag embedded assets empty (incident 394502300)" "" "$bytag_view"
byid_view="$(PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
  gh api repos/owner/repo/releases/fixture-v9.9.9/assets --jq '.[].name')"
check "fixture: by-id assets complete (5)" "5" "$(grep -c . <<<"$byid_view" || true)"
if run_fixture ensure-release.sh v9.9.9 >/dev/null 2>&1; then rc=0; else rc=$?; fi
check "by-tag empty + by-id complete -> exit 0" "0" "$rc"
check "by-tag empty + by-id complete -> no dispatch" "" "$(dispatched | tr '\n' ' ' | sed 's/ $//')"

# 9. Bug A (webfang#1535): release-plz creates the annotated tag via the
#    GitHub API (POST /git/refs) AFTER actions/checkout ran with
#    fetch-tags: true, and release-plz.yml has no fetch between `Run
#    release-plz` and `Verify the release tag`. Measured on run 35842521329:
#    `git tag --points-at HEAD` on that stale clone saw no tag and
#    outcome=failure hard-failed, while the dispatch job (fresh checkout)
#    worked. verify-release-tag.sh must fetch tags itself before trusting the
#    predicate — covered offline here with a local bare remote whose trusted
#    tag lands only AFTER the clone is taken.
BUG_A_BARE="$WORK/bug-a-remote.git"
BUG_A_SRC="$WORK/bug-a-src"
BUG_A_STALE="$WORK/bug-a-clone"
git init -q --bare -b main "$BUG_A_BARE"
git init -q -b main "$BUG_A_SRC"
git -C "$BUG_A_SRC" config user.email tester@example.com
git -C "$BUG_A_SRC" config user.name tester
git -C "$BUG_A_SRC" commit -q --allow-empty -m "chore: init"
git -C "$BUG_A_SRC" remote add origin "$BUG_A_BARE"
git -C "$BUG_A_SRC" push -q -u origin main
# The clone stands in for actions/checkout: taken BEFORE the tag exists.
git clone -q "$BUG_A_BARE" "$BUG_A_STALE"
mkdir -p "$BUG_A_STALE/scripts"
cp "$REPO_ROOT/scripts/verify-release-tag.sh" \
   "$REPO_ROOT/scripts/release-plz-tags.sh" \
   "$REPO_ROOT/scripts/release-tag-trust.sh" "$BUG_A_STALE/scripts/"
# The trusted tag lands AFTER the clone (release-plz's API-created ref).
git -C "$BUG_A_SRC" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v9.9.9 -m "chore: Release package webfang_core version 9.9.9"
git -C "$BUG_A_SRC" push -q origin v9.9.9

# The fixed verifier fetches and sees it: outcome=failure (the measured run's
# state) plus the API-created tag must end green on the "Verified:" path. The
# pre-fix script hard-failed here — that failure IS the pin.
if out="$(cd "$BUG_A_STALE" && RELEASE_OUTCOME=failure bash scripts/verify-release-tag.sh 2>&1)"; then rc=0; else rc=$?; fi
check "verify: tag created after clone -> exit 0 (fetch before predicate)" "0" "$rc"
case "$out" in
  *"Verified: v9.9.9"*) check "verify: tag created after clone -> Verified path" "yes" "yes" ;;
  *) check "verify: tag created after clone -> Verified path" "yes" "no" ;;
esac

# The fetch is load-bearing, not decoration: remove the remote (fetch fails)
# and the now-local tag. outcome=success must NOT turn the unverifiable empty
# list into a green "Nothing to release" — failed fetch + no visible tag fails
# closed, which is the second half of the Bug A contract.
git -C "$BUG_A_STALE" tag -d v9.9.9 >/dev/null
git -C "$BUG_A_STALE" remote remove origin
if out="$(cd "$BUG_A_STALE" && RELEASE_OUTCOME=success bash scripts/verify-release-tag.sh 2>&1)"; then rc=0; else rc=$?; fi
check "verify: failed fetch + no local tag + outcome=success -> exit 1" "1" "$rc"
case "$out" in
  *"Nothing to release"*) check "verify: failed fetch never green-lights 'nothing to release'" "absent" "present" ;;
  *) check "verify: failed fetch never green-lights 'nothing to release'" "absent" "absent" ;;
esac

echo "test_release_reconcile: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: retry and sweep tag-selection behave as specified."
