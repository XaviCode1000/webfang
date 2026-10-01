#!/usr/bin/env bash
# check_advisory_policy.sh — the single source of truth for this repository's
# RustSec advisory policy (issue #1607: SC-01, SC-06, SC-07, SC-14).
#
# WHAT THIS IS FOR
# ================
# deny.toml holds the [advisories] ignore list; this script is the mechanism
# that makes deny.toml mean something, and the CI step in
# .github/workflows/ci.yml calls it instead of hand-written
# `cargo audit --ignore ...` flags. The flags are DERIVED from deny.toml, so
# there is no second copy to drift.
#
# THE DEFECT BEING FIXED (measured, not assumed)
# ==============================================
# `cargo audit` 0.22.2 exits 0 for unmaintained/unsound findings unless it is
# told to deny warnings. Over this workspace, offline, with the same local
# advisory DB:
#   cargo audit --no-fetch              -> exit 0, "11 allowed warnings found"
#   cargo audit --no-fetch -D warnings  -> exit 1, "11 denied warnings found"
# The pre-#1607 CI step ran `cargo audit --ignore ...` WITHOUT -D warnings, so a
# brand-new advisory was printed and the step still went green. This script
# always passes -D warnings; that flag is the fix. Suppressions stay
# suppressions — everything NOT in the deny.toml ignore list becomes an error.
#
# THE CHECKS (all fail closed; a missing tool is a FAILURE, never a skip)
# ======================================================================
#   1. Parse the [advisories] ignore list out of deny.toml. No advisory id is
#      hardcoded here: deny.toml is genuinely the single source of truth.
#   2. Duplicate ids in the ignore list are an error (a duplicate would let two
#      justifications disagree while the set comparison stayed happy).
#   3. Every entry needs a machine-greppable `# Expires: YYYY-MM-DD` comment
#      line immediately above it. Missing, unparseable, or PAST expiry is an
#      error, reported with the id, the date, and the days overdue.
#   4. The fail-closed knobs must still say what they say: [advisories]
#      unmaintained/unsound = "all", yanked = "deny", [sources]
#      unknown-registry/unknown-git = "deny". This is what makes deny.toml
#      fail-closed instead of merely permissive-by-default: a future edit that
#      relaxes a knob fails the build instead of silently opening the gate.
#   5. `cargo audit --no-fetch -D warnings <derived ignores>` must exit 0 for
#      BOTH Cargo.lock and fuzz/Cargo.lock (SC-14 — fuzz/ is not a workspace
#      member, so nothing else audits it).
#   6. Duplicate-crate baseline. cargo-deny 0.20.2 has NO `duplicate` key in
#      [bans], so deny.toml cannot fail closed on duplicate versions. This
#      script closes that gap one level up: the crates reported as
#      `warning[duplicate]` by `cargo deny check bans` are compared against the
#      committed DESCENDING RATCHET baseline scripts/duplicate-crate-baseline.txt
#      (the same idiom as scripts/quality-baselines.json), and a crate that is
#      not in the baseline FAILS. No [bans] key is set to "deny" — the
#      intentional version conflicts documented in AGENTS.md ("Crate version
#      conflicts (DO NOT unify)") must keep passing, and
#      `multiple-versions = "warn"` is a positive control.
#      A baseline line that no longer matches is reported as a WARNING, not a
#      failure: the ratchet is enforced in one direction only, and a converged
#      dependency graph should not have to fail anything to be noticed.
#
# MODES
# =====
#   bash scripts/check_advisory_policy.sh
#       Full run: all six checks. Non-zero exit on any failure. This is the
#       mode CI calls.
#   bash scripts/check_advisory_policy.sh --print-ignores
#       Pure printer: writes the derived flags to stdout, ONE FLAG PER LINE, as
#       `--ignore RUSTSEC-XXXX-YYYY`. Consumable directly by `xargs`:
#         xargs cargo audit --no-fetch -D warnings < <(bash scripts/check_advisory_policy.sh --print-ignores)
#       Performs no gating — it is a printer, so it stays usable as a debugging
#       aid even when the policy itself is broken.
#   bash scripts/check_advisory_policy.sh --print-duplicates
#       Prints the live `warning[duplicate]` crate set, one sorted name per line,
#       for regenerating scripts/duplicate-crate-baseline.txt. Writes nothing
#       itself; the header of that file documents the redirect.
#   bash scripts/check_advisory_policy.sh --skip-duplicates
#       Full run minus check 6. Exists because `cargo deny check bans` is the
#       slow part of the gate; skipping it must be a visible, deliberate flag
#       and never the default.
#
# ENVIRONMENT OVERRIDES (all optional; the defaults are the CI path)
# ==================================================================
#   ADVISORY_POLICY_DENY_TOML           path to the deny.toml to police
#   ADVISORY_POLICY_BASELINE            path to the duplicate-crate baseline
#   ADVISORY_POLICY_OFFLINE=1           add --no-fetch to cargo audit (use only
#                                       where there is no network; the default
#                                       FETCHES a fresh advisory DB, which is
#                                       what makes a new advisory visible)
#   ADVISORY_POLICY_DENY_BANS_OUTPUT    path to a pre-computed
#                                       `cargo deny check bans` output file,
#                                       used instead of running it
# These exist so the negative tests can mutate a copy instead of the real
# deny.toml. Precedent: IGNORED_GUARD_ROOT in check_ignored_guard.sh.
#
# FORMAT CONTRACT ON deny.toml (what makes the parses work)
# ========================================================
#   * The ignore array is `[advisories] ignore = [ ... ]`, one entry per line,
#     each an inline table `{ id = "RUSTSEC-XXXX-YYYY", reason = "..." }`.
#   * Every entry is immediately preceded by `# Expires: YYYY-MM-DD`.
#     Only a line containing the literal `Expires:` inside the array is read as
#     an expiry marker, so a justification sentence that merely mentions
#     "expiry" is harmless — but do not write "Expires:" in a justification.
#   * Only `id = "..."` entries are collected; `reason` is documentation and is
#     ignored by the parser.
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

