#!/usr/bin/env bash
# test_auto_label_type.sh — semantics harness for scripts/auto_label_type.sh (#2126).
#
# WHAT IT PINS
#   - the mapping table (each conventional-commit prefix -> its `type:*` label);
#   - the breaking marker (`feat!: ...`) -> `type:breaking-change`;
#   - a commit type with no label of its own (perf, revert, build, ci, style)
#     -> `type:chore`, the closest label that exists;
#   - an unknown prefix -> NO label: the script does not guess, and the
#     validation stays red until a human labels the PR;
#   - the REPLACE behaviour: a stale `type:*` label is removed and the label
#     derived from the CURRENT title takes its place, and the END STATE
#     carries exactly ONE `type:*` label — never two, never zero;
#   - the IDEMPOTENCE: when the PR already carries exactly the derived label,
#     NOTHING happens at all — not even a redundant write;
#   - the map lives in ONE place: AGENTS.md POINTS at the script and does not
#     duplicate the table, and every label the script can apply is one the
#     repository really has;
#   - the WIRING: the labeler reacts to `opened` and `edited` only (so a
#     human's label change can never start a ping-pong with the bot), it
#     never blocks the validation, and the validation waits for it;
#   - the fail-closed paths: an unreadable PR and a refused write change
#     NOTHING.
#
# EVERYTHING RUNS OFFLINE: a fake `gh` on PATH, mktemp fixtures, no network
# and no repository state beyond AGENTS.md and the workflow (both READ).
# The fake gh KEEPS the label set up to date, so the harness asserts the
# FINAL state of the PR and not only the calls that were made. It also
# answers 99 to any invocation the script is not supposed to make (a label
# SET, an unknown endpoint), so an implementation that replaces the label
# set instead of adding to it goes red.
#
# THE VOCABULARY PIN
#   REPO_TYPE_LABELS below is the repository's `type:*` vocabulary, verified
#   against the live label list (46 labels, 7 of them `type:*`). The harness
#   is OFFLINE, so it checks the script against this PIN, never against a
#   network call. Refresh it with:
#     gh label list --limit 300 --json name --jq '.[].name'
#   or run `bash scripts/test_auto_label_type.sh --check-live-vocabulary`,
#   which compares the pin with the live list and fails when they diverge.
#
# NON-VACUITY: the last section mutates a COPY of the script four ways
# (ignore the label the PR already carries, map `fix` to the wrong label,
# guess a label for a prefix the vocabulary does not know, and do not remove
# the stale label) and asserts that the suite goes RED against each of them.
# A suite that stays green against a broken script proves nothing.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${WEBFANG_AUTO_LABEL_SCRIPT:-$REPO_ROOT/scripts/auto_label_type.sh}"
AGENTS_MD="$REPO_ROOT/AGENTS.md"
WORKFLOW="$REPO_ROOT/.github/workflows/pr-validation.yml"

# The end-state oracle: a SECOND, independently written implementation of
# the same behaviour (its own fake gh, its own state model). It is the
# witness that the suite above cannot rubber-stamp itself, so the mutation
# section below demands that it goes RED against every broken script too.
ORACLE="$REPO_ROOT/scripts/test_auto_label_type_state.sh"

# WHAT THE ORACLE DOES NOT COVER, SAID OUT LOUD.
# The oracle's nine scenarios (S1-S9) are about the LABEL SET of the PR; none
# of them uses a title the vocabulary does not know. The mutation that makes
# the script guess a label for an unknown prefix is therefore invisible to
# it BY CONSTRUCTION. The oracle is the verifier's file and its semantics may
# not be changed here, so the mutation stays in this section — the suite
# above still catches it — and the gap is reported instead of hidden.
ORACLE_BLIND_SPOT="the oracle's scenarios cover the label set (S1-S9), not the unknown-prefix rule"

# The prefix the PR gate counts ("Require exactly one type:* label").
TYPE_LABEL_PREFIX='type:'

# The repository's `type:*` vocabulary, in alphabetical order, one per line.
REPO_TYPE_LABELS="$(printf '%s\n' \
  type:breaking-change \
  type:bug \
  type:chore \
  type:docs \
  type:feature \
  type:refactor \
  type:testing)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/bin"
