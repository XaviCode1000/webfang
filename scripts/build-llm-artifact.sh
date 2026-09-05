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
#   rustdoc-markdown (Dooyo-Labs, v0.91.0) on PATH   → per-crate API renderer
#
# API rendering: per-source API files are rendered by
#   rustdoc-markdown <crate> --manifest crates/<crate>/Cargo.toml [--features "..."] --include-other
# a rustdoc-JSON backend tool that brings and auto-installs its own pinned
# nightly toolchain on first run. It does NOT consume target/doc HTML; the old
# HTML→pandoc pipeline (pandoc/sed/awk + noise gate) was removed. Each file
# keeps exactly ONE H1 — the "# <crate> API Reference" wrapper — by demoting
# the tool output's headings one level (#N → #N+1).
#
# BUILD_DATE / GITHUB_SHA: injected by CI (docs.yml). Local runs leave both
# empty; llms.txt then carries a "Generated: local run" note and omits the
# commit line. The commit line lets consumers (just docs, NotebookLM sync)
# verify artifact freshness against origin/main instead of guessing from a
# timestamp alone.
# ============================================================================
set -euo pipefail

OUT_DIR=docs/book/webfang-docs-llm
BUILD_DATE="${BUILD_DATE:-}"
SOURCE_COMMIT="${GITHUB_SHA:-}"
PAGES_PREFIX="https://xavicode1000.github.io/webfang/webfang-docs-llm"
CHAPTERS="overview debugging testing troubleshooting"
# webfang_cli is bin-only and stays excluded from the API render: it is covered
# by the 06-cli-reference chapter (#733). Only the 5 lib crates are rendered.
ALL_CRATES="webfang_core webfang_ai webfang_mcp webfang_test_utils"

for f in $CHAPTERS cli-reference; do
  if [ ! -f "docs/src/$f.md" ]; then
    echo "ERROR: docs/src/$f.md is missing. Prerequisite: mdbook build docs" >&2
    exit 1
  fi
done
if ! command -v rustdoc-markdown >/dev/null 2>&1; then
  echo "ERROR: rustdoc-markdown is not on PATH. Prerequisite:" >&2
  echo "       cargo install rustdoc-markdown --version 0.91.0 --locked" >&2
  exit 1
fi

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

# Metadata lines (Generated + Source commit + blank) in CI, nothing locally.
generated_line() {
  if [ -n "$BUILD_DATE" ]; then
    echo "Generated: $BUILD_DATE"
    if [ -n "$SOURCE_COMMIT" ]; then
      echo "Source commit: $SOURCE_COMMIT"
    fi
    echo
  fi
}

# Demote every heading by exactly ONE level in a SINGLE pass, clamped at H6.
# Fence-aware: lines starting with `# ` inside ```/~~~ code blocks are code
# (e.g. rustdoc doc-hidden `# hidden` lines), never headings.
#
# WARNING (regression guard): do NOT rewrite this as a chained `sed -e ...
# -e ...` pipeline. sed applies its expressions SEQUENTIALLY to the same line,
# so substitutions cascade (## → #### → …) — the original implementation meant
# to demote uniformly but actually pushed H4 → H8 and H6 → H12, leaving
# thousands of ≥H7 headings that CommonMark renders as plain text. awk counts
# the leading hashes exactly once and clamps at 6 (the CommonMark maximum), so
# H5 → H6 and H6 stays H6.
demote_headings() {
  awk '{
    if ($0 ~ /^(`{3,}|~{3,})/) { infence = !infence; print; next }
    if (infence) { print; next }
    n = 0
    while (substr($0, n + 1, 1) == "#") n++
    if (n >= 1 && substr($0, n + 1, 1) == " ") {
      level = (n < 6 ? n + 1 : 6)
      print substr("######", 1, level) substr($0, n + 1)
    } else {
      print
    }
  }' "$1"
}

# Drop headings that open an EMPTY LEAF section: no body lines and no child
# headings before the next same-or-shallower heading (or EOF). Sections with
# children are structural parents and are kept. These come from public items
# without doc comments; stripping them removes index noise for LLM/RAG
# consumers instead of shipping hundreds of bare signatures.
# Fence-aware: `# comment` inside ```/~~~ code blocks is NOT a heading.
strip_empty_leaf_sections() {
  python3 - "$1" <<'PYEOF'
import re
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    lines = fh.read().split("\n")

heading = re.compile(r"^(#{1,6}) ")
fence = re.compile(r"^(`{3,}|~{3,})")
out = []
in_fence = False
i = 0
while i < len(lines):
    line = lines[i]
    if fence.match(line):
        in_fence = not in_fence
        out.append(line)
        i += 1
        continue
    m = heading.match(line) if not in_fence else None
    if not m:
        out.append(line)
        i += 1
        continue
    level = len(m.group(1))
    j = i + 1
    while j < len(lines) and not lines[j].strip():
        j += 1
    empty_leaf = j >= len(lines)  # trailing heading, nothing after
    if not empty_leaf:
        m2 = heading.match(lines[j])
        empty_leaf = bool(m2) and len(m2.group(1)) <= level
    if not empty_leaf:
        out.append(line)
    i += 1
sys.stdout.write("\n".join(out))
PYEOF
}

# Render one crate's API to "$2" via rustdoc-markdown (rustdoc JSON backend,
# pinned nightly auto-installed by the tool on first run — needs rustup +
# network, both present on ubuntu-latest). The tool's own progress is streamed.
# Output convention: exactly one H1 per file — the "# <crate> API Reference"
# wrapper — with the tool output (which starts "# <crate> API (<version>)")
# demoted one level. Feature flags replicate `cargo doc --all-features` per
# crate; webfang_test_utils gets no --features flag at all.
render_api() {
  local crate="$1" out="$2" features
  case "$crate" in
  webfang_core) features="default images documents persistence console dev-tracing ai adaptive-selectors mcp chromium" ;;
  webfang_ai) features="ai" ;;
  webfang_mcp) features="mcp ai persistence" ;;
  *) features="" ;;
  esac
  local -a cmd=(rustdoc-markdown print "$crate" --manifest "crates/$crate/Cargo.toml" --include-other --output "$out.raw")
  [ -n "$features" ] && cmd+=(--features "$features")
  "${cmd[@]}"
  demote_headings "$out.raw" >"$out.tmp"
  {
    echo "# $crate API Reference"
    generated_line
    echo "---"
    echo
    strip_empty_leaf_sections "$out.tmp"
    echo
  } >"$out"
  rm -f "$out.raw" "$out.tmp"
  # H1 guard: exactly one H1 per per-source file. Fence-aware: `# ` lines
  # inside ```/~~~ code blocks are Rust doc-hidden lines, not headings.
  h1_count=$(awk '/^(`{3,}|~{3,})/ { infence = !infence; next }
                      !infence && /^# / { c++ } END { print c + 0 }' "$out")
  [ "$h1_count" -eq 1 ] || {
    echo "::error::$out does not have exactly one H1 heading (found $h1_count)"
    return 1
  }
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
  webfang_mcp) echo "03-webfang_mcp.md" ;;
  webfang_test_utils) echo "04-webfang_test_utils.md" ;;
  *) echo "" ;;
  esac
}

