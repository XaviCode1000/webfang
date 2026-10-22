#!/usr/bin/env bash
# auto_label_type.sh — apply the `type:*` label that the PR title implies (#2126).
#
# WHY THIS EXISTS
#   The gate "Require exactly one type:* label" (.github/workflows/
#   pr-validation.yml) starts RED on a PR opened without a label, and GitHub
#   cannot apply a label from a PR template: the issues template can, the
#   pull-request template cannot. The rule was already written (AGENTS.md,
#   "The 5 PR gates"); what was missing was something that APPLIES it. This
#   script is that something: the `auto-label-type` job of the same workflow
#   runs it BEFORE the validation job.
#
# THE MAP LIVES HERE, IN ONE PLACE
#   `bash scripts/auto_label_type.sh --map` prints it. AGENTS.md POINTS at
#   this script instead of duplicating the table, and the harness checks
#   both facts: the pointer is there, and no map is duplicated in AGENTS.md.
#
# THE RULES (each one is pinned by scripts/test_auto_label_type.sh)
#   1. The TITLE is the source — not the branch, not the commits.
#   2. The PR ends with EXACTLY ONE `type:*` label: the one the CURRENT
#      title derives. A stale `type:*` label — the title changed its type —
#      is REMOVED, and the derived one is added. The derived label is added
#      FIRST and the stale ones removed after, so the PR carries at least
#      one `type:*` label at every moment of the run: the end state is
#      exactly one, never two and never zero.
#   3. A title that derives NOTHING (no recognizable conventional-commit
#      prefix) changes NOTHING: the label that is already there stays, and
#      a PR with no label gets none. Guessing is worse than not acting, and
#      the validation stays red until a human labels the PR.
#   4. The vocabulary is the repository's, not an invention: the seven labels
#      in the table below are the ones that exist in the repository. This
#      script NEVER creates a label — it only calls the additive "add labels
#      to an issue" endpoint, which answers 422 for a label that does not
#      exist.
#
# WHEN IT RUNS
#   The job reacts to exactly two events: `opened` and `edited`. A bot that
#   also reacted to `labeled` would fight a human over the label forever
#   (the human sets one, the `labeled` event fires, the bot replaces it,
#   the human sets it again, ...). `edited` is the retitle: the one event
#   where the label derived from the title can legitimately change.
#
# THE BREAKING MARKER
#   `feat!: ...`, `feat(scope)!: ...` (and the malformed `feat!(scope): ...`
#   that #2072 used) declare a breaking change. The shape is the one
#   scripts/merge-when-green.sh already uses (#2081), so the label and the
#   merge guard agree on what "breaking" means instead of keeping two
#   definitions in the repository.
#
# USAGE
#   PR_NUMBER=<n> REPO=<owner/repo> GH_TOKEN=<t> bash scripts/auto_label_type.sh
#       read the PR, derive the label from its title, and make the PR carry
#       exactly that one `type:*` label: added when it is missing, and
#       replacing a stale one when the title changed its type.
#   bash scripts/auto_label_type.sh --map
#       print the mapping table (one `key<TAB>label` per line).
#   bash scripts/auto_label_type.sh --help
#
# EXIT CODES
#   0  the PR is in the state this script wants (exactly one `type:*` label,
#      or deliberately left untouched because the title says nothing)
#   1  the PR could not be read, or the label could not be applied or removed
#
# Semantics harness: scripts/test_auto_label_type.sh
set -euo pipefail

# The prefix every type label carries. The PR gate counts the labels that
# start with it, so this string is what decides "does this PR already carry
# one?" — and it is the ONLY place the prefix is written, so the harness and
# this script cannot disagree about it.
TYPE_LABEL_PREFIX='type:'

# What a title the vocabulary does not know gets: NOTHING. Guessing would put
# a `type:*` label on a PR that nobody chose, and the gate demands exactly one.
NO_LABEL=''

