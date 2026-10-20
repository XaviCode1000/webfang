#!/usr/bin/env bash
# Semantics harness for the release-pins ORCHESTRATOR.
#
# Companion to scripts/test_bump_docs_version_pins.sh, which pins the semantics
# of the WRITER. This file pins the semantics of the step the release flow
# invokes — scripts/apply_release_pins.sh — which is what makes a Release PR
# executable without a human commit.
#
# The orchestrator does git surgery: it finds the open Release PR, checks its
# branch out in a throwaway worktree, runs the writer there, commits and
# pushes. Each of those steps can fail in a way that is invisible from the
# outside, so what matters here is behaviour, not structure:
#
#   (a) the happy path leaves the remote branch carrying the pins of the
#       workspace version, with EXACTLY ONE new commit;
#   (b) a second run adds NO commit — the flow re-runs the orchestrator on
#       every push to `main` (release-plz regenerates the Release PR branch
#       each time), so anything else would push an empty commit forever;
#   (c) no open Release PR is a no-op with exit 0: release-plz decided there
#       is no release to prepare, and failing here would red every push to
#       `main` for nothing;
#   (d) when the writer fails (an ambiguous line), the orchestrator exits
#       non-zero and pushes NOTHING — a failed bump must never land as a
#       partial commit that leaves the Release PR unexplainably red.
#
# Everything runs offline: a local bare repo as the remote, a real clone, a
# real branch, and a fake `gh` on PATH. No network, no GitHub, and no mutation
# of the repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

ORCHESTRATOR="scripts/apply_release_pins.sh"

# The fake `gh` emulates `--jq` with the real jq, exactly like the fake in
# scripts/test_merge_when_green_breaking.sh: the orchestrator's own filter is
# therefore evaluated for real, so a selection bug (picking the wrong Release
# PR) shows up here instead of in production.
if ! command -v jq >/dev/null 2>&1; then
  echo "::error::this harness needs jq to emulate 'gh --jq'" >&2
  exit 1
fi
if [[ ! -f "$REPO_ROOT/$ORCHESTRATOR" ]]; then
  echo "::error::the harness has no subject: $ORCHESTRATOR is missing" >&2
  printf '\nharness: 0 ok, 1 fail\n'
  exit 1
fi

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the two reads the orchestrator performs and nothing else: the list of
# open PRs and the state of one PR. An unhandled invocation exits 99, so a
# change in the orchestrator's gh usage fails loudly instead of silently
# reading a stale fixture.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_GH_STATE"

emit() { # emit <json> [<jq-expr>]
  if [[ -n "${2:-}" ]]; then printf '%s' "$1" | jq -r "$2"; else printf '%s\n' "$1"; fi
}

case "${1:-}" in
  pr)
    sub="${2:-}"
    shift 2
    pr_number=""
    if [[ $# -gt 0 && "${1:-}" != --* ]]; then pr_number="$1"; shift; fi
    fields=""; jqexpr=""; state_filter=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
        --state) shift; state_filter="${1:-}" ;;
      esac
      shift
    done
    case "$sub" in
      list)
        [[ -f "$state/prs.json" ]] || { echo "fake gh: missing prs.json fixture" >&2; exit 99; }
        json="$(cat "$state/prs.json")"
        # 'gh pr list --state X' returns only the PRs in that state. GitHub
        # reports the state in upper case ('OPEN'); the flag is lower case.
        if [[ -n $state_filter ]]; then
          json="$(printf '%s' "$json" | jq -c --arg s "$state_filter" \
            '[.[] | select((.state | ascii_downcase) == ($s | ascii_downcase))]')"
        fi
        if [[ -n $jqexpr ]]; then printf '%s' "$json" | jq -r "$jqexpr"; else printf '%s\n' "$json"; fi
        ;;
      view)
        [[ -f "$state/prs.json" ]] || { echo "fake gh: missing prs.json fixture" >&2; exit 99; }
        pr_state="$(jq -r --argjson n "${pr_number:-0}" \
          '[.[] | select(.number == $n)][0].state // "OPEN"' "$state/prs.json")"
        emit "{\"number\":${pr_number:-0},\"state\":\"$pr_state\"}" "$jqexpr"
        ;;
      *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
    esac
    ;;
  *) echo "fake gh: unhandled invocation: $*" >&2; exit 99 ;;
