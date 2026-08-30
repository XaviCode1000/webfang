#!/usr/bin/env bash
# Gentle AI review delivery gate (ADR: #1036, wired 2026-08-30).
#
# Consults `gentle-ai review validate --gate <gate>` before allowing a commit
# or push. Decision matrix (measured behavior, see docs/test-inventory.md and
# issue #1047):
#
#   - `gentle-ai` binary absent  -> SKIP with a warning (fail-open): a machine
#     without the tool cannot gate anything, and blocking would brick the repo.
#   - validate exits non-zero or emits unparseable output -> BLOCK (fail-closed).
#   - `delivery == "unmanaged"`  -> ALLOW: no review authority governs this
#     candidate; delivery follows ordinary repository policy. This is the normal
#     path for every commit whose candidate never started a review.
#   - otherwise                  -> require `allowed == true`. Any discovered,
#     non-allow authority (reviewing, invalidated, approved-but-not-acked
#     acknowledged states) BLOCKS.
#
# Gate context: `gentle-ai review start` cannot run from a plain shell
# (`immutable_review_transport_unsupported` — the relay contract is host-only),
# so this hook only *consults* an existing verdict; it never produces one.
# Reviews are initiated through the Pi review tools, which persist the
# authority the validate call reads here.

set -euo pipefail

GATE="${1:?usage: review-gate.sh <pre-commit|pre-push>}"
REPO_ROOT="$(git rev-parse --show-toplevel)"

if ! command -v gentle-ai >/dev/null 2>&1; then
    echo "[review-gate] SKIP: 'gentle-ai' not on PATH — delivery gate cannot consult review authority." >&2
    exit 0
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "[review-gate] NOTE: 'jq' not on PATH — using the built-in scalar fallback parser." >&2
fi

# Bound the consult: a hung validate must not halt delivery indefinitely (#1048).
TIMEOUT_CMD=()
command -v timeout >/dev/null 2>&1 && TIMEOUT_CMD=(timeout 20s)

OUT="$(${TIMEOUT_CMD[@]+"${TIMEOUT_CMD[@]}"} gentle-ai review validate --gate "$GATE" --cwd "$REPO_ROOT" 2>/dev/null)" || {
    echo "[review-gate] BLOCK: 'gentle-ai review validate' failed (exit $?) — cannot verify delivery gate." >&2
    exit 1
}

# Extract a flat scalar field from the verdict: jq when available, sed
# otherwise, so a missing jq cannot disable delivery (#1048 R4-001).
gate_field() {
    local key="$1"
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$OUT" | jq -r --arg k "$key" '.[$k] // empty' 2>/dev/null
    else
        printf '%s' "$OUT" \
            | sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\{0,1\}\([^\",}]*\)\"\{0,1\}.*/\1/p" | head -1
    fi
}

DELIVERY="$(gate_field delivery)"
ALLOWED="$(gate_field allowed)"

if [[ -z "$DELIVERY" && -z "$ALLOWED" ]]; then
    echo "[review-gate] BLOCK: unparseable gate verdict — refusing to proceed without an authoritative answer." >&2
    exit 1
fi

if [[ "$DELIVERY" == "unmanaged" ]]; then
    echo "[review-gate] OK: no review authority governs this candidate; ordinary repository policy applies."
    exit 0
fi

if [[ "$ALLOWED" == "true" ]]; then
    echo "[review-gate] OK: review authority allows this delivery."
    exit 0
fi

echo "[review-gate] BLOCK: review authority governs this candidate but does not allow delivery (allowed=${ALLOWED:-<none>}, delivery=${DELIVERY:-<none>})." >&2
echo "[review-gate] Complete or acknowledge the review before committing, or run 'gentle-ai review validate --gate $GATE --cwd \"$REPO_ROOT\"' to inspect the verdict." >&2
exit 1
