#!/usr/bin/env bash
# =============================================================================
# Development workflow: Watch mode with auto-test and clippy
# =============================================================================
# Usage: ./scripts/dev.sh
# 
# This script runs:
#   1. cargo-watch to monitor file changes
#   2. cargo clippy on changes (linting)
#   3. cargo nextest on changes (tests)
# =============================================================================

set -e

echo "🚀 Starting dev mode with watch..."
echo "   - Watching src/ and tests/"
echo "   - Running clippy + nextest on changes"
echo ""

# Check dependencies
command -v cargo-nextest >/dev/null 2>&1 || { echo "❌ cargo-nextest not installed. Run: cargo install cargo-nextest"; exit 1; }
command -v cargo-watch >/dev/null 2>&1 || { echo "❌ cargo-watch not installed. Run: cargo install cargo-watch"; exit 1; }

# Run watch mode with clippy and nextest
cargo watch \
    --shells='echo "📁 Files changed, running checks..."' \
    -- \
    clippy -- -D warnings \
    && cargo nextest run --test-threads 2
