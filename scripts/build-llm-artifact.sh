#!/usr/bin/env bash
# ============================================================================
# build-llm-artifact.sh — assemble the LLM/RAG documentation artifact (#734)
#
# Emits docs/book/webfang-docs-llm/, an llms.txt v2 directory: llms.txt index,
# per-source files (00-narrative, 01..05-<crate>, 06-cli-reference — one H1
# each) and webfang-docs-full.md, the back-compat monolith (pre-#734 single
# file, unchanged heading conventions).
#
# Prerequisites (validated up front — this script does NOT build them):
#   mdbook build docs                                → docs/src chapters exist
#   cargo doc --workspace --all-features --no-deps   → target/doc/<crate>/ HTML
#
# BUILD_DATE: injected by CI (docs.yml). Local runs leave it empty; only
# llms.txt then carries a "Generated: local run" note.
# ============================================================================
set -euo pipefail

OUT_DIR=docs/book/webfang-docs-llm
BUILD_DATE="${BUILD_DATE:-}"
PAGES_PREFIX="https://xavicode1000.github.io/webfang/webfang-docs-llm"
CHAPTERS="overview debugging testing troubleshooting tui-unified-design"
# webfang_cli is bin-only: cargo doc produces no target/doc/webfang_cli/, so
# it is skipped by the same -d check the old inline step used.
ALL_CRATES="webfang_core webfang_ai webfang_tui webfang_mcp webfang_cli webfang_test_utils"

for f in $CHAPTERS cli-reference; do
  if [ ! -f "docs/src/$f.md" ]; then
    echo "ERROR: docs/src/$f.md is missing. Prerequisite: mdbook build docs" >&2
    exit 1
  fi
done
if [ ! -d target/doc/webfang_core ]; then
  echo "ERROR: target/doc/<crate>/ HTML is missing. Prerequisite:" >&2
  echo "       cargo doc --workspace --all-features --no-deps" >&2
  exit 1
fi

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Two metadata lines (Generated + blank) in CI, nothing locally.
generated_line() {
  if [ -n "$BUILD_DATE" ]; then
    echo "Generated: $BUILD_DATE"
    echo
  fi
}

# Demote every heading by exactly ONE level (deepest first, no double demotion).
demote_headings() {
  sed -e 's/^### /#### /' -e 's/^## /### /' -e 's/^# /## /' "$1"
}

# Strip rustdoc noise — the EXACT pipeline previously inlined in docs.yml.
# Writes target/doc/$1.md, then applies the fence-aware noise gate.
render_api() {
  local crate="$1"
  {
    pandoc -f html -t gfm --wrap=none "target/doc/$crate/index.html" 2>/dev/null
    find "target/doc/$crate" -name '*.html' ! -name 'index.html' ! -path '*/src/*' -print0 \
      | sort -z \
      | while IFS= read -r -d '' fl; do
          pandoc -f html -t gfm --wrap=none "$fl" 2>/dev/null
        done
  } \
  | sed -e 's/Copy item path//g' \
        -e 's/Expand description//g' \
        -e 's/§//g' \
        -e 's/<span[^>]*>//g' -e 's/<\/span>//g' \
        -e 's/<div[^>]*>//g' -e 's/<\/div>//g' \
        -e 's/<a [^>]*>//g' -e 's/<\/a>//g' \
        -e 's/<[^>]*>//g' \
        -e 's/Show [0-9][0-9]* fields[[:space:]]*//g' \
        -e 's/Show [0-9][0-9]* variants[[:space:]]*//g' \
        -e 's/^# \(Struct\|Function\|Enum\|Trait\|Constant\|Type\|Crate\|Macro\|List\) /### \1 /' \
  | awk 'BEGIN{skip=0} /^## Blanket Implementations|^## Auto Trait Implementations/{skip=1; next} skip && (/^## / || /^### (Struct|Function|Enum|Trait|Constant|Type|Crate|Macro|List) /){skip=0} !skip{print}' \
  | sed -e '/^[[:space:]]*$/N;/^\n[[:space:]]*$/D' \
  > "target/doc/$crate.md"
  # Gate is fence-aware: doctests may legitimately contain HTML inside ``` fences.
  if awk 'BEGIN{f=0} /^```/{f=!f; next} !f && /Expand description|Copy item path|href="|<a |<span|<div|<abbr|<code|§/{bad=1} END{exit !bad}' "target/doc/$crate.md"; then
    echo "::error::rustdoc noise not fully stripped for $crate"
    exit 1
  fi
}

# mdBook chapters with their leading H1 downgraded to $1 (body untouched).
emit_chapters() {
  local level="$1" f
  for f in $CHAPTERS; do
    sed "1s/^# /$level /" "docs/src/$f.md"
    echo
    echo "---"
    echo
  done
}

