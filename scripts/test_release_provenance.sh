#!/usr/bin/env bash
#
# Behavioral acceptance matrix for the release provenance L1 gate (webfang#1484).
# Tests rows I.1–I.16 against a synthetic git repo with a fake `gh` that reads
# the fixture repo itself. Hermetic: no network, no real tags, no real repo state.
#
# The fake `gh` simulates the GitHub API by reading the fixture repo. This makes
# the local-vs-server agreement assertion testable. A state-file override allows
# the harness to simulate the server DISAGREEING with the local repo (retag/TOCTOU).
#   If $STATE/ref-<tag> exists, the ref endpoint reports the SHA in that file
#   instead of the local one. This is how I.7 and I.11 become testable.
#
# Exit status: 0 = all rows pass, 1 = any row fails.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
GITHUB_OUT="$WORK/github_output"
mkdir -p "$BIN" "$STATE"

# ─── Fake `gh` ───────────────────────────────────────────────────────────────
# Simulates the GitHub REST API by reading the fixture repo directly.
# State-file override: if $STATE/ref-<tag> exists, the ref endpoint returns
# the SHA from that file instead of the actual local tag. This is how we test
# TOCTOU drift (I.7, I.11) and unreadable server (I.16).
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"
repo="$FAKE_REPO"

[[ "${1:-}" == "api" ]] || { echo "fake gh: unhandled invocation: $*" >&2; exit 99; }
shift

# Normalise: drop every flag and the value of flags that take one, then the first
# remaining argument is the endpoint. Positional parsing (`${4}` for a `--method GET`
# call, `${3}` otherwise) silently mis-reads endpoints, and a `case` on
# "${1} ${2} ${3}" can never match the prefix pattern "api repos/" - which is how this
# stub first failed: it answered "unhandled" (99) or an empty endpoint, and the gate
# correctly refused to proceed. The gate must stay unable to tell this from a real
# server, so an unrecognised call is still a hard 99.
endpoint=""
method="GET"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method) method="${2:-GET}"; shift 2 ;;
    --input|--field|-H|--header|--jq|-q|--cache|-q|-R|-X)
      [[ "$1" == "-X" ]] && method="${2:-GET}"
      shift 2 ;;
    -*) shift ;;
    *) if [[ -z "$endpoint" ]]; then endpoint="$1"; shift; else shift; fi ;;
  esac
done

case "$endpoint" in
  repos/*/git/ref/tags/*)
    tag="${endpoint##*/git/ref/tags/}"
    # $STATE/raw-ref-<tag>, when present, is echoed VERBATIM as the response body.
    # That is how I.16 simulates an unreadable server: nothing here can produce
    # parseable JSON and a garbage file at the same time.
    if [[ -f "$state/raw-ref-$tag" ]]; then
      cat "$state/raw-ref-$tag"
      exit 0
    fi
    # $STATE/ref-<tag>, when present, holds the oid of the TAG OBJECT the server
    # reports - i.e. a retag. It must flow through BOTH endpoints (here and
    # repos/*/git/tags/<oid>) so the two calls stay consistent: an override that only
    # moved the first would let the second dereference the ORIGINAL commit, and the
    # drift rows (I.7, I.11) would pass for the wrong reason - measured, they returned
    # exit 0 while the tag "drifted".
    if [[ -f "$state/ref-$tag" ]]; then
      obj_sha="$(cat "$state/ref-$tag")"
      printf '{"object":{"sha":"%s","type":"tag"}}\n' "$obj_sha"
      exit 0
    fi
    type="$(git -C "$repo" cat-file -t "refs/tags/$tag" 2>/dev/null)" || exit 1
    if [[ "$type" == "tag" ]]; then
      obj_sha="$(git -C "$repo" rev-parse "refs/tags/$tag^{tag}" 2>/dev/null)" || exit 1
    else
      # Lightweight tag: the ref points straight at a commit, so the ref object IS
      # the commit and there is no tag object to dereference.
      obj_sha="$(git -C "$repo" rev-parse "refs/tags/$tag" 2>/dev/null)" || exit 1
    fi
    printf '{"object":{"sha":"%s","type":"%s"}}\n' "$obj_sha" "$type"
    exit 0
    ;;
  repos/*/git/tags/*)
    oid="${endpoint##*/git/tags/}"
    commit="$(git -C "$repo" rev-parse "$oid^{commit}" 2>/dev/null)" || exit 1
    printf '{"object":{"sha":"%s"}}\n' "$commit"
    exit 0
    ;;
  repos/*/rulesets)
    # Three distinct server answers, and collapsing the first two is what made
    # I.12a indistinguishable from I.12c:
    #   rulesets-list present -> its body (the configured set)
    #   api-fail present      -> transport failure (exit 1), "cannot tell"
    #   neither               -> 200 with an EMPTY list: the API answered, and there
    #                            is simply no ruleset. That is "not protected", which
    #                            GitHub really returns, not an error.
    if [[ -f "$state/rulesets-list" ]]; then
      cat "$state/rulesets-list"; exit 0
    fi
    if [[ -f "$state/api-fail" ]]; then
      echo "fake gh: simulated transport failure on $endpoint" >&2
      exit 1
    fi
    printf '[]\n'
    exit 0
    ;;
  repos/*/rulesets/*)
    id="${endpoint##*/rulesets/}"
    if [[ -f "$state/ruleset-$id" ]]; then
      cat "$state/ruleset-$id"; exit 0
    fi
    exit 1
    ;;
