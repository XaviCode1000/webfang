#!/usr/bin/env bash
# test_external_link_workflow.sh
#
# Semantics harness for the lychee step of
# .github/workflows/external-link-report.yml, following the repo convention of a
# companion harness per guard (test_dependency_direction.sh,
# test_ignored_guard.sh, test_no_ai_attribution.sh). Fully offline and without
# the lychee binary, it proves that:
#
#   - every flag in the workflow's `args:` exists in the pinned lychee release,
#     long and short. That is the regression class this file exists for:
#     `--exit-zero` and `--exclude-mail` do not exist in lychee 0.24.2, so lychee
#     exited 2 before checking anything and `fail: false` turned that into a
#     green run that checked no link at all. Nothing read the args, so it
#     shipped green.
#   - a missing or unknown `lycheeVersion` pin fails closed. The flag set below
#     is derived from `lychee <version> --help`, so a version bump has to
#     re-derive it instead of silently outdating it.
#   - the committed "Assert lychee actually checked something" snippet is
#     EXECUTED against three report fixtures taken from real runs: an empty file
#     and a footer-only file fail, a real report passes. The footer-only shape
#     is the one a broken lychee actually leaves (measured: the action's
#     entrypoint.sh always cats its temp output and appends a footer), which is
#     why the committed guard cannot rely on the report simply being empty.
#   - extracting that snippet cannot pass vacuously: an extraction that yields
#     nothing, or that lost the guard's own anchors, fails the harness.
#   - both extractions are anchored to their YAML block (the next step's `- name:`
#     or a dedent), not to the blank line that follows the block, and a short-flag
#     cluster (`-qv`) fails closed instead of being dropped unread. Same class as
#     the rest of the file: a shape the extractor cannot read must fail loudly,
#     never pass quietly.
#
# Usage: bash scripts/test_external_link_workflow.sh [workflow-path]
# The optional path exists so the harness can be pointed at another revision of
# the workflow for the negative control; it defaults to the repo's copy.
#
# The flag set is deliberately version-keyed and explicit: adding a flag is
# meant to require verifying it against the pinned release. That gate is the
# point, not an obstacle.
#
# Both lists come from `lychee <version> --help`, and the short set is provable
# against the binary as well: clap accepts no short flag it does not declare, so
# `lychee -X --help` is the test for any entry (a rejection shows up as
# `unexpected argument '-X' found` — the negative control below runs exactly
# that probe for a flag the release lacks).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WORKFLOW="${1:-$ROOT/.github/workflows/external-link-report.yml}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

declare -i cases_run=0 cases_failed=0 case_fail=0

# flags_for <version> — the long and short flags of that lychee release, one
# per line, read off `lychee <version> --help`. Returns nonzero for a release
# this repo has not verified, so an unknown pin fails closed instead of passing
# blind.
flags_for() {
  case "$1" in
    v0.24.2)
      tr ' ' '\n' <<< '--accept --accept-timeouts --archive --base --base-url --basic-auth --cache --cache-exclude-status --config --cookie-jar --default-extension --dump --dump-inputs --exclude --exclude-all-private --exclude-file --exclude-link-local --exclude-loopback --exclude-path --exclude-private --extensions --fallback-extensions --files-from --format --generate --github-token --glob-ignore-case --header --help --hidden --host-concurrency --host-request-interval --host-stats --include --include-fragments --include-mail --include-verbatim --include-wikilinks --index-files --insecure --max-cache-age --max-concurrency --max-redirects --max-retries --method --min-tls --mode --no-ignore --no-progress --offline --output --preprocess --quiet --remap --require-https --retry-wait-time --root-dir --scheme --skip-missing --suggest --threads --timeout --to --user-agent --verbose --version --wrap'
      # The short set, same source. Kept on its own line so the long list stays
      # one readable block; both are emitted, because a flag is a flag whichever
      # spelling of it lands in `args:`.
      tr ' ' '\n' <<< '-a -b -c -E -f -h -H -i -m -n -o -p -q -r -s -t -T -u -v -V -X'
      ;;
    *)
      return 1
      ;;
  esac
}