per_source_file() {
  case "$1" in
    webfang_core) echo "01-webfang_core.md" ;;
    webfang_ai) echo "02-webfang_ai.md" ;;
    webfang_tui) echo "03-webfang_tui.md" ;;
    webfang_mcp) echo "04-webfang_mcp.md" ;;
    webfang_test_utils) echo "05-webfang_test_utils.md" ;;
    *) echo "" ;;
  esac
}

crate_description() {
  case "$1" in
    webfang_core) echo "core domain/application/infrastructure API" ;;
    webfang_ai) echo "ONNX embeddings and semantic cleaning API" ;;
    webfang_tui) echo "ratatui TUI selector API" ;;
    webfang_mcp) echo "MCP server (35 tools) API" ;;
    webfang_test_utils) echo "shared test utilities API" ;;
    *) echo "" ;;
  esac
}

# Phase 1: render + noise-gate every crate API (as the old inline step did).
for crate in $ALL_CRATES; do
  [ -d "target/doc/$crate" ] || continue
  render_api "$crate"
done

# Phase 2: per-source files (single H1 each).
{
  echo "# WebFang Narrative Documentation"
  generated_line
  echo "---"
  echo
  emit_chapters "##"
} > "$OUT_DIR/00-narrative.md"

for crate in $ALL_CRATES; do
  out_file=$(per_source_file "$crate")
  [ -n "$out_file" ] && [ -d "target/doc/$crate" ] || continue
  {
    echo "# $crate API Reference"
    generated_line
    echo "---"
    echo
    demote_headings "target/doc/$crate.md"
    echo
  } > "$OUT_DIR/$out_file"
done

{
  echo "# WebFang CLI Reference"
  generated_line
  echo "---"
  echo
  sed '1s/^# /## /' docs/src/cli-reference.md
} > "$OUT_DIR/06-cli-reference.md"

# Phase 3: back-compat monolith (byte-equivalent to the pre-#734 output).
{
  echo "# WebFang Documentation"
  echo
  generated_line
  echo "---"
  echo
  echo "## Narrative Documentation"
  echo
  emit_chapters "###"
  echo "## CLI Reference"
  echo
  sed '1s/^# /### /' docs/src/cli-reference.md
  echo
  echo "---"
  echo
  echo "## API Reference (rustdoc)"
  echo
  for crate in $ALL_CRATES; do
    if [ -d "target/doc/$crate" ]; then
      echo "### API: $crate"
      echo
      cat "target/doc/$crate.md"
      echo
      echo "---"
      echo
    fi
  done
} > "$OUT_DIR/webfang-docs-full.md"

# Phase 4: llms.txt (v2 index).
{
  echo "# WebFang"
  echo
  echo "> Production-ready web scraper: Clean Architecture, TUI selector, AI semantic cleaning, sitemap-based crawling. This directory bundles LLM/RAG-oriented documentation sources generated in CI."
  echo
  if [ -n "$BUILD_DATE" ]; then
    echo "Generated: $BUILD_DATE"
  else
    echo "Generated: local run (set BUILD_DATE in CI)"
  fi
  echo
  echo "Complete reference:"
  echo
  echo "- [webfang-docs-full.md]($PAGES_PREFIX/webfang-docs-full.md): everything on this page, one file — narrative + CLI + all crate APIs (~$(wc -w < "$OUT_DIR/webfang-docs-full.md") words)"
  echo
  echo "Focused sources:"
  echo
  echo "- [00-narrative.md]($PAGES_PREFIX/00-narrative.md): guides — debugging/tracing, testing, troubleshooting, TUI design ($(wc -w < "$OUT_DIR/00-narrative.md") words)"
  for crate in $ALL_CRATES; do
    out_file=$(per_source_file "$crate")
    [ -n "$out_file" ] && [ -f "$OUT_DIR/$out_file" ] || continue
    echo "- [$out_file]($PAGES_PREFIX/$out_file): $(crate_description "$crate") ($(wc -w < "$OUT_DIR/$out_file") words)"
  done
  echo
  echo "CLI:"
  echo
  echo "- [06-cli-reference.md]($PAGES_PREFIX/06-cli-reference.md): complete \`webfang\` binary flag reference (--help, env vars) ($(wc -w < "$OUT_DIR/06-cli-reference.md") words)"
  echo
  echo "Optional:"
  echo
  echo "- [GitHub Pages site](https://xavicode1000.github.io/webfang/): human-oriented docs + rustdoc HTML under /api/"
} > "$OUT_DIR/llms.txt"

# Summary.
echo
echo "Output: $OUT_DIR"
for f in llms.txt 00-narrative.md 01-webfang_core.md 02-webfang_ai.md 03-webfang_tui.md 04-webfang_mcp.md 05-webfang_test_utils.md 06-cli-reference.md webfang-docs-full.md; do
  [ -f "$OUT_DIR/$f" ] || continue
  echo "$f $(wc -w < "$OUT_DIR/$f") words"
done