esac
echo "fake gh: unhandled endpoint: $endpoint (method=$method)" >&2
exit 99
FAKE
chmod +x "$BIN/gh"

# ─── Fixture repo ────────────────────────────────────────────────────────────
FIXTURE="$WORK/repo"
git -C "$WORK" init -q -b main repo
git -C "$FIXTURE" config user.email "tester@example.com"
git -C "$FIXTURE" config user.name "tester"
# Hermeticity, not a workaround: never inherit the host's signing config. With
# tag.gpgsign=true (this maintainer's global git config) a *lightweight* `git tag
# <name>` is silently upgraded to a signed annotated tag, which opens $EDITOR and hangs
# the harness forever - and even if it returned, the fixture would hold an annotated tag
# where row I.6 needs a lightweight one, so the row would test the wrong object. Same
# class of fragility as relying on the host's default branch name.
git -C "$FIXTURE" config tag.gpgsign false
git -C "$FIXTURE" config commit.gpgsign false
git -C "$FIXTURE" config gpg.format ssh

# Initial commit
git -C "$FIXTURE" commit -q --allow-empty -m "chore: init"

# Add a few commits on main (the lineage)
git -C "$FIXTURE" commit -q --allow-empty -m "feat: add something"
COMMIT_A="$(git -C "$FIXTURE" rev-parse HEAD)"
git -C "$FIXTURE" commit -q --allow-empty -m "fix: fix something"
COMMIT_B="$(git -C "$FIXTURE" rev-parse HEAD)"
git -C "$FIXTURE" commit -q --allow-empty -m "chore: prepare release"
COMMIT_C="$(git -C "$FIXTURE" rev-parse HEAD)"

# Create the lineage ref (origin/main) at COMMIT_C
git -C "$FIXTURE" update-ref refs/remotes/origin/main "$COMMIT_C"

# Add a commit NOT reachable from origin/main (for I.2)
git -C "$FIXTURE" checkout -q -b side-branch "$COMMIT_B"
git -C "$FIXTURE" commit -q --allow-empty -m "feat: side branch only"
SIDE_SHA="$(git -C "$FIXTURE" rev-parse HEAD)"
git -C "$FIXTURE" checkout -q main

# Trusted annotated tag at COMMIT_C (release-plz fingerprint)
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v2.1.0 -m "chore: Release package webfang_core version 2.1.0"