crate_description() {
  case "$1" in
  webfang_core) echo "core domain/application/infrastructure API" ;;
  webfang_ai) echo "ONNX embeddings and semantic cleaning API" ;;
  webfang_mcp) echo "MCP server (35 tools) API" ;;
  webfang_test_utils) echo "shared test utilities API" ;;
  *) echo "" ;;
  esac
}

# Phase 1: render every crate API directly into its per-source file.
# render_api applies the H1 wrapper, one-level demote, and the H1 guard, and
# replaces the old pandoc/sed/awk noise-stripping pipeline entirely — the
# rustdoc JSON backend output carries no HTML-layout noise to strip.
for crate in $ALL_CRATES; do
  out_file=$(per_source_file "$crate")
  [ -n "$out_file" ] || continue
  render_api "$crate" "$OUT_DIR/$out_file"
done

# Phase 2: narrative + CLI reference files (single H1 each).
{
  echo "# WebFang Narrative Documentation"
  generated_line
  echo "---"
  echo
  emit_chapters "##"
} >"$OUT_DIR/00-narrative.md"

{
  echo "# WebFang CLI Reference"
  generated_line
  echo "---"
  echo
  sed '1s/^# /## /' docs/src/cli-reference.md
} >"$OUT_DIR/06-cli-reference.md"

# Phase 3: back-compat monolith (pre-#734 composition preserved; API sections
# are assembled from the same per-source files, so their nested wrapper H1s
# are intentional — only the 6 top-level files enforce the single-H1 rule).
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
    out_file=$(per_source_file "$crate")
    if [ -n "$out_file" ] && [ -f "$OUT_DIR/$out_file" ]; then
      echo "### API: $crate"
      echo
      cat "$OUT_DIR/$out_file"
      echo
      echo "---"
      echo
    fi
  done
} >"$OUT_DIR/webfang-docs-full.md"

# Phase 4: llms.txt (v2 index).
{
  echo "# WebFang"
  echo
  echo "> Production-ready web scraper: Clean Architecture, AI semantic cleaning, sitemap-based crawling. This directory bundles LLM/RAG-oriented documentation sources generated in CI."
  echo
  if [ -n "$BUILD_DATE" ]; then
    echo "Generated: $BUILD_DATE"
    if [ -n "$SOURCE_COMMIT" ]; then
      echo "Source commit: $SOURCE_COMMIT"
    fi
  else
    echo "Generated: local run (set BUILD_DATE in CI)"
  fi
  echo
  echo "Complete reference:"
  echo
  echo "- [webfang-docs-full.md]($PAGES_PREFIX/webfang-docs-full.md): everything on this page, one file — narrative + CLI + all crate APIs (~$(wc -w <"$OUT_DIR/webfang-docs-full.md") words)"
  echo
  echo "Focused sources:"
  echo
  echo "- [00-narrative.md]($PAGES_PREFIX/00-narrative.md): guides — debugging/tracing, testing, troubleshooting ($(wc -w <"$OUT_DIR/00-narrative.md") words)"
  for crate in $ALL_CRATES; do
    out_file=$(per_source_file "$crate")
    [ -n "$out_file" ] && [ -f "$OUT_DIR/$out_file" ] || continue
    echo "- [$out_file]($PAGES_PREFIX/$out_file): $(crate_description "$crate") ($(wc -w <"$OUT_DIR/$out_file") words)"
  done
  echo
  echo "CLI:"
  echo
  echo "- [06-cli-reference.md]($PAGES_PREFIX/06-cli-reference.md): complete \`webfang\` binary flag reference (--help, env vars) ($(wc -w <"$OUT_DIR/06-cli-reference.md") words)"
  echo
  echo "Optional:"
  echo
  echo "- [GitHub Pages site](https://xavicode1000.github.io/webfang/): human-oriented docs + rustdoc HTML under /api/"
} >"$OUT_DIR/llms.txt"

# Summary.
echo
echo "Output: $OUT_DIR"
for f in llms.txt 00-narrative.md 01-webfang_core.md 02-webfang_ai.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md webfang-docs-full.md; do
  [ -f "$OUT_DIR/$f" ] || continue
  echo "$f $(wc -w <"$OUT_DIR/$f") words"
done
