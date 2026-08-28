#!/usr/bin/env bash
# check_intra_crate_direction.sh
#
# CI gate for the intra-crate Clean Architecture layering (issue #984
# followup, ADR-0009). The inter-crate direction is enforced by
# `check_dependency_direction.sh` against `Cargo.toml`. This script
# enforces the **module-level** rule from AGENTS.md:
#
#   infrastructure → adapters → application → domain   (inward only)
#
# A module in a deeper layer MUST NOT import a module from a shallower
# layer. The inter-crate gate cannot see this because the same crate
# imports the other module freely — the violation #984 introduced
# (`use crate::application::crawl_options::CrawlLimits` from
# `domain::persistence`) shipped that way and the inter-crate check
# stayed green.
#
# Implementation: scan each `.rs` file under `crates/webfang_core/src/`
# and look at `use crate::<layer>::...` declarations. Map the source
# file's layer (deepest path segment) to the target module's layer; if
# the target is shallower (i.e. outward of the source), report.
#
# Layers (from outside in): infrastructure, adapters, application, domain.
# A source in `domain` must not see `application`, `adapters`, or
# `infrastructure`. A source in `application` must not see `adapters` or
# `infrastructure`. A source in `adapters` must not see `infrastructure`.
#
# Mode: by default this script runs in **WARN** mode — every violation
# is reported as a `::warning::` annotation so the count is visible
# in CI logs, but exit code is 0. Once a follow-up slice has fixed the
# pre-existing application→infrastructure violations, flip the mode
# to `strict` (set `INTRA_CRATE_MODE=strict`) and the same script
# becomes a hard gate.
#
# The top-level src files of webfang_core (lib.rs / main.rs) and
# integration tests under `tests/` are outside the layering rule (the
# crate root re-exports from every layer; tests use the public API).
# They are skipped.

set -euo pipefail

ROOT="crates/webfang_core/src"
# Mode: "warn" (default; log only) or "strict" (exit 1 on any violation).
MODE="${INTRA_CRATE_MODE:-warn}"

# Layer order: index 0 = outermost, higher = more inner.
# "domain" is the most inner.
declare -A LAYER_RANK=(
  [infrastructure]=0
  [adapters]=1
  [application]=2
  [domain]=3
)

# Returns 0 (success) and prints the layer if the path lives under a
# known layer; returns 1 and prints nothing otherwise (e.g. the crate
# root `lib.rs` or a `tests/` subdirectory).
layer_of_file() {
  local file="$1"
  local rel="${file#${ROOT}/}"
  # Take the directory portion of the source file (lib.rs / main.rs
  # are skipped because they sit directly under $ROOT).
  local dir
  dir=$(dirname "$rel")
  if [[ "$dir" == "." || -z "$dir" ]]; then
    return 1
  fi
  # Find the deepest known layer under $dir.
  local best=""
  local best_rank=-1
  for layer in infrastructure adapters application domain; do
    if [[ "$dir" == *"/$layer" || "$dir" == "$layer" || "$dir" == "$layer"/* ]]; then
      local r="${LAYER_RANK[$layer]}"
      if (( r > best_rank )); then
        best="$layer"
        best_rank=$r
      fi
    fi
  done
  if [[ -z "$best" ]]; then
    return 1
  fi
  echo "$best"
}

if [[ ! -d "$ROOT" ]]; then
  echo "::error::missing source dir $ROOT"
  exit 1
fi

# Walk every .rs file.
violations=0
while read -r file; do
  if ! src_layer=$(layer_of_file "$file"); then
    continue
  fi
  src_rank="${LAYER_RANK[$src_layer]}"

  # Find every `use crate::<target_layer>::...` in the file. We use a
  # regex with capture group, then read each match into a shell loop.
  while read -r match; do
    [[ -z "$match" ]] && continue
    # `match` is the full line; pull out the layer name after `use crate::`.
    target_layer=$(printf '%s\n' "$match" \
      | sed -nE 's/^[[:space:]]*use[[:space:]]+crate::([a-z_]+)::.*/\1/p' \
      | head -n 1)
    [[ -z "$target_layer" ]] && continue
    if [[ -z "${LAYER_RANK[$target_layer]+x}" ]]; then
      # Not a known layer (could be a sub-module of a layer; ignore).
      continue
    fi
    target_rank="${LAYER_RANK[$target_layer]}"
    if (( target_rank < src_rank )); then
      violations=$((violations + 1))
      if [[ "$MODE" == "strict" ]]; then
        echo "::error::$file: $src_layer imports $target_layer (inward-only violation; $src_layer → $target_layer is outward)"
      else
        echo "::warning::$file: $src_layer imports $target_layer (inward-only violation; $src_layer → $target_layer is outward)"
      fi
    fi
  done < <(grep -E '^[[:space:]]*use[[:space:]]+crate::[a-z_]+::' "$file" 2>/dev/null || true)
done < <(find "$ROOT" -name "*.rs" -type f)

if [[ "$MODE" == "strict" ]]; then
  if [[ $violations -eq 0 ]]; then
    echo "OK: intra-crate Clean Architecture layering is inward-only (ADR-0009, strict mode)"
    exit 0
  fi
  echo "::error::found $violations intra-crate direction violation(s) (ADR-0009, strict mode)"
  exit 1
else
  echo "OK (warn): $violations intra-crate direction violation(s) reported (ADR-0009, warn mode — flip INTRA_CRATE_MODE=strict after the follow-up slice lands)"
  exit 0
fi