# Human annotated tag (same commit, wrong provenance) — for I.5
git -C "$FIXTURE" -c user.name='Maintainer' -c user.email='maintainer@example.com' \
  tag -a v2.0.0 -m "v2.0.0"

# Lightweight tag — for I.6
git -C "$FIXTURE" tag v1.5.0 "$COMMIT_A"

# RC tag (trusted) — for I.10
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v2.2.0-rc.1 -m "chore: Release package webfang_core version 2.2.0-rc.1"

# Malformed tags — for I.9
git -C "$FIXTURE" tag v2.1 "$COMMIT_A"                    # missing patch
git -C "$FIXTURE" tag v2.1.0-rc.abc "$COMMIT_A"         # non-numeric RC

# Cargo.toml at repo root (version matches v2.1.0)
cat > "$FIXTURE/Cargo.toml" <<'TOML'
[package]
name = "webfang_core"
version = "2.1.0"
edition = "2021"
TOML

# Copy the scripts under test into the fixture so they can run hermetically
mkdir -p "$FIXTURE/scripts"
cp "$REPO_ROOT/scripts/check_release_provenance.sh" \
   "$REPO_ROOT/scripts/release-tag-trust.sh" \
   "$REPO_ROOT/scripts/release-plz-tags.sh" \
   "$REPO_ROOT/scripts/apply-tag-ruleset.sh" \
   "$FIXTURE/scripts/"

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

# Run the provenance gate subcommand against the fixture.
# Args: subcommand [VAR=value ...]
#
# `env` is required, not stylistic: the per-row assignments arrive through "$@",
# and bash only treats `VAR=value` as an assignment prefix when the word is LITERAL
# before the command. After expansion it is parsed as a COMMAND, so `"$@" bash ...`
# dies with "PROV_EVENT_NAME=push: orden no encontrada" (exit 127) on every row.
# `env VAR=value ... bash ...` routes them where they belong.
#
# GITHUB_OUTPUT is a fresh file per call, under $WORK (never inside the fixture, so a
# row cannot read a previous row's outputs). The gate appends to it only when the path
# exists, so rows that assert emitted outputs (I.1, I.10) have something to read -
# without this the entire output surface of the gate would be untested.
run_gate() {
  local subcmd="$1"
  shift
  : >"$GITHUB_OUT"
  ( cd "$FIXTURE" && \
    env \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    FAKE_REPO="$FIXTURE" \
    GITHUB_REPOSITORY="owner/repo" \
    PROV_REPO_ROOT="$FIXTURE" \
    PROV_LINE_REF="refs/remotes/origin/main" \
    GITHUB_OUTPUT="$GITHUB_OUT" \
    "$@" \
    bash "scripts/check_release_provenance.sh" "$subcmd" )
}

# Capture both exit code and stderr (where ::error:: lines go)
run_gate_capture() {
  local subcmd="$1"
  shift
  local out rc
  out="$(run_gate "$subcmd" "$@" 2>&1)" || rc=$?
  rc="${rc:-0}"
  printf '%s\n%d\n' "$out" "$rc"
}

# Extract the L1 clause from an ::error:: line
extract_clause() {
  local output="$1"
  # Match ::error::L1.X ... and return "L1.X Clause"
  grep -oE '::error::L1\.[0-9]+ [^:]+' <<<"$output" | head -1 | sed 's/::error:://' || true
}

reset_state() {
  rm -f "$STATE"/ref-* "$STATE"/raw-ref-*
  rm -f "$STATE"/rulesets-list
  rm -f "$STATE"/ruleset-*
  rm -f "$STATE"/api-fail
}

# Write a ruleset state file for the ruleset check
write_ruleset_state() {
  local id="$1"
  local json="$2"
  printf '%s' "$json" >"$STATE/ruleset-$id"
}

write_rulesets_list() {
  local json="$1"
  printf '%s' "$json" >"$STATE/rulesets-list"
}

echo "test_release_provenance: behavioral acceptance matrix I.1–I.16"

