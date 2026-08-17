#!/usr/bin/env bash
# gen-cli-reference.sh
#
# Regenerates the auto-generated help block inside docs/src/cli-reference.md
# from the REAL webfang binary.
#
# Why it works the way it does:
#   - The binary is ALWAYS built first, with EXACTLY the release feature set
#     (`ai mcp ui` — see .github/workflows/release.yml). Feature-gated flags
#     (--tui, AI flags) must appear; dev-only features (e.g.
#     adaptive-selectors) must NOT. Incremental: a no-op when already up to
#     date, so this is cheap to run locally and in CI.
#   - Help output is captured via command substitution (`$(...)`), which
#     means stdout is NOT a tty: clap emits no ANSI styling and wraps at the
#     fixed non-interactive width. This keeps the captured bytes stable across
#     machines, so the `--check` drift gate is deterministic.
#   - The generated block lives between the CLI-REFERENCE:BEGIN/END markers in
#     the chapter. The surrounding narrative is hand-written; ONLY the block
#     between the markers is machine-owned.
#   - The block is consumed twice downstream: by `mdbook build` (the chapter
#     renders it as a code fence) and by the raw-file loop in the
#     "Build LLM markdown artifact" step of .github/workflows/docs.yml. The
#     help text itself contains triple-backtick fences (doctests), so the
#     wrapper fences use FOUR backticks.
#
# Modes:
#   scripts/gen-cli-reference.sh            generate (default): rewrite the block in place
#   scripts/gen-cli-reference.sh --check    CI drift gate: exit 1 if the chapter is stale

set -euo pipefail

CHAPTER="docs/src/cli-reference.md"
BEGIN_MARKER="CLI-REFERENCE:BEGIN"
END_MARKER="CLI-REFERENCE:END"
FEATURES="ai mcp ui"

mode="generate"
case "${1:-}" in
  "") mode="generate" ;;
  --check) mode="check" ;;
  *)
    echo "usage: scripts/gen-cli-reference.sh [--check]" >&2
    exit 2
    ;;
esac

# 1. Build with the release feature set (incremental no-op when up to date).
echo "Building webfang_cli (--features \"$FEATURES\")..."
cargo build -p webfang_cli --features "$FEATURES" --locked

# 2. Capture help via command substitution (non-tty ⇒ no ANSI, fixed wrap).
HELP_MAIN="$(target/debug/webfang --help)"
HELP_COMPLETIONS="$(target/debug/webfang completions --help)"

# 3. The chapter must contain both markers; refuse to touch anything else.
if ! grep -qF "$BEGIN_MARKER" "$CHAPTER"; then
  echo "error: $CHAPTER is missing the $BEGIN_MARKER marker" >&2
  exit 1
fi
if ! grep -qF "$END_MARKER" "$CHAPTER"; then
  echo "error: $CHAPTER is missing the $END_MARKER marker" >&2
  exit 1
fi

# 4. Rebuild the block between markers:
#    head (through the BEGIN marker line) + new block + tail (END marker onward).
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

sed -n "1,/$BEGIN_MARKER/p" "$CHAPTER" > "$TMP"
{
  printf '````text\n'
  printf '$ webfang --help\n'
  printf '\n'
  printf '%s\n' "$HELP_MAIN"
  printf '````\n'
  printf '\n'
  printf '````text\n'
  printf '$ webfang completions --help\n'
  printf '\n'
  printf '%s\n' "$HELP_COMPLETIONS"
  printf '````\n'
} >> "$TMP"
sed -n "/$END_MARKER/,\$p" "$CHAPTER" >> "$TMP"

if [ "$mode" = "check" ]; then
  if ! diff -u "$CHAPTER" "$TMP"; then
    echo "error: $CHAPTER is stale — run scripts/gen-cli-reference.sh" >&2
    exit 1
  fi
  echo "ok: $CHAPTER matches the binary help"
else
  mv "$TMP" "$CHAPTER"
  trap - EXIT
  echo "ok: regenerated the generated block in $CHAPTER"
fi
