#!/usr/bin/env bash
#
# Semantics harness for the ruleset lookup in apply-tag-ruleset.sh (B49).
#
# The property is behavioral: `apply` must be IDEMPOTENT. It looks the
# ruleset up by name first, and the lookup has three distinct outcomes:
#
#   0  the ruleset exists  -> PUT it (update in place);
#   1  it genuinely does not exist -> POST it (create);
#   2  the API could not be read -> NOTHING.
#
# The defect (B49) was that `if ruleset_json="$(get_our_ruleset)"; then`
# collapsed 1 and 2: a 500 read as "absent", so `apply` POSTed a SECOND
# ruleset under the same name. The harness drives the real script with a
# fake 'gh' and asserts on what it was asked to do: a duplicate creation is
# a failure, an update is a success, and an unreadable API is a refusal.
#
# Everything runs offline against mktemp fixtures and a fake 'gh'. No
# network, no real rulesets, no repository state.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ────────────────────────────────────────────────────────────────
# Serves the ruleset list / detail reads and records the mutating calls
# (POST / PUT) instead of performing them.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

method="GET"; path=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method) shift; method="${1:-GET}" ;;
    --input) shift; if [[ $# -gt 0 ]]; then shift; cat >/dev/null; fi ;;
    -*) shift ;;
    *) path="$1" ;;
  esac
  shift
done

case "$path" in
  */rulesets)
    if [[ "$method" == "POST" ]]; then
      echo "POST" >> "$state/calls"
      cat >/dev/null
      exit 0
    fi
    # GET the list. The failure marker simulates a 500 from the API.
    if [[ -f "$state/list-fails" ]]; then
      echo "gh: simulated API failure (500)" >&2
      exit 9
    fi
    cat "$state/rulesets.json"
    ;;
  */rulesets/*)
    if [[ "$method" == "PUT" ]]; then
      echo "PUT" >> "$state/calls"
      cat >/dev/null
      exit 0
    fi
    cat "$state/ruleset-detail.json"
    ;;
  *)
    echo "fake gh: unhandled api path: $method $path" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$BIN/gh"

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
calls() { if [[ -f "$STATE/calls" ]]; then tr '\n' ' ' < "$STATE/calls" | sed 's/ $//'; fi; }

RUN_RC=0
RUN_OUT=""
run_apply() { # run_apply <subcommand>
  : > "$STATE/calls"
  RUN_RC=0
  RUN_OUT="$( cd "$REPO_ROOT" && GITHUB_REPOSITORY=owner/repo PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    bash scripts/apply-tag-ruleset.sh "$1" 2>&1 )" || RUN_RC=$?
}

echo "test_apply_tag_ruleset: an unreadable API is never mistaken for an absent ruleset"

# ─── 1. B49: the API read fails -> apply must NOT create anything ──────────
# Pre-fix this POSTed a duplicate ruleset: the 500 read as "absent".
: > "$STATE/list-fails"
printf '%s\n' '[]' > "$STATE/rulesets.json"
printf '%s\n' '{"id":42,"rules":[]}' > "$STATE/ruleset-detail.json"
run_apply apply
if (( RUN_RC != 0 )); then
  check "B49: an API failure -> non-zero exit" "yes" "yes"
else
  check "B49: an API failure -> non-zero exit" "yes" "no"
fi
check "B49: an API failure -> NO ruleset created" "" "$(calls)"
check_contains "B49: it says the read failed" "could not read the existing ruleset" "$RUN_OUT"
check_contains "B49: it names the duplicate it refused" "refusing to create a duplicate ruleset" "$RUN_OUT"

# ─── 2. the check subcommand still tells 'cannot tell' from 'not protected' ─
run_apply check
if (( RUN_RC == 2 )); then
  check "check: an API failure -> exit 2 (cannot tell)" "2" "$RUN_RC"
else
  check "check: an API failure -> exit 2 (cannot tell)" "2" "$RUN_RC"
fi

# ─── 3. the ruleset exists -> update in place, never create ────────────────
rm -f "$STATE/list-fails"
printf '%s\n' '[{"name":"release-tags-immutable","id":42}]' > "$STATE/rulesets.json"
printf '%s\n' '{"id":42,"rules":[]}' > "$STATE/ruleset-detail.json"
run_apply apply
check "existing ruleset -> exit 0" "0" "$RUN_RC"
check "existing ruleset -> PUT, never POST" "PUT" "$(calls)"
check_contains "existing ruleset -> it says it updated" "Updated existing ruleset" "$RUN_OUT"

# ─── 4. the ruleset genuinely does not exist -> create it ──────────────────
printf '%s\n' '[]' > "$STATE/rulesets.json"
run_apply apply
check "absent ruleset -> exit 0" "0" "$RUN_RC"
check "absent ruleset -> POST" "POST" "$(calls)"
check_contains "absent ruleset -> it says it created" "Created ruleset" "$RUN_OUT"

echo "test_apply_tag_ruleset: PASS=$PASS FAIL=$FAIL"
if (( FAIL > 0 )); then
  exit 1
fi
echo "OK: an unreadable ruleset API never turns into a duplicate ruleset."
