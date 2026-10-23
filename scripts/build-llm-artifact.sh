#!/usr/bin/env bash
# ============================================================================
# build-llm-artifact.sh — assemble the LLM/RAG documentation artifact (#734)
#
# Emits docs/book/webfang-docs-llm/, an llms.txt v2 directory: llms.txt index,
# per-source files (00-narrative, 01..04-<crate>, 06-cli-reference — one H1
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
# ATOMIC PUBLICATION (audit B21)
# ============================
# The whole artifact is rendered into a staging directory
# (docs/book/.webfang-docs-llm.stage.XXXXXX) and swapped into place only
# after the expected file set and every H1 guard validate. A failure on
# crate 3 used to leave 01-/03- published with no 00-narrative, no
# 06-cli-reference and no llms.txt: a partial directory that LOOKS
# publishable. The contract is now: OUT_DIR holds the complete artifact, or
# nothing — any non-zero exit empties it, so a consumer can never pick up a
# half-rendered directory.
#
# REPRODUCIBLE DATE (audit B30)
# =============================
# The build date is DERIVED, never injected. CI used to pass
# BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" (docs.yml), and that wall-clock
# value was embedded in every generated file, so two runs of the same commit
# produced two different artifacts and the reproducibility claim could not be
# made. SOURCE_DATE_EPOCH is the reproducible-build contract (the same one
# scripts/make-deterministic-zip.sh honours); when it is not set the UTC DAY
# is used, which is stable across every run of the same day. The BUILD_DATE
# environment variable is deliberately IGNORED: an injected clock is exactly
# the non-reproducible input this replaces.
#
# GITHUB_SHA: injected by CI (docs.yml). It identifies the commit the
# artifact was rendered from, so consumers (just docs, NotebookLM sync) can
# verify freshness against origin/main instead of guessing from a timestamp.
# ============================================================================
set -euo pipefail

OUT_DIR=docs/book/webfang-docs-llm
OUT_PARENT="$(dirname "$OUT_DIR")"
SOURCE_COMMIT="${GITHUB_SHA:-}"
PAGES_PREFIX="https://xavicode1000.github.io/webfang/webfang-docs-llm"
CHAPTERS="overview debugging testing troubleshooting"
# webfang_cli is bin-only and stays excluded from the API render: it is covered
# by the 06-cli-reference chapter (#733). The `webfang_ai` crate was removed in
# ADR-0004 paso 6, so the rendered set is the 3 remaining lib crates. The
# per-source numbers keep their historical values (02 is a gap where
# webfang_ai used to be), so the published URLs of the surviving files do not
# change under a reader's bookmark.
ALL_CRATES="webfang_core webfang_mcp webfang_test_utils"

# The expected published file set. The swap is gated on it: a directory that
# does not hold every one of these files is not the artifact.
EXPECTED_FILES="llms.txt 00-narrative.md 01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md webfang-docs-full.md"

# --- Fail-closed publication ------------------------------------------------
# STAGE_DIR holds the rendering in progress; PUBLISHED flips to 1 only after
# the swap. Until then, any exit (including a preflight failure) empties
# OUT_DIR: the artifact is published whole or not at all.
STAGE_DIR=""
PUBLISHED=0

cleanup_unpublished() {
  local rc=$?
  trap - EXIT
  if [ "$PUBLISHED" -ne 1 ]; then
    if [ -n "$STAGE_DIR" ] && [ -d "$STAGE_DIR" ]; then
      rm -rf "$STAGE_DIR"
    fi
    rm -rf "$OUT_DIR"
    echo "::error::el artefacto LLM no se generó por completo; no queda salida publicable en '$OUT_DIR'." >&2
  fi
  exit "$rc"
}
trap cleanup_unpublished EXIT

# --- Build date (derived, never injected) -----------------------------------
derive_build_date() {
  local epoch="${SOURCE_DATE_EPOCH:-}"
  if [ -z "$epoch" ]; then
    # No epoch: the UTC day. Stable for every run of the same day, and it
    # never pretends to be more precise than it is.
    date -u +%Y-%m-%d
    return 0
  fi
  if ! [[ "$epoch" =~ ^[0-9]+$ ]]; then
    echo "::error::SOURCE_DATE_EPOCH='$epoch' no es un timestamp UNIX entero (segundos desde la época): el artefacto no se puede fechar de forma reproducible." >&2
    return 1
  fi
  if command -v python3 >/dev/null 2>&1; then
    SOURCE_DATE_EPOCH="$epoch" python3 -c 'import datetime, os; print(datetime.datetime.fromtimestamp(int(os.environ["SOURCE_DATE_EPOCH"]), datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))'
  else
    date -u -d "@$epoch" +%Y-%m-%dT%H:%M:%SZ
  fi
}

BUILD_DATE="$(derive_build_date)"

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

# The staging directory lives in the same parent as OUT_DIR, so the swap is a
# rename inside one filesystem (an atomic move), never a cross-device copy.
mkdir -p "$OUT_PARENT"
STAGE_DIR="$(mktemp -d "$OUT_PARENT/.webfang-docs-llm.stage.XXXXXX")"

