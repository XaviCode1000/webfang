#!/usr/bin/env bash
# Negation harness for scripts/check_advisory_policy.sh — issue #2057 (S3).
#
# The defect this pins: `collect_duplicates` ran `cargo deny check bans ... ||
# true` and then `return 0`, so a broken producer and a clean tree produced the
# same empty set. The ratchet at the call site (`if ! collect_duplicates ...;
# then exit 1; fi`) was dead code and a failed `cargo deny` read as "no
# duplicates".
#
# No real cargo and no network: a fake `cargo` on PATH drives the producer, and
# the guard's own env overrides (`ADVISORY_POLICY_BASELINE`,
# `ADVISORY_POLICY_DENY_BANS_OUTPUT`) keep the real tree untouched.
#
# Usage: bash scripts/test_advisory_policy.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$ROOT/scripts/check_advisory_policy.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; }

# fake_cargo <exit-code> [stdout]
# A PATH entry with a `cargo` that answers `deny check bans` without cargo.
# The prefix also carries DEFAULT stubs for the standalone binaries, so the
# guard's two tool gates (`command -v cargo-deny`, `command -v cargo-audit`)
# resolve to the sandbox and the guard reaches the fake `cargo` on EVERY
# machine. Without them, a runner with neither binary installed fails at the
# tool gate — showing the fail-closed refusal instead of driving the fake
# producer — the exact mismatch observed on the first CI run of this step. Silent no-ops: the guard invokes the standalone binaries
# nowhere; all tool output travels through `cargo deny` / `cargo audit`.
fake_cargo() { # name exit-code body
  local dir="$SCRATCH/$1"
  mkdir -p "$dir/bin"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'printf %%s %q\n' "$3"
    printf 'exit %s\n' "$2"
  } > "$dir/bin/cargo"
  chmod +x "$dir/bin/cargo"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$dir/bin/cargo-deny"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$dir/bin/cargo-audit"
  chmod +x "$dir/bin/cargo-deny" "$dir/bin/cargo-audit"
  printf '%s\n' "$dir/bin"
}

run_guard() { # path-prefix [args...]
  local prefix="$1"
  shift
  # The dead-ignore check needs a REAL audit report; the fake `cargo` of the
  # other cases does not produce one, so those cases skip it explicitly (the
  # same pattern as --skip-duplicates). The cases that DO exercise it call
  # the guard directly.
  ( cd "$ROOT" && PATH="$prefix:$PATH" bash "$GUARD" --skip-dead-ignores "$@" ) >"$SCRATCH/out" 2>&1
}

# run_guard_policy <deny.toml> <path-prefix> — the guard in CHECK mode against
# a fixture deny.toml (the real file is never touched: the guard reads the
# path ADVISORY_POLICY_DENY_TOML points at).
run_guard_policy() { # <deny.toml> <path-prefix> [args...]
  local deny="$1" prefix="$2"
  shift 2
  ( cd "$ROOT" && PATH="$prefix:$PATH" ADVISORY_POLICY_DENY_TOML="$deny" bash "$GUARD" --skip-dead-ignores "$@" ) >"$SCRATCH/out" 2>&1
}

# print_ignores <deny.toml> — the ids the guard derives from the fixture. The
# printer mode needs no cargo: it exits right after the parse.
print_ignores() { # <deny.toml>
  ( cd "$ROOT" && ADVISORY_POLICY_DENY_TOML="$1" bash "$GUARD" --print-ignores ) 2>/dev/null
}

# write_policy <path> <header> <entry> — a COMPLETE and valid deny.toml: the
# fail-closed knobs, the ignore array and the [sources] knobs. Only the table
# header and the ignore entry vary per case, so a case that fails is about
# the header or the entry, never about a missing knob.
write_policy() { # <path> <header> <entry>
  {
    printf '%s\n' "$2"
    printf 'unmaintained = "all"\n'
    printf 'unsound = "all"\n'
    printf 'yanked = "deny"\n'
    printf 'ignore = [\n'
    printf '    # Expires: 2099-12-31\n'
    printf '    %s\n' "$3"
    printf ']\n'
    printf '\n'
    printf '[sources]\n'
    printf 'unknown-registry = "deny"\n'
    printf 'unknown-git = "deny"\n'
  } >"$1"
}

# --- case 1: a FAILED producer fails the guard ------------------------------
# NEGATION: `cargo deny` exiting non-zero must NOT read as "no duplicates".
prefix="$(fake_cargo failing 1 'error: failed to fetch advisory database')"
if run_guard "$prefix"; then
  fail "a failed cargo deny must fail the guard"
elif grep -qE 'cargo deny check bans falló|no es confiable' "$SCRATCH/out"; then
  pass "a failed cargo deny fails the guard, loudly"
else
  fail "a failed cargo deny failed the guard but without explaining why"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 2: a NEW duplicate fails the guard -------------------------------
