#!/usr/bin/env bash
#
# Release provenance L1 gate — enforces TAG^{commit} == EXPECTED_SHA == COMMIT_VALIDATED == COMMIT_BUILT == COMMIT_PUBLISHED
#
# Subcommands: preflight, publish, ruleset
# All inputs arrive via environment variables so a test harness can drive them without GitHub.
#
# Exit 0 = proceed. Non-zero = block, and every block prints exactly one ::error:: line
# naming WHICH L1 clause failed and WHAT was observed.
#
# WHY THIS EXISTS
#   Today release.yml checks only that the tag *name* matches Cargo.toml (version
#   correctness). Nothing asserts WHICH COMMIT the tag points at, WHICH LINE that
#   commit belongs to, or that the published release is the validated commit.
#   Incidents that motivated this:
#     * v2.1.1 shipped a tag with no Release at all (#1484)
#     * #1478: tag-push suppression — GITHUB_TOKEN events don't trigger workflows
#     * #1484: silent dispatch failure — a dispatch that never happened produces no run
#   These scripts close that gap. Ancestry is SECONDARY defense, never the primary control.
set -euo pipefail

SUBCOMMAND="${1:-}"
[[ -n "$SUBCOMMAND" ]] || { echo "usage: $(basename "$0") {preflight|publish|ruleset}" >&2; exit 2; }

# Source the shared trust predicate for --tag mode.
# shellcheck source=scripts/release-tag-trust.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-tag-trust.sh"

# Resolve the repository root (for scripts that need it).
# PROV_REPO_ROOT allows a test harness to drive against a synthetic repo.
# Defaults to the derivation from BASH_SOURCE (current behaviour).
REPO_ROOT="${PROV_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