STATE="$WORK/state"
mkdir -p "$BIN" "$STATE"

# ─── Fake gh ──────────────────────────────────────────────────────────────────
# Serves `pr view` from a fixture, RECORDS the label mutations instead of
# performing them, and KEEPS the label set up to date so the harness can
# assert the final state of the PR.
cat > "$BIN/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
state="$FAKE_STATE"

sub=""
path=""
method="GET"
payload=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method | -X)
      shift
      method="${1:-GET}"
      shift
      ;;
    --input)
      shift
      if [[ "${1:-}" == "-" ]]; then
        payload="$(cat)"
      else
        shift
      fi
      ;;
    --repo | -R | --json | -q | --jq)
      shift
      shift
      ;;
    -*) shift ;;
    *)
      # The first two positionals are the subcommand and its verb; the rest
      # (the PR number, for instance) are arguments the fake does not need.
      if [[ -z "$sub" ]]; then
        sub="$1"
      elif [[ -z "$path" ]]; then
        path="$1"
      fi
      shift
      ;;
  esac
done

# list_has <file> <item> — exact-line membership, without a pipeline (a
# `grep -q` in a pipeline is a SIGPIPE race under `pipefail`).
list_has() {
  local file="$1" item="$2" line
  [[ -f "$file" ]] || return 1
  while IFS= read -r line; do
    if [[ "$line" == "$item" ]]; then
      return 0
    fi
  done <"$file"
  return 1
}

if [[ "$sub" == "pr" && "$path" == "view" ]]; then
  if [[ -f "$state/read-fails" ]]; then
    echo "gh: simulated API failure (403 Resource not accessible)" >&2
    exit 1
  fi
  cat "$state/pr.json"
  exit 0
fi

# The additive endpoint: POST .../labels
if [[ "$sub" == "api" && "$method" == "POST" && "$path" == */labels ]]; then
  if [[ -f "$state/post-fails" ]]; then
    echo "gh: simulated API failure (403 Resource not accessible)" >&2
    exit 1
  fi
  printf 'POST %s %s\n' "$path" "$payload" >>"$state/calls"
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    if ! list_has "$state/labels" "$l"; then
      printf '%s\n' "$l" >>"$state/labels"
    fi
  done < <(printf '%s' "$payload" | jq -r '.labels[]?')
  exit 0
fi

# The removal endpoint: DELETE .../labels/<name>
if [[ "$sub" == "api" && "$method" == "DELETE" && "$path" == */labels/* ]]; then
  if [[ -f "$state/delete-fails" ]]; then
    echo "gh: simulated API failure (403 Resource not accessible)" >&2
    exit 1
  fi
  name="${path##*/labels/}"
  # The server decodes the path segment before matching the label: the fake
  # does the same, so the state stays the PR's real label set.
  name="${name//%3A/:}"
  printf 'DELETE %s %s\n' "$path" "$name" >>"$state/calls"
  if [[ -f "$state/labels" ]]; then
    grep -vxF -- "$name" "$state/labels" >"$state/labels.next" 2>/dev/null || true
    mv "$state/labels.next" "$state/labels"
  fi
  exit 0
fi

echo "fake gh: unhandled invocation: '$sub' '$method' '$path'" >&2
exit 99
FAKE
chmod +x "$BIN/gh"

PASS=0
FAIL=0
RUN_RC=0
RUN_OUT=""
ADDED=""
REMOVED=""
CALLS=0
FINAL_TYPE=""
FINAL_ALL=""

# check <label> <expected> <actual>
#
# The script's own output is printed under a failure: it says WHAT the
# script did, which is the fastest way to read a red case.
check() {
  if [[ "$2" == "$3" ]]; then
    printf '  OK   %s\n' "$1"
    PASS=$((PASS + 1))
  else
    printf '  FAIL %s — expected [%s], got [%s]\n' "$1" "$2" "$3"
    if [[ -n "$RUN_OUT" ]]; then
      printf '%s\n' "$RUN_OUT" | sed 's/^/        | /'
    fi
    FAIL=$((FAIL + 1))
  fi
}

# list_contains <newline-separated list> <item>
#
# A bash loop and NOT a `printf | grep -qx`: `grep -q` exits at the first
# match and closes the pipe, so the writer takes a SIGPIPE — and under
# `pipefail` the pipeline reports THAT instead of grep's verdict. The answer
# to "is this label in the list" does not depend on who wins the race.
list_contains() {
  local list="$1" item="$2" line
  while IFS= read -r line; do
    if [[ "$line" == "$item" ]]; then
      return 0
    fi
  done <<<"$list"
  return 1
}

# text_contains <text> <needle> — a SUBSTRING match.
#
# `list_contains` compares whole lines; this one answers "does this text
# mention this fragment", which is what the wiring checks ask.
text_contains() {
  case "$1" in
    *"$2"*) return 0 ;;
    *) return 1 ;;
  esac
}

