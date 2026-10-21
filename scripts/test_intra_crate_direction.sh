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
#
# NEGATION: the gate's own semantics must be pinned HERE, not by allowlist
# entries. Before this harness ran in CI, the four fail-open defects of the
# gate (audit "scripts fail-open" 2026-10-18, B36-B39: a doc comment that
# mentions `#[cfg(test)]` truncating the scan, a string literal containing
# `/*` blinding the rest of the file, five invisible `use` path forms, and a
# choke-point pattern that real Rust never emits) survived for months
# because nothing in CI executed these cases.
#
# === The gate consumes an AST extractor (Ola 3, S1-S6) ===
# The gate no longer scans source text: it consumes the AST extractor
# (`tools/intra-crate-lint`) and applies the SAME policy to the sites it
# reports. This harness therefore also pins the SWAP itself:
#
#   - the extractor absent        => the gate fails closed, naming it;
#   - the extractor unable to parse a file => the gate fails closed, naming
#     the file;
#   - the six path forms of B38    => every one of them is a violation;
#   - the test region is decided STRUCTURALLY (a doc comment mentioning
#     `#[cfg(test)]` blinds nothing; production code after a test module is
#     still scanned);
#   - the choke rule decides on the REFERENCES (any mention of the concrete
#     `ValidatingResolver`);
#   - the stale-allowlist rule (B44) still fails the gate;
#   - NO text scanner is left in the gate: there is no fallback path.
#
# Every fixture lives in a mktemp tree: this harness asserts the gate's
# DECISIONS on scratch inputs and never pins a value measured against the
# repository tree (a count of sites in the real tree rots with every change
# to the tree — the acceptance of the swap is a live comparison of the two
# implementations over the same tree, not a remembered number).

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

# run_gate_at <root> <allowlist-file> -> prints gate stdout+stderr, exits with
# the gate status
run_gate_at() {
  INTRA_CRATE_ROOT="$1" INTRA_CRATE_ALLOWLIST="$2" bash "$GATE" 2>&1 || true
}

# A module-path entry needs an ADR reason or the gate rejects the entry itself.
REASONED_ALLOW="$T/reasoned.txt"
printf '%s\n' 'infrastructure::crawler::resource_downloader # ADR-0010 segment-anchor test' > "$REASONED_ALLOW"
EMPTY_ALLOW="$T/empty.txt"
: > "$EMPTY_ALLOW"

# ---------------------------------------------------------------------------
# 0. Preflight: the gate consumes the AST extractor. If it is not available
# every case below would fail for the wrong reason, so say so once — with the
# build command — instead of reporting a wall of red.
# ---------------------------------------------------------------------------
PREFLIGHT_ROOT="$T/preflight_root"
mkdir -p "$PREFLIGHT_ROOT"
preflight_out="$(run_gate_at "$PREFLIGHT_ROOT" "$EMPTY_ALLOW")"
if printf '%s\n' "$preflight_out" | grep -q "extractor was not found\|INTRA_CRATE_LINT_BIN is set"; then
  echo "FAIL: the intra-crate lint extractor is not available — the gate cannot run."
  echo "      build it with: cargo build --manifest-path tools/intra-crate-lint/Cargo.toml --locked"
  exit 1
fi

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

# ---------------------------------------------------------------------------
# 9. Inline qualified path in a function body fails closed in strict mode
# ---------------------------------------------------------------------------
printf 'pub fn probe() { let _ = crate::infrastructure::probe::Probe; }\n' \
  > "$T/application/body_probe.rs"
if INTRA_CRATE_ROOT="$T" INTRA_CRATE_ALLOWLIST="$EMPTY_ALLOW" INTRA_CRATE_MODE=strict \
bash "$GATE" >"$T/body_probe.out" 2>&1; then
  rc=0
else
  rc=1
fi
if [ "$rc" = "1" ] && grep -qE "::error::.*application/body_probe\.rs:[0-9]+" "$T/body_probe.out"; then
  ok "inline body path fails closed in strict mode (exit 1, names file:line)"
else
  bad "inline body path did not fail closed: exit=$rc"
fi
rm -f "$T/application/body_probe.rs"