esac
FAKE
chmod +x "$BIN/gh"

# ─── Fixtures ───────────────────────────────────────────────────────────────
# setup_release_repo <name> <workspace-version> <stale-pin> [ambiguous] [remote-style]
# Builds a bare remote plus a clone whose `chore/release-*` branch carries a
# BUMPED workspace version and a STALE pin — the exact state a Release PR is in
# right after release-plz regenerates it.
#
# `remote-style` is `path` (default) or `file`. The distinction is not
# cosmetic: a bare path is a LOCAL remote (no credentials needed), while a
# `file://` URL is a network-shaped remote, so the orchestrator takes the
# credentialed path — the one that has to authenticate and therefore the one
# where a token could leak.
setup_release_repo() {
  local name="$1" version="$2" pin="$3" ambiguous="${4:-no}" remote_style="${5:-path}"
  local dir="$WORK/$name"

  mkdir -p "$dir/clone/scripts" "$dir/clone/docs/src"
  cp "$REPO_ROOT/scripts/bump_docs_version_pins.sh" "$dir/clone/scripts/"
  cp "$REPO_ROOT/$ORCHESTRATOR" "$dir/clone/scripts/"
  chmod +x "$dir/clone/scripts/bump_docs_version_pins.sh" "$dir/clone/scripts/apply_release_pins.sh"

  cat > "$dir/clone/Cargo.toml" <<EOF
[workspace]
members = []

[workspace.package]
version = "$version"
edition = "2021"
EOF

  if [[ "$ambiguous" == "yes" ]]; then
    printf 'VERSION=v%s\n' "$pin" > "$dir/clone/README.md"
    printf '# Chapter\n\nMigrated from v2.4.0 to v3.0.0 in this release.\n' > "$dir/clone/docs/src/a.md"
  else
    printf 'VERSION=v%s\n' "$pin" > "$dir/clone/README.md"
    printf '# Chapter\n\nPin: v%s\n' "$pin" > "$dir/clone/docs/src/a.md"
  fi

  git init -q --bare "$dir/remote.git"
  git init -q -b main "$dir/clone"
  if [[ "$remote_style" == "file" ]]; then
    git -C "$dir/clone" remote add origin "file://$dir/remote.git"
  else
    git -C "$dir/clone" remote add origin "$dir/remote.git"
  fi
  git -C "$dir/clone" add -A
  git -C "$dir/clone" -c user.name=seed -c user.email=seed@example.com \
    -c commit.gpgsign=false commit -q -m "seed: release branch with a bumped workspace version"
  # The branch is created at HEAD WITHOUT checking it out: `git checkout` is
  # forbidden in this repo, and a fixture does not need a branch switch.
  git -C "$dir/clone" branch "chore/release-$version"
  git -C "$dir/clone" push -q origin main "chore/release-$version"

  printf '%s\n' "$dir"
}

# write_prs <json>: the fixture the fake gh serves for `pr list`.
write_prs() {
  printf '%s\n' "$1" > "$STATE/prs.json"
}

# install_reject_hook <dir> <once|always>: a pre-receive hook on the bare
# remote that rejects the first push (or every push). It reproduces the race
# where release-plz regenerates the branch between the fetch and the push,
# offline and without touching a real remote.
install_reject_hook() {
  local dir="$1" mode="$2"
  cat > "$dir/remote.git/hooks/pre-receive" <<HOOK
#!/usr/bin/env bash
# Fake remote hook: rejects the push to simulate a non fast-forward rejection.
count_file="\$(dirname "\$0")/../reject-count"
n=0
[[ -f "\$count_file" ]] && n="\$(cat "\$count_file")"
n=\$((n + 1))
printf '%s\n' "\$n" > "\$count_file"
if [[ "$mode" == "always" || "\$n" -eq 1 ]]; then
  echo "fake remote: push rejected (simulated non fast-forward)" >&2
  exit 1
fi
exit 0
HOOK
  chmod +x "$dir/remote.git/hooks/pre-receive"
}