# labels_left <file> — the file's lines, comma separated.
labels_left() {
  local file="$1" line out=""
  [[ -f "$file" ]] || return 0
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    out="${out:+$out,}$line"
  done <"$file"
  printf '%s' "$out"
}

# type_labels_left — the `type:*` labels the PR carries after the run.
type_labels_left() {
  local file="$1" line out=""
  [[ -f "$file" ]] || return 0
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    [[ "$line" == "${TYPE_LABEL_PREFIX}"* ]] || continue
    out="${out:+$out,}$line"
  done <"$file"
  printf '%s' "$out"
}

# run_script <script> <title> [label ...]
#
# Runs the script against the fake gh with the given PR (the title and the
# label names it carries) and records what the script did: the exit code, the
# labels it added, the labels it removed and the FINAL state of the PR.
run_script() {
  local script="$1" title="$2"
  shift 2
  local labels_json label
  labels_json="$(jq -nc --args '$ARGS.positional | map({name: .})' "$@")"
  jq -n --arg t "$title" --argjson l "$labels_json" '{title: $t, labels: $l}' >"$STATE/pr.json"
  : >"$STATE/labels"
  for label in "$@"; do
    printf '%s\n' "$label" >>"$STATE/labels"
  done
  : >"$STATE/calls"
  RUN_RC=0
  RUN_OUT="$(cd "$REPO_ROOT" && PATH="$BIN:$PATH" FAKE_STATE="$STATE" \
    PR_NUMBER=42 REPO=owner/repo GH_TOKEN=fake-token \
    bash "$script" 2>&1)" || RUN_RC=$?
  ADDED="$(awk '$1 == "POST" { print $3 }' "$STATE/calls" 2>/dev/null |
    jq -s -r '[.[].labels[]?] | join(",")' 2>/dev/null || true)"
  REMOVED="$(awk '$1 == "DELETE" { print $3 }' "$STATE/calls" 2>/dev/null |
    paste -sd, - 2>/dev/null || true)"
  CALLS="$(wc -l <"$STATE/calls" 2>/dev/null | tr -d ' ')"
  CALLS="${CALLS:-0}"
  FINAL_TYPE="$(type_labels_left "$STATE/labels")"
  FINAL_ALL="$(labels_left "$STATE/labels")"
}

# ─── the map lives in ONE place ───────────────────────────────────────────────
#
# AGENTS.md is the human-facing reference for the RULE ("exactly one `type:*`
# label"), and the script is the source of truth for the MAP. The map is not
# written twice: AGENTS.md points at the script.
check_map_single_source() {
  local script="$1"
  local script_map map_labels label declared

  RUN_OUT=""
  script_map="$(bash "$script" --map 2>/dev/null)"
  map_labels="$(printf '%s\n' "$script_map" | cut -f2 | sort -u)"

  check "map: AGENTS.md points at scripts/auto_label_type.sh" "yes" \
    "$(text_contains "$(cat "$AGENTS_MD")" "scripts/auto_label_type.sh" && echo yes || echo no)"
  check "map: AGENTS.md does not duplicate a label of the vocabulary" "" \
    "$(grep -oE 'type:[a-z][a-z-]*' "$AGENTS_MD" | sort -u | paste -sd, - 2>/dev/null || true)"

  # Every label the script can apply is one the repository really has, and
  # every label of the repository is reachable: the script neither invents
  # a label nor drops one.
  while IFS= read -r label; do
    [[ -n "$label" ]] || continue
    if list_contains "$REPO_TYPE_LABELS" "$label"; then
      printf '  OK   vocabulary: %s is a label the repository has\n' "$label"
      PASS=$((PASS + 1))
    else
      printf '  FAIL vocabulary: %s is NOT a label the repository has\n' "$label"
      FAIL=$((FAIL + 1))
    fi
  done <<<"$map_labels"

  while IFS= read -r declared; do
    [[ -n "$declared" ]] || continue
    if list_contains "$map_labels" "$declared"; then
      printf '  OK   vocabulary: %s is reachable from the map\n' "$declared"
      PASS=$((PASS + 1))
    else
      printf '  FAIL vocabulary: %s is NOT reachable from the map\n' "$declared"
      FAIL=$((FAIL + 1))
    fi
  done <<<"$REPO_TYPE_LABELS"
}