# Byte collation, everywhere, always. Two reasons, both about determinism:
#   * `sort` is used to compare the live duplicate-crate set against the
#     committed baseline. Under a locale like es_ES.UTF-8, '-' and '_' are
#     ignored at the primary comparison level, so `r-efi` and `windows-sys`
#     sort into a DIFFERENT position than under LC_ALL=C. Both sides of the
#     comparison are sorted in this one process, so the gate itself stays
#     correct either way — but the committed baseline would then be written in
#     whatever order the author's locale produced, and regenerating it on
#     another machine would emit a whole-file diff that is pure collation.
#   * The deny.toml parsers match on ASCII. C locale keeps those matches
#     independent of whatever LC_* a CI runner or a developer's shell exports.
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DENY_TOML="${ADVISORY_POLICY_DENY_TOML:-$ROOT/deny.toml}"
BASELINE="${ADVISORY_POLICY_BASELINE:-$ROOT/scripts/duplicate-crate-baseline.txt}"
FUZZ_LOCK="$ROOT/fuzz/Cargo.lock"
ROOT_LOCK="$ROOT/Cargo.lock"

TODAY="$(date -u +%Y-%m-%d)"

# is_leap <year> — proleptic Gregorian, so the answer does not depend on where
# the epoch of the calendar is placed. The 10# prefix strips any leading zero
# and forces base 10, so "2027" and "02027" behave identically.
is_leap() {
    local y=$((10#$1))
    (( y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) ))
}

# days_in_month <year> <month>
#
# The month arrives ZERO-PADDED ("03"), and a `case` pattern of `3` does NOT
# match the string "03" — a subtlety that silently turns every valid date into
# "not a real date". Normalising with 10# first is load-bearing, not cosmetic.
days_in_month() {
    local y=$((10#$1)) m=$((10#$2))
    case "$m" in
        1|3|5|7|8|10|12) echo 31 ;;
        4|6|9|11) echo 30 ;;
        2) if is_leap "$y"; then echo 29; else echo 28; fi ;;
        *) echo 0 ;;
    esac
}