# Metadata lines (Generated + Source commit + blank).
generated_line() {
  echo "Generated: $BUILD_DATE"
  if [ -n "$SOURCE_COMMIT" ]; then
    echo "Source commit: $SOURCE_COMMIT"
  fi
  echo
}

# Demote every heading by exactly ONE level in a SINGLE pass, clamped at H6.
# Fence-aware: lines starting with `# ` inside ```/~~~ code blocks are code
# (e.g. rustdoc doc-hidden `# hidden` lines), never headings.
#
# FENCE FLAVOUR (audit B29): the fence state keys on the character of the
# OPENING marker. A ~~~ line inside a ``` block is CONTENT, not a new fence:
# the old "toggle on any fence line" rule flipped the state there, demoted a
# heading that was inside a real fence and skipped a heading that came right
# after it. CommonMark closes a fence only with a marker of the same
# character, at least as long, and with no info string.
#
# WARNING (regression guard): do NOT rewrite this as a chained `sed -e ...
# -e ...` pipeline. sed applies its expressions SEQUENTIALLY to the same line,
# so substitutions cascade (## → #### → …) — the original implementation meant
# to demote uniformly but actually pushed H4 → H8 and H6 → H12, leaving
# thousands of ≥H7 headings that CommonMark renders as plain text. awk counts
# the leading hashes exactly once and clamps at 6 (the CommonMark maximum), so
# H5 → H6 and H6 stays H6.
demote_headings() {
  awk '
    function fence_run(line,   n) {
      n = 0
      while (substr(line, n + 1, 1) == fence_char) n++
      return n
    }
    {
      if (in_fence) {
        # A closing fence: same character as the opener, at least as long,
        # and nothing but whitespace after it.
        if (fence_run($0) >= fence_len && $0 ~ "^" fence_char "{3,}[ \t]*$") in_fence = 0
        print
        next
      }
      if ($0 ~ /^(`{3,}|~{3,})/) {
        fence_char = substr($0, 1, 1)
        fence_len = fence_run($0)
        in_fence = 1
        print
        next
      }
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

# Count the H1 headings of a file, ignoring the ones that live inside a code
# fence. Same fence rules as demote_headings: the state keys on the flavour
# of the opening marker (audit B29).
count_h1() {
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

# Drop headings that open an EMPTY LEAF section: no body lines and no child
# headings before the next same-or-shallower heading (or EOF). Sections with
# children are structural parents and are kept. These come from public items
# without doc comments; stripping them removes index noise for LLM/RAG
# consumers instead of shipping hundreds of bare signatures.
# Fence-aware: `# comment` inside ```/~~~ code blocks is NOT a heading, and the
# fence state keys on the flavour of the opening marker (audit B29).
strip_empty_leaf_sections() {
  python3 - "$1" <<'PYEOF'
import re
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    lines = fh.read().split("\n")

heading = re.compile(r"^(#{1,6}) ")
fence = re.compile(r"^(`{3,}|~{3,})")
# A closing fence: same character as the opener, at least as long, and no
# info string after it.
closing_fence = re.compile(r"^(`{3,}|~{3,})[ \t]*$")
out = []
in_fence = False
fence_char = ""
fence_len = 0
i = 0
while i < len(lines):
    line = lines[i]
    if in_fence:
        m_close = closing_fence.match(line)
        if m_close and m_close.group(1)[0] == fence_char and len(m_close.group(1)) >= fence_len:
            in_fence = False
        out.append(line)
        i += 1
        continue
    m = fence.match(line)
    if m:
        in_fence = True
        fence_char = m.group(1)[0]
        fence_len = len(m.group(1))
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
# crate, except `loom-model`, deliberately left out: it makes build.rs emit
# `cfg(loom)`, so including it would render the instrumented tree instead of
# the real one. The lists are spelled out per crate (the tool takes one --features
# string) and an UNKNOWN name there is a hard cargo error, not a warning — so a
# feature deletion in any manifest breaks this script until the list is updated
# with it. webfang_test_utils declares no features at all and gets no --features
# flag.
render_api() {
  local crate="$1" out="$2" features
  case "$crate" in
  webfang_core) features="default persistence console dev-tracing adaptive-selectors mcp chromium" ;;
  webfang_mcp) features="mcp persistence" ;;
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
  h1_count=$(count_h1 "$out")
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
  webfang_mcp) echo "03-webfang_mcp.md" ;;
  webfang_test_utils) echo "04-webfang_test_utils.md" ;;
  *) echo "" ;;
  esac
}

# One line per crate for the LLM artifact's index.
#
# No tool count here on purpose: the MCP tool count has ONE source
# (`MCP_TOOL_COUNT` in `crates/webfang_mcp/src/mcp_server/mod.rs`), and a
# number written by hand in a shell script is a SECOND source — one that
# drifts silently. The artifact describes the crate, not its size.
crate_description() {
  case "$1" in
  webfang_core) echo "core domain/application/infrastructure API" ;;
  webfang_mcp) echo "MCP server API" ;;
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
  render_api "$crate" "$STAGE_DIR/$out_file"
