#!/usr/bin/env bash
# fuzz.sh — Run all fuzz targets for 10 minutes each.
# Requires: cargo +nightly, cargo-fuzz
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

TARGETS=(
    fuzz_parse_html
    fuzz_html_cleaner
    fuzz_parse_sitemap
    fuzz_url_normalization
    fuzz_waf_detection
    fuzz_decompression
    fuzz_compression_detect
    fuzz_parse_content_disposition
    fuzz_convert_to_markdown
    fuzz_readability_parse
    fuzz_extract_text
    fuzz_extract_links
    fuzz_url_validation
    fuzz_wikilinks
    fuzz_syntax_highlight
    fuzz_slug_from_url
    fuzz_extract_assets
)

MAX_TOTAL_TIME=600  # 10 minutes per target

for target in "${TARGETS[@]}"; do
    echo "=== Fuzzing $target (${MAX_TOTAL_TIME}s) ==="
    cargo +nightly fuzz run "$target" -- -max_total_time="$MAX_TOTAL_TIME"
    echo "=== $target finished ==="
    echo
done

echo "All fuzz targets completed."
