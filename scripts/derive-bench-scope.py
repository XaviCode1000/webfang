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
  - Reads `crates/*/Cargo.toml` with `tomllib` (stdlib since Python 3.11), so
    this is real TOML parsing rather than a grep for `[[bench]]`.
  - Fails closed when no crate declares a bench target. That state is never the
    expected one, and the caller's fallback — an unscoped `cargo bench` — would
    silently reinstate the full-workspace cold compile this scoping exists to
    avoid.
  - Skips manifests without a `[package]` table (the virtual workspace root has
    none) instead of failing on them.

Usage: `python3 scripts/derive-bench-scope.py`
"""

from __future__ import annotations

import pathlib
import sys
import tomllib

# The workspace layout this derives from. A crate outside `crates/` (a new
# top-level member) would be missed silently, so the glob is deliberately the
# same one the dependency-direction gate walks.
MANIFEST_GLOB = "crates/*/Cargo.toml"


def bench_packages(root: pathlib.Path) -> list[tuple[str, int]]:
    """Return `(package_name, bench_target_count)` for crates declaring benches."""
    found: list[tuple[str, int]] = []
    for manifest in sorted(root.glob(MANIFEST_GLOB)):
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        targets = data.get("bench") or ()
        if not targets:
            continue
        package = data.get("package")
        if package is None or "name" not in package:
            print(
                f"derive-bench-scope: {manifest} declares [[bench]] but has no "
                "[package] name; cannot scope it",
                file=sys.stderr,
            )
            sys.exit(1)
        found.append((package["name"], len(targets)))
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
