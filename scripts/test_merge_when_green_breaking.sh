#!/usr/bin/env bash
#
# Negation harness for the breaking-marker guard in merge-when-green.sh (#2081).
#
# The defect is a property of the MERGE PATH, not of any line of text: a
# squash-merge silently destroys a breaking declaration, and the damage only
# becomes visible after the tag is public. So it cannot be proven by grepping the
# script. Every case below runs the real script against a fake 'gh' and asserts
# on what it DID — refused or merged, and with which strategy.
#
# NEGATION (the mandatory case from the issue): a PR whose TITLE carries the '!'
# merged with --squash exited 0 and produced 'fix(cli): ... (#2078)' — marker
# gone, release computed as a patch. Case 1 pins exactly that input and demands
# exit 5 with no merge.
#
# Properties pinned here:
#   1. title-with-'!' + --squash is refused BEFORE the check wait (loud, early);
#   2. a commit-subject marker and a 'BREAKING CHANGE:' footer are refused too —
#      the three measured ways the marker dies (#2072, #2078);
#   3. the malformed 'feat!(ai):' of #2072 is recognised as a declaration;
#   4. --merge is the way through, and a clean PR is untouched by the guard;
#   5. a title-only declaration is refused under --merge as well, because a merge
#      commit discards the title — otherwise the guard's own advice would ship a
#      silent patch;
#   6. a '!' that is prose ('fix: don't retry') does not block anything;
#   7. an unreadable PR fails CLOSED (rc 4), never open.
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No network,
# no real repository state, no cargo.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the four reads merge-when-green.sh performs before merging, and records
# the merge it was asked to perform instead of performing one. The head owner is
# deliberately a fork owner so the post-merge remote cleanup is skipped: that
# path shells out to 'git ls-remote' and this harness must stay offline.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

emit() { # emit <json> [<jq-expr>]
  if [[ -n "${2:-}" ]]; then printf '%s' "$1" | jq -r "$2"; else printf '%s\n' "$1"; fi
}

