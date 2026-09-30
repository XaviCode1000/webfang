#!/usr/bin/env bash
#
# check_mcp_public_surface.sh — the version-marker half of issue #1614's
# compatibility gate.
#
# The Rust test (`crates/webfang_mcp/tests/public_surface_policy_test.rs`)
# already fails when the live advertised schema stops matching the committed
# fixture. What a test cannot see is whether the author then MOVED the
# contract_version marker — both live in the same test's reach, so re-recording
# the fixture answers "did the version move?" with "yes" automatically.
#
# So the marker lives in its own one-line file
# (`crates/webfang_mcp/tests/fixtures/mcp_public_surface.version`), and this
# guard compares BOTH files against their committed state at the merge-base
# with main:
#
#   surface changed + version unchanged  -> FAIL, with each changed row
#                                          classified per policy §3.1
#   surface unchanged + version changed  -> FAIL, a gratuitous bump
#   both changed                         -> OK (the decision was made)
#   neither changed                      -> OK
#
# A version bump is therefore never silent, and never automatic.
#
# Usage:
#   scripts/check_mcp_public_surface.sh [--base <ref>] [--record]
#
#   --base <ref>   compare against <ref>. The default is the last commit that
#                  touched the fixture OR the marker, which is the semantic
#                  that actually matters: the guard is about the change sitting
#                  in front of you, not about the whole branch. A merge-base
#                  with main is wrong for a feature branch whose fixture was
#                  introduced mid-stack -- every row would read as "added" and
#                  the guard would go blind after the introducing commit.
#   --record       re-derive the fixture from the live router first, then
#                  run the same check. Recording NEVER touches the version
#                  marker, so a breaking change recorded this way stays red
#                  until a human edits the marker on purpose.
#
# Exit: 0 clean · 1 policy violation · 2 cannot run (no git, no fixture).
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo ".")"
FIXTURE="crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv"
MARKER="crates/webfang_mcp/tests/fixtures/mcp_public_surface.version"
BASE=""
RECORD=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base) BASE="${2:?missing value for --base}"; shift 2 ;;
    --record) RECORD=1; shift ;;
    -h | --help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

fail() { echo "::error::$*"; exit 1; }
note() { echo "mcp-public-surface: $*"; }

[[ -f "$ROOT/$FIXTURE" ]] || fail "public-surface fixture missing: $FIXTURE"
[[ -f "$ROOT/$MARKER" ]] || fail "contract_version marker missing: $MARKER"

if ! command -v git >/dev/null 2>&1; then
  note "git unavailable; the version-marker half cannot run. The fixture test still holds the line."
  exit 2
fi

# --- resolve the base -------------------------------------------------------
# Default: the last commit that touched either tracked file. See --help for why
# a merge-base is the wrong default here.
if [[ -z "$BASE" ]]; then
  BASE="$(git -C "$ROOT" log -1 --format=%H -- "$FIXTURE" "$MARKER" 2>/dev/null || true)"
fi
[[ -n "$BASE" ]] || fail "no base commit touches $FIXTURE or $MARKER yet; pass --base <ref>"

# --- optional re-record -----------------------------------------------------
if [[ "$RECORD" == "1" ]]; then
  note "recording the fixture from the live router (contract_version untouched)"
  if ! (cd "$ROOT" && WEBFANG_MCP_SURFACE_RECORD=1 cargo test -p webfang_mcp \
    --test public_surface_policy_test 2>&1 | tail -5); then
    fail "recording run failed; the fixture was not updated"
  fi
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

git -C "$ROOT" show "$BASE:$FIXTURE" >"$WORKDIR/base.tsv" 2>/dev/null || : >"$WORKDIR/base.tsv"
git -C "$ROOT" show "$BASE:$MARKER" >"$WORKDIR/base.ver" 2>/dev/null || : >"$WORKDIR/base.ver"
cp "$ROOT/$FIXTURE" "$WORKDIR/cur.tsv"
cp "$ROOT/$MARKER" "$WORKDIR/cur.ver"

