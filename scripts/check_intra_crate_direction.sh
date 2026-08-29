#!/usr/bin/env bash
# check_intra_crate_direction.sh — hardened for ADR-0010 + ADR-0010-A
#
# CI gate for the intra-crate Clean Architecture layering (issue #990,
# ADR-0010; issue #995, ADR-0010-A). See docs/adr/0010-intra-crate-direction-allowlist.md
# and its addendum.
#
# === Scope ===
# Two scan passes per file:
#   1) `use` line scan — matches `use crate::<layer>::...;` and `pub use ...` lines.
#   2) Inline qualified-path scan — matches `crate::<layer>::...` in any position
#      of any non-comment line (function bodies, struct fields, trait bounds, etc.).
#
# === Comment filter (issue #995, ADR-0010-A) ===
# The inline pass routes every line through an awk state machine that:
#   - tracks `/* ... */` block comments (open on `/*` not preceded by `//`,
#     close on `*/`),
#   - skips lines whose first non-whitespace token starts with `//`
#     (covers `//`, `///`, `//!`).
# It does NOT attempt to parse Rust string literals. A path inside a `"..."`
# literal or after a backslash continuation cannot be disambiguated by awk.
# Residual false positives are routed through the allowlist (which already
# substring-matches on the full match), not the regex. The regex stays
# conservative; the allowlist absorbs noise. See ADR-0010-A.
#
# Both passes share the same `#[cfg(test)]` / `mod tests` skip heuristic
# (line is after the first occurrence in the file).
#
# === Aliases ===
# `use crate::ScraperConfig` (and the other PascalCase re-exports in
# ALIAS_AS_INFRA_REGEX) are canonicalized as `infrastructure` for layering
# purposes (ADR-0010). They flow through the same allowlist.

set -euo pipefail

ROOT="crates/webfang_core/src"
MODE="${INTRA_CRATE_MODE:-warn}"
ALLOWLIST="scripts/check_intra_crate_direction_allowlist.txt"
ALLOWLIST_CAP=19

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

