#!/usr/bin/env bash
# test_no_dead_ignores.sh — the semantics harness of check_no_dead_ignores.sh.
#
# The guard's rule (#1984): every id in deny.toml's [advisories] ignore list
# must be an advisory the auditor actually matches in one of the audited
# lockfiles. A suppression that matches nothing is the worst kind: it
# silences nothing today, and the day its crate comes back to the graph it
# silences the finding without anybody noticing.
#
# HERMETIC: the guard shells out to `cargo audit --json`, so this harness
# puts a STUB `cargo` on the PATH that emits a canned cargo-audit JSON
# report. No network, no advisory database, no repo state touched.
#
# NEGATION: the case 'a dead ignore fails' below creates an ignore list that
# matches NOTHING (dead id) and asserts the FAIL: a guard that reported
# success there would be the silent-suppression defect this check exists
# to prevent.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/check_no_dead_ignores.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
STUB="$WORK/bin"
mkdir -p "$STUB"
FAKE_AUDIT="$WORK/audit-report.json"

# The stub `cargo audit`: emits the canned report the guard reads.
cat >"$STUB/cargo" <<STUB
#!/usr/bin/env bash
if [[ "\$1" != "audit" ]]; then
    echo "stub cargo: only 'audit' is implemented" >&2
    exit 64
fi
cat "$FAKE_AUDIT"
STUB
chmod +x "$STUB/cargo"

: >"$WORK/lock-main"
: >"$WORK/lock-fuzz"
LOCK_MAIN="$WORK/lock-main"
LOCK_FUZZ="$WORK/lock-fuzz"

pass=0
fail=0

run_case() {
    local name="$1" expected="$2"
    shift 2
    set +e
    out=$("$@" 2>&1)
    rc=$?
    set -e
    if [[ "$rc" == "$expected" ]]; then
        echo "ok: $name"
        pass=$((pass + 1))
    else
        echo "FAIL: $name (expected exit $expected, got $rc)"
        printf '%s\n' "$out" | sed 's/^/    /'
        fail=$((fail + 1))
    fi
    LAST_OUT="$out"
}

export PATH="$STUB:$PATH"

# --- 1. a live ignore: the auditor matches the id -> guard passes ---------
jq -n '{warnings: {unmaintained: [{advisory: {id: "RUSTSEC-0000-0001"}}, {advisory: {id: "RUSTSEC-0000-0002"}}]}, vulnerabilities: {list: []}}' >"$FAKE_AUDIT"
run_case "a live ignore passes" 0 \
    bash "$GUARD" "$LOCK_MAIN" "$LOCK_FUZZ" -- "RUSTSEC-0000-0001"

# --- 2. an id matched only in the SECOND lockfile still counts -----------
run_case "a match in the second lockfile counts" 0 \
    bash "$GUARD" "$LOCK_MAIN" "$LOCK_FUZZ" -- "RUSTSEC-0000-0002"

# --- 3. NEGATION: a DEAD ignore must fail, naming the id ------------------
# Two ids at once: the LIVE one must not rescue the dead one.
run_case "a dead ignore fails and names it" 1 \
    bash "$GUARD" "$LOCK_MAIN" "$LOCK_FUZZ" -- "RUSTSEC-0000-0001" "RUSTSEC-1111-9999"

case "${LAST_OUT:-}" in
    *RUSTSEC-1111-9999*)
        echo "ok: the failure names the dead id"
        pass=$((pass + 1))
        ;;
    *)
        echo "FAIL: the dead-ignore failure does not name the dead id"
        printf '%s\n' "$LAST_OUT" | sed 's/^/    /'
        fail=$((fail + 1))
        ;;
esac

# --- 4. an auditor that produces an unparsable report is a FAIL -----------
printf 'not json at all\n' >"$FAKE_AUDIT"
run_case "an unparsable audit report fails (never an empty match set)" 1 \
    bash "$GUARD" "$LOCK_MAIN" -- "RUSTSEC-0000-0001"

# --- 5. a lockfile that does not exist is a FAIL ---------------------------
run_case "an un-auditable (missing) lockfile fails" 1 \
    bash "$GUARD" "$WORK/missing.lock" -- "RUSTSEC-0000-0001"

# --- 6. no ignores declared: nothing can be dead --------------------------
jq -n '{warnings: {unmaintained: [], unsound: []}, vulnerabilities: {list: []}}' >"$FAKE_AUDIT"
run_case "no ignores declared passes without an audit" 0 \
    bash "$GUARD" "$LOCK_MAIN" --

# --- 7. an advisory reported under `vulnerabilities.list` counts too ------
jq -n '{warnings: {unmaintained: [], unsound: []}, vulnerabilities: {list: [{advisory: {id: "RUSTSEC-0000-0003"}}]}}' >"$FAKE_AUDIT"
run_case "an id under vulnerabilities.list counts" 0 \
    bash "$GUARD" "$LOCK_MAIN" -- "RUSTSEC-0000-0003"

echo
echo "test_no_dead_ignores: $pass ok, $fail fail"
[[ "$fail" -eq 0 ]]