# ═══════════════════════════════════════════════════════════════════════════════
# I.1: trusted annotated tag at commit ancestor of origin/main, Cargo matches,
#       push event, PROV_EVENT_SHA = tag's commit -> PROCEED (exit 0)
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.1 push trusted tag ancestor -> exit 0" "0" "$rc"
if [[ "$rc" != "0" ]]; then echo "---- I.1 RAW ----"; echo "$out"; echo "-----------------"; fi
clause="$(extract_clause "$out")"
check "I.1 push trusted tag ancestor -> no error clause" "" "$clause"

# Also verify GITHUB_OUTPUT was written
if [[ -f "$GITHUB_OUT" ]]; then
  check "I.1 GITHUB_OUTPUT written" "yes" "yes"
  # Check key outputs
  if grep -q "validated_commit=$COMMIT_C" "$GITHUB_OUT"; then check "I.1 validated_commit correct" "yes" "yes"; else check "I.1 validated_commit correct" "yes" "no"; fi
  if grep -q "expected_line=main" "$GITHUB_OUT"; then check "I.1 expected_line=main" "yes" "yes"; else check "I.1 expected_line=main" "yes" "no"; fi
  if grep -q "prerelease=false" "$GITHUB_OUT"; then check "I.1 prerelease=false" "yes" "yes"; else check "I.1 prerelease=false" "yes" "no"; fi
else
  check "I.1 GITHUB_OUTPUT written" "yes" "no"
fi
rm -f "$GITHUB_OUT"

# ═══════════════════════════════════════════════════════════════════════════════
# I.2: same as I.1 but tag points at commit NOT reachable from lineage ref -> BLOCKED L1.3
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# Create a trusted tag at SIDE_SHA (not ancestor of origin/main)
# For L1.3 to be the blocker, the version MUST match Cargo.toml. Temporarily bump
# Cargo.toml to 2.3.0 so the channel+version gates pass and we reach lineage.
sed -i 's/^version = "2.1.0"$/version = "2.3.0"/' "$FIXTURE/Cargo.toml"
git -C "$FIXTURE" -c user.name='github-actions[bot]' -c user.email='bot@github.com' \
  tag -a v2.3.0 -m "chore: Release package webfang_core version 2.3.0" "$SIDE_SHA"
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$SIDE_SHA" \
  PROV_TAG=v2.3.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.2 tag not ancestor of main -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.2 tag not ancestor -> L1.3 Lineage clause" "L1.3 Lineage" "$clause"
# Restore Cargo.toml and clean up the extra tag
sed -i 's/^version = "2.3.0"$/version = "2.1.0"/' "$FIXTURE/Cargo.toml"
git -C "$FIXTURE" tag -d v2.3.0 >/dev/null 2>&1 || true
git -C "$FIXTURE" tag -d v2.3.0 >/dev/null 2>&1 || true

# ═══════════════════════════════════════════════════════════════════════════════
# I.3: tag is fine but PROV_EVENT_SHA differs from tag resolution -> BLOCKED L1.1 Identity
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_A" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.3 PROV_EVENT_SHA mismatch -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.3 PROV_EVENT_SHA mismatch -> L1.1 Identity clause" "L1.1 Identity" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.4: workflow_dispatch from default branch, PROV_EXPECTED_SHA empty -> BLOCKED L1.1
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=workflow_dispatch \
  PROV_REF=refs/heads/main \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.4 dispatch default branch no EXPECTED_SHA -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.4 no EXPECTED_SHA -> L1.1 Identity clause" "L1.1 Identity" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.5: workflow_dispatch default branch, EXPECTED_SHA correct, but tag is HUMAN-made
#      (annotated by non-bot) -> BLOCKED L1.1 Identity (not-trusted message)
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=workflow_dispatch \
  PROV_REF=refs/heads/main \
  PROV_EXPECTED_SHA="$COMMIT_C" \
  PROV_TAG=v2.0.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.5 human tag -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.5 human tag -> L1.1 Identity clause" "L1.1 Identity" "$clause"
