#!/usr/bin/env bash
# Negation harness for the build-graph lot L9 — issues #1799, #1797, #1796.
#
# WHAT THIS PINS
# ===============
# Three of the four changes in this lot are GRAPH changes: they have no
# observable behaviour, only a different dependency graph. Without a harness
# the whole lot would be unreviewable — a diff that deletes four crates and
# renames a public function cannot be judged by "the tests still pass".
#
# The graph this harness freezes:
#
#   * ONE source of RAM. `sys-info` and `sysinfo` were BOTH in the graph for
#     the same fact (total physical memory), and `sys-info` does not document
#     the unit it returns, so the unit lived only in the parameter NAME
#     (`total_kib`). `sysinfo` documents "the RAM size in bytes". The
#     duplicate is gone (#1799).
#   * NO `built`. The `built` build-dependency pulled `git2` and
#     `libgit2-sys` into the graph, and it only fed `version_string()`, a
#     function with zero call sites (#1797).
#   * NO `once_cell` in the manifests. std carries `LazyLock`/`OnceLock` since
#     1.80 and the MSRV is 1.88. `once_cell` STAYS in Cargo.lock (ahash,
#     indicatif, criterion, dashmap, sysinfo, insta, tempfile all pull it) —
#     that asymmetry is the point, and it is asserted below.
#   * NO implicit browser in the default workspace graph. The benchmark
#     asked for `features = ["chromium"]` on its `webfang_core` dependency, so
#     the whole workspace carried `chromiumoxide` in every build (#1796).
#     The benchmark now declares its OWN forwarding feature and the default
#     workspace graph is browser-free.
#
# WHY A POSITIVE CONTROL PER CHECK
# ================================
# A harness that can go green by construction is not evidence. Every negative
# assertion below is paired with a positive one that proves the DETECTOR can
# fire: the same lockfile matcher must find a crate that IS there, the same
# source matcher must fire on a fixture that DOES contain the forbidden
# import, the same advisory parser must find the ids that are still there.
# A check whose positive control fails is a broken harness, not a finding —
# so the controls are asserted too, and they are asserted on the SAME tree
# the negative checks read.
#
# The RED for this lot is a GRAPH red: against the pre-fix tree the crates
# ARE in the graph, so the negative checks fail while every positive control
# stays green. There is no behavioural red to reproduce (the `sys-info`
# "macOS returns bytes" premise of #1799 is FALSE — `sys-info` 0.9.1 returns
# KiB on every platform, including macOS), and this harness does not invent
# one.
#
# OFFLINE / NO-CARGO
# ==================
# The cargo invocations are `cargo tree --offline`: they RESOLVE a graph, they
# never compile a single crate. `cargo tree` still needs the registry sources
# of the packages it prints, so a cold machine must run `cargo fetch --locked`
# once before this harness (the CI step does exactly that).
#
# Usage: bash scripts/test_build_graph.sh
set -uo pipefail

export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

WORKSPACE_LOCK="$ROOT/Cargo.lock"
FUZZ_LOCK="$ROOT/fuzz/Cargo.lock"
BENCH_MANIFEST="$ROOT/crates/webfang_benchmark/Cargo.toml"
DENY_TOML="$ROOT/deny.toml"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0
pass() {
    PASS=$((PASS + 1))
    printf 'ok   %s\n' "$1"
}
fail() {
    FAIL=$((FAIL + 1))
    printf 'FAIL %s\n' "$1"
}

# --- detectors ---------------------------------------------------------------

# lock_has <crate> <lockfile> — exit 0 when the lockfile pins that crate.
#
# The pattern is anchored on the `name = "..."` line, so `sys-info` never
# matches `sysinfo` and `built` never matches `build-dependencies`.
lock_has() {
    grep -qE "^name = \"$1\"\$" "$2"
}

# manifests — every Cargo.toml in the tree, minus VCS and build scratch.
manifests() {
    find "$ROOT" -name Cargo.toml -not -path '*/.git/*' -not -path '*/target/*' \
        -not -path '*/node_modules/*' | sort
}

