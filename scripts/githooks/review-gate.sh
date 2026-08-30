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
    echo "[review-gate] BLOCK: 'jq' not on PATH — cannot parse gate verdict safely. Install jq or remove scripts/githooks from core.hooksPath." >&2
    exit 1
fi

OUT="$(gentle-ai review validate --gate "$GATE" --cwd "$REPO_ROOT" 2>/dev/null)" || {
    echo "[review-gate] BLOCK: 'gentle-ai review validate' failed (exit $?) — cannot verify delivery gate." >&2
    exit 1
}

DELIVERY="$(printf '%s' "$OUT" | jq -r '.delivery // empty' 2>/dev/null)" || DELIVERY=""
ALLOWED="$(printf '%s' "$OUT" | jq -r '.allowed // empty' 2>/dev/null)" || ALLOWED=""

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