# check_flags <workflow-path> — nonzero if the `args:` block names a flag that
# the pinned release does not have, or if the pin itself is missing, unknown, or
# the args block is unreadable. One line per reason.
check_flags() {
  local file="$1" version flags used flag bad=0
  version="$(sed -n 's/^[[:space:]]*lycheeVersion:[[:space:]]*\(v[0-9][^[:space:]]*\).*/\1/p' "$file" | head -n1)"
  if [[ -z "$version" ]]; then
    echo "no lycheeVersion pin found: the args cannot be validated against a release"
    return 1
  fi
  if ! flags="$(flags_for "$version")"; then
    echo "unverified lychee release '$version': run 'lychee $version --help' and add its flags to flags_for()"
    return 1
  fi
  # One pure-awk pass does the extraction AND the anchoring, so the assignment
  # below cannot fail. The previous `tr | grep | sort` pipeline returned grep's
  # nonzero status on an empty match, and the emptiness check was reached only
  # because bash suspends `-e` inside a function called as an `if` condition —
  # which made this diagnostic depend on every caller using `if`, a property this
  # file does not control. With awk that dependency is gone: the guard is reached
  # by construction.
  #
  # Anchoring: the block ends at the first non-blank line indented at or above
  # the `args:` key, so removing the blank line before the next step no longer
  # makes the block swallow that step.
  #
  # Domain: a whitespace-separated token. `--long` is a long flag, `-x` a short
  # one. A short-flag CLUSTER (`-qv`) is emitted separately and fails closed just
  # below instead of being dropped: this extractor cannot attribute a cluster to
  # individual flags, and silently ignoring that form is the same blindness the
  # rest of this file exists to prevent. Everything else — values, quoted paths,
  # attached values such as `--format=markdown` — is not a flag token.
  used="$(awk '
    /^[[:space:]]*args: >-/ { match($0, /^[[:space:]]*/); base = RLENGTH; in_args = 1; next }
    in_args {
      if ($0 ~ /^[[:space:]]*$/) next
      match($0, /^[[:space:]]*/)
      if (RLENGTH <= base) exit
      line = $0; sub(/^[[:space:]]+/, "", line)
      n = split(line, tok, /[[:space:]]+/)
      for (i = 1; i <= n; i++) {
        t = tok[i]
        if (t ~ /^--[a-z][a-z0-9-]*$/) flags[t] = 1
        else if (t ~ /^-[A-Za-z]$/) flags[t] = 1
        else if (t ~ /^-[A-Za-z]{2,}$/) clusters[t] = 1
      }
    }
    END {
      for (c in clusters) print "!cluster " c
      for (f in flags) print f
    }
  ' "$file")"
  if [[ "$used" == '!cluster '* ]]; then
    local cluster="${used#!cluster }"
    cluster="${cluster%%$'\n'*}"
    echo "short-flag cluster in the args block, which this extractor cannot attribute to flags: $cluster"
    return 1
  fi
  if [[ -z "$used" ]]; then
    echo "no flags found in the args block: the YAML shape changed and this check went blind"
    return 1
  fi
  while IFS= read -r flag; do
    if ! grep -qxF -e "$flag" <<< "$flags"; then
      echo "flag does not exist in lychee $version: $flag"
      bad=1
    fi
  done <<< "$used"
  [[ $bad -eq 0 ]]
}

# assert_snippet <workflow-path> — echoes the run block of the committed
# "Assert lychee actually checked something" step, de-indented. Anchored to the
# step's own indentation (like the args extraction above), so a missing blank line
# after the block cannot make it swallow the next step; blank lines inside the
# run script survive.
assert_snippet() {
  awk '
    /^[[:space:]]*- name: Assert lychee actually checked something/ {
      match($0, /^[[:space:]]*/); base = RLENGTH; in_step = 1; next
    }
    in_step && /^[[:space:]]*run: \|/ { in_run = 1; next }
    in_run {
      if ($0 ~ /^[[:space:]]*$/) { print ""; next }
      match($0, /^[[:space:]]*/)
      if (RLENGTH <= base) exit
      sub(/^[[:space:]]+/, ""); print
    }
  ' "$1"
}

# run_snippet <snippet> <report-fixture> — runs the snippet with the fixture in
# place as ./lychee-report.md, and returns the snippet's own exit status.
run_snippet() {
  local snippet="$1" fixture="$2"
  cp "$fixture" "$tmp/lychee-report.md"
  ( cd "$tmp" && bash -c "$snippet" )
}

# finish_case <label> — account and report one completed case.
finish_case() {
  local label="$1"
  cases_run+=1
  if [[ $case_fail -eq 0 ]]; then
    echo "ok: case $cases_run — $label"
  else
    cases_failed+=1
    echo "FAIL: case $cases_run — $label"
  fi
  case_fail=0
}