# ─── the wiring ───────────────────────────────────────────────────────────────
#
# The labeler reacts to `opened` and `edited` ONLY. A bot that also reacted
# to `labeled` would fight a human forever: the human sets a label, the
# `labeled` event fires, the bot replaces it, the human sets it again...
# `edited` (the retitle) is the one event where the derived label changes.
#
# The WORKFLOW's own event list stays wide on purpose: it also carries the
# required "Validate PR metadata" job, and a workflow that did not run on
# `synchronize` would leave the required check unreported after a new push
# (the PR would sit on "Expected" forever). The narrow trigger belongs to
# the LABELER job, which is where the ping-pong would happen.
check_wiring() {
  local labeler_if validate_if types_line

  RUN_OUT=""
  labeler_if="$(awk '/^[[:space:]]*auto-label-type:/{f=1} f && /^[[:space:]]*if:/{print; exit}' "$WORKFLOW")"
  validate_if="$(awk '/^[[:space:]]*validate:/{f=1} f && /^[[:space:]]*if:/{print; exit}' "$WORKFLOW")"
  types_line="$(awk '/^[[:space:]]*pull_request:/{f=1; next} f && /types:/{print; exit}' "$WORKFLOW")"

  check "wiring: the labeler reacts to 'opened'" "yes" \
    "$(text_contains "$labeler_if" "github.event.action == 'opened'" && echo yes || echo no)"
  check "wiring: the labeler reacts to 'edited'" "yes" \
    "$(text_contains "$labeler_if" "github.event.action == 'edited'" && echo yes || echo no)"
  check "wiring: the labeler does NOT react to 'labeled'" "no" \
    "$(text_contains "$labeler_if" "'labeled'" && echo yes || echo no)"
  check "wiring: the labeler does NOT react to 'unlabeled'" "no" \
    "$(text_contains "$labeler_if" "'unlabeled'" && echo yes || echo no)"
  check "wiring: the labeler does NOT react to 'synchronize'" "no" \
    "$(text_contains "$labeler_if" "'synchronize'" && echo yes || echo no)"
  check "wiring: the labeler does NOT react to 'reopened'" "no" \
    "$(text_contains "$labeler_if" "'reopened'" && echo yes || echo no)"

  check "wiring: the labeler never blocks the validation" "yes" \
    "$(awk '/^[[:space:]]*auto-label-type:/{f=1} f && /continue-on-error: true/{print "yes"; exit}' "$WORKFLOW")"
  check "wiring: the validation waits for the labeler (no race)" "yes" \
    "$(awk '/^[[:space:]]*validate:/{f=1} f && /needs: auto-label-type/{print "yes"; exit}' "$WORKFLOW")"
  check "wiring: the validation runs whatever the labeler did" "yes" \
    "$(text_contains "$validate_if" "!cancelled()" && echo yes || echo no)"

  check "wiring: the workflow still validates on 'synchronize'" "yes" \
    "$(text_contains "$types_line" "synchronize" && echo yes || echo no)"
  check "wiring: the workflow still validates on 'reopened'" "yes" \
    "$(text_contains "$types_line" "reopened" && echo yes || echo no)"
}

