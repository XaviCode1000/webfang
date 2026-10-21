#!/usr/bin/env bash
# scripts/test_artifact_llm_bundle.sh — semantics harness for
# scripts/build-llm-artifact.sh (audit B21, B29, B30).
#
# The canonical audit harness (webfang-audit/.../repro/artifact_repro.sh)
# observes the OUTCOME of one scenario per finding. This harness pins the
# CONTRACTS, so a regression is caught without the audit:
#
#   B21  The artifact is published whole or not at all. A failure in the
#        middle of the rendering (crate 3 of 3) must leave NO publishable
#        output at the final path — not a directory holding 01-/03- and
#        nothing else — and no staging directory behind.
#   B29  The fence state keys on the flavour of the OPENING marker: a ~~~ line
#        inside a ``` block is content, so a heading inside the block is not
#        demoted and the heading after the block IS.
#   B30  The artifact is byte-reproducible: the same inputs and the same
#        SOURCE_DATE_EPOCH produce the same bytes, and an injected wall-clock
#        BUILD_DATE does not change them.
#
# Style: user-facing messages in Spanish (repo convention for scripts/);
# internal output in English.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/build-llm-artifact.sh"

# Scratch lives on a real disk, never on the tmpfs of /tmp: the audit
# campaign refuses a tmpfs scratch for the same reason.
SCRATCH="${WEBSITE_TEST_SCRATCH:-$HOME/.cache/tmp/webfang-artifact-tests}"
mkdir -p "$SCRATCH" || exit 2
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs | ramfs)
    echo "refusing tmpfs scratch: $SCRATCH" >&2
    exit 2
    ;;
esac
WORKDIR="$(mktemp -d "$SCRATCH/llm-bundle.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

pass=0
fail=0

ok() {
  echo "  PASS  $1"
  pass=$((pass + 1))
}

bad() {
  echo "  FAIL  $1"
  fail=$((fail + 1))
}

# The fixture: a docs tree with the five chapters the script demands, a
# rustdoc-markdown stub and nothing else. The stub emits a page with a stray
# "~~~" inside a ``` block on purpose (B29) and counts its invocations so a
# failure can be injected at a chosen crate (B21).
make_fixture() { # <dir>
  local d="$1" f
  mkdir -p "$d/docs/src" "$d/docs/book" "$d/stub"
  for f in overview debugging testing troubleshooting cli-reference; do
    printf '# %s\n\nSome body text.\n' "$f" >"$d/docs/src/$f.md"
  done
  cat >"$d/stub/rustdoc-markdown" <<'STUBEOF'
#!/usr/bin/env bash
out=""
crate=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) out="$2"; shift 2 ;;
    print) crate="$2"; shift 2 ;;
    *) shift ;;
  esac
done
count_file="${RDM_COUNT:-/dev/null}"
n="$([[ -f "$count_file" ]] && cat "$count_file" || echo 0)"
n=$((n + 1))
printf '%s' "$n" >"$count_file"
if [[ "${RDM_FAIL_AT:-0}" == "$n" ]]; then
  echo "stub: injected failure on crate invocation $n" >&2
  exit 3
fi
cat >"$out" <<'PAGEEOF'
# API (0.1.0)

## Structs

```rust
# inside-fence-heading
~~~
# phantom
```

## Real Section

Body text.
PAGEEOF
exit 0
STUBEOF
  chmod +x "$d/stub/rustdoc-markdown"
  printf '0\n' >"$d/stub/count"
}

# md5 of a whole tree: the artifact's bytes, not its metadata.
tree_digest() { # <dir>
  find "$1" -type f | sort | xargs md5sum 2>/dev/null | md5sum | cut -d' ' -f1
}

