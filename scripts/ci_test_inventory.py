#!/usr/bin/env python3
"""ci_test_inventory.py — Phase 3 machine-readable test inventory.

Purpose: know what tests exist and what each protects. Walks every
``crates/*/tests/**/*.rs`` file and records, per file: path, crate, area,
test count, ignored count, snapshot usage, and tier label.

Areas reuse the ``scripts/ci_path_classifier.sh`` path predicates
*conceptually* (ai/mcp/cli/core/crawler/downloader/tests subset):
structural filename matching only, no content scans except for the
AI-gated tier signal (see below). Priority is ai > mcp > cli > crawler >
downloader > core > tests so a ``behavioral/cli/sitemap_test.rs`` lands in
``cli`` (its owning lane), not ``crawler``.

Test counting heuristic: ``test_count`` counts ``#[test]`` plus
``#[tokio::test]`` attribute occurrences; ``ignored_count`` counts
``#[ignore`` occurrences (including ``#[ignore = "..."]``). Commented-out
attributes over-count by design — the file is a budget signal, and
``scripts/check_ignored_guard.sh`` remains the exact ``#[ignore]`` budget.

Scope note (#1328): this JSON walks ``crates/*/tests/`` only and counts
mentions, so its ``ignored_count`` sum is NOT reconciled with the guard's
budget by design — the guard also covers ``src/`` test modules and compares
attributes vs doc/comment mentions per category. The two sums differ
structurally (e.g. 28 here vs 32 there); the inventory of record for the
frozen budget is ``docs/test-inventory.md``.

``snapshot_usage`` is true when the file references insta
(``insta`` word-boundary or ``assert_snapshot``) OR a sibling
``snapshots/`` dir exists next to the file.

Tier labels: ``integration_mock`` (default), ``ai_model_semantic`` for
AI-gated files (path matches the classifier AI markers, or content
mentions clean_ai/clean-ai/onnx/granite/embedding — e.g. an ignored ONNX
test inside a CLI behavioral file), ``mcp_contract`` for MCP files.

Regenerate: ``python3 scripts/ci_test_inventory.py`` (run from repo root).
Output: ``docs/test-inventory.json`` with a ``generated_by`` field naming
this script, files sorted by path. Idempotent: no timestamps, sorted
input, stable key order — running twice is byte-identical.
Does NOT touch ``docs/test-inventory.md`` (the frozen ignored-test
budget); the JSON is a new machine-readable companion.

Stdlib only.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

TEST_RE = re.compile(r"#\[(?:tokio::)?test")
IGNORE_RE = re.compile(r"#\[ignore")
INSTA_RE = re.compile(r"\binsta\b|assert_snapshot")
AI_CONTENT_RE = re.compile(r"clean.ai|onnx|granite|embedding", re.IGNORECASE)

AI_PATH_MARKS = ("webfang_ai", "ai_integration", "clean_ai", "onnx", "granite", "embedding")
MCP_PATH_MARKS = ("webfang_mcp", "mcp")
CLI_PATH_MARKS = ("webfang_cli", "cli_harness", "cli_reference")
DOWNLOADER_MARKS = (
    "downloader",
    "ssrf",
    "guard_chain",
    "guard-chain",
    "waf",
    "cookie_bridge",
    "hybrid_router",
    "spa_detector",
    "resource_governor",
)
CRAWLER_MARKS = ("crawler", "sitemap")

GENERATED_BY = "scripts/ci_test_inventory.py"


def repo_root() -> Path:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        return Path(out)
    except Exception:
        return Path(__file__).resolve().parent.parent


def classify_area(rel_posix: str) -> str:
    low = rel_posix.lower()
    if any(m in rel_posix for m in AI_PATH_MARKS):
        return "ai"
    if any(m in rel_posix for m in MCP_PATH_MARKS):
        return "mcp"
    if (
        any(m in rel_posix for m in CLI_PATH_MARKS)
        or "/cli/" in rel_posix
        or Path(rel_posix).name.startswith("cli_")
    ):
        return "cli"
    if any(m in low for m in CRAWLER_MARKS):
        return "crawler"
    if any(m in low for m in DOWNLOADER_MARKS):
        return "downloader"
    if "webfang_core" in rel_posix:
        return "core"
    return "tests"


def classify_tier(rel_posix: str, area: str, text: str) -> str:
    if any(m in rel_posix for m in AI_PATH_MARKS) or AI_CONTENT_RE.search(text):
        return "ai_model_semantic"
    if area == "mcp":
        return "mcp_contract"
    return "integration_mock"


def has_snapshot_usage(path: Path, text: str) -> bool:
    if INSTA_RE.search(text):
        return True
    return (path.parent / "snapshots").is_dir()


def main() -> int:
    root = repo_root()
    test_files = sorted(
        (p for p in (root / "crates").rglob("tests/**/*.rs") if p.is_file()),
        key=lambda p: p.relative_to(root).as_posix(),
    )
    entries = []
    for path in test_files:
        rel = path.relative_to(root).as_posix()
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"warning: cannot read {rel}: {exc}", file=sys.stderr)
            continue
        crate = rel.split("/")[1] if rel.startswith("crates/") else ""
        area = classify_area(rel)
        entries.append(
            {
                "path": rel,
                "crate": crate,
                "area": area,
                "test_count": len(TEST_RE.findall(text)),
                "ignored_count": len(IGNORE_RE.findall(text)),
                "snapshot_usage": has_snapshot_usage(path, text),
                "tier": classify_tier(rel, area, text),
            }
        )
    payload = {"generated_by": GENERATED_BY, "files": entries}
    out_path = root / "docs" / "test-inventory.json"
    out_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out_path.relative_to(root)} ({len(entries)} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
