#!/usr/bin/env bash
#
# ci_mutation_scope.sh — Phase 4 mutation scope advisor (risk engine input).
#
# Recommends WHICH area paths to mutate from the change scope. Advisory only:
# it never gates, never fails (exit 0 always), runs no cargo, touches no
# network — it only shells out to scripts/ci_path_classifier.sh.
#
# Usage:
#   scripts/ci_mutation_scope.sh [--base-ref <ref>] [--head-ref <ref>]
#                                [--base <ref>] [--head <ref>]
#                                [--files <newline-separated-list>]
#   CI_MUTATION_SCOPE_FILES=<list> scripts/ci_mutation_scope.sh
#
# Output (stdout, human-readable + greppable `key=value` first lines):
#   mutation-scope: needs_mutation_hotpath=<true|false>
#   hotpath-areas: <csv of core,crawler,downloader,ai | none>
#   then either `scope-paths:` (one path glob per line) or
#   `scope: none` + `reason: ...`.
#
# Consistency with the mutants workflow (.github/workflows/mutants.yml):
#   the `mutants-pr` job greps the PR diff for exactly these 6 hot paths
#   (mirrored in .cargo/mutants.toml `examine_globs`):
#     crates/webfang_core/src/application/crawler/
#     crates/webfang_core/src/application/pipeline/
#     crates/webfang_core/src/extractor/
#     crates/webfang_core/src/infrastructure/bridge.rs
#     crates/webfang_core/src/infrastructure/http/waf_engine.rs
#     crates/webfang_core/src/infrastructure/network/
#   This helper recommends that SAME set (as `/**` globs), partitioned by
#   classifier area so the scope stays proportional to the change:
#     crawler area    -> application/crawler/** + application/pipeline/**
#                        + extractor/**
#     downloader area -> infrastructure/network/** + waf_engine.rs + bridge.rs
#     core area       -> the FULL 6-path set (conservative: see delta 1)
#     ai area         -> the semantic-cleaning surface inside webfang_core
#                        (advisory; see delta 3)
#
# Documented deltas (classifier is filename-broad, mutants gate is exact):
#   1. `core_changed` covers ALL of crates/webfang_core/**, but the mutants
#      gate only watches the 6 paths above. A pure-domain change (e.g.
#      value_objects.rs) sets needs_mutation_hotpath=true here while the
#      mutants-pr grep reports hotpath-changed=false. Conservative direction:
#      this helper recommends the full gated set so the risk engine reviews
#      the hot paths, never less.
#   2. The classifier `downloader` predicate is broader than the gate: it
#      also matches ssrf/guard_chain/cookie_bridge/hybrid_router/spa_detector/
#      resource_governor substrings anywhere in the tree. Files matching only
#      those (outside the 3 gated downloader paths) still recommend the 3
#      gated paths — guard order is load-bearing, so any guard-adjacent
#      change deserves the gated scope.
#   3. The classifier `ai` predicate (model markers: embedding, onnx, granite,
#      clean_ai, ai_integration) has NO counterpart in the current mutants
#      workflow, which runs `cargo mutants -p webfang_core` over the 6 gated
#      hot paths only. Since ADR-0004 paso 6 the `webfang_ai` crate is gone
#      and the semantic-cleaning surface lives inside webfang_core
#      (EmbeddingPort, the remote embedding adapter, the cleaner), so an
#      ai-only change is reported with those scope-paths flagged as
#      OUT-OF-CURRENT-SCOPE (future lane, not today's mutants-pr gate).
#      Blocking semantics of mutants.yml are unchanged by this phase.
#
# Exit code: always 0 (advisory). Classifier failure degrades to
# needs_mutation_hotpath=true (fail closed to the full gated set).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLASSIFIER="$SCRIPT_DIR/ci_path_classifier.sh"

BASE_REF="origin/main"
HEAD_REF="HEAD"
FILES_OVERRIDE="${CI_MUTATION_SCOPE_FILES:-}"
FILES_GIVEN=false
if [[ -n "$FILES_OVERRIDE" ]]; then
  FILES_GIVEN=true
fi

usage() {
  sed -n '2,20p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-ref | --base) BASE_REF="${2:?missing value for $1}"; shift 2 ;;
    --head-ref | --head) HEAD_REF="${2:?missing value for $1}"; shift 2 ;;
    --files) FILES_OVERRIDE="$2"; FILES_GIVEN=true; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 0 ;;
  esac
done

