#!/usr/bin/env bash
# check_intra_crate_direction.sh — hardened for ADR-0010 + ADR-0010-A
#
# CI gate for the intra-crate Clean Architecture layering (issue #990,
# ADR-0010; issue #995, ADR-0010-A). See docs/adr/0010-intra-crate-direction-allowlist.md
# and its addendum.
#
# === Scope ===
# Two scan passes per file:
#   1) `use` line scan — matches `use crate::<layer>::...;` and `pub use ...`
#      lines, plus the PascalCase re-export aliases (see `Aliases` below).
#   2) Inline qualified-path scan — matches `crate::<layer>::...` in any position
#      of any non-comment line (function bodies, struct fields, trait bounds, etc.).
#
# === Record merge and dedupe (issue #1068, blocker B1) ===
# The inline pass scans every non-comment line, so it also sees the `use` lines
# that the `use` pass reports. Both passes therefore emit records into one merged
# stream that is deduped on `(file, line, path)` BEFORE anything is counted or
# reported. The number the gate prints is the number of distinct violation SITES,
# not the number of regex hits. Before this fix every `use crate::infrastructure::X;`
# was emitted once per pass and the reported count was inflated ~1.6x (84 rows for
# 52 real sites).
#
# The dedupe key includes the matched PATH, not just `(file, line)`, because a
# single `use` line carrying a brace group legitimately yields several distinct
# sites on one line (see `Brace expansion` below).
#
# === Brace expansion (issue #1068, blocker B2) ===
# `use crate::infrastructure::crawler::{A, B, C};` used to yield the single
# truncated match `crate::infrastructure::crawler`, because the layer regex stops
# at `{`. No per-symbol allowlist entry can ever match that bare form, which makes
# "narrow the entry before deleting it" unimplementable. The extractor now expands
# a brace group into one record per symbol (`crate::infrastructure::crawler::A`,
# `…::B`, `…::C`). It handles nested groups (`a::{b::{c, d}, e}`), `self`, glob
# items, ` as Rename` suffixes (the module path is what layering cares about, not
# the local binding), whitespace after commas, and groups whose closing `}` lands
# on a later line — multi-line `use` blocks are idiomatic `rustfmt` output in this
# crate. Every symbol of a group is attributed to the line the `use` statement
# starts on, which is the line a developer has to edit.
#
# === Comment filter (issue #995, ADR-0010-A) ===
# Both passes route every line through the same awk state machine that:
#   - tracks `/* ... */` block comments,
#   - skips lines whose first non-whitespace token starts with `//`
#     (covers `//`, `///`, `//!`),
#   - extracts EVERY occurrence of the inline layer regex on the surviving
#     code (one output record per match), and
#   - preserves the original 1-indexed line number as `NR<TAB>MATCH`.
# All matching happens inside this single awk pass — one process per file per
# pass, not one per line (see ADR-0010-A: the per-line subshell variant was
# measured at >30s across ~93k lines and is forbidden).
#
# It does NOT attempt to parse Rust string literals. A path inside a `"..."`
# literal or after a backslash continuation cannot be disambiguated by awk.
# Residual false positives are routed through the allowlist (which matches module
# paths on `::` segment boundaries — see `Allowlist matching` below), not the
# regex. The regex stays conservative; the allowlist absorbs noise.
# See ADR-0010-A.
#
# === cli composition edge (ADR-0012-B 3.H, #1097) ===
# `cli/` is the outermost composition edge (rank -1, below `infrastructure`):
# it owns construction of infrastructure concretes (`StateStore`,
# `RecordStore`, fetchers) and injects them into `application` through domain
# ports. Because the layering rule is `target_rank < src_rank` (outward
# only), a `cli` source (rank -1) can never flag — every inner layer ranks
# higher. This is deliberate: the gate pins `application`/`domain` purity
# while leaving construction to the edge. Consumed by #1100 (the remaining
# cli concrete namings drain through the same edge, no allowlist entry).
#
# Both passes share the same `#[cfg(test)]` / `mod tests` skip heuristic
# (line is after the first occurrence in the file).
#
# === Aliases ===
# `use crate::ScraperConfig` (and the other PascalCase re-exports in ALIAS_NAMES)
# are canonicalized as `infrastructure` for layering purposes (ADR-0010). They
# flow through the same allowlist. ALIAS_NAMES is the single source of truth for
# the list; the awk `use` pass and the bash classifier both consume it.
#
# === Allowlist matching (issue #1068, blocker B3) ===
# A pattern absorbs a MODULE PATH only at `::` segment boundaries: the character
# immediately before the occurrence must be `:` or the occurrence must start at
# position 0, and the character immediately after it must be `:` or end-of-string.
# Plain substring matching failed open: `infrastructure::crawler::resource_downloader`
# also absorbed `crate::infrastructure::crawler::resource_downloader_v2::EvilThing`
# and `…::resource_downloader_legacy::X`, and `infrastructure::export::state_store`
# absorbed `crate::infrastructure::export::state_store_backup::X`.