# --- fixtures ---------------------------------------------------------------

# A real report, byte-for-byte what lychee 0.24.2 leaves for a 4-file tree
# (measured; the emoji-prefixed rows are why the committed guard greps for
# `Total` anywhere in the line rather than anchoring at the start).
cat > "$tmp/report-real.md" <<'EOF'
# Summary

| Status         | Count |
|----------------|-------|
| 🔍 Total       | 3     |
| 🔗 Unique      | 3     |
| ✅ Successful  | 3     |
| ⏳ Timeouts    | 0     |
| 🔀 Redirected  | 0     |
| 👻 Excluded    | 0     |
| ❓ Unknown     | 0     |
| 🚫 Errors      | 0     |
| ⛔ Unsupported | 0     |
EOF

# What a broken run actually leaves: the action always writes its temp output
# and appends this footer for markdown, so the report is never 0 bytes.
cat > "$tmp/report-footer-only.md" <<'EOF'
[Full Github Actions output](https://github.com/lycheeverse/lychee-action/actions/runs/0)
EOF

: > "$tmp/report-empty.md"

# Minimal workflows for the extraction-boundary cases. Their whole point is to
# have an `args:` (or `run:`) block with NO blank line before the next step, plus
# a flag-shaped token in that next step: absorbing the next step then shows up as
# a falsely rejected flag (args) or as a marker leaking into the snippet.
cat > "$tmp/wf-args-noblank.yml" <<'EOF'
jobs:
  j:
    steps:
      - name: Lychee external link check
        with:
          lycheeVersion: v0.24.2
          args: >-
            --no-progress
            --verbose
      - name: Decoy step
        run: |
          lychee --exit-zero .
EOF

cat > "$tmp/wf-snippet-noblank.yml" <<'EOF'
jobs:
  j:
    steps:
      - name: Assert lychee actually checked something
        run: |
          grep -qE 'Total *\| *[0-9]' lychee-report.md || exit 1
      - name: Show report in job summary
        run: |
          echo NEXT-STEP-MARKER
EOF

# A short-flag cluster, with the blank line present so the anchoring is not the
# variable under test here.
cat > "$tmp/wf-cluster.yml" <<'EOF'
jobs:
  j:
    steps:
      - name: Lychee external link check
        with:
          lycheeVersion: v0.24.2
          args: >-
            --no-progress
            -qv

      - name: Decoy step
        run: |
          echo decoy
EOF

# --- cases ------------------------------------------------------------------

# Case 1: the committed workflow only names flags the pinned release has.
if check_flags "$WORKFLOW" > "$tmp/out.txt" 2>&1; then
  echo "  ok: args only use flags present in the pinned lychee release"
else
  echo "  FAIL: args name flags the pinned release does not have"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "committed args validated against the pinned release"

# Case 2: negative control — a re-introduced nonexistent flag is rejected and
# named. This is the shipped bug reproduced as a test input.
sed 's/^            --no-progress$/            --no-progress\n            --exit-zero/' "$WORKFLOW" > "$tmp/wf-bogus.yml"
if cmp -s "$WORKFLOW" "$tmp/wf-bogus.yml"; then
  echo "  FAIL: fixture injection did not apply — the sed anchor missed and this case proved nothing"
  case_fail=1
elif check_flags "$tmp/wf-bogus.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: re-added --exit-zero passed the check (negative control)"
  case_fail=1
elif grep -q -e '--exit-zero' "$tmp/out.txt"; then
  echo "  ok: re-added --exit-zero is rejected and named"
else
  echo "  FAIL: rejected, but the offending flag was not named"
  case_fail=1
fi
finish_case "negative control: nonexistent flag rejected"

# Case 3: the same negative control for a SHORT flag. A release the harness only
# knows the long flags of would let a short flag through unread, which is the
# same blindness in a smaller shape.
sed 's/^            --no-progress$/            --no-progress\n            -Z/' "$WORKFLOW" > "$tmp/wf-bogus-short.yml"
if cmp -s "$WORKFLOW" "$tmp/wf-bogus-short.yml"; then
  echo "  FAIL: fixture injection did not apply — the sed anchor missed and this case proved nothing"
  case_fail=1
elif check_flags "$tmp/wf-bogus-short.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: re-added -Z passed the check (short-flag negative control)"
  case_fail=1
elif grep -qF -e ': -Z' "$tmp/out.txt"; then
  echo "  ok: re-added -Z is rejected and named"
else
  echo "  FAIL: rejected, but the offending short flag was not named"
  case_fail=1
fi
finish_case "negative control: nonexistent short flag rejected"

# Case 4: a missing pin fails closed — without a pin there is no flag set to
# validate against, so silence would be the wrong answer.
grep -v -e 'lycheeVersion:' "$WORKFLOW" > "$tmp/wf-unpinned.yml"
if check_flags "$tmp/wf-unpinned.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a workflow with no lycheeVersion pin passed"
  case_fail=1
else
  echo "  ok: missing pin fails closed"
fi
finish_case "missing lycheeVersion pin fails closed"

# Case 5: an unverified release fails closed, so a bump must re-derive the flag
# set rather than validating new args against a stale list.
sed 's/lycheeVersion: *v[0-9][^ ]*/lycheeVersion: v0.99.0/' "$WORKFLOW" > "$tmp/wf-unknown-version.yml"
if cmp -s "$WORKFLOW" "$tmp/wf-unknown-version.yml"; then
  echo "  FAIL: fixture injection did not apply — this case proved nothing"
  case_fail=1
elif check_flags "$tmp/wf-unknown-version.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: an unverified lychee release passed"
  case_fail=1
else
  echo "  ok: unverified release fails closed and asks for re-derivation"
fi
finish_case "unverified lychee release fails closed"

# Case 6: the snippet extraction is non-vacuous. Without this, a re-indent that
# broke the awk would leave cases 7-9 running an empty script and passing.
SNIPPET="$(assert_snippet "$WORKFLOW")"
if [[ -z "$SNIPPET" ]]; then
  echo "  FAIL: no snippet extracted from the assert step — the harness went blind"
  case_fail=1
elif [[ "$SNIPPET" != *lychee-report.md* || "$SNIPPET" != *'exit 1'* ]]; then
  echo "  FAIL: extracted snippet lost the guard's anchors (lychee-report.md / exit 1)"
  case_fail=1
else
  echo "  ok: snippet extracted with both anchors"
fi
finish_case "committed guard snippet extracted non-vacuously"

# Case 7: an empty report is rejected.
if run_snippet "$SNIPPET" "$tmp/report-empty.md" > /dev/null 2>&1; then
  echo "  FAIL: an empty report passed the guard"
  case_fail=1
else
  echo "  ok: empty report rejected"
fi
finish_case "guard rejects an empty report"

# Case 8: a non-empty report with no summary table is rejected. This is the
# shape a broken checker leaves, so it is the case that actually has to fire.
if run_snippet "$SNIPPET" "$tmp/report-footer-only.md" > /dev/null 2>&1; then
  echo "  FAIL: a footer-only report passed the guard — a broken run would go green"
  case_fail=1
else
  echo "  ok: non-empty report without a summary table rejected"
fi
finish_case "guard rejects a report with no summary table"

# Case 9: a real report passes, so report-only semantics survive the guard: a
# run that finds dead links must still be green.
if run_snippet "$SNIPPET" "$tmp/report-real.md" > /dev/null 2>&1; then
  echo "  ok: real report accepted"
else
  echo "  FAIL: a real report was rejected — the guard would block a healthy run"
  case_fail=1
fi
finish_case "guard accepts a real report (report-only preserved)"

# Case 10: the args extraction is anchored to the args block, not to the blank
# line that happens to follow it. Without that blank line the old extractor
# absorbed the next step, so a flag-shaped token there was read as an arg. The
# decoy step carries `--exit-zero`, absent from the pinned release, so absorbing
# it shows up as a falsely rejected flag instead of passing quietly.
if check_flags "$tmp/wf-args-noblank.yml" > "$tmp/out.txt" 2>&1; then
  echo "  ok: args extraction stopped at the block boundary (no trailing blank line)"
else
  echo "  FAIL: without the trailing blank line the extraction absorbed the next step"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "args extraction anchored to the block, not the blank line"

# Case 11: the same anchoring for the snippet extraction. The decoy line lives in
# the step AFTER the assert step, so absorbing it leaks a marker into the snippet.
SNIPPET_NOBLANK="$(assert_snippet "$tmp/wf-snippet-noblank.yml")"
if [[ "$SNIPPET_NOBLANK" != *lychee-report.md* ]]; then
  echo "  FAIL: extraction went blind on the no-blank-line fixture"
  case_fail=1
elif [[ "$SNIPPET_NOBLANK" == *NEXT-STEP-MARKER* ]]; then
  echo "  FAIL: without the trailing blank line the snippet extraction absorbed the next step"
  case_fail=1
else
  echo "  ok: snippet extraction stopped at the step boundary (no trailing blank line)"
fi
finish_case "snippet extraction anchored to the step, not the blank line"

# Case 12: a short-flag cluster (`-qv`) is a shape this harness does not parse.
# Dropping it silently is the same blindness the whole file exists to prevent, so
# it fails closed and names the token instead.
if check_flags "$tmp/wf-cluster.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: a short-flag cluster (-qv) in args was accepted silently"
  case_fail=1
elif grep -qF -- '-qv' "$tmp/out.txt"; then
  echo "  ok: short-flag cluster fails closed and is named"
else
  echo "  FAIL: cluster rejected, but the offending token was not named"
  case_fail=1
fi
finish_case "short-flag cluster fails closed and is named"

# Case 13: the emptiness diagnostic has to be reachable when check_flags is
# called from a PLAIN statement context, not only as an `if` condition. Under
# `set -e` bash suspends `-e` inside a function used as a condition, so the old
# `... | grep ...` pipeline returned nonzero on an empty match and reached its
# message only by accident of the call site: moved out of an `if`, it died with
# no output at all. Sourcing the definitions up to the cases marker and calling
# the function plainly is that shape.
cat > "$tmp/wf-empty-args.yml" <<'EOF'
jobs:
  j:
    steps:
      - name: Lychee external link check
        with:
          lycheeVersion: v0.24.2
          args: >-
      - name: Step after an empty args block
        run: |
          echo next
EOF
awk '/^# --- cases ---/{exit} {print}' "${BASH_SOURCE[0]}" > "$tmp/defs.sh"
cat > "$tmp/plain-context.sh" <<'EOF'
set -euo pipefail
source "$1"
check_flags "$2"
echo "UNREACHABLE: check_flags returned 0 for an empty args block"
EOF
if bash "$tmp/plain-context.sh" "$tmp/defs.sh" "$tmp/wf-empty-args.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: an empty args block passed instead of failing closed"
  case_fail=1
elif grep -qF 'no flags found in the args block' "$tmp/out.txt"; then
  echo "  ok: empty args block fails closed and reaches its diagnostic in a plain call context"
else
  echo "  FAIL: empty args block failed without reaching the diagnostic (silent death)"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "emptiness diagnostic no longer depends on an if/elif call site"

# Case 14: blank lines INSIDE the run script are part of the snippet and survive
# extraction; only a dedent terminates it. This is case 11 seen from the other
# side: the old extractor stopped at the first blank line it met, so blending the
# two boundaries (blank line vs dedent) truncated the snippet instead of
# over-reading it.
cat > "$tmp/wf-snippet-blank.yml" <<'EOF'
jobs:
  j:
    steps:
      - name: Assert lychee actually checked something
        run: |
          first=1

          second=2
      - name: Show report in job summary
        run: |
          echo NEXT-STEP-MARKER
EOF
SNIPPET_BLANK="$(assert_snippet "$tmp/wf-snippet-blank.yml")"
if [[ "$SNIPPET_BLANK" != *first=1* || "$SNIPPET_BLANK" != *second=2* ]]; then
  echo "  FAIL: extraction truncated the snippet at an internal blank line"
  case_fail=1
elif [[ "$SNIPPET_BLANK" != *$'\n\n'* ]]; then
  echo "  FAIL: the internal blank line was dropped from the snippet"
  case_fail=1
elif [[ "$SNIPPET_BLANK" == *NEXT-STEP-MARKER* ]]; then
  echo "  FAIL: the extraction absorbed the next step"
  case_fail=1
else
  echo "  ok: internal blank lines preserved and the step boundary still holds"
fi
finish_case "blank lines inside the run snippet are preserved"

if [[ $cases_failed -eq 0 ]]; then
  # Both sides are the count actually run: a hardcoded total is one more number
  # that can drift out of step with the cases, and this file exists to stop
  # exactly that.
  echo "OK: $cases_run/$cases_run external-link-report workflow semantics cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run external-link-report workflow semantics cases failed"
exit 1
