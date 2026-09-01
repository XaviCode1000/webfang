#!/usr/bin/env bash
set -euo pipefail
# Intra-crate direction gate — semantics harness (RED before GREEN)
#
# Pins the three behaviors that ADR-0012-B's "narrow broad entries before
# deleting them" strategy depends on, plus the two properties it must NOT
# accidentally change. Issue #1068.
#
#   1. the reported count is distinct violation SITES, not regex hits
#   2. a brace import yields one site per symbol, so a narrow entry can name
#      the symbol it exempts
#   3. allowlist matching on module paths is anchored at segment boundaries,
#      so `foo` never absorbs `foo_v2`
#   4. a genuine unallowlisted outward violation still fails closed
#   5. lateral infrastructure -> infrastructure is still NOT flagged — existing
#      intended behavior (rank rule), the known blind spot behind #1060/#1061.
#      This test documents it so nobody "fixes" it by accident.
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures live in a mktemp tree and are reached through INTRA_CRATE_ROOT /
# INTRA_CRATE_ALLOWLIST, so this harness never reads or writes the repository.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_intra_crate_direction.sh"
fail=0
ok() { echo "OK: $1"; }
bad() { echo "FAIL: $1"; fail=1; }

[ -f "$GATE" ] || { echo "FAIL: gate script not found at $GATE"; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/application" "$T/infrastructure" "$T/domain"

# run_gate <allowlist-file> -> prints gate stdout+stderr, exits with gate status
run_gate() {
  INTRA_CRATE_ROOT="$T" INTRA_CRATE_ALLOWLIST="$1" bash "$GATE" 2>&1 || true
}

# count_sites <output> <file-basename> -> number of ::error:: rows naming that file
count_sites() { printf '%s\n' "$1" | grep -c "::error::.*$2" || true; }

# A module-path entry needs an ADR reason or the gate rejects the entry itself.
REASONED_ALLOW="$T/reasoned.txt"
printf '%s\n' 'infrastructure::crawler::resource_downloader # ADR-0010 segment-anchor test' > "$REASONED_ALLOW"
EMPTY_ALLOW="$T/empty.txt"
: > "$EMPTY_ALLOW"

# ---------------------------------------------------------------------------
# 1. Brace imports expand to one site per symbol
# ---------------------------------------------------------------------------
printf 'use crate::infrastructure::crawler::{SitemapConfig, SitemapError, SitemapParser};\n' \
  > "$T/application/brace.rs"
out="$(run_gate "$EMPTY_ALLOW")"
n="$(count_sites "$out" brace.rs)"
if [ "$n" = "3" ]; then
  ok "brace import yields 3 distinct sites, not 1 truncated match"
else
  bad "brace import: expected 3 sites, got $n"
  printf '%s\n' "$out" | grep "::error::" | sed 's/^/      /' || true
fi

# the truncated bare form must be gone
if printf '%s\n' "$out" | grep -qE '— crate::infrastructure::crawler$'; then
  bad "brace import still emits the truncated bare module path"
else
  ok "no truncated bare-module record emitted for a brace import"
fi

# ---------------------------------------------------------------------------
# 2. A plain `use` line is counted once, not twice
# ---------------------------------------------------------------------------
printf 'use crate::infrastructure::export::StateStore;\n' > "$T/application/single.rs"
out="$(run_gate "$EMPTY_ALLOW")"
n="$(count_sites "$out" single.rs)"
if [ "$n" = "1" ]; then
  ok "a use line is counted once (was double-counted: 84 rows for 52 sites)"
else
  bad "single use line: expected 1 site, got $n"
fi

# ---------------------------------------------------------------------------
# 3. Segment-boundary matching: narrow entry does not absorb longer siblings
# ---------------------------------------------------------------------------
printf 'pub fn h() { let _ = crate::infrastructure::crawler::resource_downloader_v2::EvilThing; }\n' \
  > "$T/application/prefix.rs"
printf 'pub fn i() { let _ = crate::infrastructure::crawler::resource_downloader::ResourceDownloader; }\n' \
  > "$T/application/exact.rs"
out="$(run_gate "$REASONED_ALLOW")"

if printf '%s\n' "$out" | grep -q "prefix.rs"; then
  ok "narrow entry does NOT absorb resource_downloader_v2 (fails closed)"
else
  bad "prefix shadowing: resource_downloader_v2 was absorbed by the resource_downloader entry"
fi

if printf '%s\n' "$out" | grep -q "exact.rs"; then
  bad "exact segment should be absorbed by the narrow entry, but it was reported"
else
  ok "narrow entry DOES absorb resource_downloader::ResourceDownloader"
fi

# ---------------------------------------------------------------------------
# 4. A genuine unallowlisted violation fails closed and names file:line
# ---------------------------------------------------------------------------
printf 'pub fn j() { let _ = crate::infrastructure::nowhere::Thing; }\n' \
  > "$T/application/real.rs"
if INTRA_CRATE_ROOT="$T" INTRA_CRATE_ALLOWLIST="$REASONED_ALLOW" bash "$GATE" >"$T/strict.out" 2>&1; then
  rc=0
else
  rc=1
fi
if [ "$rc" = "1" ] && grep -qE "::error::.*application/real\.rs:[0-9]+" "$T/strict.out"; then
  ok "unallowlisted outward violation exits 1 and names file:line"
else
  bad "fail-closed broken: exit=$rc, no file:line in diagnostics"
fi

# ---------------------------------------------------------------------------
# 5. Lateral infrastructure -> infrastructure is NOT flagged (intended)
# ---------------------------------------------------------------------------
printf 'use crate::infrastructure::crawler::UrlQueue;\n' \
  > "$T/infrastructure/lateral.rs"
rm -f "$T/application/real.rs" "$T/application/brace.rs" "$T/application/single.rs" \
      "$T/application/prefix.rs" "$T/application/exact.rs"
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "lateral.rs"; then
  bad "lateral infra->infra got flagged — this changes established gate semantics"
else
  ok "lateral infrastructure -> infrastructure still not flagged (blind spot is #1061's job, not this gate's)"
fi

# ---------------------------------------------------------------------------
# 6. Trailing comma must not invent an empty symbol
# ---------------------------------------------------------------------------
printf 'use crate::infrastructure::export::{StateStore, RecordStore,};\n' \
  > "$T/application/trailing.rs"
out="$(run_gate "$EMPTY_ALLOW")"
n="$(count_sites "$out" trailing.rs)"
if [ "$n" = "2" ]; then
  ok "trailing comma yields 2 sites, not an empty third"
else
  bad "trailing comma: expected 2 sites, got $n"
fi

# ---------------------------------------------------------------------------
# 7. An allowlist entry without an ADR reason is rejected
# ---------------------------------------------------------------------------
printf '%s\n' 'infrastructure::export::StateStore' > "$T/unreasoned.txt"
out="$(run_gate "$T/unreasoned.txt")"
if printf '%s\n' "$out" | grep -qi "must contain ADR reason"; then
  ok "allowlist entry without an ADR reason is rejected (#1032 prose rot)"
else
  bad "unreasoned allowlist entry was accepted"
fi

# ---------------------------------------------------------------------------
# 8. Inward imports are never flagged
# ---------------------------------------------------------------------------
rm -rf "$T/application"; mkdir -p "$T/application"
printf 'use crate::domain::crawler_port::UrlSource;\n' > "$T/domain/inward.rs"
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "inward.rs"; then
  bad "domain -> crate::domain was flagged — inward direction must always pass"
else
  ok "crate::domain imports are not flagged"
fi

echo
if [ "$fail" = "0" ]; then
  echo "All intra-crate gate semantics checks passed."
  exit 0
fi
echo "Some intra-crate gate semantics checks FAILED."
exit 1