case "${1:-}" in
  repo)
    shift
    fields=""; jqexpr=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
      esac
      shift
    done
    case "$fields" in
      nameWithOwner) emit '{"nameWithOwner":"owner/repo"}' "$jqexpr" ;;
      owner) emit '{"owner":{"login":"owner"}}' "$jqexpr" ;;
      *) echo "fake gh: unhandled repo view --json $fields" >&2; exit 99 ;;
    esac
    ;;
  api)
    case "${2:-}" in
      repos/*/branches/main/protection)
        emit '{"required_status_checks":{"checks":[{"context":"Validate PR metadata"},{"context":"CI Gate"}]}}' \
          '.required_status_checks.checks[]?.context // empty'
        ;;
      *) echo "fake gh: unhandled api ${2:-}" >&2; exit 99 ;;
    esac
    ;;
  pr)
    sub="${2:-}"
    shift 2
    if [[ $# -gt 0 && "${1:-}" != --* ]]; then shift; fi   # drop the PR number
    fields=""; jqexpr=""; strategy=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --json) shift; fields="${1:-}" ;;
        --jq) shift; jqexpr="${1:-}" ;;
        --squash|--merge) strategy="$1" ;;
      esac
      shift
    done
    case "$sub" in
      checks)
        cat "$state/checks.json"
        ;;
      view)
        case "$fields" in
          mergeStateStatus) emit '{"mergeStateStatus":"CLEAN"}' "$jqexpr" ;;
          headRefName) emit '{"headRefName":"fixture-branch"}' "$jqexpr" ;;
          headRepositoryOwner) emit '{"headRepositoryOwner":{"login":"fork-owner"}}' "$jqexpr" ;;
          title,commits)
            [[ -f "$state/pr-meta-fail" ]] && { echo "gh: boom" >&2; exit 1; }
            cat "$state/pr-meta.json"
            ;;
          *) echo "fake gh: unhandled pr view --json $fields" >&2; exit 99 ;;
        esac
        ;;
      merge)
        printf '%s\n' "$strategy" >> "$state/merged"
        ;;
      *) echo "fake gh: unhandled pr $sub" >&2; exit 99 ;;
    esac
    ;;
  *)
    echo "fake gh: unhandled invocation: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

printf '%s\n' '[{"name":"Validate PR metadata","bucket":"pass"},{"name":"CI Gate","bucket":"pass"}]' \
  > "$STATE/checks.json"

# ─── Fixtures ───────────────────────────────────────────────────────────────
# write_pr <title> <commits-json>: the exact shape 'gh pr view --json title,commits'
# returns, which is what the guard reads.
write_pr() {
  jq -n --arg t "$1" --argjson c "$2" '{title: $t, commits: $c}' > "$STATE/pr-meta.json"
  rm -f "$STATE/pr-meta-fail"
}

RUN_OUT=""
RUN_RC=0
run_merge() {
  rm -f "$STATE/merged"
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && \
    PATH="$BIN:$PATH" \
    FAKE_STATE="$STATE" \
    bash scripts/merge-when-green.sh "$@" 2>&1 )" || RUN_RC=$?
}
merged_strategy() { if [[ -f "$STATE/merged" ]]; then tr -d '\n' < "$STATE/merged"; fi; }

PASS=0
FAIL=0
check() {
  local label="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$label" "$expected" "$actual"; FAIL=$((FAIL + 1))
  fi
}
check_contains() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] not found in output\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_absent() {
  local label="$1" needle="$2" haystack="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — [%s] unexpectedly present\n' "$label" "$needle"; FAIL=$((FAIL + 1))
  fi
}
check_no_merge() {
  local label="$1" got
  got="$(merged_strategy)"
  if [[ -z "$got" ]]; then
    printf '  OK   %s\n' "$label"; PASS=$((PASS + 1))
  else
    printf '  FAIL %s — gh pr merge was invoked with [%s]\n' "$label" "$got"; FAIL=$((FAIL + 1))
  fi
}

echo "test_merge_when_green_breaking: the squash path cannot destroy a breaking marker (webfang#2081)"

# ─── 1. THE MANDATORY NEGATION: title marker + squash ───────────────────────
# This is #2078 exactly: 'fix(cli)!:' in the title, clean commits, --squash.
# Pre-guard it exited 0 and merged, producing a subject with no '!'.
write_pr 'fix(cli)!: el aviso de ONNX describe la retirada ya ejecutada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":""}]'
run_merge 2081 --squash
check "title marker + --squash -> exit 5" "5" "$RUN_RC"
check_no_merge "title marker + --squash -> nothing merged"
check_contains "title marker + --squash -> names the offending title" "fix(cli)!: el aviso" "$RUN_OUT"
check_contains "title marker + --squash -> names the remedy" "scripts/merge-when-green.sh 2081 --merge" "$RUN_OUT"
check_absent "title marker + --squash -> refused BEFORE the check wait" \
  "Waiting for required checks" "$RUN_OUT"

# ─── 2. commit-subject marker, clean title ──────────────────────────────────
# The #2077 shape: the declaration travels in a real commit, which is the only
# thing release-plz reads.
write_pr 'chore(release): declarar la ruptura del paso 6' \
  '[{"oid":"437357e7aaaa","messageHeadline":"chore(release)!: declarar la ruptura del paso 6","messageBody":""}]'
run_merge 2081
check "commit marker + default squash -> exit 5" "5" "$RUN_RC"
check_no_merge "commit marker + default squash -> nothing merged"
check_contains "commit marker -> shows the offending commit" "437357e7" "$RUN_OUT"

# ─── 3. BREAKING CHANGE: footer ─────────────────────────────────────────────
# Measured: the squashed #2078 commit carried this footer and release-plz still
# computed a patch. A footer is not a substitute for the subject marker.
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"07e81f8f0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle.\n\nBREAKING CHANGE: la feature ai ya no existe"}]'
run_merge 2081
check "footer marker + squash -> exit 5" "5" "$RUN_RC"
check_no_merge "footer marker + squash -> nothing merged"

# ─── 4. the malformed 'feat!(ai):' that #2072 shipped ───────────────────────
# Not valid Conventional Commits, but it is the intent this repo actually wrote.
write_pr 'feat!(ai): retirar la feature ai y el crate webfang_ai' \
  '[{"oid":"e9b7217a0000","messageHeadline":"feat!(ai): retirar la feature ai","messageBody":""}]'
run_merge 2081
check "malformed feat!(ai): + squash -> exit 5" "5" "$RUN_RC"
check_no_merge "malformed feat!(ai): -> nothing merged"

# ─── 5. --merge is the way through ──────────────────────────────────────────
run_merge 2081 --merge
check "commit marker + --merge -> exit 0" "0" "$RUN_RC"
check "commit marker + --merge -> merged with --merge" "--merge" "$(merged_strategy)"

# ─── 6. a clean PR is untouched ─────────────────────────────────────────────
write_pr 'fix(cli): el aviso describe la retirada' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"fix(cli): el aviso describe la retirada","messageBody":"Detalle sin footer."}]'
run_merge 2081
check "clean PR + default squash -> exit 0" "0" "$RUN_RC"
check "clean PR + default squash -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 7. title-only declaration under --merge ────────────────────────────────
# A merge commit keeps the branch commits and DISCARDS the title, so this
# declaration never reaches main either. Refusing keeps the guard's own advice
# (use --merge) from shipping a silent patch.
write_pr 'feat(ai)!: retirar la feature ai' \
  '[{"oid":"aaaabbbb0000","messageHeadline":"feat(ai): retirar la feature ai","messageBody":""}]'
run_merge 2081 --merge
check "title-only marker + --merge -> exit 5" "5" "$RUN_RC"
check_no_merge "title-only marker + --merge -> nothing merged"
check_contains "title-only marker -> says the commit is what release-plz reads" \
  "no commit does" "$RUN_OUT"

# ─── 8. --dry-run refuses too ───────────────────────────────────────────────
run_merge 2081 --dry-run
check "title-only marker + --dry-run -> exit 5" "5" "$RUN_RC"
check_no_merge "title-only marker + --dry-run -> nothing merged"

# ─── 9. prose is not a marker ───────────────────────────────────────────────
# '!' after the colon and 'BREAKING CHANGE:' mid-line are prose. Blocking on
# them would make the guard noise, and noise gets bypassed.
write_pr "fix(cli): don't reintentar en 5xx" \
  '[{"oid":"ccccdddd0000","messageHeadline":"fix(cli): dont reintentar en 5xx","messageBody":"El trailer BREAKING CHANGE: tiene que ir al inicio de linea para contar."}]'
run_merge 2081
check "prose '!' and mid-line footer -> exit 0" "0" "$RUN_RC"
check "prose case -> merged with --squash" "--squash" "$(merged_strategy)"

# ─── 10. unreadable PR fails CLOSED ─────────────────────────────────────────
write_pr 'fix(cli): cualquiera' '[]'
: > "$STATE/pr-meta-fail"
run_merge 2081
check "unreadable PR -> exit 4 (not a silent merge)" "4" "$RUN_RC"
check_no_merge "unreadable PR -> nothing merged"
check_contains "unreadable PR -> says it refuses to merge blind" "blind" "$RUN_OUT"
rm -f "$STATE/pr-meta-fail"

echo "test_merge_when_green_breaking: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: a breaking declaration cannot be squashed away, and the refusal is loud and early."
