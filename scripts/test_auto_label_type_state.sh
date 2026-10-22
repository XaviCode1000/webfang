#!/usr/bin/env bash
# shellcheck disable=SC2034  # RC is captured for the report; the oracle asserts on the label set
# test_auto_label_type_state.sh — end-state oracle for scripts/auto_label_type.sh.
# Independent of scripts/test_auto_label_type.sh on purpose: different fake gh
# (JSON state instead of line state), asserts the PR's FINAL label set, not calls.
# Covers: S1 no label / S2 matching label / S3 stale label / S4 two stale labels /
# S5 non-type label present / S6 retitle replaces / S7 non-type labels survive /
# S8 no PUT (label set replacement) / S9 DELETE path percent-encodes the colon.
# Run: bash scripts/test_auto_label_type_state.sh
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${WEBFANG_AUTO_LABEL_SCRIPT:-$REPO_ROOT/scripts/auto_label_type.sh}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"; mkdir -p "$BIN"
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
S="${PROBE_STATE}"; METHOD=GET; ENDPOINT=""; BODY=""; VERB=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method|-X) shift; METHOD="${1:-GET}"; shift;;
    --input) shift
 if [[ "${1:-}" == "-" ]]; then BODY="$(cat)"; shift; else BODY="$(cat < "$1")"; shift; fi;;
    --json|--jq|-q|--repo|-R) shift; shift;;
    --*) shift;;
    *) if [[ -z "$VERB" ]]; then VERB="$1"; elif [[ -z "$ENDPOINT" ]]; then ENDPOINT="$1"; fi; shift;;
  esac
done
printf '%s %s %s\n' "$METHOD" "$VERB" "$ENDPOINT" >> "${S}/calls"
case "$VERB" in
  pr) jq '{title: .title, labels: [.labels[] | {name: .}]}' "${S}/pr.json"; exit 0;;
  api) case "$METHOD" in
    POST|PUT)
      [[ "$ENDPOINT" == */labels ]] || { echo "fake gh: unexpected $METHOD $ENDPOINT" >&2; exit 99; }
      jq -r '.labels[]?' <<<"$BODY" | while IFS= read -r l; do
        [[ -n "$l" ]] || continue
        jq --arg n "$l" '.labels = (.labels|map(select(. != $n)) + [$n])' "${S}/pr.json" > "${S}/t" && mv "${S}/t" "${S}/pr.json"
      done; echo '[]';;
    DELETE)
      n="${ENDPOINT##*/labels/}"
      d="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.unquote(sys.argv[1]))' "$n")"
      jq --arg n "$d" '.labels = (.labels|map(select(. != $n)))' "${S}/pr.json" > "${S}/t" && mv "${S}/t" "${S}/pr.json"; echo '{}';;
    *) echo "fake gh: unexpected $METHOD $ENDPOINT" >&2; exit 99;;
  esac;;
 *) echo "fake gh: unhandled verb '$VERB'" >&2; exit 99;;
esac
FAKE
chmod +x "$BIN/gh"
PASS=0; FAIL=0
case_run() { # <title> <initial labels...>
  local title="$1"; shift
  local d="$WORK/case"; rm -rf "$d"; mkdir -p "$d"
  # shellcheck disable=SC2155  # the JSON literal is built from a command substitution on purpose
  local j='{"title":'"$(jq -Rn --arg t "$title" '$t')"',"labels":[]}' l
  for l in "$@"; do j="$(jq --arg n "$l" '.labels += [$n]' <<<"$j")"; done
  printf '%s' "$j" > "$d/pr.json"; : > "$d/calls"
  OUT="$(PATH="$BIN:$PATH" PROBE_STATE="$d" PR_NUMBER=42 REPO=o/r GH_TOKEN=t bash "$SCRIPT" 2>&1)"; RC=$?
  N_TYPE="$(jq -r '[.labels[]|select(startswith("type:"))]|length' "$d/pr.json")"
  TYPE="$(jq -r '[.labels[]|select(startswith("type:"))]|join(",")' "$d/pr.json")"
  ALL="$(jq -r '.labels|join(",")' "$d/pr.json")"
  PUTS="$(grep -cE '^PUT' "$d/calls" || true)"
  DELS="$(grep -E '^DELETE' "$d/calls" | awk '{print $3}' | paste -sd, -)"
}
expect() { if [[ "$2" == "$3" ]]; then printf '  OK   %s\n' "$1"; PASS=$((PASS+1));
  else printf '  FAIL %s expected [%s] got [%s]\n' "$1" "$2" "$3"; printf '%s\n' "$OUT"|sed 's/^/       | /'; FAIL=$((FAIL+1)); fi; }
case_run "feat(cli): x"; expect "S1 no label ->1 type:* label" "1" "$N_TYPE"; expect "S1 derived" "type:feature" "$TYPE"
case_run "feat(cli): x" type:feature;    expect "S2 matching -> 1" "1" "$N_TYPE"; expect "S2 unchanged" "type:feature" "$TYPE"
case_run "feat(cli): x" type:bug;        expect "S3 stale -> 1" "1" "$N_TYPE"; expect "S3 derived" "type:feature" "$TYPE"
case_run "feat(cli): x" type:bug type:docs; expect "S4 two stale -> 1" "1" "$N_TYPE"; expect "S4 derived" "type:feature" "$TYPE"
case_run "fix(cli): x" bug;              expect "S5 non-type -> 1 type:*" "1" "$N_TYPE"; expect "S5 human label kept" "bug,type:bug" "$ALL"
case_run "fix(cli): x" type:feature;     expect "S6 retitle -> 1" "1" "$N_TYPE"; expect "S6 retitled" "type:bug" "$TYPE"
case_run "feat(cli): x" type:bug wip;    expect "S7 other labels survive" "type:feature,wip" "$(tr ',' '\n' <<<"$ALL" | sort | paste -sd, -)"
case_run "fix(cli): x" wip;              expect "S8 no PUT (no set replacement)" "0" "$PUTS"
case_run "feat(cli): x" type:bug;        expect "S9 DELETE encodes the colon" "repos/o/r/issues/42/labels/type%3Abug" "$DELS"
printf '%s\n' "state oracle: PASS=$PASS FAIL=$FAIL"; [[ "$FAIL" -eq 0 ]]