# ---------------------------------------------------------------------------
# 10. Lateral `pub use` shim inside application/ stays green
# ---------------------------------------------------------------------------
printf 'pub use crate::application::ports::Thing;\n' \
  > "$T/application/shim.rs"
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "shim.rs"; then
  bad "lateral application -> application pub use shim got flagged"
else
  ok "lateral pub use shim stays green"
fi
rm -f "$T/application/shim.rs"

# ---------------------------------------------------------------------------
# 11. The same probe path under cli/ stays green (composition edge, #1097)
# ---------------------------------------------------------------------------
mkdir -p "$T/cli"
printf 'pub fn probe() { let _ = crate::infrastructure::probe::Probe; }\n' \
  > "$T/cli/compose.rs"
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "compose.rs"; then
  bad "cli/ composition edge got flagged — rank -1 must never flag"
else
  ok "same probe path under cli/ stays green (consumes #1097, re-decides nothing)"
fi
rm -f "$T/cli/compose.rs"

# ---------------------------------------------------------------------------
# 12. Doc-comment paths are ignored
# ---------------------------------------------------------------------------
printf '/// See crate::infrastructure::probe::Probe for details.\n// crate::infrastructure::probe::Probe\npub fn documented() {}\n' \
  > "$T/application/docced.rs"
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "docced.rs"; then
  bad "doc-comment path got flagged — // lines are not code (the extractor drops them)"
else
  ok "doc-comment path ignored"
fi
rm -f "$T/application/docced.rs"

# ---------------------------------------------------------------------------
# 13. The extractor is ABSENT => the gate fails closed, naming it (S6).
#     There is no fallback scanner: a gate that degrades to the weak scanner
#     when the strong one is unavailable is the same fail-open being closed.
# ---------------------------------------------------------------------------
OUT="$(INTRA_CRATE_ROOT="$T" INTRA_CRATE_ALLOWLIST="$EMPTY_ALLOW" \
      INTRA_CRATE_LINT_BIN="$T/no-such-extractor" bash "$GATE" 2>&1)" && RC=0 || RC=$?
if [ "$RC" -ne 0 ] && printf '%s\n' "$OUT" | grep -q "no-such-extractor"; then
  ok "an absent extractor fails the gate closed and names the missing binary (rc=$RC)"
else
  bad "an absent extractor did not fail closed with a named error (rc=$RC): $OUT"
fi

# The same, with NO candidate at all: an isolated copy of the gate (so the
# repository-local build paths do not exist) and a PATH that carries no
# `intra-crate-lint`.
MINBIN="$T/minbin"
mkdir -p "$MINBIN"
for tool in bash env mktemp sed grep rm dirname; do
  tool_path="$(command -v "$tool" || true)"
  if [ -n "$tool_path" ]; then
    ln -sf "$tool_path" "$MINBIN/$tool"
  fi
done
ISOLATED="$T/isolated"
mkdir -p "$ISOLATED/scripts"
cp "$GATE" "$ISOLATED/scripts/check_intra_crate_direction.sh"
# `env -u` so an INTRA_CRATE_LINT_BIN inherited from the environment cannot
# make this case find a binary: the point is that NO candidate exists.
OUT="$(cd "$T" && env -u INTRA_CRATE_LINT_BIN PATH="$MINBIN" INTRA_CRATE_ROOT="$T" \
      INTRA_CRATE_ALLOWLIST="$EMPTY_ALLOW" \
      bash "$ISOLATED/scripts/check_intra_crate_direction.sh" 2>&1)" && RC=0 || RC=$?
if [ "$RC" -ne 0 ] && printf '%s\n' "$OUT" | grep -q "extractor was not found"; then
  ok "with no extractor anywhere the gate fails closed and says so (rc=$RC)"
else
  bad "with no extractor anywhere the gate did not fail closed (rc=$RC): $OUT"
fi

# ---------------------------------------------------------------------------
# 14. The extractor cannot PARSE a file => the gate fails closed, NAMING the
#     file (exit 2 of the extractor contract).
# ---------------------------------------------------------------------------
PARSE_ROOT="$T/parse_root"
mkdir -p "$PARSE_ROOT/application"
printf 'pub fn broken( {\n' > "$PARSE_ROOT/application/broken.rs"
rc=0
INTRA_CRATE_ROOT="$PARSE_ROOT" INTRA_CRATE_ALLOWLIST="$EMPTY_ALLOW" \
  bash "$GATE" >"$T/gate_parse.out" 2>&1 || rc=$?
