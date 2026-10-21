#!/usr/bin/env bash
# check_dependency_direction.sh
#
# CI gate for the inter-crate dependency policy (issue #513).
#
# WHAT IT CHECKS
#   The inter-crate dependency direction documented in AGENTS.md
#   ("Inter-crate dependency direction (ENFORCED POLICY)"): which webfang_*
#   crate may depend on which, and at which tier (prod / dev / build).
#
# HOW THE EDGES ARE EXTRACTED (rewritten 2026-10-18, audit B33/B34/B35)
#   The edges come from the REAL build graph:
#
#       cargo metadata --no-deps --offline --format-version 1
#
#   parsed with jq. `cargo metadata` compiles nothing, runs offline in well
#   under a second, and resolves — in one pass — the three bypasses the old
#   hand-rolled awk extractor had:
#
#     B33  table-shape bypass. The old extractor only recognised the exact
#          headers `[(dev-)?dependencies]` and `[target.*dependencies]`, so a
#          SUB-TABLE (`[dependencies.webfang_ai]`), a spaced header
#          (`[ dependencies ]`), a quoted header (`["dependencies"]`) or a
#          target+sub-table header (`[target.'cfg(unix)'.dependencies.X]`)
#          was invisible to it. Cargo resolves all of them, so cargo
#          metadata reports them as ordinary dependency edges.
#     B34  identity bypass. The old extractor read the MANIFEST KEY, so
#          `webfang_core = { package = "webfang_benchmark", … }` was reported
#          under the allowed key while cargo linked the forbidden package.
#          cargo metadata reports the REAL package name in `name` and the
#          manifest key in `rename`; this gate reads `name`.
#     B35  inventory bypass. The old extractor hardcoded the crate regex
#          `(core|ai|test_utils|benchmark|mcp|cli)` and a fixed manifest
#          list, so a brand-new internal crate was invisible. cargo metadata
#          reports the real workspace membership.
#
#   The POLICY stays hand-written below: which crate may depend on which is
#   policy, it is not derivable from the graph. Only the EXTRACTION of the
#   edges is metadata-driven.
#
# POLICY MATRIX (source of truth — keep in sync with AGENTS.md)
#   webfang_core:       (none)
#   webfang_mcp:        webfang_core
#   webfang_cli:        webfang_core, webfang_mcp
#   webfang_test_utils: webfang_core
#   webfang_benchmark:  webfang_core
#
#   webfang_ai was removed in ADR-0004 paso 6 (see docs/adr/
#   0004-onnx-local-removal.md): the crate and its ai feature no longer
#   exist. A surviving webfang_ai edge is now caught in three ways: it
#   cannot be resolved by cargo (metadata fails → the gate fails closed), it
#   becomes a workspace member the policy does not know (named error), and
#   any webfang_* dependency name is treated as internal even when it is not
#   a member (edge violation). It never passes silently.
#
# DEV TIER (#1825) — unchanged
#   [dev-dependencies] may additionally target webfang_test_utils from ANY
#   crate — it is the shared test harness (publish = false, never shipped).
#   A [dependencies] edge into webfang_test_utils is still forbidden.
#
# BUILD DEPENDENCIES — DECISION
#   cargo metadata reports [build-dependencies] edges with kind "build".
#   THIS GATE TREATS THEM LIKE NORMAL DEPENDENCIES (tier "build"): a build
#   edge links the crate into the build graph exactly like a normal edge
#   does, so a forbidden direction is a real violation, not a technicality.
#   Ignoring build edges would be the fail-open shape this rewrite removes.
#   The dev-tier exemption does NOT extend to build edges: only
#   webfang_test_utils from [dev-dependencies] is exempt. Pinned by
#   scripts/test_dependency_direction.sh.
#
# UNKNOWN WORKSPACE MEMBERS — DECISION
#   A workspace member the policy matrix does not know is a POLICY GAP, not
#   a bug and not something to auto-allow: the gate reports it as a named
#   error naming the member, and the maintainer decides whether to add it to
#   the matrix. fuzz/ stays outside this policy because the root manifest
#   excludes it from the workspace — it is not a member, so it never reaches
#   the graph, and it is not special-cased in the gate.
#
# FAIL CLOSED
#   A gate that cannot evaluate the policy must NOT report OK. If the
#   workspace root cannot be located, cargo cannot be resolved, jq is
#   missing, or `cargo metadata` fails for any reason (malformed manifest,
#   unresolvable path dependency, missing dependency), the gate exits
#   non-zero with a named ::error:: line naming the cause. There is no
#   "no edges found" fallback: that is the fail-open shape being removed.
#
# CARGO RESOLUTION (load-bearing)
#   cargo is resolved with `rustup which cargo`, falling back to
#   `$HOME/.cargo/bin/cargo`. `command -v cargo` is NEVER used: on this
#   machine it returns the mbx build-cache shim, which is a build wrapper,
#   not the toolchain. The gate refuses an mbx path outright.
#
# Usage: bash scripts/check_dependency_direction.sh
#
# Semantics harness: scripts/test_dependency_direction.sh