# --- classify (fail closed to hotpath=true on any error) -----------------------
CLASS_TMP="$(mktemp)"
trap 'rm -f "$CLASS_TMP"' EXIT

CLASS_OK=true
if $FILES_GIVEN; then
  if ! bash "$CLASSIFIER" --files "$FILES_OVERRIDE" --github-output "$CLASS_TMP" 2>/dev/null; then
    CLASS_OK=false
  fi
else
  if ! bash "$CLASSIFIER" --base-ref "$BASE_REF" --head-ref "$HEAD_REF" --github-output "$CLASS_TMP" 2>/dev/null; then
    CLASS_OK=false
  fi
fi

core_changed=false
crawler_changed=false
downloader_changed=false
ai_changed=false
# Pre-initialised so shellcheck (SC2154) sees a real assignment: the loop
# below only writes it via `printf -v "$key"` (github key `all`), which
# static analysis cannot follow. `all_flag` is the local alias used after.
all=false
all_flag=false
needs_mutation_hotpath=false
if $CLASS_OK; then
  for key in core_changed crawler_changed downloader_changed ai_changed all needs_mutation_hotpath; do
    val="$(grep -E "^${key}=" "$CLASS_TMP" 2>/dev/null | tail -1 | cut -d= -f2)"
    printf -v "$key" '%s' "${val:-false}"
  done
  # `all` arrives under the github key `all`, not `all_flag`.
  all_flag="$all"
else
  core_changed=true
  crawler_changed=true
  downloader_changed=true
  ai_changed=true
  all_flag=true
  needs_mutation_hotpath=true
fi

# --- recommend -----------------------------------------------------------------
echo "mutation-scope: needs_mutation_hotpath=$needs_mutation_hotpath"

if [[ "$needs_mutation_hotpath" != "true" ]]; then
  echo "hotpath-areas: none"
  echo "scope: none"
  echo "reason: no hot-path areas changed (core/crawler/downloader/ai all false, all=false); mutation testing not recommended for this change."
  exit 0
fi

areas=""
[[ "$core_changed" == "true" ]] && areas="${areas:+$areas,}core"
[[ "$crawler_changed" == "true" ]] && areas="${areas:+$areas,}crawler"
[[ "$downloader_changed" == "true" ]] && areas="${areas:+$areas,}downloader"
[[ "$ai_changed" == "true" ]] && areas="${areas:+$areas,}ai"
[[ -z "$areas" ]] && areas="all (unknown scope — full gated set)"
echo "hotpath-areas: $areas"
echo "scope-paths:"

# Core is broader than the gate (delta 1): recommend the full gated set.
if [[ "$core_changed" == "true" || "$all_flag" == "true" ]]; then
  echo "  crates/webfang_core/src/application/crawler/**"
  echo "  crates/webfang_core/src/application/pipeline/**"
  echo "  crates/webfang_core/src/extractor/**"
  echo "  crates/webfang_core/src/infrastructure/network/**"
  echo "  crates/webfang_core/src/infrastructure/http/waf_engine.rs"
  echo "  crates/webfang_core/src/infrastructure/bridge.rs"
else
  if [[ "$crawler_changed" == "true" ]]; then
    echo "  crates/webfang_core/src/application/crawler/**"
    echo "  crates/webfang_core/src/application/pipeline/**"
    echo "  crates/webfang_core/src/extractor/**"
  fi
  if [[ "$downloader_changed" == "true" ]]; then
    echo "  crates/webfang_core/src/infrastructure/network/**"
    echo "  crates/webfang_core/src/infrastructure/http/waf_engine.rs"
    echo "  crates/webfang_core/src/infrastructure/bridge.rs"
  fi
fi
if [[ "$ai_changed" == "true" ]]; then
  echo "  crates/webfang_core/src/domain/embedding_port.rs (OUT-OF-CURRENT-SCOPE: mutants workflow runs -p webfang_core over the 6 gated hot paths only; future lane)"
  echo "  crates/webfang_core/src/infrastructure/llm/** (OUT-OF-CURRENT-SCOPE: mutants workflow runs -p webfang_core over the 6 gated hot paths only; future lane)"
  echo "  crates/webfang_core/src/infrastructure/semantic_cleaner.rs (OUT-OF-CURRENT-SCOPE: mutants workflow runs -p webfang_core over the 6 gated hot paths only; future lane)"
fi
echo "recommendation: mutate the paths above (gated subset of .cargo/mutants.toml examine_globs); mutants-pr blocking semantics unchanged."

exit 0