out="$(<"$T/gate_parse.out")"
if [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q "broken.rs"; then
  ok "an unparseable file fails the gate closed and names the file (rc=$rc)"
else
  bad "an unparseable file did not fail closed naming the file (rc=$rc): $out"
fi
if printf '%s\n' "$out" | grep -q "^OK:"; then
  bad "the gate reported OK on a tree it could not fully scan"
else
  ok "the gate does not report OK on a tree it could not fully scan"
fi
rm -rf "$PARSE_ROOT"

# ---------------------------------------------------------------------------
# 15. The six path forms of B38 are violations (the previous matcher saw
#     none of them: a brace group, the bare module, a glob, an alias, a
#     rooted group and the spaces around `::`).
# ---------------------------------------------------------------------------
b38_forms=(
  "use crate::infrastructure::probe::{Alpha, Beta};"
  "use crate::infrastructure;"
  "use crate::infrastructure::*;"
  "use crate::infrastructure as infra;"
  "use crate::{infrastructure::probe::Alpha};"
  "use crate :: infrastructure :: probe :: Alpha;"
)
b38_index=0
for b38_form in "${b38_forms[@]}"; do
  b38_index=$((b38_index + 1))
  printf '%s\n' "$b38_form" > "$T/application/form$b38_index.rs"
  out="$(run_gate "$EMPTY_ALLOW")"
  if printf '%s\n' "$out" | grep -q "::error::.*application/form$b38_index\.rs:"; then
    ok "B38 form $b38_index is a violation: $b38_form"
  else
    bad "B38 form $b38_index was NOT reported: $b38_form"
    printf '%s\n' "$out" | grep "::error::" | sed 's/^/      /' || true
  fi
  rm -f "$T/application/form$b38_index.rs"
done

# ---------------------------------------------------------------------------
# 16. The test region is decided STRUCTURALLY (B36): a doc comment that
#     mentions `#[cfg(test)]` blinds nothing, and the production code that
#     FOLLOWS a test module is still scanned.
# ---------------------------------------------------------------------------
cat > "$T/application/doc_trap.rs" <<'EOF'
//! Handler layer. The `#[cfg(test)]` below marks the test module.
use crate::infrastructure::probe::Alpha;
EOF
cat > "$T/application/test_region.rs" <<'EOF'
pub fn before() {}

#[cfg(test)]
mod tests {
    use crate::infrastructure::probe::Alpha;
}

pub fn after() {
    let _ = crate::infrastructure::probe::Beta;
}
EOF
out="$(run_gate "$EMPTY_ALLOW")"
doc_line=$(grep -n "probe::Alpha" "$T/application/doc_trap.rs" | cut -d: -f1)
inside_line=$(grep -n "probe::Alpha" "$T/application/test_region.rs" | cut -d: -f1)
after_line=$(grep -n "probe::Beta" "$T/application/test_region.rs" | cut -d: -f1)
if printf '%s\n' "$out" | grep -q "::error::.*application/doc_trap\.rs:$doc_line:"; then
  ok "a doc comment mentioning #[cfg(test)] blinds nothing (line $doc_line is reported)"
else
  bad "a doc comment mentioning #[cfg(test)] still truncates the scan"
fi
if printf '%s\n' "$out" | grep -q "application/test_region\.rs:$inside_line:"; then
  bad "a site inside a #[cfg(test)] module was reported (line $inside_line)"
else
  ok "a site inside a #[cfg(test)] module is not reported (line $inside_line)"
fi
if printf '%s\n' "$out" | grep -q "::error::.*application/test_region\.rs:$after_line:"; then
  ok "production code after a test module is still scanned (line $after_line is reported)"
else
  bad "production code after a test module is not scanned (line $after_line missing)"
fi
rm -f "$T/application/doc_trap.rs" "$T/application/test_region.rs"

# ---------------------------------------------------------------------------
# 17. The port-choke rule (B39) decides on the REFERENCES: any mention of the
#     concrete `ValidatingResolver` outside the guard modules is a violation.
# ---------------------------------------------------------------------------
cat > "$T/application/choke.rs" <<'EOF'
pub fn build(resolver: ValidatingResolver) -> Client {
    ValidatingResolver::new()
}
EOF
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "::error::.*application/choke\.rs:1:.*choke point"; then
  ok "a mention of ValidatingResolver is a port-adoption violation"
else
  bad "a mention of ValidatingResolver was not reported: $out"
fi
cat > "$T/application/choke_path.rs" <<'EOF'
pub fn build() {
    let _ = crate::infrastructure::ssrf::ValidatingResolver::new();
}
EOF
out="$(run_gate "$EMPTY_ALLOW")"
if printf '%s\n' "$out" | grep -q "::error::.*application/choke_path\.rs:2:.*choke point"; then
  ok "a qualified path mentioning ValidatingResolver is a port-adoption violation"
else
  bad "a qualified path mentioning ValidatingResolver was not reported: $out"
fi
rm -f "$T/application/choke.rs" "$T/application/choke_path.rs"

# ---------------------------------------------------------------------------
# 18. The stale-allowlist rule (B44) still fails the gate: an entry that
#     absorbs ZERO sites is a removal request, not a permanent exemption.
# ---------------------------------------------------------------------------
LIVE_ROOT="$T/stale_live_root"
mkdir -p "$LIVE_ROOT/application"
printf 'use crate::infrastructure::evil::direct as _;\n' > "$LIVE_ROOT/application/new_file.rs"
printf '%s\n' 'infrastructure::evil::direct # ADR-3000 live entry, removal condition: repro' > "$T/live_entry.txt"
rc=0
INTRA_CRATE_ROOT="$LIVE_ROOT" INTRA_CRATE_ALLOWLIST="$T/live_entry.txt" \
  bash "$GATE" >"$T/gate_live.out" 2>&1 || rc=$?
out="$(<"$T/gate_live.out")"
if [ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q "allowlisted 1"; then
  ok "a live allowlist entry still absorbs its site and keeps the gate green"
else
  bad "a live allowlist entry stopped working (rc=$rc): $out"
fi
STALE_ROOT="$T/stale_root"
mkdir -p "$STALE_ROOT/application"
printf '%s\n' 'nonexistent::subsystem::path # ADR-9999 stale entry, removal condition: repro' > "$T/stale_entry.txt"
rc=0
INTRA_CRATE_ROOT="$STALE_ROOT" INTRA_CRATE_ALLOWLIST="$T/stale_entry.txt" \
  bash "$GATE" >"$T/gate_stale.out" 2>&1 || rc=$?
out="$(<"$T/gate_stale.out")"
if [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q "nonexistent::subsystem::path"; then
  ok "a stale allowlist entry (zero sites) fails the gate and is named (rc=$rc)"
else
  bad "a stale allowlist entry passed silently (rc=$rc): $out"
fi
if printf '%s\n' "$out" | grep -q "absorbed 0 sites"; then
  ok "the stale entry is reported as an error, not as a warning"
else
  bad "the stale entry is not reported as an error: $out"
fi

# ---------------------------------------------------------------------------
# 19. NO text scanner is left in the gate (S6): the AST extractor is the only
#     path, and the gate still decides (it was not neutered by the swap).
# ---------------------------------------------------------------------------
if grep -qiE '(^|[^[:alnum:]_])awk([^[:alnum:]_]|$)' "$GATE"; then
  bad "the gate still contains a text-scanner invocation — there must be no fallback"
  grep -niE '(^|[^[:alnum:]_])awk([^[:alnum:]_]|$)' "$GATE" | sed 's/^/      /' || true
else
  ok "the gate contains no text scanner: the AST extractor is the only path"
fi
if grep -q "filter_and_match" "$GATE"; then
  bad "the deleted scanner is still present in the gate"
else
  ok "the deleted scanner is gone from the gate"
fi
if ! grep -q "intra-crate-lint" "$GATE"; then
  bad "the gate does not consume the AST extractor"
else
  ok "the gate consumes the AST extractor"
fi
if printf '%s\n' "$out" | grep -q "^OK:"; then
  bad "the gate reported OK on a tree with a stale allowlist entry"
else
  ok "the gate still decides (it is not neutered by the swap)"
fi

echo
if [ "$fail" = "0" ]; then
  echo "All intra-crate gate semantics checks passed."
  exit 0
fi
echo "Some intra-crate gate semantics checks FAILED."
exit 1