# Verify the message mentions trust
if grep -q "not trusted" <<<"$out"; then check "I.5 human tag -> mentions not trusted" "yes" "yes"; else check "I.5 human tag -> mentions not trusted" "yes" "no"; fi

# ═══════════════════════════════════════════════════════════════════════════════
# I.6: lightweight tag whose commit matches and Cargo matches -> BLOCKED L1.1 (lightweight message)
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_A" \
  PROV_TAG=v1.5.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.6 lightweight tag -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.6 lightweight tag -> L1.1 Identity clause" "L1.1 Identity" "$clause"
if grep -q "lightweight" <<<"$out"; then check "I.6 lightweight tag -> mentions lightweight" "yes" "yes"; else check "I.6 lightweight tag -> mentions lightweight" "yes" "no"; fi

# ═══════════════════════════════════════════════════════════════════════════════
# I.7: preflight passes, then server ref moved via state override; run publish
#      with original PROV_VALIDATED_COMMIT -> BLOCKED L1.4 TOCTOU
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# First run preflight to get validated_commit
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.7 preflight baseline -> exit 0" "0" "$rc"
# Extract validated_commit from output
validated_commit="$(grep -o 'validated_commit=[a-f0-9]*' <<<"$out" | cut -d= -f2)"
[[ -n "$validated_commit" ]] || validated_commit="$COMMIT_C"

# Now simulate a server-side retag: the ref endpoint reports a DIFFERENT tag object,
# one whose object points at COMMIT_A instead of COMMIT_C. The harness builds that
# object locally (a throwaway tag) so the dereference call resolves consistently.
git -C "$FIXTURE" tag -a drift-v2.1.0 -m "drift" "$COMMIT_A" >/dev/null 2>&1
# shellcheck disable=SC1083  # git rev-parse ^{tag} suffix is not brace expansion
printf '%s' "$(git -C "$FIXTURE" rev-parse drift-v2.1.0^{tag})" >"$STATE/ref-v2.1.0"

# Run publish with the ORIGINAL validated_commit
out_rc="$(run_gate_capture publish \
  PROV_VALIDATED_COMMIT="$validated_commit" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.7 TOCTOU drift -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.7 TOCTOU drift -> L1.4 TOCTOU clause" "L1.4 TOCTOU" "$clause"
if grep -q "drifted" <<<"$out"; then check "I.7 TOCTOU -> mentions drifted" "yes" "yes"; else check "I.7 TOCTOU -> mentions drifted" "yes" "no"; fi

# ═══════════════════════════════════════════════════════════════════════════════
# I.8: trusted tag, correct SHA, but Cargo.toml version differs -> BLOCKED L1.2 Version
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# Temporarily change Cargo.toml version
sed -i 's/version = "2.1.0"/version = "2.9.9"/' "$FIXTURE/Cargo.toml"
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.8 Cargo version mismatch -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.8 Cargo mismatch -> L1.2 Version clause" "L1.2 Version" "$clause"
# Restore Cargo.toml
sed -i 's/version = "2.9.9"/version = "2.1.0"/' "$FIXTURE/Cargo.toml"

# ═══════════════════════════════════════════════════════════════════════════════
# I.9: malformed tag names (v2.1, v2.1.0-rc.abc) -> BLOCKED channel/name clause
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_A" \
  PROV_TAG=v2.1)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.9a malformed v2.1 (missing patch) -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.9a malformed -> L1.2 Version clause" "L1.2 Version" "$clause"

reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_A" \
  PROV_TAG=v2.1.0-rc.abc)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.9b malformed v2.1.0-rc.abc (non-numeric RC) -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.9b malformed RC -> L1.2 Version clause" "L1.2 Version" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.10: RC tag vX.Y.Z-rc.N, trusted, correct SHA, Cargo matches -> PROCEED,
