#!/usr/bin/env python3
"""derive-bench-scope.py — the `-p` list that `cargo bench` must be given.

Purpose: keep the nightly Benches workflow from silently dropping a bench.

`cargo bench -p <pkg>` is an allow-list, not a filter. A `[[bench]]` target
declared by a crate that is not on the list is neither built nor run, `cargo`
still exits 0, and the nightly reports green with the coverage gone. A
hardcoded list in the workflow therefore cannot be trusted to stay correct:
every added or removed bench target would need the workflow edited by hand, and
nothing would fail if someone forgot.

So the list is derived from the manifests, which makes it incapable of going
stale. Output is the shell fragment `-p <name> [-p <name> ...]`, ready to
interpolate into a `cargo bench` invocation.

Scope and failures:
  - Reads `crates/*/Cargo.toml` with a line scan, NOT `tomllib`. `tomllib` is
    stdlib only from Python 3.11, and this script runs on whatever `python3`
    the runner image ships; on 3.10 or older the import alone would abort the
    step before a single bench runs (#1913 review, `R3-python-version-floor`).
    A line scan keeps the floor at "any Python 3" with no new dependency and no
    interpreter to pin.
  - Only two facts are read, both unambiguous in Cargo.toml: whether the file
    declares any `[[bench]]` table, and the `name` inside `[package]`. The `name`
    is read ONLY while inside `[package]`, because every `[[bench]]` entry also
    carries a `name` key and matching either would pick the wrong one.
  - Fails closed when no crate declares a bench target. That state is never the
    expected one, and the caller's fallback — an unscoped `cargo bench` — would
    silently reinstate the full-workspace cold compile this scoping exists to
    avoid.

Usage: `python3 scripts/derive-bench-scope.py`
"""

from __future__ import annotations

import pathlib
import re
import sys

# The workspace layout this derives from. A crate outside `crates/` (a new
# top-level member) would be missed silently, so the glob is deliberately the
# same one the dependency-direction gate walks.
MANIFEST_GLOB = "crates/*/Cargo.toml"

# A table header, with or without leading whitespace. `[[bench]]` is the only
# array-of-tables form that matters here; single `[section]` headers just move
# the "am I inside [package]" cursor.
_TABLE_HEADER = re.compile(r"^\s*(\[\[?)([A-Za-z0-9_.-]+)(\]\]?)\s*$")
# `name = "value"` with an optional trailing comment.
_NAME_ENTRY = re.compile(r'^\s*name\s*=\s*"([^"]*)"')


def _strip_comment(line: str) -> str:
    """Drop a trailing `#` comment, respecting quotes so a `#` in a value stays."""
    in_quotes = False
    for index, char in enumerate(line):
        if char == '"':
            in_quotes = not in_quotes
        elif char == "#" and not in_quotes:
            return line[:index]
    return line


def scan_manifest(text: str) -> tuple[str | None, int]:
    """Return `(package_name_or_None, bench_target_count)` for one manifest."""
    package_name: str | None = None
    inside_package = False
    bench_count = 0

    for raw in text.splitlines():
        line = _strip_comment(raw)
        header = _TABLE_HEADER.match(line)
        if header:
            is_array = header.group(1) == "[["
            inside_package = not is_array and header.group(2) == "package"
            if is_array and header.group(2) == "bench":
                bench_count += 1
            continue
        if inside_package:
            name = _NAME_ENTRY.match(line)
            if name:
                package_name = name.group(1)
    return package_name, bench_count


def bench_packages(root: pathlib.Path) -> list[tuple[str, int]]:
    """Return `(package_name, bench_target_count)` for crates declaring benches."""
    found: list[tuple[str, int]] = []
    for manifest in sorted(root.glob(MANIFEST_GLOB)):
        package_name, bench_count = scan_manifest(manifest.read_text(encoding="utf-8"))
        if bench_count == 0:
            continue
        if package_name is None:
            print(
                f"derive-bench-scope: {manifest} declares [[bench]] but has no "
                "[package] name; cannot scope it",
                file=sys.stderr,
            )
            sys.exit(1)
        found.append((package_name, bench_count))
    return found


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    found = bench_packages(root)
    if not found:
        print(
            f"derive-bench-scope: no crate under {MANIFEST_GLOB} declares a "
            "[[bench]] target; refusing to return an empty scope",
            file=sys.stderr,
        )
        return 1
    for name, count in found:
        print(f"  {name}: {count} bench target(s)", file=sys.stderr)
    print(" ".join(f"-p {name}" for name, _ in found))
    return 0


if __name__ == "__main__":
    sys.exit(main())