# manifest_mentions <needle> — exit 0 when any manifest mentions the token.
#
# `-w` keeps the match on whole words: `sys-info` and `sysinfo` are different
# crates and must never be confused by a substring.
manifest_mentions() {
    local needle="$1"
    local m
    while IFS= read -r m; do
        if grep -qwE -- "$needle" "$m"; then
            return 0
        fi
    done < <(manifests)
    return 1
}

# source_uses_once_cell <dir> — the `once_cell` crate paths in the sources.
source_uses_once_cell() {
    grep -rlwE --include='*.rs' -- 'once_cell' "$1" 2>/dev/null
}

# manifest_code_lines <manifest> — the manifest WITHOUT its full-line comments.
#
# A detector that matches inside a comment reads a NOTE as a DECLARATION: the
# benchmark manifest documents the shape it used to have, and a naive matcher
# sees that note as the shape it still has. Only code lines are read here.
manifest_code_lines() {
    grep -vE '^[[:space:]]*#' "$1" || true
}

# manifest_requests_chromium <manifest> — exit 0 when the manifest asks its
# `webfang_core` dependency for the `chromium` feature.
manifest_requests_chromium() {
    manifest_code_lines "$1" | grep -qE 'features[[:space:]]*=[[:space:]]*\[[^]]*"chromium"'
}

# manifest_declares_chromium_feature <manifest> — exit 0 when the manifest
# declares its own `chromium = ["webfang_core/chromium"]` forwarding feature.
manifest_declares_chromium_feature() {
    manifest_code_lines "$1" |
        grep -qE '^[[:space:]]*chromium[[:space:]]*=[[:space:]]*\[[^]]*"webfang_core/chromium"'
}

# The cargo that resolves the graph. Overridable so the FAIL-CLOSED path can
# be exercised with a cargo that cannot resolve anything (control 8).
BUILD_GRAPH_CARGO="${BUILD_GRAPH_CARGO:-cargo}"

# chromiumoxide_count <cargo-tree args...>
#
# Sets two globals instead of printing the count, because a command
# substitution would run the function in a SUBSHELL and the failure flag would
# not survive it:
#
#   CHROMIUM_COUNT     — how many `chromiumoxide*` packages the graph carries
#   CHROMIUM_RESOLVED  — 0 when cargo could not resolve the graph AT ALL
#
# The distinction is load-bearing: "I could not look" is NOT "there is nothing
# there". A resolver that fails — a machine with no registry sources (the
# `cargo tree --offline` in this harness READS them), a broken lockfile, a
# missing feature — must never read as a clean graph. That is the fail-open
# shape this harness exists to avoid: a green that says nothing about the
# defect. The failing check says so and names the fix.
CHROMIUM_COUNT=0
CHROMIUM_RESOLVED=1
chromiumoxide_count() {
    local out
    if ! out="$("$BUILD_GRAPH_CARGO" tree --offline -e normal --prefix none "$@" 2>"$SCRATCH/tree.err")"; then
        CHROMIUM_COUNT=0
        CHROMIUM_RESOLVED=0
        return 1
    fi
    CHROMIUM_COUNT="$(printf '%s\n' "$out" | grep -c '^chromiumoxide' || true)"
    CHROMIUM_RESOLVED=1
    return 0
}

# tree_failure <what> — report a check that could not RUN because the graph
# could not be resolved. This is a FAILURE (the guard is fail-closed): a
# machine that cannot look at the graph says so loudly instead of reading as a
# clean tree.
tree_failure() {
    fail "$1"
    printf '       the graph could not be resolved (cargo tree --offline failed).\n'
    printf '       Most common cause: this machine has no registry sources yet — run: cargo fetch --locked\n'
    sed 's/^/       /' "$SCRATCH/tree.err" 2>/dev/null
}

# --- positive controls -------------------------------------------------------
#
# These prove the detectors above can fire. They are asserted FIRST: a
# harness whose own machinery is broken must say so instead of reporting a
# pile of findings.

# Control 1: the lockfile matcher finds a crate that IS in the lock.
if lock_has "sysinfo" "$WORKSPACE_LOCK" && lock_has "sysinfo" "$FUZZ_LOCK"; then
    pass "control: the lockfile matcher finds 'sysinfo' in both lockfiles"
else
    fail "control: the lockfile matcher cannot find 'sysinfo' in both lockfiles"
fi