set -euo pipefail

# INTRA_CRATE_ROOT / INTRA_CRATE_ALLOWLIST exist so scripts/test_intra_crate_gate.sh
# can point the scanner at a throwaway fixture tree. Production CI relies on the
# defaults and never sets them.
ROOT="${INTRA_CRATE_ROOT:-crates/webfang_core/src}"
MODE="${INTRA_CRATE_MODE:-strict}"
ALLOWLIST="${INTRA_CRATE_ALLOWLIST:-scripts/check_intra_crate_direction_allowlist.txt}"
# Hard cap. ADR-0012-B's 10→2 path has landed: the only entries expected now are
# the two permanent ADR-0011 exemptions (DI root + transversal tracing), so this
# is §2.2's terminal cap, not a temporary one. Warn (do not fail) within 2 of cap.
ALLOWLIST_CAP=5
ALLOWLIST_WARN_AT=3

declare -A LAYER_RANK=(
  [cli]=-1
  [infrastructure]=0
  [adapters]=1
  [application]=2
  [domain]=3
)

# Known PascalCase re-export aliases that bypass the `crate::<layer>::` regex.
# `crate::ScraperConfig` etc. are `lib.rs` re-exports of `infrastructure::config::*`
# (ADR-0010). They must be treated as `infrastructure` for layering purposes.
ALIAS_NAMES='ScraperConfig|AutotuningConfig|SitemapConfig|ElasticConfig|ElasticOverrides'
# awk/ERE form: matches the alias PATH itself inside a `use` line.
ALIAS_AWK_REGEX="crate::(${ALIAS_NAMES})"

# Layer regex for the qualified-path extractor (consumed by filter_and_match via
# `awk -v`). Matches the FULL path `crate::<layer>::seg(::seg)*` in ANY position
# of non-comment code, EVERY occurrence per line. Full-path capture is what makes
# narrow allowlist entries (e.g. `infrastructure::http::waf_engine`) match the
# recorded violation on segment boundaries instead of forcing a broad
# `infrastructure::http` entry. Does NOT match `crate::domain::` (innermost, never
# outward) or `crate::<PascalCase>` (aliases handled separately).
INLINE_LAYER_REGEX='crate::(infrastructure|adapters|application)::[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*'