# Layer regex for the inline qualified-path pass. Matches `crate::<layer>::...`
# where <layer> is a lowercase snake_case layer (infrastructure, adapters,
# application). Does NOT match `crate::domain::` (innermost, never outward) or
# `crate::<PascalCase>` (aliases handled separately).
INLINE_LAYER_REGEX='crate::(infrastructure|adapters|application)::[a-z_]+'

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
  if (( ${#ALLOW_PATTERNS[@]} > ALLOWLIST_CAP )); then
    echo "::error::allowlist $ALLOWLIST has ${#ALLOW_PATTERNS[@]} entries, max is $ALLOWLIST_CAP (ADR-0010-A temporary cap raised 12→19; the `crate::ScraperConfig` alias entry is removed after #994 sub-slice 1 ports the ScraperConfig family, dropping the cap by 1; remaining entries drop incrementally as sub-slices 3 and 4 land)"
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

# Awk comment-filter state machine. Reads a single file from stdin and prints
# each non-comment line with its ORIGINAL 1-indexed line number prepended as
# `NR<TAB>LINE`. Block comments (`/* ... */`) are tracked; line comments
# (`//`, `///`, `//!`) drop the whole line. Rust string literals are NOT
# parsed — residual false positives are absorbed by the allowlist, not the
# regex (ADR-0010-A).
filter_comments() {
  awk '
    BEGIN { in_block = 0 }
    {
      line = $0
      out = ""
      i = 1
      n = length(line)
      # Strip leading whitespace to detect `//`-style line comments
      stripped = line
      sub(/^[[:space:]]+/, "", stripped)
      if (!in_block && substr(stripped, 1, 2) == "//") {
        # entire line is a comment
        next
      }
      # Walk the line, tracking block-comment state
      while (i <= n) {
        if (in_block) {
          close_idx = index(substr(line, i), "*/")
          if (close_idx == 0) {
            # rest of line inside block comment
            i = n + 1
          } else {
            i = i + close_idx + 1
            in_block = 0
          }
        } else {
          open_idx = index(substr(line, i), "/*")
          if (open_idx == 0) {
            # remainder of line is non-comment
            out = out substr(line, i)
            i = n + 1
          } else {
            # copy segment before /*, then enter block
            out = out substr(line, i, open_idx - 1)
            i = i + open_idx + 1
            in_block = 1
          }
        }
      }
      if (length(out) > 0) {
        # Drop `//` line comments that appear AFTER code on the same line.
        # e.g. `let x = 1; // comment` — naive state machine would keep the
        # code but it is followed by a comment; for our use the only thing
        # that matters is that we do not match `crate::...` across the `//`.
        # Strip from the FIRST `//` that is NOT inside a `crate::...::ident`
        # boundary. Simple heuristic: find any `//` and drop from there.
        # This is a conservative filter; it is allowed to drop some code as
        # long as the allowlist absorbs the noise.
        # We do not apply this heuristic to avoid complexity: a path like
        # `crate::foo` cannot legally appear after a `//` on the same line
        # in idiomatic Rust (it would be commented out). The block-comment
        # state machine above is sufficient.
        printf("%d\t%s\n", NR, out)
      }
    }
  '
}

# Classify a target_layer extracted from a `use` line. `match` is the full
# line; returns the target layer (infrastructure|adapters|application|domain)
# or empty when no layer can be extracted.
classify_use_target() {
  local match="$1"
  local target_layer
  target_layer=$(printf '%s\n' "$match" \
    | sed -nE 's/^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::([a-z_]+)::.*/\2/p' \
    | head -n 1)
  if [[ -z "$target_layer" ]]; then
    # Try the alias regex: `crate::ScraperConfig` etc. map to `infrastructure`.
    if printf '%s\n' "$match" | grep -q -E "$ALIAS_AS_INFRA_REGEX"; then
      target_layer="infrastructure"
    fi
  fi
  printf '%s' "$target_layer"
}

# Classify a target_layer from an inline qualified-path match (the match is
# the full substring like `crate::infrastructure::foo::Bar`). The first path
# segment after `crate::` is the layer.
classify_inline_target() {
  local match="$1"
  printf '%s' "$match" | sed -nE 's/^crate::([a-z_]+)::.*/\1/p' | head -n 1
}

# Process a single match (either a use line or an inline qualified path) and
# update the violation/allowlist counters. Emits the diagnostic if applicable.
# Args: file lineno match kind
#   kind is "use" or "inline" (used for the diagnostic only).
process_match() {
  local file="$1"
  local lineno="$2"
  local match="$3"
  local kind="$4"

  local target_layer
  if [[ "$kind" == "use" ]]; then
    target_layer=$(classify_use_target "$match")
  else
    target_layer=$(classify_inline_target "$match")
  fi
  [[ -z "$target_layer" ]] && return 0
  [[ -z "${LAYER_RANK[$target_layer]+x}" ]] && return 0

  local src_layer="$5"
  local src_rank="${LAYER_RANK[$src_layer]}"
  local target_rank="${LAYER_RANK[$target_layer]}"

  if (( target_rank < src_rank )); then
    if is_allowlisted "$file" "$match" "$target_layer"; then
      allowlisted_count=$((allowlisted_count + 1))
      return 0
    fi
    violations=$((violations + 1))
    if [[ "$MODE" == "strict" ]]; then
      echo "::error::$file:$lineno: $src_layer $kind imports $target_layer (inward-only violation; $src_layer → $target_layer is outward) — $match"
    else
      echo "::warning::$file:$lineno: $src_layer $kind imports $target_layer (inward-only violation; $src_layer → $target_layer is outward) — $match"
    fi
  fi
}

violations=0
allowlisted_count=0

while read -r file; do
  if ! src_layer=$(layer_of_file "$file"); then
    continue
  fi

  # Find line numbers of #[cfg(test)] and mod tests for skip heuristic
  # If file has a #[cfg(test)] block, all uses after its first occurrence are test-only.
  first_test_line=$(grep -n -E '#\[cfg\(test\)\]|mod tests' "$file" 2>/dev/null | head -n1 | cut -d: -f1 || true)
  if [[ -z "$first_test_line" ]]; then
    first_test_line=999999
  fi

  # --- Pass 1: `use` line scan ---
  while IFS=: read -r lineno match; do
    [[ -z "$match" ]] && continue

    # Skip test-only imports: if use line is after first #[cfg(test)]/mod tests
    if (( lineno > first_test_line )); then
      continue
    fi
    # Also skip if the use line itself is directly under #[cfg(test)] attribute (previous 3 lines)
    start=$(( lineno > 3 ? lineno - 3 : 1))
    if sed -n "${start},$((lineno-1))p" "$file" 2>/dev/null | grep -q -E '#\[cfg\(test\)\]'; then
      continue
    fi

    process_match "$file" "$lineno" "$match" "use" "$src_layer"
  done < <(grep -n -E '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::[A-Za-z_]+(::|;)' "$file" 2>/dev/null || true)

  # --- Pass 2: inline qualified-path scan ---
  # Step A: filter out comments via awk, preserving line numbers.
  # Step B: grep the filtered output for inline qualified paths in any position.
  # Step C: apply the same #[cfg(test)] / mod tests skip heuristic.
  inline_tmp=$(mktemp)
  trap 'rm -f "$inline_tmp"' EXIT
  filter_comments < "$file" > "$inline_tmp"

  while IFS=$'\t' read -r lineno rest; do
    [[ -z "$rest" ]] && continue

    # Skip test-only inline paths: line is after the first #[cfg(test)]/mod tests
    if (( lineno > first_test_line )); then
      continue
    fi
    # Also skip if the line is inside a test attribute (preceded by #[cfg(test)])
    start=$(( lineno > 3 ? lineno - 3 : 1))
    if sed -n "${start},$((lineno-1))p" "$file" 2>/dev/null | grep -q -E '#\[cfg\(test\)\]'; then
      continue
    fi

    # Extract the first inline qualified path of interest from this line.
    # Use sed -n to pick the first match.
    inline_match=$(printf '%s\n' "$rest" \
      | sed -nE "0,/${INLINE_LAYER_REGEX}/{s/.*(${INLINE_LAYER_REGEX}).*/\\1/p;}" \
      | head -n 1)
    [[ -z "$inline_match" ]] && continue

    process_match "$file" "$lineno" "$inline_match" "inline" "$src_layer"
  done < "$inline_tmp"
  rm -f "$inline_tmp"
  trap - EXIT

done < <(find "$ROOT" -name "*.rs" -type f)

if [[ -f "$ALLOWLIST" ]]; then
  echo "allowlisted $allowlisted_count (max $ALLOWLIST_CAP, file: $ALLOWLIST, entries: ${#ALLOW_PATTERNS[@]})"
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