# NEGATION: a duplicate absent from the baseline must fail, not warn.
printf 'known-crate\n' > "$SCRATCH/baseline.txt"
prefix="$(fake_cargo dup 0 "warning[duplicate]: found 2 duplicate entries for crate 'surprise-crate'
warning[duplicate]: found 2 duplicate entries for crate 'known-crate'")"
if ADVISORY_POLICY_BASELINE="$SCRATCH/baseline.txt" run_guard "$prefix"; then
  fail "a duplicate missing from the baseline must fail the guard"
elif grep -qF 'surprise-crate' "$SCRATCH/out"; then
  pass "a duplicate missing from the baseline fails the guard and names it"
else
  fail "a new duplicate failed the guard without naming the crate"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 3: a baselined duplicate still passes ----------------------------
prefix="$(fake_cargo dup-ok 0 "warning[duplicate]: found 2 duplicate entries for crate 'known-crate'")"
if ADVISORY_POLICY_BASELINE="$SCRATCH/baseline.txt" run_guard "$prefix"; then
  pass "a duplicate already in the baseline passes"
else
  fail "a baselined duplicate must not fail the guard"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 4: a missing producer file fails closed --------------------------
# NEGATION: the override path naming a file that does not exist must fail.
prefix="$(fake_cargo unused 0 '')"
if ADVISORY_POLICY_DENY_BANS_OUTPUT="$SCRATCH/does-not-exist.txt" run_guard "$prefix"; then
  fail "a missing ADVISORY_POLICY_DENY_BANS_OUTPUT file must fail the guard"
elif grep -qF 'no existe' "$SCRATCH/out"; then
  pass "a missing producer file fails the guard, loudly"
else
  fail "a missing producer file failed the guard without explaining why"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 5: the id bound to the `id` KEY wins over an id quoted in the
# reason. A TOML inline table is UNORDERED, so the first advisory id quoted on
# the line is NOT the entry's id: the old parser took it, so cargo audit was
# told to ignore a DIFFERENT advisory than the policy declares (B40).
cat > "$SCRATCH/b40.toml" <<'EOF'
[advisories]
ignore = [
    # Expires: 2099-12-31
    { reason = "RUSTSEC-2025-0141", id = "RUSTSEC-2026-9999" },
]
EOF
got="$(print_ignores "$SCRATCH/b40.toml")"
if [[ "$got" == "--ignore RUSTSEC-2026-9999" ]]; then
  pass "the id bound to the id key wins over an id quoted in the reason"
else
  fail "the id bound to the id key must win over an id quoted in the reason"
  printf '       got: %s\n' "${got:-<empty>}"
fi

# --- case 6: a single-quoted (literal) id is not dropped. A TOML literal
# string is as valid as a basic one, and the old parser only recognised the
# double-quoted form, so the entry vanished (B40).
cat > "$SCRATCH/b40b.toml" <<'EOF'
[advisories]
ignore = [
    # Expires: 2099-12-31
    { id = 'RUSTSEC-2026-8888', reason = "single-quoted id" },
]
EOF
got="$(print_ignores "$SCRATCH/b40b.toml")"
if [[ "$got" == "--ignore RUSTSEC-2026-8888" ]]; then
  pass "a single-quoted id is not dropped"
else
  fail "a single-quoted id must not be dropped"
  printf '       got: %s\n' "${got:-<empty>}"
fi

# --- case 7: a commented-out entry is not an entry. A `#` outside a string
# starts a comment, so a commented-out ignore must not be handed to cargo
# audit as a suppression.
cat > "$SCRATCH/commented.toml" <<'EOF'
[advisories]
ignore = [
    # Expires: 2099-12-31
    # { id = "RUSTSEC-2026-0000", reason = "commented out" },
    { id = "RUSTSEC-2026-1111", reason = "live entry" },
]
EOF
got="$(print_ignores "$SCRATCH/commented.toml")"
if [[ "$got" == "--ignore RUSTSEC-2026-1111" ]]; then
  pass "a commented-out entry is not an entry"
else
  fail "a commented-out entry must not be counted as an entry"
  printf '       got: %s\n' "${got:-<empty>}"
fi

# --- case 8: a table header with a TRAILING COMMENT is a header. `[advisories]
# # policy` is valid TOML; the old parser only recognised /^\[.*\]$/, so
# nothing was parsed out of a valid file and the guard failed it (B41).
prefix="$(fake_cargo policy 0 '')"
write_policy "$SCRATCH/b41.toml" '[advisories] # bump policy, see docs' \
  '{ id = "RUSTSEC-2026-7777", reason = "trailing comment on the header" }'
got="$(print_ignores "$SCRATCH/b41.toml")"
run_guard_policy "$SCRATCH/b41.toml" "$prefix"
rc=$?
if [[ "$got" == "--ignore RUSTSEC-2026-7777" && "$rc" -eq 0 ]]; then
  pass "a header with a trailing comment is recognised"