# Bootstrap: the base commit predates the fixture, so there is nothing to
# compare against and "everything is new" is not a finding. Reported explicitly
# so the first run cannot be mistaken for a green gate on a real change.
if [[ ! -s "$WORKDIR/base.tsv" ]]; then
  note "OK: no fixture at $BASE — this is the bootstrap commit, nothing to compare."
  exit 0
fi

# --- flatten each fixture into sorted, comparable identity lines -------------
# `kind<TAB>identity<TAB>field=value ...` — one line per compatibility-bearing
# datum, so a diff of two such lists is a list of moved DATA, not of moved text.
# The layout is generated and uniform, so a flat section-aware reader is a
# reader with fewer ways to be wrong than a general parser would be.
flatten_sectioned() {
  awk -F'\t' '
    /^[[:space:]]*#/ { next }
    /^[[:space:]]*$/ { next }
    /^\[/ { section = substr($0, 2, length($0) - 2); next }
    section == "tools" {
      printf "tool\t%s\tadditionalProperties=%s\n", $1, $2; next
    }
    section == "properties" {
      printf "prop\t%s/%s\trequired=%s type=%s default=%s min=%s max=%s\n", \
        $1, $2, $3, $4, $5, $6, $7; next
    }
    section == "defs" {
      printf "def\t%s/%s\tconsts=%s\n", $1, $2, $3; next
    }
  ' "$1"
}

flatten_sectioned "$WORKDIR/base.tsv" | LC_ALL=C sort >"$WORKDIR/base.flat"
flatten_sectioned "$WORKDIR/cur.tsv" | LC_ALL=C sort >"$WORKDIR/cur.flat"

if diff -q "$WORKDIR/base.flat" "$WORKDIR/cur.flat" >/dev/null; then
  SURFACE_CHANGED=0
else
  SURFACE_CHANGED=1
fi

BASE_VER="$(tr -d '[:space:]' <"$WORKDIR/base.ver")"
CUR_VER="$(tr -d '[:space:]' <"$WORKDIR/cur.ver")"
[[ -n "$CUR_VER" ]] || fail "$MARKER is empty; it must hold one positive integer"
[[ "$CUR_VER" =~ ^[0-9]+$ ]] || fail "$MARKER holds '$CUR_VER', which is not an integer"
[[ "$CUR_VER" -ge 1 ]] || fail "contract_version starts at 1; found $CUR_VER"

# --- the gate ---------------------------------------------------------------
if [[ "$SURFACE_CHANGED" == "0" && "$CUR_VER" == "$BASE_VER" ]]; then
  note "OK: the advertised surface is unchanged and contract_version is $CUR_VER."
  exit 0
fi

if [[ "$SURFACE_CHANGED" == "1" && "$CUR_VER" == "$BASE_VER" ]]; then
  {
    echo "::error::MCP public surface changed without a contract_version bump (still $CUR_VER)."
    echo ""
    echo "Every row below is compatibility-bearing. Classify each per"
    echo "docs/src/mcp-public-surface-policy.md §3.1, then either:"
    echo "  * it is S0 (the fixture was stale / it was drift) -> fix the code, revert the fixture"
    echo "  * it is S1 (minor) or S2 (breaking) -> bump $MARKER and state the"
    echo "    required release in the commit body"
    echo ""
    echo "Changed rows:"
    diff -u "$WORKDIR/base.flat" "$WORKDIR/cur.flat" | sed -n 's/^+\(tool\|prop\|def\)\t/  /p'
    echo ""
    echo "A property ADDED here is S1 unless the new row says required=required (S2)."
    echo "A property REMOVED here is always S2."
    echo "A default that moved is S0 DEFECT unless the runtime moved with it."
  } >&2
  exit 1
fi

if [[ "$SURFACE_CHANGED" == "0" && "$CUR_VER" != "$BASE_VER" ]]; then
  fail "contract_version moved ($BASE_VER -> $CUR_VER) but no advertised row changed. A bump with no change silences this guard without a reason; revert it."
fi

note "OK: the surface changed and contract_version moved $BASE_VER -> $CUR_VER."
exit 0