# run_orchestrator <clone-dir> [token]: the orchestrator, offline, against the
# fixture. The token is the credential the real flow uses (the App token); it
# is passed empty to exercise the "no credential" path.
run_orchestrator() {
  local dir="$1" token="${2:-}"
  (
    cd "$dir" || exit 1
    PATH="$BIN:$PATH" \
      FAKE_GH_STATE="$STATE" \
      RELEASE_PINS_REPO_ROOT="$dir/clone" \
      RELEASE_PINS_BRANCH_PREFIX="chore/release-" \
      RELEASE_PINS_REMOTE="origin" \
      GITHUB_TOKEN="$token" \
      bash "$dir/clone/scripts/apply_release_pins.sh" >"$WORK/stdout" 2>"$WORK/stderr"
  )
}

# remote_branch_sha <dir>: the head of the release branch on the bare remote.
remote_branch_sha() {
  git -C "$1/remote.git" rev-parse "refs/heads/chore/release-$2"
}

# remote_commit_count <dir>: how many commits the release branch carries.
remote_commit_count() {
  git -C "$1/remote.git" rev-list --count "refs/heads/chore/release-$2"
}

# ─── (a) the happy path ─────────────────────────────────────────────────────
# NEGATION: pre-fix nothing owned the bump, so the Release PR was red until a
# human committed the pins — and release-plz deleted that commit on the next
# push to `main`, so the PR came back red every time.
D="$(setup_release_repo happy 3.0.1 3.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
BEFORE_COUNT="$(remote_commit_count "$D" 3.0.1)"
if run_orchestrator "$D"; then
  AFTER_SHA="$(remote_branch_sha "$D" 3.0.1)"
  AFTER_COUNT="$(remote_commit_count "$D" 3.0.1)"
  README_PIN="$(git -C "$D/remote.git" show "refs/heads/chore/release-3.0.1:README.md")"
  COMMIT_SUBJECT="$(git -C "$D/remote.git" log -1 --format=%s "refs/heads/chore/release-3.0.1")"
  if [[ "$README_PIN" == *"VERSION=v3.0.1"* ]]; then
    pass "(a) the remote branch carries the pins of the workspace version"
  else
    fail "(a) the remote branch carries the pins of the workspace version ($README_PIN)"
  fi
  if [[ "$AFTER_COUNT" -eq $((BEFORE_COUNT + 1)) ]]; then
    pass "(a) exactly one new commit landed on the branch"
  else
    fail "(a) exactly one new commit landed on the branch (count $BEFORE_COUNT -> $AFTER_COUNT)"
  fi
  if [[ "$AFTER_SHA" != "$BEFORE_SHA" ]]; then
    pass "(a) the branch moved"
  else
    fail "(a) the branch moved ($AFTER_SHA)"
  fi
  if [[ "$COMMIT_SUBJECT" == "chore(docs): actualizar los pines de versión a v3.0.1" ]]; then
    pass "(a) the commit message names the version it bumps"
  else
    fail "(a) the commit message names the version it bumps ($COMMIT_SUBJECT)"
  fi
else
  fail "(a) the happy path exits 0 ($(cat "$WORK/stderr"))"
fi

# ─── (b) a second run adds no commit ────────────────────────────────────────
# NEGATION: pre-fix every run of the flow would have to redo the bump by hand;
# an orchestrator that commits on every run would push an empty commit per
# push to `main` and wake the whole CI pipeline for nothing.
if run_orchestrator "$D"; then
  SECOND_COUNT="$(remote_commit_count "$D" 3.0.1)"
  if [[ "$SECOND_COUNT" -eq $((BEFORE_COUNT + 1)) ]]; then
    pass "(b) a second run adds no commit (idempotent)"
  else
    fail "(b) a second run adds no commit (count $SECOND_COUNT)"
  fi
else
  fail "(b) a second run adds no commit (idempotent) ($(cat "$WORK/stderr"))"
fi

# ─── (c) no open Release PR is a no-op ──────────────────────────────────────
# NEGATION: pre-fix nothing consumed the "there is no release to prepare"
# outcome; a flow that treated it as an error would red every push to `main`
# that carries no version bump.
D="$(setup_release_repo no-pr 3.0.1 3.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"CLOSED"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
if run_orchestrator "$D"; then
  if [[ "$(remote_branch_sha "$D" 3.0.1)" == "$BEFORE_SHA" ]]; then
    pass "(c) no open Release PR exits 0 and leaves the branch intact"
  else
    fail "(c) no open Release PR exits 0 and leaves the branch intact"
  fi