# Control 2: the source matcher fires on a fixture that DOES import it.
mkdir -p "$SCRATCH/fixture"
printf 'use once_cell::sync::Lazy;\n' >"$SCRATCH/fixture/probe.rs"
if [ -n "$(source_uses_once_cell "$SCRATCH/fixture")" ]; then
    pass "control: the source matcher fires on a fixture that imports once_cell"
else
    fail "control: the source matcher does not fire on a fixture that imports once_cell"
fi

# Control 3: the advisory parser finds the ids that are still declared.
if grep -qE 'RUSTSEC-[0-9]{4}-[0-9]{4}' "$DENY_TOML"; then
    pass "control: deny.toml still declares RUSTSEC ids"
else
    fail "control: deny.toml declares no RUSTSEC id — the parser is looking at the wrong file"
fi

# Control 4: the benchmark manifest is readable and declares its dependency.
if grep -qE '^[[:space:]]*webfang_core[[:space:]]*=' "$BENCH_MANIFEST"; then
    pass "control: the benchmark manifest declares its webfang_core dependency"
else
    fail "control: the benchmark manifest declares no webfang_core dependency"
fi

# Control 5: the feature matcher fires on a fixture that DOES request it.
printf '[dependencies]\nwebfang_core = { path = "../webfang_core", features = ["chromium"] }\n' \
    >"$SCRATCH/fixture_bench.toml"
if manifest_requests_chromium "$SCRATCH/fixture_bench.toml"; then
    pass "control: the chromium matcher fires on a fixture that requests the feature"
else
    fail "control: the chromium matcher does not fire on a fixture that requests the feature"
fi

# Control 6: the matcher reads CODE lines only — a commented-out shape is a
# note about the past, not a declaration of the present.
printf '[dependencies]\n# webfang_core = { path = "../webfang_core", features = ["chromium"] }\nwebfang_core = { path = "../webfang_core" }\n' \
    >"$SCRATCH/fixture_bench_commented.toml"
if manifest_requests_chromium "$SCRATCH/fixture_bench_commented.toml"; then
    fail "control: a commented-out chromium request must not be read as a declaration"
else
    pass "control: a commented-out chromium request is not read as a declaration"
fi

# Control 7: the browser feature of webfang_core is still alive — the graph
# check below must not be satisfiable by DELETING the browser tier.
chromiumoxide_count -p webfang_core --features chromium
if [ "$CHROMIUM_RESOLVED" -eq 0 ]; then
    tree_failure "control: the graph could not be resolved — run 'cargo fetch --locked' first"
elif [ "$CHROMIUM_COUNT" -ge 1 ]; then
    pass "control: webfang_core --features chromium still reaches the browser tier"
else
    fail "control: webfang_core --features chromium no longer reaches the browser tier"
fi

# Control 8: a resolver that CANNOT resolve is not an empty graph.
#
# This is the fail-open shape the harness must not have: if a broken resolver
# read as "the graph carries nothing", the two graph checks below would go
# green on a machine that never looked at anything.
mkdir -p "$SCRATCH/stub"
printf '#!/usr/bin/env bash\nexit 1\n' >"$SCRATCH/stub/cargo"
chmod +x "$SCRATCH/stub/cargo"
saved_cargo="$BUILD_GRAPH_CARGO"
BUILD_GRAPH_CARGO="$SCRATCH/stub/cargo"
chromiumoxide_count --workspace
if [ "$CHROMIUM_RESOLVED" -eq 0 ] && [ "$CHROMIUM_COUNT" -eq 0 ]; then
    pass "control: a cargo that cannot resolve the graph is NOT read as an empty graph"
else
    fail "control: a cargo that cannot resolve the graph was read as an empty graph"
fi
BUILD_GRAPH_CARGO="$saved_cargo"

# --- S1: one source of RAM ---------------------------------------------------

if lock_has "sys-info" "$WORKSPACE_LOCK"; then
    fail "S1: Cargo.lock still pins 'sys-info' — two crates read the same fact"
else
    pass "S1: Cargo.lock does not pin 'sys-info'"
fi

if manifest_mentions "sys-info"; then
    fail "S1: a manifest still declares 'sys-info'"
else
    pass "S1: no manifest declares 'sys-info'"