layer_of_file() {
  local file="$1"
  local rel="${file#"${ROOT}"/}"
  local dir
  dir=$(dirname "$rel")
  if [[ "$dir" == "." || -z "$dir" ]]; then
    return 1
  fi
  local best=""
  local best_rank=-2
  for layer in infrastructure adapters application domain cli; do
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
    echo "::error::allowlist $ALLOWLIST has ${#ALLOW_PATTERNS[@]} entries, max is $ALLOWLIST_CAP (ADR-0012-B terminal cap — only the two permanent ADR-0011 entries are expected. A new entry needs a removal condition cited by SYMBOL, never by line number, per #1032.)"
    exit 1
  fi
  if (( ${#ALLOW_PATTERNS[@]} >= ALLOWLIST_WARN_AT )); then
    echo "::warning::allowlist has ${#ALLOW_PATTERNS[@]} entries (warn threshold $ALLOWLIST_WARN_AT, hard cap $ALLOWLIST_CAP) — steady state is the 2 permanent ADR-0011 entries; prune before adding, and never re-add a broad module entry (narrow per-symbol entries fail closed, broad ones silently absorb)"
  fi
fi

# Segment-aware containment for MODULE PATHS (issue #1068, blocker B3).
# `pat` may absorb `hay` only when it occurs at `::` segment boundaries: the
# character immediately before the occurrence must be ':' (the tail of a `::`
# separator, or of the leading `crate::` prefix) or the occurrence must start at
# position 0, and the character immediately after it must be ':' or end-of-string.
# Plain substring matching let the entry `infrastructure::crawler::resource_downloader`
# silently absorb `crate::infrastructure::crawler::resource_downloader_v2::EvilThing`.
path_segment_contains() {
  local hay="$1"
  local pat="$2"
  local rest="$hay"
  local pre suf
  local at_start=1
  local ok_before ok_after

  [[ -n "$pat" ]] || return 1

  while [[ "$rest" == *"$pat"* ]]; do
    # Text before this occurrence, and the text right after it.
    pre="${rest%%"$pat"*}"
    suf="${rest#*"$pat"}"

    ok_after=0
    if [[ -z "$suf" || "${suf:0:1}" == ":" ]]; then
      ok_after=1
    fi

    if (( ok_after )); then
      ok_before=0
      if [[ -z "$pre" ]]; then
        (( at_start )) && ok_before=1
      elif [[ "${pre: -1}" == ":" ]]; then
        ok_before=1
      fi
      (( ok_before )) && return 0
    fi

    # Subsequent iterations look at text that is no longer at the start of the
    # path, so an occurrence beginning at position 0 of `rest` is mid-path.
    at_start=0
    rest="$suf"
  done

  return 1
}

is_allowlisted() {
  local file="$1"
  local match="$2"
  local target="$3"
  local pat
  for pat in "${ALLOW_PATTERNS[@]}"; do
    # FILE PATHS are not module paths: a per-file entry is an explicit, reviewed
    # exemption for the whole file (`application/container.rs` in ADR-0010 §2), so
    # substring matching is kept here ON PURPOSE. Segment anchoring applies to
    # module paths only, where `foo` and `foo_v2` are genuinely different modules.
    if [[ "$file" == *"$pat"* ]]; then
      return 0
    fi
    if path_segment_contains "$match" "$pat"; then
      return 0
    fi
    if path_segment_contains "$target" "$pat"; then
      return 0
    fi
  done
  return 1
}

# filter_and_match <use|inline> — the shared record extractor.
#
# Reads a single file from stdin and prints one `NR<TAB>MATCH` record per distinct
# qualified-path SITE found in non-comment code. Both scan passes run this same
# extractor so their records are directly comparable, which is what makes the
# (file, line, path) dedupe possible:
#   - `use`    → only `use`/`pub use` statements, plus the PascalCase alias
#                re-exports that the layer regex cannot see by construction.
#   - `inline` → every non-comment line, every occurrence (a superset that also
#                covers the `use` lines; the dedupe collapses the overlap).
# Each mode is ONE awk process per file, so the whole scan stays at O(1) processes
# per file (ADR-0010-A forbids the per-line subshell variant).
#
# Block comments (`/* ... */`) are tracked; line comments (`//`, `///`, `//!`) drop
# the whole line. Rust string literals are NOT parsed — residual false positives
# are absorbed by the allowlist, not the regex (ADR-0010-A).
#
# Brace groups are EXPANDED (issue #1068, blocker B2): a match immediately followed
# by `{` yields one record per symbol with the group prefix joined back on, so
# `use crate::infrastructure::crawler::{A, B};` emits
# `crate::infrastructure::crawler::A` and `…::B` instead of the single bare
# `crate::infrastructure::crawler` that no narrow entry could ever match. Nested
# groups, `self`, ` as Rename` suffixes, and groups whose closing `}` lands on a
# later line are all handled.
filter_and_match() {
  local mode="$1"
  awk -v re="$INLINE_LAYER_REGEX" -v alias_re="$ALIAS_AWK_REGEX" -v mode="$mode" '
    function trim(s) {
      gsub(/^[[:space:]]+/, "", s)
      gsub(/[[:space:]]+$/, "", s)
      return s
    }

    # Index (1-based) of the `}` that closes the group whose BODY starts at position
    # `from` of `s` (so `s[from-1]` is the `{` that opened it), or 0 when the group is
    # not closed inside `s`. Callers must pass the first body character, not the brace.
    function close_of(s, from,   i, d, ch) {
      d = 1
      for (i = from; i <= length(s); i++) {
        ch = substr(s, i, 1)
        if (ch == "{") d++
        else if (ch == "}") { d--; if (d == 0) return i }
      }
      return 0
    }

    # Emit one record per symbol of a brace group. `body` is the text between the
    # outer braces; `prefix` is the path the group was attached to.
    function expand(prefix, body, lineno,   i, n, ch, d, cur, parts, np, k) {
      n = length(body)
      d = 0
      cur = ""
      np = 0
      for (i = 1; i <= n; i++) {
        ch = substr(body, i, 1)
        if (ch == "{") { d++; cur = cur ch }
        else if (ch == "}") { d--; cur = cur ch }
        else if (ch == "," && d == 0) { parts[++np] = cur; cur = "" }
        else cur = cur ch
      }
          if (trim(cur) != "") parts[++np] = cur
          # An empty group carries no symbol to attribute the reference to; keep the
          # violation visible as the bare prefix rather than dropping it silently.
          if (np == 0) {
            printf("%d\t%s\n", lineno, prefix)
            return
          }
          for (k = 1; k <= np; k++) emit_item(prefix, parts[k], lineno)
    }

    function emit_item(prefix, item, lineno,   open, cl, head, nested) {
      item = trim(item)
      if (item == "") return
      # ` as Alias` — the module path is what layering cares about, not the local
      # binding name.
      if (match(item, /[[:space:]]as[[:space:]]/)) item = trim(substr(item, 1, RSTART - 1))
      # Nested group: `sub::{A, B}`.
      open = index(item, "::{")
      if (open > 0) {
        head = trim(substr(item, 1, open - 1))
        nested = substr(item, open + 3)
        cl = close_of(nested, 1)
        if (cl > 0) {
          expand(prefix "::" head, substr(nested, 1, cl - 1), lineno)
          return
        }
      }
      # `self` and the `*` glob both denote the module itself, i.e. the prefix.
      if (item == "self" || item == "*") { printf("%d\t%s\n", lineno, prefix); return }
      # Anything that is not a plain path segment falls back to the bare prefix so
      # a malformed line can never silently drop a violation.
      if (item !~ /^[A-Za-z0-9_#]+(::[A-Za-z0-9_#]+)*$/) { printf("%d\t%s\n", lineno, prefix); return }
      printf("%d\t%s\n", lineno, prefix "::" item)
    }

    BEGIN {
      in_block = 0
      pend_prefix = ""; pend_body = ""; pend_line = 0; pend_lines = 0
    }
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

      # Inside a multi-line brace group: accumulate until the group closes. A `//`
      # inside a `use` group is always a comment (a use path cannot contain a
      # string literal), so it is safe to drop it here.
      if (pend_prefix != "") {
        pend_lines++
        tmp = out
        sub(/\/\/.*/, "", tmp)
        pend_body = pend_body " " tmp
        cl = close_of(pend_body, 1)
        if (cl > 0) {
          expand(pend_prefix, substr(pend_body, 1, cl - 1), pend_line)
          pend_prefix = ""
        } else if (pend_lines > 20 || index(pend_body, ";") > 0) {
          # Malformed or over-long group: fall back to the bare prefix.
          printf("%d\t%s\n", pend_line, pend_prefix)
          pend_prefix = ""
        }
        next
      }

      is_use = (line ~ /^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::[A-Za-z_]+(::|;|[{])/)
      if (mode == "use" && !is_use) next

      # PascalCase re-export aliases (`use crate::ScraperConfig;`) bypass the layer
      # regex entirely; canonicalize them as their own records so they flow
      # through the same allowlist (ADR-0010 §3). The inline pass skips them,
      # exactly as before.
      if (mode == "use") {
        tmp = out
        while (match(tmp, alias_re)) {
          printf("%d\t%s\n", NR, substr(tmp, RSTART, RLENGTH))
          tmp = substr(tmp, RSTART + RLENGTH)
        }
      }

      # Emit EVERY regex match on the surviving code (not just the first).
      # A trailing `// comment` on a code line is not stripped: an idiomatic
      # `crate::layer::...` path cannot legally live inside a comment, so any hit
      # there is a residual false positive absorbed by the allowlist.
      # `text` holds the un-scanned remainder of the code segments; each iteration
      # advances past the last match (RSTART/RLENGTH from the match() above are
      # still valid for the substr calls that follow).
      text = out
      while (match(text, re)) {
        m = substr(text, RSTART, RLENGTH)
        after = substr(text, RSTART + RLENGTH)
            # The layer regex cannot consume the `::` before a brace group (`{` is not a
            # valid segment start), so a group always reads `::{` in `after`: the body
            # starts at index 4. Testing index 1 here made the whole branch dead code
            # and restored the truncation the expansion exists to remove.
            if (substr(after, 1, 2) == "::" && substr(after, 3, 1) == "{") {
              cl = close_of(after, 4)
              if (cl > 0) {
                expand(m, substr(after, 4, cl - 4), NR)
                text = substr(after, cl + 1)
                continue
              }
              # Group opens here but closes on a later line.
              pend_prefix = m
              pend_line = NR
              pend_body = substr(after, 4)
              pend_lines = 0
              text = ""
              break
            }
        printf("%d\t%s\n", NR, m)
        text = substr(text, RSTART + RLENGTH)
      }
    }
    END {
      # Unterminated group at EOF: keep the violation visible as the bare prefix.
      if (pend_prefix != "") printf("%d\t%s\n", pend_line, pend_prefix)
    }
  '
}

# Classify the target layer of a normalized path record. Records from both passes
# are now plain `crate::<seg>::<seg>…` paths, so one classifier serves both.
# Returns the layer (infrastructure|adapters|application|domain) or empty when no
# known layer can be extracted.
classify_target() {
  local match="$1"
  if [[ "$match" =~ ^crate::([a-z_]+):: ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
    return 0
  fi
  # Alias form: `crate::ScraperConfig` etc. map to `infrastructure` (ADR-0010 §3).
  if [[ "$match" =~ ^crate::(${ALIAS_NAMES})$ ]]; then
    printf 'infrastructure'
    return 0
  fi
}

# Process a single normalized path record and update the violation/allowlist
# counters. Emits the diagnostic if applicable.
# Args: file lineno match kind src_layer
#   kind is "use" or "inline" (used for the diagnostic only).
process_match() {
  local file="$1"
  local lineno="$2"
  local match="$3"
  local kind="$4"
  local src_layer="$5"

  local target_layer
  target_layer=$(classify_target "$match")
  [[ -z "$target_layer" ]] && return 0
  [[ -z "${LAYER_RANK[$target_layer]+x}" ]] && return 0

  local src_rank="${LAYER_RANK[$src_layer]}"
  local target_rank="${LAYER_RANK[$target_layer]}"

  # The rule is `target_rank < src_rank`: OUTWARD only. A lateral reference
  # (target_rank == src_rank, e.g. infrastructure -> infrastructure) is
  # intentionally NOT flagged. That is the known blind spot behind #1060/#1061 and
  # is out of scope for this gate; scripts/test_intra_crate_gate.sh pins it so it
  # is never "fixed" by accident.
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

# Dedupe set for (file, line, match) — issue #1068, blocker B1. The two scan
# passes overlap on every `use` line, so without this the reported number is regex
# hits rather than distinct violation sites.
declare -A SEEN_SITE=()

# Scratch file for each pass' records — created ONCE, outside the per-file loop,
# and removed by a single EXIT trap (a trap inside the loop is fragile: any
# unhandled error between the trap and the rm leaks the temp file).
inline_tmp=$(mktemp)
trap 'rm -f "$inline_tmp"' EXIT

# Run one scan pass over a file and feed its records into the merged stream.
# Applies the #[cfg(test)] / `mod tests` skip heuristic and the (file, line,
# match) dedupe BEFORE any counting happens.
# Args: file src_layer kind mode   (kind == mode today; kept separate so the
#   diagnostic label can diverge from the extractor mode without touching calls)
run_pass() {
  local file="$1"
  local src_layer="$2"
  local kind="$3"
  local mode="$4"
  local lineno match key start

  filter_and_match "$mode" < "$file" > "$inline_tmp"

  while IFS=$'\t' read -r lineno match; do
    [[ -z "$lineno" || -z "$match" ]] && continue

    # Skip test-only paths: line is after the first #[cfg(test)]/mod tests
    if (( lineno > first_test_line )); then
      continue
    fi
    # Also skip if the line is directly under a #[cfg(test)] attribute (previous 3 lines)
    start=$(( lineno > 3 ? lineno - 3 : 1))
    if sed -n "${start},$((lineno-1))p" "$file" 2>/dev/null | grep -q -E '#\[cfg\(test\)\]'; then
      continue
    fi

    # Merge point: the same site reached by both passes is counted once. The `use`
    # pass runs first, so a shared site keeps the more informative `use` label.
    key="$file:$lineno:$match"
    if [[ -n "${SEEN_SITE[$key]+set}" ]]; then
      continue
    fi
    SEEN_SITE["$key"]=1

    process_match "$file" "$lineno" "$match" "$kind" "$src_layer"
  done < "$inline_tmp"
}

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

  # --- Pass 1: `use` line scan (runs first so a shared site is reported as a
  #     `use` violation rather than an inline one). ---
  run_pass "$file" "$src_layer" "use" "use"

  # --- Pass 2: inline qualified-path scan. Catches qualified paths in function
  #     bodies, struct fields and trait bounds, which pass 1 cannot see. Records
  #     that overlap pass 1 are collapsed by SEEN_SITE. ---
  run_pass "$file" "$src_layer" "inline" "inline"

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
