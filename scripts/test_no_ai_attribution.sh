#!/usr/bin/env bash
# Semantics harness for check_no_ai_attribution.sh.
#
# Every case is a real message shape taken from this repository's history or
# from the PR that motivated the check. The two cases that matter most are the
# ones a naive implementation gets wrong:
#
#   - `Co-authored-by: release-plz-tu-repo[bot]` MUST pass. It is on every
#     release commit; a blanket `Co-Authored-By` ban would fail the release
#     flow. Measured: 19 such commits in the last 200 on main.
#   - `Co-Authored-By: Claude Opus 4.8` MUST fail. It is the same
#     attribution shape as the Codebuff trailer that slipped through, and it
#     is already on main 11 times — this check is forward-looking and does not
#     rewrite that history, but it must not accept the pattern again.
#
# It also pins the ONE case where the fixture reader and the production reader
# used to disagree: a message that already ends in a newline. The production
# path reads it through a command substitution, which strips the trailing
# newlines; the --fixture path did not, so the trailer block was read from
# different bytes and a marker read as clean.
#
# And it pins the whole-IDENTITY rule: a co-author name that merely STARTS
# with an allowlisted actor (`renovate-bot, actually a human`) is a human and
# is rejected, even though the prefix used to authorize it.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK="$ROOT/scripts/check_no_ai_attribution.sh"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

pass=0
fail=0

# expect <verdict: ok|reject> <label> <message>
expect() {
  local want="$1" label="$2" msg="$3"
  local fixture="$WORKDIR/fixture"
  printf '%s\0' "$msg" > "$fixture"

  local out rc
  out="$(bash "$CHECK" --fixture "$fixture" 2>&1)"
  rc=$?

  local got="ok"
  [ "$rc" -ne 0 ] && got="reject"

  if [ "$got" = "$want" ]; then
    echo "  PASS  $label"
    pass=$((pass + 1))
  else
    echo "  FAIL  $label (want=$want got=$got)"
    echo "        ---- output ----"
    printf '%s\n' "$out" | sed 's/^/        /'
    fail=$((fail + 1))
  fi
}

echo "check_no_ai_attribution.sh"

# --- must be accepted ------------------------------------------------------

expect ok "plain conventional commit" \
"fix(cli): authorize Tier 2 remote embeddings before the probe dials

Body prose with no trailer at all.

Verified: cargo nextest run."

expect ok "release-plz bot co-author" \
"chore: release v2.10.0 (#1967)

Co-authored-by: release-plz-tu-repo[bot] <337500568+release-plz-tu-repo[bot]@users.noreply.github.com>"

expect ok "github-actions bot co-author" \
"ci: refresh generated schemas

Co-authored-by: github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com>"

# The legitimate automation identities, written in the EXACT form the bot
# stamps. The allowlist compares whole names, so these are literal identities:
# `release-plz-tu-repo[bot]` is not `release-plz`, and `renovate[bot]` is not
# `renovate`.
expect ok "renovate bot co-author (exact actor[bot] identity)" \
"chore(deps): bump the wreq group

Co-authored-by: renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>"

expect ok "dependabot bot co-author" \
"chore(deps): bump the wreq group

Co-authored-by: dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>"

expect ok "trailer-like text in the BODY, not a trailer line" \
"docs: explain why the trailer ban is not blanket

The message body may discuss Co-Authored-By without being one: this line is
indented prose, so it is not matched as a trailer.
"

expect ok "quoted Generated marker in BODY prose, clean trailer block" \
"ci: explain the trailer rule to the next reader

This commit explains why a line shaped like \"Generated with Codebuff\" is
rejected when it sits in the trailer block, and why the release-plz bot is
allowed. Quoting the marker here must not fire: only the trailing block —
everything after the final blank line — is scanned.

Nothing attributable follows."

expect ok "clean message that ends in a newline" \
$'chore: tidy imports\n\nBody prose, no trailer.\n'

# --- must be rejected ------------------------------------------------------

expect reject "Codebuff co-author (the case that slipped through)" \
"fix(cli): Tier 2 sin --tier2-embedding-provider usa el pool local (#1981)

Generated with Codebuff 🤖
Co-Authored-By: Codebuff <noreply@codebuff.com>"

expect reject "Claude co-author (same shape, already on main)" \
"feat(ai): ungate the semantic cleaner and its CLI flags (ADR-0004 slice C)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"

expect reject "Generated marker alone, no co-author" \
"chore: tidy imports

Generated with SomeTool"

expect reject "robot emoji alone" \
"chore: tidy imports 🤖"

expect reject "human colleague as co-author" \
"feat(crawler): add polite crawl delay

Co-Authored-By: Ada Lovelace <ada@example.com>"

expect reject "case-insensitive key" \
"feat(crawler): add polite crawl delay

co-authored-by: claude <noreply@anthropic.com>"

expect reject "indented trailer still counts" \
"feat(crawler): add polite crawl delay

    Co-Authored-By: Codebuff <noreply@codebuff.com>"

expect reject "allowlisted token merely CONTAINED in the name (A1)" \
"fix(cli): the co-author is not an automation actor

Co-Authored-By: Codebuff, reviewed by renovate <bot@example.com>"

expect reject "allowlisted token at the END of the name, not the start (A1)" \
"fix(cli): the co-author is not an automation actor

Co-Authored-By: Ada, with help from renovate <ada@example.com>"

# The co-author name is compared by WHOLE IDENTITY, not by prefix. A human
# whose display name merely STARTS with an allowlisted actor is not that
# actor, and must not inherit its authorization.
expect reject "human name that merely STARTS with an allowlisted token" \
"fix(cli): the co-author is a human, not an automation actor

Co-Authored-By: renovate-bot, actually a human <someone@example.com>"

expect reject "allowlisted actor with a suffix wearing the bot shape" \
"chore(deps): bump the wreq group

Co-Authored-By: release-plz-fan[bot] <somebot@users.noreply.github.com>"

expect reject "allowlisted actor with a suffix wearing the dependabot shape" \
"chore(deps): bump the wreq group

Co-Authored-By: dependabot-wrangler[bot] <somebot@users.noreply.github.com>"

expect reject "allowlisted actor with a suffix wearing the github-actions shape" \
"ci: refresh generated schemas

Co-Authored-By: github-actions[bot]-fan <somebot@users.noreply.github.com>"

expect reject "second of two commits is the offender" \
"refactor(ai): slice 1

Co-authored-by: release-plz-tu-repo[bot] <337500568+release-plz-tu-repo[bot]@users.noreply.github.com>
---NEXT---
fix(cli): the actual offender

Co-Authored-By: Codebuff <noreply@codebuff.com>"

# The fixture reader keeps the newline that precedes the NUL separator, so
# these messages arrive with a trailing newline the production reader never
# sees. They must be read exactly like their newline-less twins.
expect reject "marker message that ends in a newline" \
$'chore: a\n\nGenerated with Codebuff\n'

expect reject "co-author message that ends in a newline" \
$'chore: b\n\nCo-Authored-By: Codebuff <noreply@codebuff.com>\n'

expect reject "robot emoji in a message that ends in a newline" \
$'chore: c\n\n🤖\n'

# --- fail closed -----------------------------------------------------------

expect reject "empty fixture has no messages" ""

echo ""
echo "  $pass passed, $fail failed"
[ "$fail" -eq 0 ]
