#!/usr/bin/env bash
#
# Tag ref protection ruleset — ensures v* tags are immutable (update + deletion blocked).
#
# Subcommands: apply, check
# GITHUB_REPOSITORY required (owner/repo).
#
# Ruleset shape (verified against the LIVE GitHub API 2026-09-22, not just the docs):
# {
#   "name": "release-tags-immutable",
#   "target": "tag",
#   "enforcement": "active",
#   "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
#   "rules": [ { "type": "update",
#                "parameters": { "update_allows_fetch_and_merge": false } },
#              { "type": "deletion" } ]
# }
#
# Two live-API constraints the previous shape violated (both 422 on create/PUT):
#   - update with "parameters": {} -> 422 "Invalid property /rules/0: data matches
#     no possible input." When parameters is present it MUST carry the boolean
#     update_allows_fetch_and_merge; omitting the parameters key entirely is also
#     accepted, but the explicit false is what we send (creation must not imply
#     fetch-and-merge). deletion carries NO parameters key (matches the GET shape).
#   - include ["v*"] -> 422 "Invalid target patterns: 'v*'". Tag ruleset patterns
#     must be fully qualified: ["refs/tags/v*"].
#
# - target: "tag" with conditions.ref_name fnmatch patterns; refs/tags/v* matches
#   every release tag (* does not cross /, which is fine for tags).
# - update = "Restrict updates": only users with bypass permission may push to matching refs.
# - deletion blocks retag-by-delete-and-recreate.
# - Creation is deliberately NOT restricted — release-plz and cut-patch-tag.yml must still
#   cut new tags. Blocking creation would break the pipeline it protects.
#
# check: verifies a ruleset exists with target=tag, enforcement=active, include containing
# refs/tags/v*, and both update + deletion rules. Exit 0 if found and correct; 1 if absent/incomplete
# (message names exactly what is missing and prints remediation command); 2 if API call fails
# or response cannot be parsed — a caller MUST be able to tell "not protected" from "cannot tell".
# Reading rulesets needs administration:read on the token, which release.yml's preflight
# does not have today — call this from a context that has it.
#
# apply: idempotent — runs check's detection first; if a ruleset with the right name exists,
# PUT it rather than creating a duplicate; otherwise POST. Prints what it did. Never deletes
# a ruleset it did not create.
set -euo pipefail

SUBCOMMAND="${1:-}"
[[ -n "$SUBCOMMAND" ]] || { echo "usage: $(basename "$0") {apply|check}" >&2; exit 2; }

GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"
[[ -n "$GITHUB_REPOSITORY" ]] || { echo "::error::GITHUB_REPOSITORY is required" >&2; exit 2; }

RULESET_NAME="release-tags-immutable"
API_BASE="repos/$GITHUB_REPOSITORY/rulesets"