# The mapping table: conventional-commit type -> the `type:*` label.
#
# The labels below are the ones that EXIST in the repository (the seven
# `type:*` labels); the script never invents one and never creates one.
#
# The commit types with no label of their own (perf, revert, build, ci,
# style) map to `type:chore`: the closest label that exists. A new label
# would need the maintainer, not this script.
#
# `breaking` is not a title prefix: it is the BREAKING MARKER (`feat!: ...`).
# It sits in the same table because the label it maps to is a real one.
declare -A TYPE_LABEL=(
  [breaking]=type:breaking-change
  [build]=type:chore
  [chore]=type:chore
  [ci]=type:chore
  [docs]=type:docs
  [feat]=type:feature
  [fix]=type:bug
  [perf]=type:chore
  [refactor]=type:refactor
  [revert]=type:chore
  [style]=type:chore
  [test]=type:testing
)

# The conventional-commit prefix of a title: `type:` or `type(scope):`.
# The TYPE is group 1 (the scope is group 2), so the derivation reads the
# type and never the scope.
TYPE_PREFIX_RE='^([A-Za-z]+)(\([^)]*\))?:'

# The breaking marker: `type!:`, `type(scope)!:` and the malformed
# `type!(scope):` (#2072). Same shape scripts/merge-when-green.sh uses.
# The TYPE is group 1 here too.
BREAKING_PREFIX_RE='^([A-Za-z]+)(\([^)]*\))?!(\([^)]*\))?:'