# calendar_date_is_real <year> <month> <day>
calendar_date_is_real() {
    local y="$1" m="$2" d="$3" max
    [[ "$y" =~ ^[0-9]{4}$ ]] || return 1
    [[ "$m" =~ ^[0-9]{2}$ ]] || return 1
    [[ "$d" =~ ^[0-9]{2}$ ]] || return 1
    (( 10#$m >= 1 && 10#$m <= 12 )) || return 1
    max="$(days_in_month "$y" "$m")"
    (( 10#$d >= 1 && 10#$d <= max ))
}

# days_from_civil <year> <month> <day> — days since 1970-01-01. Howard
# Hinnant's algorithm, so no `date -d`, no timezone, and no platform-specific
# flag: this script must behave identically on a maintainer's macOS and on an
# ubuntu-latest runner.
days_from_civil() {
    local y=$((10#$1)) m=$((10#$2)) d=$((10#$3))
    local era yoe doy doe
    (( y -= (m <= 2) ))
    if (( y >= 0 )); then
        era=$((y / 400))
    else
        era=$(((y - 399) / 400))
    fi
    yoe=$((y - era * 400))
    # Spelled out with named intermediates rather than the compact
    # `doy=$(((153 * (m - 3) + 2) / 5) + d - 1)`: a `$((` immediately followed
    # by `(` is parsed as a nested command substitution by some bash builds, and
    # the failure is a syntax error that would take the whole gate down.
    local shifted=0
    if (( m > 2 )); then
        shifted=$((153 * (m - 3) + 2))
    else
        shifted=$((153 * (m + 9) + 2))
    fi
    doy=$((shifted / 5 + d - 1))
    doe=$((yoe * 365 + yoe / 4 - yoe / 100 + doy))
    echo $((era * 146097 + doe - 719468))
}

# days_between <y1> <m1> <d1> <y2> <m2> <d2> — whole days FROM date 1 TO date 2.
# Positive when date 2 is later. `days_from_civil` counts UP from 1970, so the
# subtraction is date2 - date1; getting this backwards would report a 171-day
# overdue ignore as 171 days in the future.
days_between() {
    echo $(( $(days_from_civil "$4" "$5" "$6") - $(days_from_civil "$1" "$2" "$3") ))
}

MODE="check"
SKIP_DUPLICATES=0
case "${1:-}" in
    "")             MODE="check" ;;
    --print-ignores) MODE="print-ignores" ;;
    --print-duplicates) MODE="print-duplicates" ;;
    --skip-duplicates) MODE="check"; SKIP_DUPLICATES=1 ;;
    *)
        echo "usage: bash scripts/check_advisory_policy.sh [--print-ignores|--print-duplicates|--skip-duplicates]" >&2
        exit 2
        ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[[ -f "$DENY_TOML" ]] || {
    echo "::error::deny.toml no encontrado en $DENY_TOML — la política de avisos no se puede leer."
    exit 1
}

# ---------------------------------------------------------------------------
# Parse deny.toml
# ---------------------------------------------------------------------------

# Ignore entries, one "id<TAB>expiry" record per line, in file order.
# Inside the [advisories] ignore array: a line containing `Expires:` sets the
# pending expiry, and the next RUSTSEC- id consumes it. An unparseable expiry
# is carried through verbatim so the check below can report it as unparseable
# rather than silently as missing.
parse_ignore_entries() {
    awk -F'\t' -v OFS='\t' '
        /^\[.*\]$/ {
            section = $0
            sub(/^\[/, "", section)
            sub(/\]$/, "", section)
            in_ignore = 0
            next
        }
        section == "advisories" && !in_ignore && $0 ~ /^[[:space:]]*ignore[[:space:]]*=/ { in_ignore = 1 }
        in_ignore && $0 ~ /^[[:space:]]*\]/ { in_ignore = 0; next }
        !in_ignore { next }
        /Expires:/ {
            value = $0
            sub(/^.*Expires:[[:space:]]*/, "", value)
            sub(/[[:space:]]*$/, "", value)
            pending = value
            next
        }
        match($0, /"RUSTSEC-[0-9]{4}-[0-9]{4}"/) && $0 ~ /id[[:space:]]*=[[:space:]]*"RUSTSEC-/ {
            # `id =` is required here because a `reason` string may legitimately
            # quote another advisory id (e.g. "same path as RUSTSEC-2026-0183")
            # and that must not be counted as a second entry.
            quoted = substr($0, RSTART, RLENGTH)
            gsub(/"/, "", quoted)
            print quoted, (pending == "" ? "-" : pending)
            pending = ""
            next
        }
    ' "$1"
}

# "section.key=value" for every top-level `key = "value"` line in the file.
parse_scalar_keys() {
    awk '
        /^\[.*\]$/ {
            section = $0
            sub(/^\[/, "", section)
            sub(/\]$/, "", section)
            next
        }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            eq = index(line, "=")
            if (eq == 0) { next }
            key = substr(line, 1, eq - 1)
            value = substr(line, eq + 1)
            gsub(/[[:space:]]/, "", key)
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            gsub(/"/, "", value)
            print section "." key "=" value
        }
    ' "$1"
}

