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
# The inline pass routes every line through a single awk state machine that:
#   - tracks `/* ... */` block comments,
#   - skips lines whose first non-whitespace token starts with `//`
#     (covers `//`, `///`, `//!`),
#   - extracts EVERY occurrence of the inline layer regex on the surviving
#     code (one output line per match), and
#   - preserves the original 1-indexed line number as `NR<TAB>MATCH`.
# All matching happens inside this single awk pass — one process per file, not
# one per line (see ADR-0010-A: the per-line subshell variant was measured at
# >30s across ~93k lines and is forbidden).
#
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
# Hard cap. 19 current entries + headroom, so a NEW one-file violation does not
# force an ADR edit on every PR. Warn (do not fail) when within 2 of the cap.
ALLOWLIST_CAP=22
ALLOWLIST_WARN_AT=20

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

# Layer regex for the inline qualified-path pass (consumed by filter_and_match
# via `awk -v`). Matches the FULL path `crate::<layer>::seg(::seg)*` in ANY
# position of non-comment code, EVERY occurrence per line. Full-path capture is
# what makes narrow allowlist entries (e.g. `infrastructure::http::waf_engine`)
# substring-match the recorded violation. Does NOT match `crate::domain::`
# (innermost, never outward) or `crate::<PascalCase>` (aliases handled
# separately).
INLINE_LAYER_REGEX='crate::(infrastructure|adapters|application)::[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*'

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
    echo "::error::allowlist $ALLOWLIST has ${#ALLOW_PATTERNS[@]} entries, max is $ALLOWLIST_CAP (ADR-0010-A temporary cap; entries drop incrementally as #994 sub-slices 1, 3 and 4 land)"
    exit 1
  fi
  if (( ${#ALLOW_PATTERNS[@]} >= ALLOWLIST_WARN_AT )); then
    echo "::warning::allowlist has ${#ALLOW_PATTERNS[@]} entries (warn threshold $ALLOWLIST_WARN_AT, hard cap $ALLOWLIST_CAP) — prune entries as #994 sub-slices land before raising the cap"
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

# Awk comment-filter + match extractor. Reads a single file from stdin and
# prints one line per inline qualified-path match found in non-comment code,
# as `NR<TAB>MATCH`. This is the ONLY process spawned for the inline pass —
# matching inside awk keeps the whole scan at O(1) process per file (ADR-0010-A).
# Block comments (`/* ... */`) are tracked; line comments (`//`, `///`, `//!`)
# drop the whole line. Rust string literals are NOT parsed — residual false
# positives are absorbed by the allowlist, not the regex (ADR-0010-A).
filter_and_match() {
  awk -v re="$INLINE_LAYER_REGEX" '
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
      # Emit EVERY regex match on the surviving code (not just the first).
      # A trailing `// comment` on a code line is not stripped: an idiomatic
      # `crate::layer::...` path cannot legally live inside a comment, so any
      # hit there is a residual false positive absorbed by the allowlist.
      while (match(out, re)) {
        printf("%d\t%s\n", NR, substr(out, RSTART, RLENGTH))
        out = substr(out, RSTART + RLENGTH)
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
# the substring like `crate::infrastructure::foo::Bar`). Pure bash regex —
# no subprocess per match.
classify_inline_target() {
  local match="$1"
  if [[ "$match" =~ ^crate::([a-z_]+):: ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
  fi
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
  # Single awk pass per file: strips comments and emits EVERY
  # `crate::<layer>::X` match as `NR<TAB>MATCH` (see filter_and_match).
  inline_tmp=$(mktemp)
  trap 'rm -f "$inline_tmp"' EXIT
  filter_and_match < "$file" > "$inline_tmp"

  while IFS=$'\t' read -r lineno inline_match; do
    [[ -z "$inline_match" ]] && continue

    # Skip test-only inline paths: line is after the first #[cfg(test)]/mod tests
    if (( lineno > first_test_line )); then
      continue
    fi
    # Also skip if the line is inside a test attribute (preceded by #[cfg(test)])
    start=$(( lineno > 3 ? lineno - 3 : 1))
    if sed -n "${start},$((lineno-1))p" "$file" 2>/dev/null | grep -q -E '#\[cfg\(test\)\]'; then
      continue
    fi

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
