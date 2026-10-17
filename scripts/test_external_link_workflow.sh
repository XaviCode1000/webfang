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
  # The pipeline's status is sort's, not grep's, so an args block with no flags
  # falls through to the emptiness check below instead of aborting under -e.
  # Domain: a whitespace-separated flag token, long or short. An attached value
  # (`--format=markdown`, `-m=markdown`) is a different token shape and is not
  # read here, so keep `args:` in the `--flag value` form this workflow uses.
  used="$(awk '/args: >-/{f=1;next} f{if ($0 !~ /^[[:space:]]+[^[:space:]]/) exit; sub(/^[[:space:]]+/,""); print}' "$file" \
    | tr ' ' '\n' | grep -oE -e '^--[a-z][a-z0-9-]*$' -e '^-[A-Za-z]$' | sort -u)"
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
# "Assert lychee actually checked something" step, de-indented.
assert_snippet() {
  awk '
    /- name: Assert lychee actually checked something/ { in_step = 1; next }
    in_step && /^[[:space:]]+run: \|/ { in_run = 1; next }
    in_run {
      if ($0 !~ /^[[:space:]]+[^[:space:]]/) exit
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

if [[ $cases_failed -eq 0 ]]; then
  # Both sides are the count actually run: a hardcoded total is one more number
  # that can drift out of step with the cases, and this file exists to stop
  # exactly that.
  echo "OK: $cases_run/$cases_run external-link-report workflow semantics cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run external-link-report workflow semantics cases failed"
exit 1