else
  fail "a header with a trailing comment must not fail a valid policy"
  printf '       derived: %s\n' "${got:-<empty>}"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 9: an INDENTED table header is a header (valid TOML, same defect
# as case 8).
write_policy "$SCRATCH/b41b.toml" '   [advisories]' \
  '{ id = "RUSTSEC-2026-6666", reason = "indented header" }'
got="$(print_ignores "$SCRATCH/b41b.toml")"
run_guard_policy "$SCRATCH/b41b.toml" "$prefix"
rc=$?
if [[ "$got" == "--ignore RUSTSEC-2026-6666" && "$rc" -eq 0 ]]; then
  pass "an indented header is recognised"
else
  fail "an indented header must not fail a valid policy"
  printf '       derived: %s\n' "${got:-<empty>}"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 10: NEGATION — a file that is NOT valid TOML still fails closed,
# with the same Spanish error as before. Tolerance for the two valid-TOML
# shapes above is not permission to be lenient about a real parse error.
printf 'this is not a policy file at all\n' > "$SCRATCH/garbage.toml"
if run_guard_policy "$SCRATCH/garbage.toml" "$prefix"; then
  fail "a file that cannot be parsed into advisories must fail the guard"
else
  if grep -qF 'No se pudo extraer' "$SCRATCH/out"; then
    pass "an unparseable file still fails closed with the same Spanish error"
  else
    fail "an unparseable file failed the guard without the usual error"
    sed 's/^/       /' "$SCRATCH/out"
  fi
fi

# --- case 11: NEGATION — a header that is not a header (a trailing token
# after the bracket) must NOT be accepted as one: a malformed file still
# fails closed.
printf '[advisories] garbage\nignore = [\n    # Expires: 2099-12-31\n    { id = "RUSTSEC-2026-5555", reason = "x" },\n]\n' > "$SCRATCH/malformed.toml"
if run_guard_policy "$SCRATCH/malformed.toml" "$prefix"; then
  fail "a malformed table header must not be accepted as a header"
else
  if grep -qF 'No se pudo extraer' "$SCRATCH/out"; then
    pass "a malformed table header still fails closed"
  else
    fail "a malformed table header failed the guard without the usual error"
    sed 's/^/       /' "$SCRATCH/out"
  fi
fi

echo ""# --- #1984: an ignore that matches NOTHING is DEAD --------------------------
# The audit reports the advisories it finds; the fake reports a DIFFERENT
# one, so the declared ignore matches no crate in either lockfile. A dead
# ignore silences nothing today and would silence the finding the day its
# crate came back — so the guard must FAIL, naming it.
# The report SHAPE is cargo-audit's own: `warnings` is an object with an
# `unmaintained` and an `unsound` array, and `vulnerabilities` carries a
# `list`. A fake written to a GUESSED shape proves nothing: it agrees with
# the guess by construction. This one is the shape a real `cargo audit
# --json` printed, and the real guard run is the oracle.
AUDIT_JSON_MATCHED='{"warnings":{"unmaintained":[{"advisory":{"id":"RUSTSEC-2024-0436"}}],"unsound":[]},"vulnerabilities":{"found":false,"count":0,"list":[]}}'
AUDIT_JSON_OTHER='{"warnings":{"unmaintained":[{"advisory":{"id":"RUSTSEC-2099-0001"}}],"unsound":[]},"vulnerabilities":{"found":false,"count":0,"list":[]}}'
write_policy "$SCRATCH/dead.toml" "[advisories]" '{ id = "RUSTSEC-2024-0436", reason = "fixture: an advisory that matches no crate in the graph" }'

prefix="$(fake_cargo alive 0 "$AUDIT_JSON_MATCHED")"
if ( cd "$ROOT" && PATH="$prefix:$PATH" ADVISORY_POLICY_DENY_TOML="$SCRATCH/dead.toml" bash "$GUARD" --skip-duplicates ) >"$SCRATCH/out" 2>&1; then
  pass "an ignore that DOES match a crate passes the guard (#1984)"
else
  fail "an ignore that matches a crate must pass the guard (#1984): $(cat "$SCRATCH/out")"
fi

prefix="$(fake_cargo dead 0 "$AUDIT_JSON_OTHER")"
if ( cd "$ROOT" && PATH="$prefix:$PATH" ADVISORY_POLICY_DENY_TOML="$SCRATCH/dead.toml" bash "$GUARD" --skip-duplicates ) >"$SCRATCH/out" 2>&1; then
  fail "an ignore that matches NO crate must fail the guard (#1984)"
else
  if grep -qi 'muerto' "$SCRATCH/out"; then
    pass "an ignore that matches no crate fails the guard, naming it (#1984)"
  else
    fail "the guard must fail naming the dead ignore: $(cat "$SCRATCH/out")"
  fi
fi


if [ "$FAIL" -gt 0 ]; then
  echo "FAIL: $FAIL case(s) failed, $PASS passed"
  exit 1
fi
echo "OK: $PASS case(s) passed"
exit 0