done

# Phase 2: narrative + CLI reference files (single H1 each).
{
  echo "# WebFang Narrative Documentation"
  generated_line
  echo "---"
  echo
  emit_chapters "##"
} >"$STAGE_DIR/00-narrative.md"

{
  echo "# WebFang CLI Reference"
  generated_line
  echo "---"
  echo
  sed '1s/^# /## /' docs/src/cli-reference.md
} >"$STAGE_DIR/06-cli-reference.md"

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
    if [ -n "$out_file" ] && [ -f "$STAGE_DIR/$out_file" ]; then
      echo "### API: $crate"
      echo
      cat "$STAGE_DIR/$out_file"
      echo
      echo "---"
      echo
    fi
  done
} >"$STAGE_DIR/webfang-docs-full.md"

# Phase 4: llms.txt (v2 index).
{
  echo "# WebFang"
  echo
  echo "> Production-ready web scraper: Clean Architecture, AI semantic cleaning, sitemap-based crawling. This directory bundles LLM/RAG-oriented documentation sources generated in CI."
  echo
  echo "Generated: $BUILD_DATE"
  if [ -n "$SOURCE_COMMIT" ]; then
    echo "Source commit: $SOURCE_COMMIT"
  fi
  echo
  echo "Complete reference:"
  echo
  echo "- [webfang-docs-full.md]($PAGES_PREFIX/webfang-docs-full.md): everything on this page, one file — narrative + CLI + all crate APIs (~$(wc -w <"$STAGE_DIR/webfang-docs-full.md") words)"
  echo
  echo "Focused sources:"
  echo
  echo "- [00-narrative.md]($PAGES_PREFIX/00-narrative.md): guides — debugging/tracing, testing, troubleshooting ($(wc -w <"$STAGE_DIR/00-narrative.md") words)"
  for crate in $ALL_CRATES; do
    out_file=$(per_source_file "$crate")
    [ -n "$out_file" ] && [ -f "$STAGE_DIR/$out_file" ] || continue
    echo "- [$out_file]($PAGES_PREFIX/$out_file): $(crate_description "$crate") ($(wc -w <"$STAGE_DIR/$out_file") words)"
  done
  echo
  echo "CLI:"
  echo
  echo "- [06-cli-reference.md]($PAGES_PREFIX/06-cli-reference.md): complete \`webfang\` binary flag reference (--help, env vars) ($(wc -w <"$STAGE_DIR/06-cli-reference.md") words)"
  echo
  echo "Optional:"
  echo
  echo "- [GitHub Pages site](https://xavicode1000.github.io/webfang/): human-oriented docs + rustdoc HTML under /api/"
} >"$STAGE_DIR/llms.txt"

# --- Validate the staged artifact BEFORE publishing it ----------------------
# The swap is gated on the whole file set and on the H1 guard of every
# published source file. The monolith is exempt: it embeds the per-source
# files, so it holds one H1 per API section by design.
for f in $EXPECTED_FILES; do
  if [ ! -s "$STAGE_DIR/$f" ]; then
    echo "::error::el artefacto LLM está incompleto: '$f' falta o está vacío en el directorio de stage; no se publica nada." >&2
    exit 1
  fi
done
for f in 00-narrative.md 01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md; do
  h1_count=$(count_h1 "$STAGE_DIR/$f")
  if [ "$h1_count" -ne 1 ]; then
    echo "::error::'$f' no tiene exactamente un encabezado H1 (se encontraron $h1_count); no se publica nada." >&2
    exit 1
  fi
done

# --- Swap the validated artifact into place ---------------------------------
# A rename inside one directory (same filesystem): OUT_DIR is observed either
# as the previous complete artifact or as the new complete one, never as a
# half-rendered directory.
OLD_DIR=""
if [ -e "$OUT_DIR" ]; then
  OLD_DIR="$OUT_DIR.previous.$$"
  mv "$OUT_DIR" "$OLD_DIR"
fi
if ! mv "$STAGE_DIR" "$OUT_DIR"; then
  if [ -n "$OLD_DIR" ] && [ -e "$OLD_DIR" ]; then
    mv "$OLD_DIR" "$OUT_DIR"
  fi
  echo "::error::no se pudo publicar el artefacto LLM en '$OUT_DIR'." >&2
  exit 1
fi
STAGE_DIR=""
PUBLISHED=1
if [ -n "$OLD_DIR" ] && [ -e "$OLD_DIR" ]; then
  rm -rf "$OLD_DIR"
fi

# Summary.
echo
echo "Output: $OUT_DIR"
for f in llms.txt 00-narrative.md 01-webfang_core.md 03-webfang_mcp.md 04-webfang_test_utils.md 06-cli-reference.md webfang-docs-full.md; do
  [ -f "$OUT_DIR/$f" ] || continue
  echo "$f $(wc -w <"$OUT_DIR/$f") words"
done
