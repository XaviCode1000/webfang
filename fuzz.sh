#!/usr/bin/env bash
# fuzz.sh — Run all fuzz targets for 10 minutes each.
# Requires: cargo +nightly, cargo-fuzz
#
# Tier policy (#507): cargo-fuzz reads NO config file — libFuzzer options only
# reach the fuzz binary when passed after `--`. The per-target flags returned
# by fuzz_flags() below ARE the policy (mirrors the justfile fuzz recipes).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Per-target libFuzzer flags (max_len/timeout from the tier policy table):
# - max_len=16384 — core HTML pipeline + link extraction + compression detect
# - max_len=8192  — URL processing, WAF detection, content processing, assets
# - max_len=4096  — HTTP-header-like inputs (content-disposition)
# - max_len=32768 + timeout=15 — sitemap XML, decompression (slower work)
fuzz_flags() {
    case "$1" in
        fuzz_parse_html|fuzz_html_cleaner|fuzz_convert_to_markdown|fuzz_readability_parse|fuzz_extract_text|fuzz_extract_links|fuzz_compression_detect)
            echo "-max_len=16384"
            ;;
        fuzz_url_validation|fuzz_url_normalization|fuzz_waf_detection|fuzz_wikilinks|fuzz_syntax_highlight|fuzz_slug_from_url|fuzz_extract_assets)
            echo "-max_len=8192"
            ;;
        fuzz_parse_content_disposition)
            echo "-max_len=4096"
            ;;
        fuzz_parse_sitemap|fuzz_decompression)
            echo "-max_len=32768 -timeout=15"
            ;;
        *)
            echo ""
            ;;
    esac
}

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
    flags="$(fuzz_flags "$target")"
    echo "=== Fuzzing $target (${MAX_TOTAL_TIME}s) ${flags:+[$flags] }==="
    cargo +nightly fuzz run "$target" -- $flags -max_total_time="$MAX_TOTAL_TIME"
    echo "=== $target finished ==="
    echo
done

echo "All fuzz targets completed."