# ─── the case suite ───────────────────────────────────────────────────────────
run_suite() {
  local script="$1"
  # The suite's own verdict: it is red when the failure counter moved. The
  # counter is a global so the case output can be counted, but the snapshot
  # is local so the mutated runs (a subshell) cannot be confused with this
  # one.
  local before_fail="$FAIL"
  local entry type expected

  echo "-- the mapping table --"
  for entry in \
    "feat:type:feature" \
    "fix:type:bug" \
    "refactor:type:refactor" \
    "docs:type:docs" \
    "chore:type:chore" \
    "test:type:testing"; do
    type="${entry%%:*}"
    expected="${entry#*:}"
    run_script "$script" "$type(cli): un cambio con alcance"
    check "mapping: $type -> $expected" "$expected" "$ADDED"
    check "mapping: $type adds exactly one label" "1" "$CALLS"
    check "mapping: $type ends with exactly one type label" "$expected" "$FINAL_TYPE"
    check "mapping: $type exits 0" "0" "$RUN_RC"
  done

  echo "-- a prefix with no scope --"
  run_script "$script" "fix: un arreglo sin alcance"
  check "mapping: fix without scope -> type:bug" "type:bug" "$ADDED"

  echo "-- the breaking marker --"
  run_script "$script" "feat!: se rompe la API"
  check "breaking: feat!: -> type:breaking-change" "type:breaking-change" "$ADDED"
  run_script "$script" "feat(cli)!: se rompe la CLI"
  check "breaking: feat(cli)!: -> type:breaking-change" "type:breaking-change" "$ADDED"
  run_script "$script" "feat!(cli): forma malformada (#2072)"
  check "breaking: feat!(cli): -> type:breaking-change" "type:breaking-change" "$ADDED"
  check "breaking: adds exactly one label" "1" "$CALLS"

  echo "-- a commit type with no label of its own -> type:chore --"
  for entry in perf revert build ci style; do
    run_script "$script" "$entry: un cambio sin label propio"
    check "no own label: $entry -> type:chore" "type:chore" "$ADDED"
    check "no own label: $entry adds exactly one label" "1" "$CALLS"
  done

  echo "-- an unknown prefix gets NO label --"
  run_script "$script" "WIP: todavia no es un commit"
  check "unknown prefix: no label is applied" "" "$ADDED"
  check "unknown prefix: no write call at all" "0" "$CALLS"
  check "unknown prefix: no type label at the end" "" "$FINAL_TYPE"
  check "unknown prefix: exits 0 (the validation is the red net)" "0" "$RUN_RC"
  run_script "$script" "un titulo sin ningun prefijo"
  check "no prefix at all: no label is applied" "" "$ADDED"
  check "no prefix at all: no write call at all" "0" "$CALLS"

  echo "-- REPLACE: a stale type label gives way to the derived one --"
  run_script "$script" "feat(cli): el titulo cambio de tipo" "type:bug"
  check "replace: the derived label is added" "type:feature" "$ADDED"
  check "replace: the stale label is removed" "type:bug" "$REMOVED"
  check "replace: the removal targets the ENCODED label path" "yes" \
    "$(list_contains "$(awk '$1 == "DELETE" { print $2 }' "$STATE/calls")" \
      "repos/owner/repo/issues/42/labels/type%3Abug" && echo yes || echo no)"
  check "replace: the end state carries exactly one type label" "type:feature" "$FINAL_TYPE"
  check "replace: two write calls (add, then remove)" "2" "$CALLS"
  check "replace: exits 0" "0" "$RUN_RC"

  run_script "$script" "fix(cli): el titulo cambio de tipo" "type:feature"
  check "replace: the derived label is added" "type:bug" "$ADDED"
  check "replace: the stale label is removed" "type:feature" "$REMOVED"
  check "replace: the end state carries exactly one type label" "type:bug" "$FINAL_TYPE"

  run_script "$script" "feat(cli): dos labels viejos" "type:bug" "type:chore"
  check "replace: both stale labels are removed" "type:bug,type:chore" "$REMOVED"
  check "replace: the end state carries exactly one type label" "type:feature" "$FINAL_TYPE"

  echo "-- IDEMPOTENCE: the derived label is already there -> nothing at all --"
  run_script "$script" "feat(cli): sin cambios" "type:feature"
  check "idempotence: no label is added" "" "$ADDED"
  check "idempotence: no label is removed" "" "$REMOVED"
  check "idempotence: no write call at all" "0" "$CALLS"
  check "idempotence: the end state still carries exactly one type label" "type:feature" "$FINAL_TYPE"
  check "idempotence: exits 0" "0" "$RUN_RC"

  echo "-- a title that derives nothing keeps the label that is there --"
  run_script "$script" "WIP: todavia no es un commit" "type:bug"
  check "underivable title: nothing is added" "" "$ADDED"
  check "underivable title: nothing is removed" "" "$REMOVED"
  check "underivable title: the existing label stays" "type:bug" "$FINAL_TYPE"
  check "underivable title: no write call at all" "0" "$CALLS"

  echo "-- the other labels of the PR survive --"
  run_script "$script" "fix(cli): un arreglo" "bug"
  check "other labels: the type label is added" "type:bug" "$ADDED"
  check "other labels: exactly one write call" "1" "$CALLS"
  check "other labels: the label set is ADDED to, never replaced" "bug,type:bug" "$FINAL_ALL"
  check "other labels: the run exits 0" "0" "$RUN_RC"

  echo "-- fail closed --"
  : >"$STATE/read-fails"
  run_script "$script" "feat(cli): un cambio"
  check "fail closed: an unreadable PR exits non-zero" "non-zero" "$([[ $RUN_RC -ne 0 ]] && echo non-zero || echo zero)"
  check "fail closed: an unreadable PR changes nothing" "0" "$CALLS"
  rm -f "$STATE/read-fails"

  : >"$STATE/post-fails"
  run_script "$script" "feat(cli): un cambio"
  check "fail closed: a refused write exits non-zero" "non-zero" "$([[ $RUN_RC -ne 0 ]] && echo non-zero || echo zero)"
  check "fail closed: a refused write leaves the label set alone" "" "$FINAL_TYPE"
  rm -f "$STATE/post-fails"

  : >"$STATE/delete-fails"
  run_script "$script" "feat(cli): un cambio" "type:bug"
  check "fail closed: a refused removal exits non-zero" "non-zero" "$([[ $RUN_RC -ne 0 ]] && echo non-zero || echo zero)"
  rm -f "$STATE/delete-fails"

  echo "-- the map lives in ONE place --"
  check_map_single_source "$script"

  echo "-- the wiring --"
  check_wiring

  [[ "$FAIL" -eq "$before_fail" ]]
}

