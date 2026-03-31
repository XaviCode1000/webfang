#!/usr/bin/env bash
# =============================================================================
# Coverage report generation using LLVM (fast, ~30s vs 5min for tarpaulin)
# =============================================================================
# Usage: ./scripts/coverage.sh
#
# This script generates:
#   - HTML coverage report in coverage-llvm/
#   - Summary printed to console
# =============================================================================

set -e

echo "📊 Generating coverage report (LLVM)..."
echo ""

# Check dependencies
command -v cargo-llvm-cov >/dev/null 2>&1 || { 
    echo "❌ cargo-llvm-cov not installed."
    echo "   Install with: cargo install cargo-llvm-cov"
    exit 1
}

# Generate coverage report
cargo llvm-cov --html --output-dir coverage-llvm --lcov

echo ""
echo "✅ Coverage report generated!"
echo "   📂 Open: coverage-llvm/index.html"
echo ""

# Show summary
cargo llvm-cov --summary-only || true