#        prerelease=true written to GITHUB_OUTPUT
# ═══════════════════════════════════════════════════════════════════════════════
# Need Cargo.toml version to match RC base version (2.2.0)
sed -i 's/version = "2.1.0"/version = "2.2.0"/' "$FIXTURE/Cargo.toml"
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$(git -C "$FIXTURE" rev-parse 'refs/tags/v2.2.0-rc.1^{commit}')" \
  PROV_TAG=v2.2.0-rc.1)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.10 RC tag trusted -> exit 0" "0" "$rc"
clause="$(extract_clause "$out")"
check "I.10 RC tag -> no error clause" "" "$clause"
# Check GITHUB_OUTPUT for prerelease=true
if [[ -f "$GITHUB_OUT" ]]; then
  if grep -q "prerelease=true" "$GITHUB_OUT"; then check "I.10 prerelease=true in output" "yes" "yes"; else check "I.10 prerelease=true in output" "yes" "no"; fi
  if grep -q "expected_line=main" "$GITHUB_OUT"; then check "I.10 expected_line=main" "yes" "yes"; else check "I.10 expected_line=main" "yes" "no"; fi
else
  check "I.10 GITHUB_OUTPUT written" "yes" "no"
fi
rm -f "$GITHUB_OUT"
# Restore Cargo.toml
sed -i 's/version = "2.2.0"/version = "2.1.0"/' "$FIXTURE/Cargo.toml"

# ═══════════════════════════════════════════════════════════════════════════════
# I.11: publish where server ref resolves to different commit than PROV_VALIDATED_COMMIT
#       -> BLOCKED L1.4 TOCTOU
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# Preflight first
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
validated_commit="$(grep -o 'validated_commit=[a-f0-9]*' <<<"$out" | cut -d= -f2)"
[[ -n "$validated_commit" ]] || validated_commit="$COMMIT_C"

# Retag the server ref to an object pointing at COMMIT_A (different from validated)
git -C "$FIXTURE" tag -a drift-v2.1.0-b -m "drift" "$COMMIT_A" >/dev/null 2>&1
# shellcheck disable=SC1083  # git rev-parse ^{tag} suffix is not brace expansion
printf '%s' "$(git -C "$FIXTURE" rev-parse drift-v2.1.0-b^{tag})" >"$STATE/ref-v2.1.0"

out_rc="$(run_gate_capture publish \
  PROV_VALIDATED_COMMIT="$validated_commit" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.11 publish drift -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.11 publish drift -> L1.4 TOCTOU clause" "L1.4 TOCTOU" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.12: ruleset subcommand - three distinguishable outcomes
# ═══════════════════════════════════════════════════════════════════════════════
# I.12a: no ruleset state file -> exit 1 (not protected)
reset_state
out_rc="$(run_gate_capture ruleset)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.12a no ruleset -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.12a no ruleset -> L1.5 Ruleset clause" "L1.5 Ruleset" "$clause"
if grep -q "not found" <<<"$out"; then check "I.12a mentions not found" "yes" "yes"; else check "I.12a mentions not found" "yes" "no"; fi

# I.12b: ruleset present but missing deletion rule -> exit 1
reset_state
write_rulesets_list '[{"id": 123, "name": "release-tags-immutable"}]'
write_ruleset_state 123 '{"target":"tag","enforcement":"active","conditions":{"ref_name":{"include":["v*"]}},"rules":[{"type":"update","parameters":{}}]}'
out_rc="$(run_gate_capture ruleset)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.12b ruleset missing deletion -> exit 1" "1" "$rc"
clause="$(extract_clause "$out")"
check "I.12b missing deletion -> L1.5 Ruleset clause" "L1.5 Ruleset" "$clause"
if grep -q "deletion" <<<"$out"; then check "I.12b mentions deletion missing" "yes" "yes"; else check "I.12b mentions deletion missing" "yes" "no"; fi