# mutate_and_expect_red <label> <sed-expression> [oracle-covers: yes|no]
#
# The mutation is applied to a COPY of the script (never to the tree) and the
# whole suite is re-run against it. Green against a broken script means the
# harness is decoration, not coverage.
#
# The ORACLE is held to the same standard: a mutation that this suite
# catches must also turn the independent implementation red, or the "second
# implementation" is not a witness. `oracle-covers` says whether the
# oracle's own scenarios exercise that rule: when they do not, the gap is
# PRINTED (a NOTE) instead of being silently tolerated.
mutate_and_expect_red() {
  local label="$1" expr="$2" oracle_covers="${3:-yes}"
  local mutant="$WORK/mutant.sh" rc=0
  if ! cp "$SCRIPT" "$mutant"; then
    printf '  FAIL %s — could not copy the script\n' "$label"
    FAIL=$((FAIL + 1))
    return 0
  fi
  if ! sed -i "$expr" "$mutant"; then
    printf '  FAIL %s — the mutation could not be applied\n' "$label"
    FAIL=$((FAIL + 1))
    return 0
  fi
  if cmp -s "$SCRIPT" "$mutant"; then
    printf '  FAIL %s — the mutation did not change the script (the anchor is stale)\n' "$label"
    FAIL=$((FAIL + 1))
    return 0
  fi
  if (run_suite "$mutant") >"$WORK/mutant.log" 2>&1; then
    rc=0
  else
    rc=$?
  fi
  if [[ $rc -eq 0 ]]; then
    printf '  FAIL %s — the suite stayed GREEN against the broken script (the harness is vacuous)\n' "$label"
    FAIL=$((FAIL + 1))
  else
    printf '  OK   %s — the suite goes red against the broken script\n' "$label"
    PASS=$((PASS + 1))
  fi
  # The oracle, against the same broken script.
  if [[ ! -f "$ORACLE" ]]; then
    printf '  FAIL %s — the end-state oracle is missing (%s)\n' "$label" "$ORACLE"
    FAIL=$((FAIL + 1))
    return 0
  fi
  if (WEBFANG_AUTO_LABEL_SCRIPT="$mutant" bash "$ORACLE") >"$WORK/oracle.log" 2>&1; then
    if [[ "$oracle_covers" == "yes" ]]; then
      printf '  FAIL %s — the ORACLE stayed GREEN against the broken script (it is not a witness)\n' "$label"
      FAIL=$((FAIL + 1))
    else
      printf '  NOTE %s — the ORACLE stayed GREEN: %s\n' "$label" "$ORACLE_BLIND_SPOT"
    fi
  else
    printf '  OK   %s — the oracle goes red against the broken script too\n' "$label"
    PASS=$((PASS + 1))
  fi
}