else
  fail "(c) no open Release PR exits 0 and leaves the branch intact ($(cat "$WORK/stderr"))"
fi

# ─── (d) a failing writer pushes nothing ────────────────────────────────────
# NEGATION: pre-fix a half-applied bump was indistinguishable from a good
# one; the branch must stay untouched when the writer cannot decide.
D="$(setup_release_repo ambiguous 3.0.1 3.0.0 yes)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
BEFORE_COUNT="$(remote_commit_count "$D" 3.0.1)"
if run_orchestrator "$D"; then
  fail "(d) a failing writer makes the orchestrator exit non-zero"
else
  if [[ "$(remote_branch_sha "$D" 3.0.1)" == "$BEFORE_SHA" \
    && "$(remote_commit_count "$D" 3.0.1)" -eq "$BEFORE_COUNT" ]]; then
    pass "(d) a failing writer exits non-zero and pushes nothing"
  else
    fail "(d) a failing writer exits non-zero and pushes nothing"
  fi
fi

# ─── (e) the most recently updated Release PR wins ──────────────────────────
# Two release PRs can be open at once (a stale one from an aborted release).
# Picking the wrong one would bump the pins on a branch nobody merges.
# NEGATION: pre-fix nothing selected a Release PR at all; picking the first
# match would rewrite a branch that is not the release being prepared.
D="$(setup_release_repo pick-latest 3.0.1 3.0.0)"
write_prs '[{"number":41,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-19T10:00:00Z","state":"OPEN"},
           {"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
if run_orchestrator "$D"; then
  if grep -q '#42' "$WORK/stdout" "$WORK/stderr"; then
    pass "(e) the most recently updated Release PR is the one that gets bumped"
  else
    fail "(e) the most recently updated Release PR is the one that gets bumped ($(cat "$WORK/stdout"))"
  fi
else
  fail "(e) the most recently updated Release PR is the one that gets bumped ($(cat "$WORK/stderr"))"
fi

# ─── (f) a PR that is no longer OPEN is left alone ──────────────────────────
# The PR can be merged between the list and the re-read. Pushing to a merged
# branch would resurrect it.
# NEGATION: pre-fix nothing re-read the PR before pushing; a push to a merged
# branch reopens a release that was already done.
D="$(setup_release_repo closed-pr 3.0.1 3.0.0)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"MERGED"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
if run_orchestrator "$D"; then
  if [[ "$(remote_branch_sha "$D" 3.0.1)" == "$BEFORE_SHA" ]]; then
    pass "(f) a PR that is no longer OPEN exits 0 and pushes nothing"
  else
    fail "(f) a PR that is no longer OPEN exits 0 and pushes nothing"
  fi
else
  fail "(f) a PR that is no longer OPEN exits 0 and pushes nothing ($(cat "$WORK/stderr"))"
fi

# ─── (g) a rejected push is retried exactly once ───────────────────────────
# release-plz regenerates the Release PR branch, so the branch can move
# between the fetch and the push; the push is then rejected as non
# fast-forward. The orchestrator re-fetches, re-runs the writer and pushes
# again — ONCE. A pre-receive hook that rejects the first push and accepts the
# next one reproduces that race offline, with no network.
# NEGATION: pre-fix the bump was applied by a human who re-ran it by hand
# after every rejection; the flow has to absorb the race itself.
D="$(setup_release_repo retry-once 3.0.1 3.0.0)"
install_reject_hook "$D" once
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
BEFORE_COUNT="$(remote_commit_count "$D" 3.0.1)"
if run_orchestrator "$D"; then
  README_PIN="$(git -C "$D/remote.git" show "refs/heads/chore/release-3.0.1:README.md")"
  REJECTS="$(cat "$D/remote.git/reject-count" 2>/dev/null || echo 0)"
  if [[ "$README_PIN" == *"VERSION=v3.0.1"* \
    && "$(remote_commit_count "$D" 3.0.1)" -eq $((BEFORE_COUNT + 1)) \
    && "$REJECTS" -ge 2 ]]; then
    pass "(g) a rejected push is retried once and the branch ends up bumped"
  else
    fail "(g) a rejected push is retried once and the branch ends up bumped (pin=$README_PIN rejects=$REJECTS)"
  fi
else
  fail "(g) a rejected push is retried once and the branch ends up bumped ($(cat "$WORK/stderr"))"
fi

# ─── (h) a push that keeps being rejected fails closed ─────────────────────
# One retry is the contract. A second rejection must be loud, and the branch
# must be untouched: an orchestrator that pushed anyway would publish a bump
# nobody could verify.
# NEGATION: pre-fix a rejection was resolved by re-running the flow by hand;
# the failure has to name itself inside the flow.
D="$(setup_release_repo retry-twice 3.0.1 3.0.0)"
install_reject_hook "$D" always
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
if run_orchestrator "$D"; then
  fail "(h) a push rejected twice exits non-zero"
else
  if [[ "$(remote_branch_sha "$D" 3.0.1)" == "$BEFORE_SHA" ]]; then
    pass "(h) a push rejected twice exits non-zero and the branch is untouched"
  else
    fail "(h) a push rejected twice exits non-zero and the branch is untouched"
  fi
fi

# ─── (i) the token never lands in the repo ──────────────────────────────────
# The credential travels in a per-invocation `http.extraheader`, so it must
# not survive the push: not in the remote URL, not in `.git/config`, and not
# anywhere under `.git/`. A leak here is invisible in the log — the whole
# point of the per-invocation header is that nothing persists — so the only
# way to know is to look for the token AFTER a successful push.
# NEGATION: pre-fix there was no push at all; a flow that authenticated by
# writing the token into the remote URL or into `.git/config` would leak it
# into every later git invocation of that runner.
D="$(setup_release_repo auth-hygiene 3.0.1 3.0.0 no file)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
TOKEN="ghs_LEAKCHECK_TOKEN_abc123"
if run_orchestrator "$D" "$TOKEN"; then
  PUSH_URL="$(git -C "$D/clone" remote get-url --push origin)"
  PERSISTED_HEADER="$(git -C "$D/clone" config --get-all http.extraheader 2>/dev/null || true)"
  LEAKS="$(grep -rlF "$TOKEN" "$D/clone/.git" 2>/dev/null || true)"
  if [[ "$PUSH_URL" != *"$TOKEN"* && -z $PERSISTED_HEADER && -z $LEAKS ]]; then
    pass "(i) the token reaches neither the remote URL nor .git/config nor .git/"
  else
    fail "(i) the token reaches neither the remote URL nor .git/config nor .git/ (url=$PUSH_URL header=$PERSISTED_HEADER leaked=${LEAKS:-none})"
  fi
  # The throwaway worktree must not survive the run: a leftover one keeps a
  # checkout of the release branch on the runner and shows up in
  # `git worktree list` for every later job on that runner.
  WORKTREE_COUNT="$(git -C "$D/clone" worktree list | wc -l)"
  if [[ "$WORKTREE_COUNT" -eq 1 ]]; then
    pass "(i) no temporary worktree survives the run"
  else
    fail "(i) no temporary worktree survives the run (worktrees=$WORKTREE_COUNT)"
  fi
else
  fail "(i) the credentialed path exits 0 ($(cat "$WORK/stderr"))"
fi

# ─── (j) a network-shaped remote without a token fails closed ──────────────
# The App token is not optional: a push without it would not deliver the
# `pull_request` event, so the Release PR head SHA would sit without checks
# and branch protection would block the merge. The failure has to name
# itself instead of pushing an unauthenticated branch.
# NEGATION: pre-fix nothing authenticated at all; a silent skip would leave
# the Release PR red with no clue why.
D="$(setup_release_repo no-token 3.0.1 3.0.0 no file)"
write_prs '[{"number":42,"headRefName":"chore/release-3.0.1","updatedAt":"2026-10-20T10:00:00Z","state":"OPEN"}]'
BEFORE_SHA="$(remote_branch_sha "$D" 3.0.1)"
if run_orchestrator "$D" ""; then
  fail "(j) a remote that is not local without a token exits non-zero"
else
  if grep -qF "GITHUB_TOKEN is empty" "$WORK/stderr" \
    && [[ "$(remote_branch_sha "$D" 3.0.1)" == "$BEFORE_SHA" ]]; then
    pass "(j) a remote that is not local without a token fails closed naming the reason"
  else
    fail "(j) a remote that is not local without a token fails closed naming the reason ($(cat "$WORK/stderr"))"
  fi
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