# I.12c: gh unable to answer (transport failure) -> exit 2 (cannot tell)
reset_state
# The knob is api-fail, NOT "leave the state empty": with the server's real
# "200 + empty list" behaviour restored above, an empty state now means "the API
# answered and no ruleset exists", which is I.12a. Only a call that fails outright is
# "cannot tell". Separating them is the point of the 1-vs-2 contract.
printf 'x' >"$STATE/api-fail"
out_rc="$(run_gate_capture ruleset)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.12c API failure -> exit 2" "2" "$rc"
clause="$(extract_clause "$out")"
check "I.12c API failure -> L1.5 Ruleset clause" "L1.5 Ruleset" "$clause"
if grep -q "could not be determined" <<<"$out"; then check "I.12c mentions cannot tell" "yes" "yes"; else check "I.12c mentions cannot tell" "yes" "no"; fi

# ═══════════════════════════════════════════════════════════════════════════════
# I.13: push path must NOT require PROV_EXPECTED_SHA (attested by event)
#       -> assert I.1 passes with PROV_EXPECTED_SHA unset
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# Run without PROV_EXPECTED_SHA (explicitly unset)
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.13 push without PROV_EXPECTED_SHA -> exit 0" "0" "$rc"
clause="$(extract_clause "$out")"
check "I.13 push without EXPECTED_SHA -> no error" "" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.14: --ref-pinned dispatch (PROV_REF=refs/tags/<tag>, PROV_REF_TYPE=tag,
#       PROV_EVENT_SHA = tag commit) -> PROCEEDS (cut-patch-tag.yml shape)
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=workflow_dispatch \
  PROV_REF="refs/tags/v2.1.0" \
  PROV_REF_TYPE=tag \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.14 pinned tag dispatch -> exit 0" "0" "$rc"
clause="$(extract_clause "$out")"
check "I.14 pinned dispatch -> no error" "" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.15: default-branch dispatch with correct PROV_EXPECTED_SHA -> PROCEEDS
#       (dispatch-release shape, must never trust github.sha)
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=workflow_dispatch \
  PROV_REF=refs/heads/main \
  PROV_EXPECTED_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.15 dispatch default branch with EXPECTED_SHA -> exit 0" "0" "$rc"
clause="$(extract_clause "$out")"
check "I.15 dispatch with EXPECTED_SHA -> no error" "" "$clause"

# ═══════════════════════════════════════════════════════════════════════════════
# I.16: fail-closed on unreadable server - ref endpoint returns garbage JSON
#       -> non-zero exit, message must NOT claim tag untrusted or drifted
# ═══════════════════════════════════════════════════════════════════════════════
reset_state
# Raw garbage body on the ref endpoint - an unreadable server, not a drift.
printf 'not-json' >"$STATE/raw-ref-v2.1.0"
out_rc="$(run_gate_capture preflight \
  PROV_EVENT_NAME=push \
  PROV_EVENT_SHA="$COMMIT_C" \
  PROV_TAG=v2.1.0)"
out="$(head -n -1 <<<"$out_rc")"
rc="$(tail -1 <<<"$out_rc")"
check "I.16 garbage server response -> exit non-zero" "1" "$rc"
if [[ "$rc" != "1" ]]; then echo "---- I.16 RAW ----"; echo "$out"; echo "------------------"; fi
clause="$(extract_clause "$out")"
check "I.16 garbage -> L1.1 Identity clause" "L1.1 Identity" "$clause"
# Must NOT say untrusted or drifted
if ! grep -q "not trusted" <<<"$out"; then check "I.16 does NOT claim untrusted" "yes" "yes"; else check "I.16 does NOT claim untrusted" "yes" "no"; fi
if ! grep -q "drifted" <<<"$out"; then check "I.16 does NOT claim drifted" "yes" "yes"; else check "I.16 does NOT claim drifted" "yes" "no"; fi
# Must say unparseable or failed to resolve
if grep -qE "unparseable|failed to resolve" <<<"$out"; then check "I.16 mentions unparseable/failed" "yes" "yes"; else check "I.16 mentions unparseable/failed" "yes" "no"; fi

echo "test_release_provenance: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: all provenance matrix rows behave as specified."