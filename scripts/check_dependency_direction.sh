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
#   webfang_core: (none)
#   webfang_ai:   webfang_core
#   webfang_mcp:  webfang_core, webfang_ai      (ai feature-gated, #433)
#   webfang_cli:  webfang_core, webfang_ai, webfang_mcp
#
# Crates outside the policy (webfang_test_utils, fuzz/) are not checked.

set -euo pipefail

declare -A ALLOWED=(
  [webfang_core]=""
  [webfang_ai]="webfang_core"
  [webfang_mcp]="webfang_core webfang_ai"
  [webfang_cli]="webfang_core webfang_ai webfang_mcp"
  # webfang_benchmark: leaf harness crate (benchmark tooling, no production
  # dependents). Depends on webfang_core + webfang_test_utils (the latter is
  # outside this policy and never extracted).
  [webfang_benchmark]="webfang_core"
)

CRATES=(webfang_core webfang_ai webfang_mcp webfang_cli webfang_benchmark)
status=0

# Extract internal webfang_* dependency names from a crate manifest.
# Only [dependencies]-style tables are considered ([dependencies],
# [dev-dependencies], [target.'cfg(...)'.dependencies]); [features] entries
# like `ai = ["dep:webfang_ai", ...]` and [workspace.dependencies] are skipped.
extract_internal_deps() {
  awk -F= '
    /^\[/ {
      in_deps = (($0 ~ /^\[(dev-)?dependencies\]/) || ($0 ~ /^\[target\..*dependencies\]/)) \
                && ($0 !~ /workspace/)
      next
    }
    in_deps && $1 ~ /^[[:space:]]*webfang_(core|ai|tui|mcp|cli)[[:space:]]*$/ {
      gsub(/[[:space:]]/, "", $1)
      print $1
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
  while read -r dep; do
    [[ -n "$dep" ]] || continue
    if [[ " $allowed " != *" $dep "* ]]; then
      echo "::error::$crate must NOT depend on $dep (policy: ${allowed:-none})"
      status=1
    fi
  done < <(extract_internal_deps "$manifest")
done

if [[ $status -eq 0 ]]; then
  echo "OK: inter-crate dependency direction matches policy (issue #513)"
  for crate in "${CRATES[@]}"; do
    deps=$(extract_internal_deps "crates/$crate/Cargo.toml" | sort -u | paste -sd' ' -)
    printf '  %-14s -> %s\n' "$crate" "${deps:-none}"
  done
fi

exit "$status"
