#!/usr/bin/env bash
# check_intra_crate_direction.sh — CI gate for the intra-crate Clean Architecture
# layering (issue #990, ADR-0010; issue #995, ADR-0010-A). See
# docs/adr/0010-intra-crate-direction-allowlist.md and its addendum.
#
# === Architecture: the POLICY lives here, the EXTRACTION lives in the tool ===
#
# The gate does not scan source text any more. It consumes the AST extractor:
#
#   intra-crate-lint --root <dir>
#     stdout: <relative-file>\t<line>\t<kind>\t<path>   kind ∈ {use, inline, choke}
#     exit 0  = the whole tree was scanned
#     exit 1  = usage error
#     exit 2  = a file could not be read/parsed (every failure named on stderr)
#
# The extractor REPORTS the sites; this script DECIDES what a violation is. The
# layer set, the allowlist, the verdict criteria and the output format are
# unchanged by the swap — only the way the sites are found changed.
#
# The previous line-oriented text scanner is DELETED (S6). A gate that degrades
# to the weak scanner when the strong one is unavailable is the same fail-open
# being closed, so there is NO fallback path here: if the extractor is missing,
# unusable, or unable to parse a file, the gate fails CLOSED with the file
# named. `scripts/test_intra_crate_direction.sh` pins that absence.
#
# === What the swap closes (audit "scripts fail-open" 2026-10-18) ===
#   B36  the test region is decided STRUCTURALLY on the cfg predicate, so a
#        doc comment that mentions `#[cfg(test)]` blinds nothing, and the
#        production code that FOLLOWS a test module is still scanned.
#   B37  comments and string literals are not text: a `"/*"` literal cannot
#        open a phantom block comment that hides the rest of the file.
#   B38  every path form is reported: `use crate::infra::{A,B}`,
#        `use crate::infra;`, `::*`, `as infra`, `crate::{infra::Foo}` and
#        the spaces around `::`.
#   B39  the port-choke rule is decided on the REFERENCES the extractor emits
#        (any mention of the concrete `ValidatingResolver`, plus the raw
#        client wiring), not on a literal real Rust never emits.
#
# === Extractor binary resolution ===
# In this order, first hit wins:
#   1. $INTRA_CRATE_LINT_BIN
#   2. <repo>/tools/intra-crate-lint/target/release/intra-crate-lint
#   3. <repo>/tools/intra-crate-lint/target/debug/intra-crate-lint
#   4. `intra-crate-lint` on PATH
# (2) and (3) are resolved against the repository root — the script's own
# directory — so the gate behaves the same from any working directory. An
# explicit $INTRA_CRATE_LINT_BIN is used EXCLUSIVELY: when it is set and is
# not an executable file the gate fails closed naming it, because a request
# that cannot be honoured must not silently become a different binary. If no
# candidate exists the gate fails closed naming the tool: there is no
# fallback scanner to degrade into.
#
# === Layer model (unchanged) ===
# `infrastructure → adapters → application → domain`, inward only. A SITE is
# an outward reference (`target_rank < src_rank`) in a layer body. The TARGET
# layer set is the one the previous path matcher recognised:
# infrastructure, adapters, application. `domain` is the innermost layer and
# can never be outward; `cli` is the composition edge (rank -1) and is never
# named by `crate::` inside a library. Both stay out of the target set, so the
# set of layers the gate judges does not change with the scanner.
#
# === cli composition edge (ADR-0012-B 3.H, #1097) ===
# `cli/` is the outermost composition edge (rank -1): it owns construction of
# infrastructure concretes (`StateStore`, `RecordStore`, fetchers) and injects
# them into `application` through domain ports. Because the layering rule is
# `target_rank < src_rank` (outward only), a `cli` source (rank -1) can never
# flag — every inner layer ranks higher. This is deliberate: the gate pins
# `application`/`domain` purity while leaving construction to the edge.
#
# === Allowlist matching (issue #1068, blocker B3) ===
# A pattern absorbs a MODULE PATH only at `::` segment boundaries: the
# character immediately before the occurrence must be `:` or the occurrence
# must start at position 0, and the character immediately after it must be `:`
# or end-of-string. Plain substring matching failed open:
# `infrastructure::crawler::resource_downloader` also absorbed
# `crate::infrastructure::crawler::resource_downloader_v2::EvilThing` and
# `…::resource_downloader_legacy::X`. A per-FILE entry is an explicit,
# reviewed exemption for the whole file, so it keeps substring matching.
#
# === Min-match rule (audit B44) ===
# Every allowlist entry MUST absorb at least one site. An entry that absorbs
# ZERO sites is STALE: it exempts nothing and only hides that the code it was
# written for has moved or is gone. The allowlist is a budget, not a wish
# list — a stale entry is a removal request (the whole point of citing a
# removal condition), so it FAILS in strict mode instead of riding silently
# behind a `::warning::` nobody reads.
#
# === Greenness taxonomy (issue #1100, ADR-0010-B) ===
#   - PERMANENT, counted: a production site goes green ONLY via a named,
#     ADR-reasoned allowlist entry (today exactly the 2 permanent ADR-0011
#     entries: the `application/container.rs` DI root and the
#     `infrastructure::observability` transversal). Each entry cites its
#     removal condition by SYMBOL, never by line number.
#   - DOCUMENTED EXCLUSIONS, never counted: test-only code (a `#[cfg(test)]`
#     region, decided structurally by the extractor), doc/block comments and
#     string literals (not text at all), lateral same-layer references (the
#     rule fires outward only), and the `cli/` composition edge. These stay
#     green by documented rule and are pinned by
#     scripts/test_intra_crate_direction.sh, never by allowlist entries.
# A new outward path in production code that matches neither class fails
# closed (`::error::`, exit 1 in strict mode) — that is the acceptance proof
# of #1100.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# INTRA_CRATE_ROOT / INTRA_CRATE_ALLOWLIST exist so scripts/test_intra_crate_direction.sh
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