# jq_json [--arg name value ...] <expr> <json> — read one value from a server response
# WITHOUT letting a parse error kill the script. Under `set -e` a bare `x="$(jq ...)"`
# aborts on garbage, so the caller exits 5 with a raw jq error instead of reporting
# "cannot tell" (exit 2). Failing closed is correct; failing closed with no diagnosis is not.
#
# Leading jq flags that take arguments (--arg, --argjson, --rawfile, --slurpfile) pass
# through to jq before the positional <expr> <json>. Without this, a call like
# `jq_json --arg name "$RULESET_NAME" 'expr' "$json"` bound expr='--arg', json='name',
# jq failed silently, and every lookup returned empty — which made the ruleset check
# report "not found" against a ruleset that existed (measured against the live API).
jq_json() {
  local opts=()
  while [[ $# -ge 2 && "$1" == --* ]]; do
    case "$1" in
      --arg|--argjson|--rawfile|--slurpfile)
        [[ $# -ge 3 ]] || break
        opts+=("$1" "$2" "$3"); shift 3 ;;
      *)
        opts+=("$1"); shift ;;
    esac
  done
  local expr="${1:-}" json="${2:-}"
  printf '%s' "$json" | jq -r ${opts[@]+"${opts[@]}"} "$expr" 2>/dev/null || true
}

# Fetch all rulesets and find ours by name.
# The list endpoint may not inline 'rules', so we fetch the specific ruleset by ID.
get_our_ruleset() {
  local list_response
  list_response="$(gh api "$API_BASE" 2>/dev/null)" || {
    echo "::error::failed to list rulesets for $GITHUB_REPOSITORY" >&2
    return 2
  }

  if ! jq_json '. | type' "$list_response" | grep -qx 'array'; then
    echo "::error::the rulesets list response is not a JSON array - cannot tell whether the tags are protected." >&2
    return 2
  fi
  local ruleset_id
  # shellcheck disable=SC2016  # jq $name is a jq variable, not shell expansion
  ruleset_id="$(jq_json --arg name "$RULESET_NAME" '.[] | select(.name == $name) | .id // empty' "$list_response")"
  [[ -n "$ruleset_id" ]] || return 1

  gh api "$API_BASE/$ruleset_id" 2>/dev/null || {
    echo "::error::failed to fetch ruleset $ruleset_id details" >&2
    return 2
  }
}

check_ruleset() {
  local ruleset_json
  local rc=0
  ruleset_json="$(get_our_ruleset)" || rc=$?
  if [[ $rc -ne 0 ]]; then
    if [[ $rc -eq 2 ]]; then
      # API failure / unparseable — exit 2 so caller can distinguish "not protected" from "cannot tell".
      return 2
    fi
    # Ruleset not found — exit 1 (not protected).
    echo "::error::ruleset '$RULESET_NAME' not found — run: gh api --method POST '$API_BASE' --input - <<'EOF'" >&2
    cat <<'EOF' >&2
{
  "name": "release-tags-immutable",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
  "rules": [ { "type": "update", "parameters": { "update_allows_fetch_and_merge": false } },
             { "type": "deletion" } ]
}
EOF
    return 1
  fi

  # Verify the shape.
  local target enforcement include_rules rules_json
  target="$(jq_json '.target // empty' "$ruleset_json")"
  enforcement="$(jq_json '.enforcement // empty' "$ruleset_json")"
  include_rules="$(jq_json '.conditions.ref_name.include[]?' "$ruleset_json")"
  rules_json="$(printf '%s' "$ruleset_json" | jq -c '.rules // []' 2>/dev/null || true)"
  # A ruleset body we cannot read is NOT evidence of a missing rule: report "cannot
  # tell" (2) rather than "not protected" (1).
  if [[ -z "$target" && -z "$enforcement" ]]; then
    echo "::error::the ruleset response could not be parsed - cannot tell whether the tags are protected." >&2
    return 2
  fi

  local missing=()

  [[ "$target" == "tag" ]] || missing+=("target=tag (got '$target')")
  [[ "$enforcement" == "active" ]] || missing+=("enforcement=active (got '$enforcement')")

  # Check include contains refs/tags/v* (the only form the live API accepts for
  # tag rulesets — bare "v*" is rejected with 422 Invalid target patterns).
  local has_vstar=false
  while IFS= read -r pattern; do
    [[ "$pattern" == "refs/tags/v*" ]] && has_vstar=true
  done <<<"$include_rules"
  $has_vstar || missing+=("conditions.ref_name.include containing refs/tags/v*")

  # Check rules contain both update and deletion.
  local has_update=false has_deletion=false
  while IFS= read -r rule; do
    local rtype
    rtype="$(jq_json '.type // empty' "$rule")"
    [[ "$rtype" == "update" ]] && has_update=true
    [[ "$rtype" == "deletion" ]] && has_deletion=true
  done < <(jq -c '.[]' <<<"$rules_json")

  $has_update || missing+=("rules: update (restrict updates)")
  $has_deletion || missing+=("rules: deletion (block delete-and-recreate)")

  if [[ ${#missing[@]} -gt 0 ]]; then
    printf '::error::ruleset '"'$RULESET_NAME'"' exists but is incomplete — missing: %s\n' "$(IFS=', '; echo "${missing[*]}")" >&2
    echo "Remediation: run 'bash scripts/apply-tag-ruleset.sh apply' (requires admin token with administration:write)" >&2
    return 1
  fi

  return 0
}

apply_ruleset() {
  local ruleset_json
  local existing_id=""

  # First check if it exists and get its ID.
  if ruleset_json="$(get_our_ruleset 2>/dev/null)"; then
    existing_id="$(jq_json '.id // empty' "$ruleset_json")"
    [[ -n "$existing_id" ]] || existing_id=""
  fi

  # Ruleset payload — LIVE-verified shape (see header): fully-qualified include
  # pattern, update parameters carry the required boolean, deletion has no
  # parameters key. Any deviation is a 422 from the real API.
  local payload
  payload='{
    "name": "release-tags-immutable",
    "target": "tag",
    "enforcement": "active",
    "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
    "rules": [ { "type": "update", "parameters": { "update_allows_fetch_and_merge": false } },
               { "type": "deletion" } ]
  }'

  if [[ -n "$existing_id" ]]; then
    # Update existing ruleset.
    if gh api --method PUT "$API_BASE/$existing_id" --input - <<<"$payload" >/dev/null 2>&1; then
      echo "Updated existing ruleset '$RULESET_NAME' (id=$existing_id)"
      return 0
    else
      echo "::error::failed to update ruleset $existing_id" >&2
      return 1
    fi
  else
    # Create new ruleset.
    if gh api --method POST "$API_BASE" --input - <<<"$payload" >/dev/null 2>&1; then
      echo "Created ruleset '$RULESET_NAME'"
      return 0
    else
      echo "::error::failed to create ruleset '$RULESET_NAME'" >&2
      return 1
    fi
  fi
}

case "$SUBCOMMAND" in
  check)
    check_ruleset
    exit $?
    ;;
  apply)
    apply_ruleset
    exit $?
    ;;
  *)
    echo "::error::unknown subcommand '$SUBCOMMAND'" >&2
    exit 2
    ;;
esac