usage() {
  cat <<'EOF'
auto_label_type.sh — apply the `type:*` label that the PR title implies (#2126)

Usage:
  PR_NUMBER=<n> REPO=<owner/repo> GH_TOKEN=<t> bash scripts/auto_label_type.sh
      Read the PR, derive the label from its title, and make the PR carry
      exactly that one `type:*` label: added when it is missing, replacing a
      stale one when the title changed its type, and untouched when the title
      derives nothing.

  bash scripts/auto_label_type.sh --map
      Print the mapping table (one `key<TAB>label` per line).

  bash scripts/auto_label_type.sh --help
      This text.
EOF
}

# print_map — the mapping table, one `key<TAB>label` per line, sorted.
print_map() {
  local key
  while IFS= read -r key; do
    printf '%s\t%s\n' "$key" "${TYPE_LABEL[$key]}"
  done < <(printf '%s\n' "${!TYPE_LABEL[@]}" | sort)
}

# derive_label <title> — print the label the title implies, or print nothing
# when the title implies none. It NEVER prints a label that is not in the
# table, and it never guesses.
derive_label() {
  local title="$1"
  local type="" label=""

  if [[ "$title" =~ $BREAKING_PREFIX_RE ]]; then
    type="$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
    # The marker only counts for a type the vocabulary knows: an unknown type
    # means the title is not a conventional commit at all, and rule 3 says
    # not to guess.
    if [[ -n "${TYPE_LABEL[$type]+known}" ]]; then
      printf '%s\n' "${TYPE_LABEL[breaking]}"
    fi
    return 0
  fi

  if [[ "$title" =~ $TYPE_PREFIX_RE ]]; then
    type="$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')"
    label="${TYPE_LABEL[$type]:-$NO_LABEL}"
    if [[ -n "$label" ]]; then
      printf '%s\n' "$label"
    fi
  fi
  return 0
}

# add_label <label> — ADD the label to the PR. The endpoint is additive: it
# never replaces the label set (the human's other labels stay) and it never
# creates a label (an unknown name is answered with 422).
add_label() {
  local label="$1"
  local payload
  payload="$(printf '{"labels":["%s"]}' "$label")"
  if ! gh api --method POST "repos/$REPO/issues/$PR_NUMBER/labels" --input - >/dev/null <<<"$payload"; then
    echo "::error::No se pudo aplicar el label '$label' al PR #$PR_NUMBER." >&2
    exit 1
  fi
}

# remove_label <label> — REMOVE one label from the PR. Only the stale
# `type:*` label is ever removed, and only when the derived one is already
# on the PR: the end state must carry exactly one.
remove_label() {
  local label="$1"
  # The label name is a PATH SEGMENT: the colon is percent-encoded, the way
  # a REST client encodes a name that goes into a URL. The server decodes it
  # back before matching the label.
  local encoded="${label//:/%3A}"
  if ! gh api --method DELETE "repos/$REPO/issues/$PR_NUMBER/labels/$encoded" >/dev/null; then
    echo "::error::No se pudo quitar el label '$label' del PR #$PR_NUMBER." >&2
    exit 1
  fi
}

main() {
  : "${PR_NUMBER:?PR_NUMBER is required}"
  : "${REPO:?REPO is required (owner/name)}"
  : "${GH_TOKEN:?GH_TOKEN is required}"

  if ! command -v gh >/dev/null 2>&1; then
    echo "::error::gh CLI is not installed or not on PATH." >&2
    exit 1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    echo "::error::jq is not installed or not on PATH: la respuesta de la API no se puede parsear." >&2
    exit 1
  fi

  local pr_json="" title="" derived=""
  # gh's own error goes to the log, untouched: this script's message says
  # WHAT failed, gh's says WHY.
  if ! pr_json="$(gh pr view "$PR_NUMBER" --repo "$REPO" --json title,labels)"; then
    echo "::error::No se pudo leer el PR #$PR_NUMBER en $REPO." >&2
    exit 1
  fi

  title="$(printf '%s' "$pr_json" | jq -r '.title // ""')"
  derived="$(derive_label "$title")"

  # The `type:*` labels the PR carries right now, in ONE jq call: a
  # `printf | grep | head` pipeline would depend on who closes the pipe
  # first, and `grep -q` closing it early is a SIGPIPE race under
  # `pipefail` — a flaky answer to a question that has an exact one.
  local -a current_type_labels=()
  local current_label=""
  while IFS= read -r current_label; do
    [[ -n "$current_label" ]] || continue
    current_type_labels+=("$current_label")
  done < <(printf '%s' "$pr_json" |
    jq -r --arg p "$TYPE_LABEL_PREFIX" '(.labels // [])[]? | .name // empty | select(startswith($p))')

  if [[ -z "$derived" ]]; then
    # Rule 3: the title says nothing. The label that is already there stays
    # (removing it would leave the PR with NO `type:*` label, and the gate
    # demands exactly one), and a PR with no label gets none.
    if [[ ${#current_type_labels[@]} -gt 0 ]]; then
      echo "OK: el título del PR #$PR_NUMBER ('$title') no declara un tipo: el label existente se deja como está."
      return 0
    fi
    # The warning is the visible signal; the validation stays red until a
    # human labels the PR.
    echo "::warning::El título del PR #$PR_NUMBER ('$title') no empieza con un prefijo convencional reconocible: no se aplica ningún label 'type:*'."
    echo "::warning::La validación 'Validate PR metadata' va a fallar por falta de label: etiquetá el PR a mano."
    return 0
  fi

  # The derived label is the one the PR must carry. The stale ones — every
  # `type:*` label that is not it — are the ones to remove.
  local -a stale_labels=()
  local derived_present=false
  for current_label in ${current_type_labels[@]+"${current_type_labels[@]}"}; do
    if [[ "$current_label" == "$derived" ]]; then
      derived_present=true
    else
      stale_labels+=("$current_label")
    fi
  done

  if [[ "$derived_present" == true && ${#stale_labels[@]} -eq 0 ]]; then
    # Rule 2, idempotence: the PR already carries exactly the derived label.
    # NOTHING happens — not even a redundant write.
    echo "OK: el PR #$PR_NUMBER ya tiene el label '$derived': no se toca nada."
    return 0
  fi

  # Add FIRST, remove after: the PR carries at least one `type:*` label at
  # every moment of the run, and the end state is exactly one.
  if [[ "$derived_present" == false ]]; then
    add_label "$derived"
  fi
  for current_label in ${stale_labels[@]+"${stale_labels[@]}"}; do
    remove_label "$current_label"
  done

  echo "OK: el PR #$PR_NUMBER lleva el label '$derived' (derivado del título '$title')."
  return 0
}

case "${1:-}" in
  --map)
    print_map
    exit 0
    ;;
  --help | -h)
    usage
    exit 0
    ;;
  "")
    main
    exit $?
    ;;
  *)
    echo "::error::argumento desconocido: '$1' (ver --help)" >&2
    exit 2
    ;;
esac