set -euo pipefail

declare -A ALLOWED=(
  [webfang_core]=""
  [webfang_mcp]="webfang_core"
  [webfang_cli]="webfang_core webfang_mcp"
  [webfang_test_utils]="webfang_core"
  # webfang_benchmark: leaf harness crate (benchmark tooling, no production
  # dependents). Its [dev-dependencies] edge into webfang_test_utils is
  # accepted by the dev tier below.
  [webfang_benchmark]="webfang_core"
)

CRATES=(webfang_core webfang_mcp webfang_cli webfang_test_utils webfang_benchmark)

status=0

# resolve_workspace_root — the nearest ancestor of $PWD (inclusive) whose
# Cargo.toml declares a [workspace] table. The gate must work from any cwd;
# it never assumes it was started at the repository root.
resolve_workspace_root() {
  local dir="$PWD"
  while :; do
    if [[ -f "$dir/Cargo.toml" ]] && grep -qE '^[[:space:]]*\[workspace\]' "$dir/Cargo.toml"; then
      printf '%s\n' "$dir"
      return 0
    fi
    if [[ "$dir" == "/" ]]; then
      return 1
    fi
    dir="$(dirname "$dir")"
  done
}

# resolve_cargo — the toolchain's cargo, never `command -v cargo` (which can
# resolve to the mbx build-cache shim on this machine).
# Exit codes: 0 = path printed, 1 = not found, 2 = mbx shim (refused).
resolve_cargo() {
  local candidate=""
  if command -v rustup >/dev/null 2>&1; then
    # The `|| true` only keeps `set -e` from aborting the script when rustup
    # cannot answer; the resolution verdict is computed below, not from it.
    candidate="$(rustup which cargo 2>/dev/null || true)"
  fi
  if [[ -z "$candidate" || ! -x "$candidate" ]]; then
    candidate="$HOME/.cargo/bin/cargo"
  fi
  if [[ -z "$candidate" || ! -x "$candidate" ]]; then
    return 1
  fi
  case "$candidate" in
    *mbx*) return 2 ;;
  esac
  printf '%s\n' "$candidate"
}

root=""
if ! root="$(resolve_workspace_root)"; then
  echo "::error::could not locate the workspace root: no ancestor of $PWD has a Cargo.toml with a [workspace] table"
  exit 1
fi

# Every crate the policy knows must have its manifest on disk — otherwise its
# edges would be invisible (checked before cargo, so the cause is named).
for crate in "${CRATES[@]}"; do
  if [[ ! -f "$root/crates/$crate/Cargo.toml" ]]; then
    echo "::error::missing manifest crates/$crate/Cargo.toml"
    status=1
  fi
done
if [[ $status -ne 0 ]]; then
  exit 1
fi

cargo_rc=0
CARGO_BIN="$(resolve_cargo)" || cargo_rc=$?
if [[ $cargo_rc -eq 1 ]]; then
  echo "::error::cargo not found: 'rustup which cargo' and \$HOME/.cargo/bin/cargo are both unavailable"
  exit 1
