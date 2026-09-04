#!/usr/bin/env bash
# Main-push gate — `refs/heads/main` advances only via PR merges (#1091).
#
# Wired as the second pre-push gate (scripts/githooks/pre-push execs this after
# review-gate.sh) and reads git's stdin ref-update lines, one per update:
#   <local_ref> <local_sha> <remote_ref> <remote_sha>
#
# Decision matrix:
#   remote_ref != refs/heads/main                    -> SKIP silently (only main is gated)
#   remote_sha all-zeros (refs/heads/main created)   -> SKIP + warning
#   merge commit (>= 2 parents) in the pushed range  -> gh api repos/{owner}/{repo}/commits/
#                                                       <sha>/pulls must list >= 1 PR, else BLOCK
#   plain commit in the pushed range                 -> subject must end in `(#N)` AND
#                                                       `gh pr view N --json state` must be
#                                                       MERGED, else BLOCK
#   gh absent, gh call fails (network/auth), or rev-list
#   cannot enumerate the pushed commits              -> SKIP + warning (FAIL-OPEN)
#
# BLOCK prints one line and exits 1:
#   main-push-gate: BLOCKED <sha> <reason> — main advances only via PR merges (bypass: --no-verify)
#
# The --no-verify hatch is deliberate: this hook is defense-in-depth, not the
# primary control — branch protection governs the server side, and the reflog
# shows 0 local pushes to main in 172 updates. Fail-open matches review-gate.sh
# philosophy: a machine without gh or network must still be able to deliver;
# this gate never becomes the reason a push is impossible.

set -u

# Bound every gh call: a hung GitHub API must not stall the push (#1048 precedent).
TIMEOUT_CMD=()
command -v timeout >/dev/null 2>&1 && TIMEOUT_CMD=(timeout 10s)

block() {
    echo "main-push-gate: BLOCKED ${1} ${2} — main advances only via PR merges (bypass: --no-verify)" >&2
    exit 1
}

fail_open() {
    echo "[main-push-gate] SKIP: ${1} — fail-open." >&2
    exit 0
}

if ! command -v gh >/dev/null 2>&1; then
    fail_open "'gh' not on PATH — cannot verify PR provenance"
fi

gate_one_update() {
    local local_sha="$1" remote_sha="$2"

    # Enumerate the commits this push would add to main.
    local commits
    if ! commits="$(git rev-list "${remote_sha}..${local_sha}")"; then
        fail_open "cannot enumerate commits (${remote_sha}..${local_sha})"
    fi

    local sha parents nparents subject n pr_count state
    while read -r sha; do
        [[ -n "${sha}" ]] || continue
        parents="$(git rev-list --parents -n 1 "${sha}")" || fail_open "cannot inspect parents of ${sha}"
        nparents=$(( $(wc -w <<< "${parents}") - 1 ))
        if [[ ${nparents} -ge 2 ]]; then
            # Merge commit: acceptable only if GitHub associates it with a PR.
            pr_count="$(${TIMEOUT_CMD[@]+"${TIMEOUT_CMD[@]}"} gh api "repos/${REPO}/commits/${sha}/pulls" --jq 'length' </dev/null 2>/dev/null)" \
                || fail_open "gh api failed for ${sha} (network/auth?)"
            [[ "${pr_count}" =~ ^[0-9]+$ ]] || fail_open "gh api returned unparseable output for ${sha}"
            [[ "${pr_count}" -ge 1 ]] || block "${sha}" "merge commit with no associated PR (gh api lists ${pr_count})"
        else
            # Plain commit: must reference a PR in the subject, and it must be MERGED.
            subject="$(git log -1 --format=%s "${sha}")"
            if [[ ! "${subject}" =~ \(#[0-9]+\)$ ]]; then
                block "${sha}" "subject does not end in a PR reference '(#N)'"
            fi
            n="${subject%\)}"
            n="${n##*#}"
            state="$(${TIMEOUT_CMD[@]+"${TIMEOUT_CMD[@]}"} gh pr view "${n}" --json state --jq .state </dev/null 2>/dev/null)" \
                || fail_open "gh pr view #${n} failed (network/auth?)"
            [[ -n "${state}" ]] || fail_open "gh pr view #${n} returned empty output"
            [[ "${state}" == "MERGED" ]] || block "${sha}" "PR #${n} is not MERGED (state=${state})"
        fi
    done <<< "${commits}"
}

REPO=""
main_seen=0
while read -r _ local_sha remote_ref remote_sha; do
    [[ -n "${remote_ref:-}" && -n "${local_sha:-}" && -n "${remote_sha:-}" ]] || continue
    [[ "${remote_ref}" == "refs/heads/main" ]] || continue   # gate only guards main
    if [[ "${remote_sha}" =~ ^0+$ ]]; then
        echo "[main-push-gate] SKIP: refs/heads/main is being created (all-zero remote sha) — nothing to gate." >&2
        continue
    fi
    main_seen=1
    if [[ -z "${REPO}" ]]; then
        REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner </dev/null 2>/dev/null)" \
            || fail_open "could not resolve owner/repo via gh"
    fi
    gate_one_update "${local_sha}" "${remote_sha}"
done

if [[ ${main_seen} -eq 1 ]]; then
    echo "[main-push-gate] OK: refs/heads/main update verified as PR merges." >&2
fi
exit 0