parse_ignore_entries "$DENY_TOML" >"$WORK/entries.tsv"
parse_scalar_keys "$DENY_TOML" >"$WORK/keys.txt"

IDS=()
while IFS=$'\t' read -r id expiry; do
    IDS+=("$id")
done <"$WORK/entries.tsv"

# The derived --ignore flags: one per line, `--ignore <ID>`.
derive_ignore_flags() {
    local id
    for id in "${IDS[@]}"; do
        printf -- '--ignore %s\n' "$id"
    done
}

scalar() {
    local key="$1"
    sed -n "s/^${key}=//p" "$WORK/keys.txt" | tail -1
}

# --- Mode: --print-ignores -------------------------------------------------
if [[ "$MODE" == "print-ignores" ]]; then
    derive_ignore_flags
    exit 0
fi

# ---------------------------------------------------------------------------
# Structural checks: 1 (parse), 2 (duplicates), 3 (expiry), 4 (knobs)
# ---------------------------------------------------------------------------
# These run BEFORE the slow tools and short-circuit on failure: when the policy
# is already broken there is nothing to learn from a 4-minute cargo deny run,
# and running it against a mutated deny.toml would only muddy the report.
STRUCT_FAIL=0

if [[ ${#IDS[@]} -eq 0 ]]; then
    echo "::error::No se pudo extraer ningún aviso de [advisories] ignore en $DENY_TOML — el formato cambió bajo el guard."
    STRUCT_FAIL=1
fi

# Check 2: duplicate ids.
DUP_IDS="$(printf '%s\n' "${IDS[@]:-}" | sort | uniq -d || true)"
if [[ -n "$DUP_IDS" ]]; then
    while read -r dup; do
        [[ -n "$dup" ]] || continue
        echo "::error::ID de aviso duplicado en [advisories] ignore: $dup — dos justificaciones pueden discrepar mientras la comparación de conjuntos sigue en verde. Deja una sola entrada."
    done <<<"$DUP_IDS"
    STRUCT_FAIL=1
fi

# Check 3: expiry present, parseable, and not in the past.
while IFS=$'\t' read -r id expiry; do
    if [[ "$expiry" == "-" || -z "$expiry" ]]; then
        echo "::error::$id no tiene línea '# Expires: YYYY-MM-DD' inmediatamente encima. Sin fecha de expiración, un ignore es eterno — que es exactamente el defecto de #1607."
        STRUCT_FAIL=1
        continue
    fi
    if ! [[ "$expiry" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
        echo "::error::$id tiene un '# Expires:' ilegible ('$expiry'); se exige el formato YYYY-MM-DD."
        STRUCT_FAIL=1
        continue
    fi
    # Calendar validity, WITHOUT `date -d`. GNU date has -d; BSD/macOS date does
    # not, and this script must behave identically on a maintainer's Mac and on
    # an ubuntu-latest runner — a `date -u -d` that silently yields nothing there
    # would report every entry as "not a real date" and fail a policy that is
    # perfectly valid. Arithmetic on YYYYMMDD integers is locale- and
    # implementation-independent, and the real comparison below needs no epoch
    # either; the epoch is only used to phrase the overdue count.
    y="${expiry:0:4}" m="${expiry:5:2}" d="${expiry:8:2}"
    if ! calendar_date_is_real "$y" "$m" "$d"; then
        echo "::error::$id declara '# Expires: $expiry', que no es una fecha real."
        STRUCT_FAIL=1
        continue
    fi
    if [[ "${expiry//-/}" -lt "${TODAY//-/}" ]]; then
        # Days overdue, computed from the civil date rather than from `date`, so
        # the number is identical on GNU and BSD userlands.
        days_overdue=$(days_between "$y" "$m" "$d" "${TODAY:0:4}" "${TODAY:5:2}" "${TODAY:8:2}")
        echo "::error::$id expiró el $expiry (hace $days_overdue días; hoy es $TODAY). Resuelve el aviso o renueva la entrada con justificación nueva — nunca dejes un ignore vencido en silencio."
        STRUCT_FAIL=1
    fi
done <"$WORK/entries.tsv"

# Check 4: the fail-closed knobs have not regressed.
check_knob() {
    local section_key="$1" expected="$2" actual
    actual="$(scalar "$section_key")"
    if [[ -z "$actual" ]]; then
        echo "::error::Falta la clave $section_key en $DENY_TOML — el guard no puede verificar un knob ausente. Decláralo explícitamente como \"$expected\"."
        STRUCT_FAIL=1
    elif [[ "$actual" != "$expected" ]]; then
        echo "::error::Regresión de knob fail-closed: $section_key es \"$actual\" y debe ser \"$expected\". Relajar un knob abre la puerta en silencio; revierte el cambio o actualiza el guard en el mismo commit, con su justificación."
        STRUCT_FAIL=1
    fi
}
check_knob "advisories.unmaintained" "all"
check_knob "advisories.unsound" "all"
check_knob "advisories.yanked" "deny"
check_knob "sources.unknown-registry" "deny"
check_knob "sources.unknown-git" "deny"

if [[ "$STRUCT_FAIL" -ne 0 ]]; then
    echo "::error::La política de avisos en $DENY_TOML está rota; no se ejecutó cargo audit ni cargo deny (los chequeos estructurales ya fallaron)."
    exit 1
fi

# Progress/status lines go to STDERR so that the two printer modes emit a pure
# stdout stream: `--print-duplicates > scripts/duplicate-crate-baseline.txt`
# must not capture a status line as if it were a crate name.
echo "advisory policy: ${#IDS[@]} ignore entries from deny.toml, all unique, none expired, knobs intact." >&2

# ---------------------------------------------------------------------------
# Check 6a (prerequisite): the live duplicate-crate set
# ---------------------------------------------------------------------------
# Resolved before the audit so `--print-duplicates` and the comparison share one
# code path. `cargo deny` reads deny.toml from the working directory, so this
# runs from the repo root.
collect_duplicates() {
    local out="$1"
    if [[ -n "${ADVISORY_POLICY_DENY_BANS_OUTPUT:-}" ]]; then
        if [[ ! -f "$ADVISORY_POLICY_DENY_BANS_OUTPUT" ]]; then
            echo "::error::ADVISORY_POLICY_DENY_BANS_OUTPUT apunta a $ADVISORY_POLICY_DENY_BANS_OUTPUT, que no existe."
            return 1
        fi
        cp "$ADVISORY_POLICY_DENY_BANS_OUTPUT" "$out"
        return 0
    fi
    if ! command -v cargo-deny >/dev/null 2>&1; then
        echo "::error::cargo-deny no está instalado y este guard NO se salta. Instálalo con: cargo install cargo-deny --locked"
        return 1
    fi
    (cd "$ROOT" && cargo deny check bans) >"$out" 2>&1 || true
    return 0
}

extract_duplicate_crates() {
    # "warning[duplicate]: found N duplicate entries for crate 'X'" -> X
    grep -oE "found [0-9]+ duplicate entries for crate '[^']+'" "$1" \
        | sed -E "s/.*crate '([^']+)'/\1/" \
        | sort -u \
        || true
}

if [[ "$MODE" == "print-duplicates" || "$SKIP_DUPLICATES" -eq 0 ]]; then
    if ! collect_duplicates "$WORK/deny-bans.txt"; then
        exit 1
    fi
    extract_duplicate_crates "$WORK/deny-bans.txt" >"$WORK/live-dups.txt"
fi

if [[ "$MODE" == "print-duplicates" ]]; then
    cat "$WORK/live-dups.txt"
    exit 0
fi

# ---------------------------------------------------------------------------
# Check 5: cargo audit --no-fetch -D warnings over BOTH lockfiles
# ---------------------------------------------------------------------------
# Not installed means FAIL, not skip: a missing auditor must never read as a
# green gate. `-D warnings` is the whole point — without it cargo audit exits 0
# on a finding, which is the SC-01/SC-07 defect.
if ! command -v cargo-audit >/dev/null 2>&1; then
    echo "::error::cargo-audit no está instalado y este guard NO se salta: sin auditor, un aviso nuevo no puede detectarse y el paso leería verde. Instálalo con: cargo install cargo-audit --locked"
    exit 1
fi

AUDIT_FETCH_ARGS=()
if [[ "${ADVISORY_POLICY_OFFLINE:-0}" == "1" ]]; then
    AUDIT_FETCH_ARGS+=(--no-fetch)
fi

audit_lock() {
    local lock="$1" label="$2"
    local -a args=("${AUDIT_FETCH_ARGS[@]}" -D warnings --file "$lock")
    local id
    for id in "${IDS[@]}"; do
        args+=(--ignore "$id")
    done
    echo "--- cargo audit ${label}: $(basename "$lock") (${#IDS[@]} derived ignores, -D warnings)" >&2
    if cargo audit "${args[@]}"; then
        return 0
    fi
    echo "::error::cargo audit falló sobre $lock. Un hallazgo NO listado en [advisories] ignore de deny.toml es un error (por eso -D warnings). Si el hallazgo es legítimo, añádelo a deny.toml con motivo, seguimiento y '# Expires:' — nunca lo silencies en el comando de CI."
    return 1
}

AUDIT_FAIL=0
audit_lock "$ROOT_LOCK" "workspace" || AUDIT_FAIL=1
if [[ -f "$FUZZ_LOCK" ]]; then
    # SC-14: fuzz/ is not a workspace member, so nothing else audits this lock.
    audit_lock "$FUZZ_LOCK" "fuzz" || AUDIT_FAIL=1
else
    echo "::error::fuzz/Cargo.lock no existe (SC-14): el lock del harness de fuzz debe auditarse también. Si el harness se eliminó a propósito, bórralo en el mismo commit que este hallazgo."
    AUDIT_FAIL=1
fi

# ---------------------------------------------------------------------------
# Check 6b: the duplicate-crate ratchet
# ---------------------------------------------------------------------------
DUP_FAIL=0
if [[ "$SKIP_DUPLICATES" -eq 1 ]]; then
    echo "duplicate-crate baseline: SKIPPED (--skip-duplicates passed explicitly)." >&2
else
    [[ -f "$BASELINE" ]] || {
        echo "::error::Baseline de crates duplicados no encontrado en $BASELINE. [bans] de cargo-deny 0.20.2 no tiene clave 'duplicate', así que este archivo es el único knob fail-closed para duplicados. Genera el contenido inicial con: bash scripts/check_advisory_policy.sh --print-duplicates > $BASELINE"
        DUP_FAIL=1
    }
    if [[ "$DUP_FAIL" -eq 0 ]]; then
        # Strip comments/blank lines, then require sorted + unique: a hand-edited
        # baseline that drifts from canonical form is itself a review hazard.
        grep -vE '^[[:space:]]*(#|$)' "$BASELINE" | sed 's/[[:space:]]*$//' | sort -u >"$WORK/base-dups.txt" || true
        if [[ ! -s "$WORK/base-dups.txt" ]]; then
            echo "::error::$BASELINE no contiene ningún nombre de crate. Un baseline vacío rechazaría todo duplicado conocido; revisa el formato (un nombre por línea)."
            DUP_FAIL=1
        fi
    fi
    if [[ "$DUP_FAIL" -eq 0 ]]; then
        NEW_DUPS="$(comm -13 "$WORK/base-dups.txt" "$WORK/live-dups.txt" || true)"
        if [[ -n "$NEW_DUPS" ]]; then
            while read -r crate; do
                [[ -n "$crate" ]] || continue
                echo "::error::Crate duplicado NUEVO (no está en el baseline descendente): $crate. Una dependencia ahora trae dos versiones. Resuélvelo (bump/unificar) o, si el conflicto es intencional, añádelo a $BASELINE en un commit que explique por qué — nunca subas el baseline para silenciar el gate."
            done <<<"$NEW_DUPS"
            DUP_FAIL=1
        fi
        STALE_DUPS="$(comm -23 "$WORK/base-dups.txt" "$WORK/live-dups.txt" || true)"
        if [[ -n "$STALE_DUPS" ]]; then
            # Advisory only: the graph converged, which is a win, not a failure.
            echo "--- Crates duplicados que ya no aparecen (buena noticia; bórralos de $BASELINE en un commit aparte):"
            while read -r crate; do
                [[ -n "$crate" ]] || continue
                echo "  - $crate"
            done <<<"$STALE_DUPS"
        fi
    fi
    if [[ "$DUP_FAIL" -eq 0 ]]; then
        echo "duplicate-crate baseline: $(wc -l <"$WORK/live-dups.txt" | tr -d ' ') live crates, all in baseline." >&2
    fi
fi

if [[ "$AUDIT_FAIL" -ne 0 || "$DUP_FAIL" -ne 0 ]]; then
    exit 1
fi

if [[ "$SKIP_DUPLICATES" -eq 1 ]]; then
    echo "OK: advisory policy is fail-closed (issue #1607) — ${#IDS[@]} justified ignores, no expired expiry. Duplicate-crate ratchet NOT EVALUATED (--skip-duplicates)."
else
    echo "OK: advisory policy is fail-closed (issue #1607) — ${#IDS[@]} justified ignores, no expired expiry, no new duplicate crate."
fi