# A trailing separator would double up in every reported path.
while [[ "$ROOT" != "/" && "$ROOT" == */ ]]; do
  ROOT="${ROOT%/}"
done

declare -A LAYER_RANK=(
  [cli]=-1
  [infrastructure]=0
  [adapters]=1
  [application]=2
  [domain]=3
)

# The layers a reference may point AT. Same set the previous path matcher
# recognised: `domain` can never be outward, `cli` is never named by `crate::`
# inside a library.
TARGET_LAYERS=(infrastructure adapters application)

# The layer a file belongs to, from its path relative to the scanned root.
layer_of_file() {
  local rel="$1"
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
# Per-entry count of absorbed sites (audit B44). An entry that absorbs ZERO
# sites is STALE: it exempts nothing and is a removal request, not a
# permanent exemption. The counter is what makes that observable.
declare -A ALLOW_MATCHES=()
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
    # Pattern is the first whitespace-separated token of the entry.
    pattern="${trimmed%%[[:space:]]*}"
    ALLOW_PATTERNS+=("$pattern")
    ALLOW_MATCHES[$((${#ALLOW_PATTERNS[@]} - 1))]=0
  done < "$ALLOWLIST"
  if (( ${#ALLOW_PATTERNS[@]} > ALLOWLIST_CAP )); then
    echo "::error::allowlist $ALLOWLIST has ${#ALLOW_PATTERNS[@]} entries, max is $ALLOWLIST_CAP (ADR-0012-B terminal cap — only the two permanent ADR-0011 entries are expected. A new entry needs a removal condition cited by SYMBOL, never by line number, per #1032.)"
    exit 1
  fi
  if (( ${#ALLOW_PATTERNS[@]} >= ALLOWLIST_WARN_AT )); then
    echo "::warning::allowlist has ${#ALLOW_PATTERNS[@]} entries (warn threshold $ALLOWLIST_WARN_AT, hard cap $ALLOWLIST_CAP) — steady state is the 2 permanent ADR-0011 entries; prune before adding, and never re-add a broad module entry (narrow per-symbol entries fail closed, broad ones silently absorb)"
  fi
fi

# --- extractor resolution (no fallback: the gate fails closed) ---
resolve_lint_bin() {
  if [[ -n "${INTRA_CRATE_LINT_BIN:-}" ]]; then
    if [[ -x "$INTRA_CRATE_LINT_BIN" ]]; then
      printf '%s' "$INTRA_CRATE_LINT_BIN"
      return 0
    fi
    echo "::error::INTRA_CRATE_LINT_BIN is set to '$INTRA_CRATE_LINT_BIN' but that path is not an executable file — the gate fails closed (there is no fallback scanner)" >&2
    return 1
  fi
  local candidates=(
    "$REPO_ROOT/tools/intra-crate-lint/target/release/intra-crate-lint"
    "$REPO_ROOT/tools/intra-crate-lint/target/debug/intra-crate-lint"
  )
  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  candidate="$(command -v intra-crate-lint || true)"
  if [[ -n "$candidate" && -x "$candidate" ]]; then
    printf '%s' "$candidate"
    return 0
  fi
  echo "::error::the intra-crate lint extractor was not found — the gate fails closed (there is no fallback scanner). Build it with: cargo build --manifest-path tools/intra-crate-lint/Cargo.toml --locked (searched: ${candidates[*]}, PATH: intra-crate-lint)" >&2
  return 1
}

if ! LINT_BIN="$(resolve_lint_bin)"; then
  exit 1
fi

# --- run the extractor ---
sites_tmp=$(mktemp)
lint_err_tmp=$(mktemp)
trap 'rm -f "$sites_tmp" "$lint_err_tmp"' EXIT

lint_rc=0
"$LINT_BIN" --root "$ROOT" >"$sites_tmp" 2>"$lint_err_tmp" || lint_rc=$?

if [[ $lint_rc -ne 0 ]]; then
  # A tool that cannot scan the whole tree cannot decide anything: fail closed
  # and name every file the extractor could not read or parse.
  echo "::error::the intra-crate lint extractor '$LINT_BIN' failed (exit $lint_rc) — the gate fails closed, no fallback scanner exists"
  while IFS= read -r err_line; do
    [[ -n "$err_line" ]] && echo "::error::$err_line"
  done < "$lint_err_tmp"
  exit 1
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
  local i
  local found=0
  for i in "${!ALLOW_PATTERNS[@]}"; do
    pat="${ALLOW_PATTERNS[$i]}"
    # FILE PATHS are not module paths: a per-file entry is an explicit, reviewed
    # exemption for the whole file (`application/container.rs` in ADR-0010 §2), so
    # substring matching is kept here ON PURPOSE. Segment anchoring applies to
    # module paths only, where `foo` and `foo_v2` are genuinely different modules.
    if [[ "$file" == *"$pat"* ]]; then
      ALLOW_MATCHES[$i]=$(( ${ALLOW_MATCHES[$i]:-0} + 1 ))
      found=1
      continue
    fi
    if path_segment_contains "$match" "$pat"; then
      ALLOW_MATCHES[$i]=$(( ${ALLOW_MATCHES[$i]:-0} + 1 ))
      found=1
      continue
    fi
    if path_segment_contains "$target" "$pat"; then
      ALLOW_MATCHES[$i]=$(( ${ALLOW_MATCHES[$i]:-0} + 1 ))
      found=1
      continue
    fi
  done
  return $(( found == 0 ? 1 : 0 ))
}

# Classify the target layer of a reported path. The path is `crate::<seg>::…`
# or the bare module form `crate::<seg>` (the module itself, which the
# previous matcher could not see at all — B38).
classify_target() {
  local match="$1"
  if [[ "$match" =~ ^crate::([a-z_]+)(::|$) ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
    return 0
  fi
}

# Whether a target layer is one the gate judges.
is_target_layer() {
  local layer="$1"
  local candidate
  for candidate in "${TARGET_LAYERS[@]}"; do
    if [[ "$candidate" == "$layer" ]]; then
      return 0
    fi
  done
  return 1
}

violations=0
allowlisted_count=0

# Dedupe set for (file, line, path) — issue #1068, blocker B1. The `use` sites
# and the inline sites overlap (a `use` statement also carries a qualified
# path), so without this the reported number is hits rather than distinct
# violation sites.
declare -A SEEN_SITE=()

# Process a single reported path record and update the violation/allowlist
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
  is_target_layer "$target_layer" || return 0

  local src_rank="${LAYER_RANK[$src_layer]}"
  local target_rank="${LAYER_RANK[$target_layer]}"

  # The rule is `target_rank < src_rank`: OUTWARD only. A lateral reference
  # (target_rank == src_rank, e.g. infrastructure -> infrastructure) is
  # intentionally NOT flagged. That is the known blind spot behind #1060/#1061
  # and is out of scope for this gate; scripts/test_intra_crate_direction.sh pins it so it
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

# Run one pass over the reported records of a given kind.
# The `use` pass runs first, so a site reached by both keeps the more
# informative `use` label — the same order the previous two-pass scanner used.
# Args: kind
run_pass() {
  local kind="$1"
  local rel lineno rec_kind match file src_layer key

  while IFS=$'\t' read -r rel lineno rec_kind match; do
    [[ -z "$rel" || -z "$lineno" || -z "$match" ]] && continue
    [[ "$rec_kind" == "$kind" ]] || continue

    file="$ROOT/$rel"
    if ! src_layer=$(layer_of_file "$rel"); then
      continue
    fi

    # Merge point: the same site reached by both passes is counted once.
    key="$file:$lineno:$match"
    if [[ -n "${SEEN_SITE[$key]+set}" ]]; then
      continue
    fi
    SEEN_SITE["$key"]=1

    process_match "$file" "$lineno" "$match" "$kind" "$src_layer"
  done < "$sites_tmp"
}

run_pass "use"
run_pass "inline"

# === Port-adoption choke point (issue #1061) ===
# Layer direction is necessary but not sufficient: a port owns an invariant and
# must be the ONLY path to that effect. `domain::ssrf_guard::SsrfGuard` (whose
# `secure_client` impl lives in `infrastructure/ssrf.rs` — domain types cannot
# reference infrastructure I/O) is the single choke point composing both SSRF
# layers (`redirect_policy()` + `ValidatingResolver`). The direction gate scores
# infrastructure → infrastructure as legal, so a hand-wired
# `.redirect(redirect_policy()).dns_resolver(ValidatingResolver::new())` client
# (the 4 sites #1060 found, migrated to the guard in the RC-3 slices) passes
# direction while bypassing the port. This pass fails closed on any raw
# client-wiring primitive outside the two guard modules: a new surface must call
# `ssrf_guard()` / `secure_client()` instead of wiring its own resolver or
# redirect policy.
#
# The constructs are the ones the extractor reports: the raw client wiring
# methods and any mention of the concrete `ValidatingResolver` — decided on the
# REFERENCE, not on the text of a line (B39). A mention inside a test region is
# not a construct at all: the extractor does not report it.
CHOKE_OWNERS=("domain/ssrf_guard.rs" "infrastructure/ssrf.rs")
declare -A SEEN_CHOKE=()
while IFS=$'\t' read -r rel lineno rec_kind match; do
  [[ -z "$rel" || -z "$lineno" ]] && continue
  [[ "$rec_kind" == "choke" ]] || continue

  file="$ROOT/$rel"
  is_owner=0
  for owner in "${CHOKE_OWNERS[@]}"; do
    [[ "$file" == *"$owner" ]] && is_owner=1
  done
  (( is_owner )) && continue

  # One violation per offending line, whatever the number of constructs on it.
  if [[ -n "${SEEN_CHOKE[$file:$lineno]+set}" ]]; then
    continue
  fi
  SEEN_CHOKE["$file:$lineno"]=1

  violations=$((violations + 1))
  if [[ "$MODE" == "strict" ]]; then
    echo "::error::$file:$lineno: raw HTTP client wiring outside the SsrfGuard choke point (port-adoption violation, #1061) — call ssrf_guard()/secure_client() instead"
  else
    echo "::warning::$file:$lineno: raw HTTP client wiring outside the SsrfGuard choke point (port-adoption violation, #1061)"
  fi
done < "$sites_tmp"

if [[ -f "$ALLOWLIST" ]]; then
  echo "allowlisted $allowlisted_count (max $ALLOWLIST_CAP, file: $ALLOWLIST, entries: ${#ALLOW_PATTERNS[@]})"
fi

# === Stale allowlist entries (audit B44) ===
# An entry that absorbed ZERO sites is STALE: it exempts nothing, it only
# hides the fact that the code it was written for has moved or is gone. The
# allowlist is a budget, not a wish list: a stale entry is a removal request
# (the whole point of citing a removal condition), so it FAILS in strict mode
# instead of riding silently behind a `::warning::` that nobody reads.
STALE_ENTRIES=0
if [[ -f "$ALLOWLIST" ]]; then
  for i in "${!ALLOW_PATTERNS[@]}"; do
    if [[ "${ALLOW_MATCHES[$i]:-0}" -eq 0 ]]; then
      STALE_ENTRIES=$((STALE_ENTRIES + 1))
      if [[ "$MODE" == "strict" ]]; then
        echo "::error::allowlist entry absorbed 0 sites (stale entry — a removal request, not a permanent exemption): ${ALLOW_PATTERNS[$i]} (file: $ALLOWLIST)"
      else
        echo "::warning::allowlist entry absorbed 0 sites (stale entry): ${ALLOW_PATTERNS[$i]} (file: $ALLOWLIST)"
      fi
    fi
  done
fi

if [[ "$MODE" == "strict" ]]; then
  if [[ $violations -eq 0 && $STALE_ENTRIES -eq 0 ]]; then
    echo "OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)"
    exit 0
  fi
  if [[ $violations -ne 0 ]]; then
    echo "::error::found $violations intra-crate direction violation(s) (ADR-0010, strict mode)"
  fi
  if [[ $STALE_ENTRIES -ne 0 ]]; then
    echo "::error::found $STALE_ENTRIES stale allowlist entry(ies) (ADR-0010, strict mode — every entry must absorb at least one site)"
  fi
  exit 1
else
  echo "OK (warn): $violations intra-crate direction violation(s) reported (ADR-0010, warn mode — flip INTRA_CRATE_MODE=strict after the follow-up slice lands)"
  exit 0
fi
