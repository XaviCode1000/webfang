#!/usr/bin/env bash
# check_dependency_direction.sh
#
# CI gate for the inter-crate dependency policy (issue #513).
# Enforces the direction documented in AGENTS.md ("Inter-crate dependency
# direction (ENFORCED POLICY)") against the REAL build graph: the internal
# webfang_* references in each crate's Cargo.toml [dependencies] and
# [dev-dependencies] tables. Cargo.toml is the source of truth for crate-level
# dependencies, so this catches violations at build level (including
# feature-gated optional deps) without needing cargo-modules in CI.
#
# Policy matrix (source of truth — keep in sync with AGENTS.md):
#   webfang_core:       (none)
#   webfang_ai:         webfang_core
#   webfang_mcp:        webfang_core, webfang_ai      (ai feature-gated, #433)
#   webfang_cli:        webfang_core, webfang_ai, webfang_mcp
#   webfang_test_utils: webfang_core
#   webfang_benchmark:  webfang_core
#
# Dev tier (#1825): [dev-dependencies] may additionally target
# webfang_test_utils from ANY crate — it is the shared test harness
# (publish = false, never shipped). A [dependencies] edge into
# webfang_test_utils is still forbidden. fuzz/ remains outside this policy
# (not a workspace crate). Semantics harness:
# scripts/test_dependency_direction.sh.
#
# The extractor also recognizes the dotted-key source form
# (`webfang_core.workspace = true`), which the pre-#1825 regex silently
# ignored.

set -euo pipefail

declare -A ALLOWED=(
  [webfang_core]=""
  [webfang_ai]="webfang_core"
  [webfang_mcp]="webfang_core webfang_ai"
  [webfang_cli]="webfang_core webfang_ai webfang_mcp"
  [webfang_test_utils]="webfang_core"
  # webfang_benchmark: leaf harness crate (benchmark tooling, no production
  # dependents). Its [dev-dependencies] edge into webfang_test_utils is
  # accepted by the dev tier below.
  [webfang_benchmark]="webfang_core"
)

CRATES=(webfang_core webfang_ai webfang_mcp webfang_cli webfang_test_utils webfang_benchmark)
status=0

# Extract internal webfang_* dependency names from a crate manifest, tagged
# with their tier: "prod <dep>" from [dependencies] and
# [target.'cfg(...)'.dependencies], "dev <dep>" from [dev-dependencies] and
# [target.'cfg(...)'.dev-dependencies]. Only those tables are considered:
# [features] entries like `ai = ["dep:webfang_ai", ...]` and
# [workspace.dependencies] are skipped.
extract_internal_deps() {
  awk -F= '
    /^\[/ {
      in_deps = (($0 ~ /^\[(dev-)?dependencies\]/) || ($0 ~ /^\[target\..*dependencies\]/)) \
                && ($0 !~ /workspace/)
      is_dev = ($0 ~ /dev-/) ? 1 : 0
      next
    }
    in_deps && $1 ~ /^[[:space:]]*webfang_(core|ai|test_utils|benchmark|mcp|cli)(\.workspace)?[[:space:]]*$/ {
      gsub(/[[:space:]]/, "", $1)
      sub(/\.workspace$/, "", $1)
      print (is_dev ? "dev " : "prod ") $1
    }
  ' "$1"
}

for crate in "${CRATES[@]}"; do
  manifest="crates/$crate/Cargo.toml"
  if [[ ! -f "$manifest" ]]; then
    echo "::error::missing manifest $manifest"
    status=1
    continue
  fi
  allowed=${ALLOWED[$crate]}
  while read -r tier dep; do
    [[ -n "$dep" ]] || continue
    # Shared test harness: any crate may target it from [dev-dependencies].
    if [[ "$tier" == "dev" && "$dep" == "webfang_test_utils" ]]; then
      continue
    fi
    if [[ " $allowed " != *" $dep "* ]]; then
      echo "::error::$crate must NOT depend on $dep (${tier}-dependency; policy: ${allowed:-none})"
      status=1
    fi
  done < <(extract_internal_deps "$manifest")
done

if [[ $status -eq 0 ]]; then
  echo "OK: inter-crate dependency direction matches policy (issue #513)"
  for crate in "${CRATES[@]}"; do
    deps=$(extract_internal_deps "crates/$crate/Cargo.toml" | awk '{print $2}' | sort -u | paste -sd' ' -)
    printf '  %-18s -> %s\n' "$crate" "${deps:-none}"
  done
fi

exit "$status"