# The H1 count of a file, ignoring headings inside code fences.
count_h1() { # <file>
  awk '
    function fence_run(line,   n) {
      n = 0
      while (substr(line, n + 1, 1) == fence_char) n++
      return n
    }
    /^(`{3,}|~{3,})/ {
      if (in_fence) {
        if (fence_run($0) >= fence_len && $0 ~ "^" fence_char "{3,}[ \t]*$") in_fence = 0
        next
      }
      fence_char = substr($0, 1, 1)
      fence_len = fence_run($0)
      in_fence = 1
      next
    }
    !in_fence && /^# / { c++ }
    END { print c + 0 }' "$1"
}

echo "build-llm-artifact.sh"

# --- B21: a failure mid-render leaves NO publishable output ----------------
# NEGATION: before the fix, a failure on crate 3 left 01-/03- published with
# no 00-narrative, no 06-cli-reference and no llms.txt — a partial directory
# that LOOKS publishable. This case accepts that directory.
fixture="$WORKDIR/b21-failure"
make_fixture "$fixture"
out="$(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" RDM_FAIL_AT=3 bash "$SCRIPT" 2>&1)"
rc=$?
if [ "$rc" -ne 0 ]; then
  ok "B21 a failure on crate 3 exits non-zero (rc=$rc)"
else
  bad "B21 a failure on crate 3 exits non-zero (rc=$rc)"
fi
if [ ! -e "$fixture/docs/book/webfang-docs-llm" ]; then
  ok "B21 the output directory does not exist after the failure"
else
  bad "B21 the output directory does not exist after the failure"
fi
leftovers="$(ls -A "$fixture/docs/book" 2>/dev/null | tr '\n' ' ')"
if [ -z "$leftovers" ]; then
  ok "B21 no staging directory is left behind"
else
  bad "B21 no staging directory is left behind (found: $leftovers)"
fi

# --- B21: a successful run publishes the WHOLE file set --------------------
fixture="$WORKDIR/b21-success"
make_fixture "$fixture"
success_fixture="$fixture"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" bash "$SCRIPT" >/dev/null 2>&1)
missing=""
for f in llms.txt 00-narrative.md 01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md webfang-docs-full.md; do
  [ -s "$fixture/docs/book/webfang-docs-llm/$f" ] || missing="$missing $f"
done
if [ -z "$missing" ]; then
  ok "B21 the published directory holds the whole expected file set"
else
  bad "B21 the published directory holds the whole expected file set (missing:$missing)"
fi
bad_h1=""
for f in 00-narrative.md 01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md; do
  n="$(count_h1 "$fixture/docs/book/webfang-docs-llm/$f")"
  [ "$n" -eq 1 ] || bad_h1="$bad_h1 $f($n)"
done
if [ -z "$bad_h1" ]; then
  ok "B21 every published source file carries exactly one H1"
else
  bad "B21 every published source file carries exactly one H1 ($bad_h1)"
fi

# --- B21: a failed re-run does not leave the previous artifact behind -------
# The contract is "the artifact is published whole or not at all": a producer
# that cannot finish must leave NOTHING publishable at the final path.
fixture="$WORKDIR/b21-refail"
make_fixture "$fixture"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" bash "$SCRIPT" >/dev/null 2>&1)
if [ -d "$fixture/docs/book/webfang-docs-llm" ]; then
  printf '0\n' >"$fixture/stub/count"
  (cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" RDM_FAIL_AT=1 bash "$SCRIPT" >/dev/null 2>&1)
  if [ ! -e "$fixture/docs/book/webfang-docs-llm" ]; then
    ok "B21 a failed re-run leaves no publishable output at the final path"
  else
    bad "B21 a failed re-run leaves no publishable output at the final path"
  fi
else
  bad "B21 the first run published the artifact (precondition)"
fi

# --- B29: the fence state keys on the flavour of the opening marker --------
# NEGATION: before the fix, a ~~~ line inside a ``` block closed the fence, so
# the heading inside the block was demoted and the heading right after the
# block was skipped. This case accepts both.
rendered="$success_fixture/docs/book/webfang-docs-llm/01-webfang_core.md"
if [ -f "$rendered" ]; then
  if grep -qx '# phantom' "$rendered"; then
    ok 'B29 a heading inside a ``` block is not demoted (stray ~~~ ignored)'
  else
    bad 'B29 a heading inside a ``` block is not demoted (stray ~~~ ignored)' 
  fi
  if grep -qx '### Real Section' "$rendered"; then
    ok "B29 the heading after the fence is demoted (fence state intact)"
  else
    bad "B29 the heading after the fence is demoted (fence state intact)"
    grep -n 'Real Section' "$rendered" | sed 's/^/        /'
  fi
else
  bad "B29 the rendered API file exists (precondition)"
fi

# --- B30: the artifact is byte-reproducible --------------------------------
# NEGATION: before the fix, the date came from the injected BUILD_DATE (a wall
# clock), so the same commit produced different bytes on every run and the
# reproducibility claim could not be made. These cases accept that drift.
fixture="$WORKDIR/b30"
make_fixture "$fixture"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" >/dev/null 2>&1)
digest_a="$(tree_digest "$fixture/docs/book/webfang-docs-llm")"
rm -rf "$fixture/docs/book/webfang-docs-llm"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" SOURCE_DATE_EPOCH=1700000000 bash "$SCRIPT" >/dev/null 2>&1)
digest_b="$(tree_digest "$fixture/docs/book/webfang-docs-llm")"
if [ -n "$digest_a" ] && [ "$digest_a" = "$digest_b" ]; then
  ok "B30 two runs with the same SOURCE_DATE_EPOCH are byte-identical"
else
  bad "B30 two runs with the same SOURCE_DATE_EPOCH are byte-identical ($digest_a vs $digest_b)"
fi
rm -rf "$fixture/docs/book/webfang-docs-llm"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" SOURCE_DATE_EPOCH=1700000000 BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" bash "$SCRIPT" >/dev/null 2>&1)
digest_c="$(tree_digest "$fixture/docs/book/webfang-docs-llm")"
if [ "$digest_a" = "$digest_c" ]; then
  ok "B30 an injected wall-clock BUILD_DATE does not change the bytes"
else
  bad "B30 an injected wall-clock BUILD_DATE does not change the bytes"
fi
if grep -q 'Generated: 2023-11-14T22:13:20Z' "$fixture/docs/book/webfang-docs-llm/llms.txt"; then
  ok "B30 the build date is derived from SOURCE_DATE_EPOCH"
else
  bad "B30 the build date is derived from SOURCE_DATE_EPOCH"
  grep 'Generated:' "$fixture/docs/book/webfang-docs-llm/llms.txt" | sed 's/^/        /'
fi
rm -rf "$fixture/docs/book/webfang-docs-llm"
(cd "$fixture" && PATH="$fixture/stub:$PATH" RDM_COUNT="$fixture/stub/count" bash "$SCRIPT" >/dev/null 2>&1)
if grep -qE 'Generated: [0-9]{4}-[0-9]{2}-[0-9]{2}' "$fixture/docs/book/webfang-docs-llm/llms.txt"; then
  ok "B30 without SOURCE_DATE_EPOCH the date falls back to the UTC day"
else
  bad "B30 without SOURCE_DATE_EPOCH the date falls back to the UTC day"
  grep 'Generated:' "$fixture/docs/book/webfang-docs-llm/llms.txt" | sed 's/^/        /'
fi

echo
if [ "$fail" -eq 0 ]; then
  echo "build-llm-artifact.sh: $pass passed, 0 failed"
  exit 0
fi
echo "build-llm-artifact.sh: $pass passed, $fail failed"
exit 1
