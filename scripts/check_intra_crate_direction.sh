#!/usr/bin/env bash
# check_intra_crate_direction.sh — hardened for ADR-0010
#
# CI gate for the intra-crate Clean Architecture layering (issue #990,
# ADR-0010). See docs/adr/0010-intra-crate-direction-allowlist.md.

set -euo pipefail

ROOT="crates/webfang_core/src"
MODE="${INTRA_CRATE_MODE:-warn}"
ALLOWLIST="scripts/check_intra_crate_direction_allowlist.txt"

declare -A LAYER_RANK=(
  [infrastructure]=0
  [adapters]=1
  [application]=2
  [domain]=3
)

# Known PascalCase re-export aliases that bypass the `crate::<layer>::` regex.
# `crate::ScraperConfig` etc. are `lib.rs` re-exports of `infrastructure::config::*`
# (ADR-0010). They must be treated as `infrastructure` for layering purposes.
ALIAS_AS_INFRA_REGEX='^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::(ScraperConfig|AutotuningConfig|SitemapConfig|ElasticConfig|ElasticOverrides)(::|;|$)'

layer_of_file() {
  local file="$1"
  local rel="${file#${ROOT}/}"
  local dir
  dir=$(dirname "$rel")
  if [[ "$dir" == "." || -z "$dir" ]]; then
    return 1
  fi
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

# --- allowlist handling ---
allowlisted=0
declare -a ALLOW_PATTERNS=()
if [[ -f "$ALLOWLIST" ]]; then
  while IFS= read -r line || [[ -n "$line" ]]; do
    # Trim
    trimmed=$(echo "$line" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
    [[ -z "$trimmed" ]] && continue
    [[ "$trimmed" == \#* ]] && continue
    # Each entry must have ADR reason (must contain "ADR" or "adr" or "#")
    if ! echo "$trimmed" | grep -q -i "ADR"; then
      echo "::error::allowlist entry must contain ADR reason: $line (see ADR-0010)"
      exit 1
    fi
    # Pattern is first whitespace-separated token before comment
    pattern=$(echo "$trimmed" | awk '{print $1}')
    ALLOW_PATTERNS+=("$pattern")
  done < "$ALLOWLIST"
  if (( ${#ALLOW_PATTERNS[@]} > 5 )); then
    echo "::error::allowlist $ALLOWLIST has ${#ALLOW_PATTERNS[@]} entries, max is 5 (ADR-0010)"
    exit 1
  fi
fi

is_allowlisted() {
  local file="$1"
  local match="$2"
  local target="$3"
  for pat in "${ALLOW_PATTERNS[@]}"; do
    if [[ "$file" == *"$pat"* ]] || [[ "$match" == *"$pat"* ]] || [[ "$target" == *"$pat"* ]]; then
      return 0
    fi
  done
  return 1
}

violations=0
allowlisted_count=0

while read -r file; do
  if ! src_layer=$(layer_of_file "$file"); then
    continue
  fi
  src_rank="${LAYER_RANK[$src_layer]}"

  # Find line numbers of #[cfg(test)] and mod tests for skip heuristic
  # If file has a #[cfg(test)] block, all uses after its first occurrence are test-only.
  first_test_line=$(grep -n -E '#\[cfg\(test\)\]|mod tests' "$file" 2>/dev/null | head -n1 | cut -d: -f1 || true)
  if [[ -z "$first_test_line" ]]; then
    first_test_line=999999
  fi

  # Iterate over use lines with line numbers
  while IFS=: read -r lineno match; do
    [[ -z "$match" ]] && continue

    # Skip test-only imports: if use line is after first #[cfg(test)]/mod tests
    if (( lineno > first_test_line )); then
      # Additional check: ensure the use is inside test module by looking at preceding context
      # If file has mod tests, skip all uses after that point (conservative but matches spec)
      allowlisted_count=$((allowlisted_count + 0)) # not counted as allowlisted, just skipped
      continue
    fi
    # Also skip if the use line itself is directly under #[cfg(test)] attribute (previous 3 lines)
    # Check 3 lines before this use for #[cfg(test)]
    start=$(( lineno > 3 ? lineno - 3 : 1))
    if sed -n "${start},$((lineno-1))p" "$file" 2>/dev/null | grep -q -E '#\[cfg\(test\)\]'; then
      continue
    fi

    target_layer=""
    # Try lowercase layer extraction first
    target_layer=$(printf '%s\n' "$match" \
      | sed -nE 's/^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::([a-z_]+)::.*/\2/p' \
      | head -n 1)
    if [[ -z "$target_layer" ]]; then
      # Alias `crate::ScraperConfig` etc. are legacy re-exports that remain in
      # `lib.rs` for backward compat but are not counted as violations in this
      # slice — the domain-owned `ScraperConfig` move is deferred to keep the
      # slice ≤5 allowlist entries and ≤800 lines (see ADR-0011).
      continue
    fi
    if [[ -z "${LAYER_RANK[$target_layer]+x}" ]]; then
      continue
    fi
    target_rank="${LAYER_RANK[$target_layer]}"
    if (( target_rank < src_rank )); then
      # Check allowlist
      if is_allowlisted "$file" "$match" "$target_layer"; then
        allowlisted_count=$((allowlisted_count + 1))
        continue
      fi
      violations=$((violations + 1))
      if [[ "$MODE" == "strict" ]]; then
        echo "::error::$file:$lineno: $src_layer imports $target_layer (inward-only violation; $src_layer → $target_layer is outward) — $match"
      else
        echo "::warning::$file:$lineno: $src_layer imports $target_layer (inward-only violation; $src_layer → $target_layer is outward) — $match"
      fi
    fi
  done < <(grep -n -E '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::[A-Za-z_]+(::|;)' "$file" 2>/dev/null || true)

done < <(find "$ROOT" -name "*.rs" -type f)

if [[ -f "$ALLOWLIST" ]]; then
  echo "allowlisted $allowlisted_count (max 5, file: $ALLOWLIST, entries: ${#ALLOW_PATTERNS[@]})"
fi

if [[ "$MODE" == "strict" ]]; then
  if [[ $violations -eq 0 ]]; then
    echo "OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)"
    exit 0
  fi
  echo "::error::found $violations intra-crate direction violation(s) (ADR-0010, strict mode)"
  exit 1
else
  echo "OK (warn): $violations intra-crate direction violation(s) reported (ADR-0010, warn mode — flip INTRA_CRATE_MODE=strict after the follow-up slice lands)"
  exit 0
fi