case "$SUBCOMMAND" in
  preflight)
    # ========================================================================
    # L1.1 Identity — resolve EXPECTED_SHA per trigger, then prove the tag points at it.
    # Three trigger shapes, NOT equivalent:
    #   1. push                    -> EXPECTED_SHA = $PROV_EVENT_SHA (platform-attested)
    #   2. workflow_dispatch (default branch) -> EXPECTED_SHA = $PROV_EXPECTED_SHA (required)
    #   3. workflow_dispatch (pinned to tag)  -> EXPECTED_SHA = $PROV_EVENT_SHA + assert tag ref
    # ========================================================================

    PROV_EVENT_NAME="${PROV_EVENT_NAME:-}"
    PROV_EVENT_SHA="${PROV_EVENT_SHA:-}"
    PROV_REF="${PROV_REF:-}"
    PROV_REF_TYPE="${PROV_REF_TYPE:-}"
    PROV_TAG="${PROV_TAG:-}"
    PROV_EXPECTED_SHA="${PROV_EXPECTED_SHA:-}"
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"

    [[ -n "$PROV_EVENT_NAME" ]] || { echo "::error::L1.1 Identity: PROV_EVENT_NAME is required" >&2; exit 1; }
    [[ -n "$PROV_TAG" ]] || { echo "::error::L1.1 Identity: PROV_TAG is required" >&2; exit 1; }
    [[ -n "$GITHUB_REPOSITORY" ]] || { echo "::error::L1.1 Identity: GITHUB_REPOSITORY is required" >&2; exit 1; }

    # ---- Channel gate FIRST: is this even a release tag name? ----
    # Deliberately before the server round-trip. A name like "v2.1" is not a release
    # tag at all, so spending an API call on it - and then reporting it under whichever
    # unrelated clause tripped first - tells the operator the wrong story. This is the
    # same acceptance rule release.yml applied inline before it moved here: stable
    # vX.Y.Z -> prerelease=false, RC vX.Y.Z-rc.N -> prerelease=true, anything else -> block.
    if [[ ! "$PROV_TAG" =~ $RELEASE_TAG_NAME_RE ]]; then
      echo "::error::L1.2 Version: tag '$PROV_TAG' is not a release tag name (expected vX.Y.Z or vX.Y.Z-rc.N)" >&2
      exit 1
    fi
    if [[ "$PROV_TAG" =~ -rc\.[0-9]+$ ]]; then
      PRERELEASE="true"
    else
      PRERELEASE="false"
    fi

    EXPECTED_SHA=""
    case "$PROV_EVENT_NAME" in
      push)
        [[ -n "$PROV_EVENT_SHA" ]] || { echo "::error::L1.1 Identity: push event requires PROV_EVENT_SHA" >&2; exit 1; }
        EXPECTED_SHA="$PROV_EVENT_SHA"
        ;;
      workflow_dispatch)
        if [[ "$PROV_REF" == "refs/tags/$PROV_TAG" ]]; then
          # Shape 3: dispatch pinned with --ref <tag>
          [[ -n "$PROV_EVENT_SHA" ]] || { echo "::error::L1.1 Identity: pinned tag dispatch requires PROV_EVENT_SHA" >&2; exit 1; }
          [[ "$PROV_REF_TYPE" == "tag" ]] || { echo "::error::L1.1 Identity: pinned dispatch requires PROV_REF_TYPE=tag, got '$PROV_REF_TYPE'" >&2; exit 1; }
          EXPECTED_SHA="$PROV_EVENT_SHA"
        else
          # Shape 2: dispatch from default branch (dispatch-release, release-reconcile, ensure-release.sh)
          # TRAP: github.sha here is the DEFAULT-BRANCH HEAD, NOT the tag's commit.
          # Never infer provenance from github.sha in this shape.
          [[ -n "$PROV_EXPECTED_SHA" ]] || { echo "::error::L1.1 Identity: default-branch dispatch requires PROV_EXPECTED_SHA input" >&2; exit 1; }
          EXPECTED_SHA="$PROV_EXPECTED_SHA"
        fi
        ;;
      *)
        echo "::error::L1.1 Identity: unsupported PROV_EVENT_NAME='$PROV_EVENT_NAME'" >&2
        exit 1
        ;;
    esac

    # Resolve the tag SERVER-SIDE, not from the local checkout.
    # This is the primary identity control — the local checkout may be stale or spoofed.
    tag_ref_response="$(gh api --method GET "repos/$GITHUB_REPOSITORY/git/ref/tags/$PROV_TAG" 2>/dev/null)" || {
      echo "::error::L1.1 Identity: failed to resolve tag '$PROV_TAG' via GitHub API (tag missing or no access)" >&2
      exit 1
    }

    # Extract object SHA and type from the ref response.
    tag_obj_sha="$(jq -r '.object.sha // empty' <<<"$tag_ref_response" 2>/dev/null || true)"
    tag_obj_type="$(jq -r '.object.type // empty' <<<"$tag_ref_response" 2>/dev/null || true)"
    [[ -n "$tag_obj_sha" && -n "$tag_obj_type" ]] || {
      echo "::error::L1.1 Identity: unparseable tag ref response for '$PROV_TAG'" >&2
      exit 1
    }

    # Dereference annotated tag to the commit it points at.
    resolved_commit=""
    case "$tag_obj_type" in
      commit)
        # Lightweight tag — ref points directly to commit.
        # REJECTED: every release tag MUST be annotated so the fingerprint has something to read.
        echo "::error::L1.1 Identity: tag '$PROV_TAG' is lightweight (ref object type=commit) — release tags must be annotated" >&2
        exit 1
        ;;
      tag)
        # Annotated tag — fetch the tag object to get the commit it points to.
        tag_obj_response="$(gh api "repos/$GITHUB_REPOSITORY/git/tags/$tag_obj_sha" 2>/dev/null)" || {
          echo "::error::L1.1 Identity: failed to dereference annotated tag '$PROV_TAG' (oid=$tag_obj_sha)" >&2
          exit 1
        }
        resolved_commit="$(jq -r '.object.sha // empty' <<<"$tag_obj_response" 2>/dev/null || true)"
        [[ -n "$resolved_commit" ]] || {
          echo "::error::L1.1 Identity: unparseable tag object response for '$PROV_TAG'" >&2
          exit 1
        }
        ;;
      *)
        echo "::error::L1.1 Identity: unexpected tag ref object type '$tag_obj_type' for '$PROV_TAG'" >&2
        exit 1
        ;;
    esac

    # Compare resolved commit to EXPECTED_SHA — this is the primary identity check.
    if [[ "$resolved_commit" != "$EXPECTED_SHA" ]]; then
      echo "::error::L1.1 Identity: tag '$PROV_TAG' resolves to $resolved_commit but EXPECTED_SHA=$EXPECTED_SHA" >&2
      exit 1
    fi

    # Apply trust predicate to the tag via release-plz-tags.sh --tag.
    # NOTE: this dereferences the LOCAL tag object, so it is only meaningful when
    # the local ref agrees with the server-side resolution. Assert that agreement.
    local_commit=""
    rc=0
    local_commit="$(bash "$REPO_ROOT/scripts/release-plz-tags.sh" --tag "$PROV_TAG" 2>/dev/null)" || rc=$?
    if [[ $rc -ne 0 ]]; then
      case $rc in
        3) echo "::error::L1.1 Identity: tag '$PROV_TAG' exists locally but is not trusted (not a release-plz tag)" >&2; exit 1 ;;
        4) echo "::error::L1.1 Identity: tag '$PROV_TAG' exists server-side but not locally — local checkout missing fetch-tags: true" >&2; exit 1 ;;
        *) echo "::error::L1.1 Identity: release-plz-tags.sh --tag '$PROV_TAG' failed with exit $rc" >&2; exit 1 ;;
      esac
    fi

    # Assert local == server-side resolution (fail closed on divergence).
    if [[ "$local_commit" != "$resolved_commit" ]]; then
      echo "::error::L1.1 Identity: local resolution ($local_commit) disagrees with server-side ($resolved_commit) for '$PROV_TAG' — fetch-depth: 0 + fetch-tags: true required" >&2
      exit 1
    fi

    # ========================================================================
    # L1.2 Version — root Cargo.toml version must equal tag version.
    # ========================================================================
    cargo_version="$(grep -m1 '^version' "$REPO_ROOT/Cargo.toml" | sed -E 's/version = "(.*)"/\1/')"
    tag_version="${PROV_TAG#v}"
    tag_version="${tag_version%-rc.*}"

    if [[ "$tag_version" != "$cargo_version" ]]; then
      echo "::error::L1.2 Version: tag '$PROV_TAG' version '$tag_version' != Cargo.toml '$cargo_version'" >&2
      exit 1
    fi

    # ========================================================================
    # L1.3 Lineage — derived, never supplied.
    #   -rc.N -> main; stable -> main.
    # Ancestry is SECONDARY: ancestor-of-main is necessary, not sufficient.
    # The identity check (L1.1) is what governs.
    # ========================================================================
    # Lineage is DERIVED, never supplied: a caller-selectable line is not a control.
    # Both channels live on main today (stable -> main, rc -> main); a support/* line
    # is out of scope until one exists. PRERELEASE was decided by the channel gate above.
    EXPECTED_LINE="main"

    # PROV_LINE_REF allows a test harness to drive against a synthetic repo with no 'origin'.
    # Defaults to origin/$EXPECTED_LINE (current behaviour).
    # Fail closed when the lineage ref is unresolvable.
    LINEAGE_REF="${PROV_LINE_REF:-origin/$EXPECTED_LINE}"
    if ! git merge-base --is-ancestor "$EXPECTED_SHA" "$LINEAGE_REF" 2>/dev/null; then
      echo "::error::L1.3 Lineage: EXPECTED_SHA $EXPECTED_SHA is not an ancestor of $LINEAGE_REF — fetch-depth: 0 + fetch-tags: true required" >&2
      exit 1
    fi

    # Outputs for downstream jobs (append to GITHUB_OUTPUT when present).
    if [[ -n "${GITHUB_OUTPUT:-}" && -f "$GITHUB_OUTPUT" ]]; then
      {
        echo "validated_commit=$EXPECTED_SHA"
        echo "expected_line=$EXPECTED_LINE"
        echo "tag=$PROV_TAG"
        echo "prerelease=$PRERELEASE"
      } >> "$GITHUB_OUTPUT"
    fi

    # Also print for visibility.
    echo "L1 preflight passed: tag=$PROV_TAG validated_commit=$EXPECTED_SHA expected_line=$EXPECTED_LINE prerelease=$PRERELEASE"
    ;;

  publish)
    # ========================================================================
    # L1.4 TOCTOU re-read — re-run server-side tag resolution and compare
    # against PROV_VALIDATED_COMMIT; any drift blocks.
    # Runs at the publish boundary because gh release create addresses the
    # release BY NAME, so the ref can move between validation and publication.
    # ========================================================================
    PROV_VALIDATED_COMMIT="${PROV_VALIDATED_COMMIT:-}"
    PROV_TAG="${PROV_TAG:-}"
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"

    [[ -n "$PROV_VALIDATED_COMMIT" ]] || { echo "::error::L1.4 TOCTOU: PROV_VALIDATED_COMMIT is required" >&2; exit 1; }
    [[ -n "$PROV_TAG" ]] || { echo "::error::L1.4 TOCTOU: PROV_TAG is required" >&2; exit 1; }
    [[ -n "$GITHUB_REPOSITORY" ]] || { echo "::error::L1.4 TOCTOU: GITHUB_REPOSITORY is required" >&2; exit 1; }

    # Re-resolve server-side (same logic as preflight).
    tag_ref_response="$(gh api --method GET "repos/$GITHUB_REPOSITORY/git/ref/tags/$PROV_TAG" 2>/dev/null)" || {
      echo "::error::L1.4 TOCTOU: failed to resolve tag '$PROV_TAG' via GitHub API" >&2
      exit 1
    }

    tag_obj_sha="$(jq -r '.object.sha // empty' <<<"$tag_ref_response" 2>/dev/null || true)"
    tag_obj_type="$(jq -r '.object.type // empty' <<<"$tag_ref_response" 2>/dev/null || true)"
    [[ -n "$tag_obj_sha" && -n "$tag_obj_type" ]] || {
      echo "::error::L1.4 TOCTOU: unparseable tag ref response for '$PROV_TAG'" >&2
      exit 1
    }

    current_commit=""
    case "$tag_obj_type" in
      commit)
        echo "::error::L1.4 TOCTOU: tag '$PROV_TAG' became lightweight (was annotated at preflight)" >&2
        exit 1
        ;;
      tag)
        tag_obj_response="$(gh api "repos/$GITHUB_REPOSITORY/git/tags/$tag_obj_sha" 2>/dev/null)" || {
          echo "::error::L1.4 TOCTOU: failed to dereference annotated tag '$PROV_TAG'" >&2
          exit 1
        }
        current_commit="$(jq -r '.object.sha // empty' <<<"$tag_obj_response" 2>/dev/null || true)"
        [[ -n "$current_commit" ]] || {
          echo "::error::L1.4 TOCTOU: unparseable tag object response for '$PROV_TAG'" >&2
          exit 1
        }
        ;;
      *)
        echo "::error::L1.4 TOCTOU: unexpected tag ref object type '$tag_obj_type' for '$PROV_TAG'" >&2
        exit 1
        ;;
    esac

    if [[ "$current_commit" != "$PROV_VALIDATED_COMMIT" ]]; then
      echo "::error::L1.4 TOCTOU: tag '$PROV_TAG' drifted from $PROV_VALIDATED_COMMIT to $current_commit between preflight and publish" >&2
      exit 1
    fi

    echo "L1.4 TOCTOU passed: tag=$PROV_TAG still points to $PROV_VALIDATED_COMMIT"
    ;;

  ruleset)
    # Delegate to apply-tag-ruleset.sh check.
    # Capture the real exit code to distinguish 1 (not protected) from 2 (cannot tell).
    #
    # GITHUB_REPOSITORY is required to even ask the API. Without it we cannot tell
    # whether the tags are protected — that is a "cannot tell" (exit 2), NOT
    # "not protected" (exit 1), matching the preflight/publish fail-closed style.
    GITHUB_REPOSITORY="${GITHUB_REPOSITORY:-}"
    if [[ -z "$GITHUB_REPOSITORY" ]]; then
      echo "::error::L1.5 Ruleset: GITHUB_REPOSITORY is required - cannot tell whether the tags are protected" >&2
      exit 2
    fi
    rc=0
    bash "$REPO_ROOT/scripts/apply-tag-ruleset.sh" check || rc=$?
    case $rc in
      0)
        echo "L1.5 Ruleset: tag protection ruleset is active and correct"
        ;;
      1)
        # apply-tag-ruleset.sh already printed the ::error:: with details.
        echo "::error::L1.5 Ruleset: tags are not protected by the required ruleset" >&2
        exit 1
        ;;
      2)
        echo "::error::L1.5 Ruleset: protection status could not be determined (API failure or unparseable response)" >&2
        exit 2
        ;;
      *)
        echo "::error::L1.5 Ruleset: apply-tag-ruleset.sh check failed with unexpected exit $rc" >&2
        exit 2
        ;;
    esac
    ;;

  *)
    echo "::error::unknown subcommand '$SUBCOMMAND'" >&2
    exit 2
    ;;
esac