fi

# --- S2: once_cell out of the manifests, still in the lock -------------------

if manifest_mentions "once_cell"; then
    fail "S2: a manifest still declares 'once_cell' (std carries LazyLock since 1.80)"
else
    pass "S2: no manifest declares 'once_cell'"
fi

if lock_has "once_cell" "$WORKSPACE_LOCK"; then
    pass "S2: 'once_cell' is still in Cargo.lock (ahash/indicatif/criterion/dashmap pull it)"
else
    fail "S2: 'once_cell' vanished from Cargo.lock — it was expected to STAY there"
fi

# --- S2: no once_cell in the sources -----------------------------------------

if [ -n "$(source_uses_once_cell "$ROOT/crates")" ]; then
    fail "S2: the sources still import 'once_cell'"
    source_uses_once_cell "$ROOT/crates" | sed 's/^/       /'
else
    pass "S2: no source under crates/ imports 'once_cell'"
fi

# --- S3: built / git2 / libgit2-sys out of both lockfiles --------------------

for crate in built git2 libgit2-sys; do
    if lock_has "$crate" "$WORKSPACE_LOCK"; then
        fail "S3: Cargo.lock still pins '$crate'"
    else
        pass "S3: Cargo.lock does not pin '$crate'"
    fi
done

for crate in built git2 libgit2-sys; do
    if lock_has "$crate" "$FUZZ_LOCK"; then
        fail "S3: fuzz/Cargo.lock still pins '$crate'"
    else
        pass "S3: fuzz/Cargo.lock does not pin '$crate'"
    fi
done

if manifest_mentions "built"; then
    fail "S3: a manifest still declares the 'built' build-dependency"
else
    pass "S3: no manifest declares the 'built' build-dependency"
fi

# --- S3: the advisory ignores that justified 'built' are purged --------------
#
# Their written reason said, literally, "reachable only through the built
# build-dependency of webfang_core". With `built` out of the graph that
# justification is void, and leaving it would be a promise the file cannot
# keep.

for advisory in RUSTSEC-2026-0183 RUSTSEC-2026-0184; do
    if grep -qE "id[[:space:]]*=[[:space:]]*\"$advisory\"" "$DENY_TOML"; then
        fail "S3: deny.toml still ignores '$advisory' — its reason names the removed 'built' dependency"
    else
        pass "S3: deny.toml does not ignore '$advisory'"
    fi
done

# --- S4: the benchmark owns its browser feature ------------------------------

if manifest_declares_chromium_feature "$BENCH_MANIFEST"; then
    pass "S4: the benchmark declares its own chromium = [\"webfang_core/chromium\"]"
else
    fail "S4: the benchmark declares no chromium = [\"webfang_core/chromium\"] feature"
fi

if manifest_requests_chromium "$BENCH_MANIFEST"; then
    fail "S4: the benchmark still asks its webfang_core dependency for features = [\"chromium\"]"
else
    pass "S4: the benchmark does not force the chromium feature onto webfang_core"
fi

# --- S4: the default workspace graph carries no browser ----------------------

chromiumoxide_count --workspace
if [ "$CHROMIUM_RESOLVED" -eq 0 ]; then
    tree_failure "S4: the default workspace graph could not be resolved"
elif [ "$CHROMIUM_COUNT" -eq 0 ]; then
    pass "S4: the default workspace graph carries no chromiumoxide package"
else
    fail "S4: the default workspace graph carries $CHROMIUM_COUNT chromiumoxide package(s)"
fi

# Positive control for the check above, in the benchmark's OWN feature: the
# browser tier must still be reachable on demand, or the graph is clean for
# the wrong reason (the tier was deleted instead of being gated).
chromiumoxide_count -p webfang_benchmark --features chromium
if [ "$CHROMIUM_RESOLVED" -eq 0 ]; then
    tree_failure "S4: the benchmark's own chromium feature could not be resolved"
elif [ "$CHROMIUM_COUNT" -ge 1 ]; then
    pass "S4: the benchmark's own chromium feature still reaches the browser tier"
else
    fail "S4: the benchmark's own chromium feature reaches no browser tier"
fi

# --- report ------------------------------------------------------------------

printf '\n'
echo "test_build_graph: PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