fi
if [[ $cargo_rc -eq 2 ]]; then
  echo "::error::refusing to run cargo: resolved to the mbx build-cache shim, not the toolchain"
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "::error::jq not found: cargo metadata output cannot be parsed"
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The jq program that turns cargo metadata into the graph the policy is
# checked against:
#   members — the workspace members (by package name);
#   edges   — the internal dependency edges, with the REAL package name
#             (`name`, not the manifest key `rename`), the tier derived
#             from `kind` (null → prod, "dev" → dev, "build" → build) and
#             the target-specific cfg when the edge is target-gated.
cat > "$work/graph.jq" <<'JQ'
def tier:
  if .kind == "dev" then "dev"
  elif .kind == "build" then "build"
  else "prod" end;

[.workspace_members[]] as $ids
| [ .packages[] | select(.id as $id | $ids | index($id)) | .name ] as $names
| {
    members: ($names | sort),
    edges: (
      [ .packages[]
        | select(.id as $id | $ids | index($id))
        | .name as $src
        | .dependencies[]
        | select(
            ((.name as $d | $names | index($d)) != null)
            or ((.name | startswith("webfang_")))
          )
        | { source: $src, target: .name, tier: tier, target_cfg: (.target // "-") }
      ]
      | unique_by([.source, .target, .tier])
    )
  }
JQ

meta_json="$work/metadata.json"
if ! "$CARGO_BIN" metadata --no-deps --offline --format-version 1 \
  --manifest-path "$root/Cargo.toml" >"$meta_json" 2>"$work/metadata.err"; then
  reason="$(head -n 5 "$work/metadata.err" | tr '\n' ' ')"
  echo "::error::cargo metadata failed for $root/Cargo.toml: ${reason:-unknown error}"
  exit 1
fi

graph="$work/graph.json"
if ! jq -f "$work/graph.jq" "$meta_json" >"$graph" 2>"$work/jq.err"; then
  reason="$(head -n 3 "$work/jq.err" | tr '\n' ' ')"
  echo "::error::could not parse cargo metadata output: ${reason:-unknown error}"
  exit 1
fi

members="$(jq -r '.members[]' "$graph")"

# A workspace member the policy matrix does not know is a POLICY GAP: report
# it by name instead of silently allowing it.
while read -r member; do
  [[ -n "$member" ]] || continue
  if [[ -z "${ALLOWED[$member]+known}" ]]; then
    echo "::error::unknown workspace member '$member': the dependency-direction policy does not cover it (add it to CRATES and ALLOWED in this script)"
    status=1
  fi
done <<< "$members"

# And the other way round: a known crate that is not a workspace member has
# edges the graph would not report.
for crate in "${CRATES[@]}"; do
  if ! printf '%s\n' "$members" | grep -qxF "$crate"; then
    echo "::error::crate '$crate' is not a workspace member: its dependencies would be invisible to the policy"
    status=1
  fi
done

jq -r '.edges[] | [.source, .tier, .target, .target_cfg] | @tsv' "$graph" > "$work/edges.tsv"
while IFS=$'\t' read -r src tier target cfg; do
  [[ -n "$target" ]] || continue
  # Shared test harness: any crate may target it from [dev-dependencies].
  if [[ "$tier" == "dev" && "$target" == "webfang_test_utils" ]]; then
    continue
  fi
  if [[ -z "${ALLOWED[$src]+known}" ]]; then
    allowed="<not covered by the policy>"
  else
    allowed="${ALLOWED[$src]}"
  fi
  if [[ " $allowed " != *" $target "* ]]; then
    suffix=""
    if [[ "$cfg" != "-" ]]; then
      suffix="; target: $cfg"
    fi
    echo "::error::$src must NOT depend on $target (${tier}-dependency; policy: ${allowed:-none}$suffix)"
    status=1
  fi
done < "$work/edges.tsv"

if [[ $status -eq 0 ]]; then
  echo "OK: inter-crate dependency direction matches policy (issue #513)"
  for crate in "${CRATES[@]}"; do
    deps="$(jq -r --arg c "$crate" '.edges[] | select(.source == $c) | .target' "$graph" \
      | sort -u | paste -sd' ' -)"
    printf '  %-18s -> %s\n' "$crate" "${deps:-none}"
  done
fi

exit "$status"