# ─── the optional live vocabulary check ───────────────────────────────────────
#
# OFF by default: the harness never touches the network. This mode compares
# the PIN with the repository's live label list and fails when they diverge,
# so a new `type:*` label cannot go unnoticed.
if [[ "${1:-}" == "--check-live-vocabulary" ]]; then
  if ! command -v gh >/dev/null 2>&1; then
    echo "::notice::gh no esta instalado: el vocabulario en vivo no se puede verificar."
    exit 0
  fi
  live="$(gh label list --limit 300 --json name --jq '.[].name' 2>/dev/null |
    grep -E "^${TYPE_LABEL_PREFIX}" | sort -u || true)"
  if [[ -z "$live" ]]; then
    echo "::notice::No se pudo leer el vocabulario en vivo (red o permisos): el pin offline se conserva."
    exit 0
  fi
  if [[ "$live" == "$REPO_TYPE_LABELS" ]]; then
    echo "OK: el pin coincide con el vocabulario en vivo."
    exit 0
  fi
  echo "::error::El pin del harness no coincide con el vocabulario en vivo."
  echo "::error::  pin:  $REPO_TYPE_LABELS"
  echo "::error::  live: $live"
  exit 1
fi

echo "auto_label_type.sh semantics harness"
echo ""

run_suite "$SCRIPT"

echo ""
echo "-- non-vacuity: the suite must go red against a broken script --"
# Green-then-red, for the ORACLE too: a witness that is red against the
# intact script proves nothing when it goes red against a mutant. The
# order matters — the oracle is checked against the intact script FIRST.
if [[ ! -f "$ORACLE" ]]; then
  printf '  FAIL the end-state oracle is missing (%s)\n' "$ORACLE"
  FAIL=$((FAIL + 1))
elif bash "$ORACLE" >/dev/null 2>&1; then
  printf '  OK   the end-state oracle is green against the intact script\n'
  PASS=$((PASS + 1))
else
  printf '  FAIL the end-state oracle is not green against the intact script (its red proves nothing)\n'
  FAIL=$((FAIL + 1))
fi
mutate_and_expect_red "mutation: the existing type:* label is ignored" \
  "s|^TYPE_LABEL_PREFIX=.*|TYPE_LABEL_PREFIX='zzz:'|"
mutate_and_expect_red "mutation: fix maps to the wrong label" \
  "s|\[fix\]=type:bug|[fix]=type:feature|"
mutate_and_expect_red "mutation: an unknown prefix is guessed" \
  "s|^NO_LABEL=.*|NO_LABEL='type:chore'|" "no"
mutate_and_expect_red "mutation: the stale type label is not removed" \
  "s|--method DELETE|--method PATCH|"

echo ""
if [[ "$FAIL" -eq 0 ]]; then
  echo "OK: $PASS auto-label cases passed"
  exit 0
fi
echo "FAILED: $FAIL auto-label cases failed"
exit 1
