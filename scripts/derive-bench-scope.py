#!/usr/bin/env python3
"""derive-bench-scope.py — the `-p` list that `cargo bench` must be given.

Purpose: keep the nightly Benches workflow from silently dropping a bench.

`cargo bench -p <pkg>` is an allow-list, not a filter. A crate with bench
targets that is not on the list is neither built nor run, `cargo` still exits
0, and the nightly reports green with the coverage gone. A list hardcoded in
the workflow therefore cannot be trusted to stay correct: every added or
removed bench target would need the workflow edited by hand, and nothing would
fail if someone forgot.

So the list is derived, which makes it incapable of going stale. Output is the
shell fragment `-p <name> [-p <name> ...]`, ready to interpolate into a
`cargo bench` invocation.

Asks CARGO which targets are benches instead of reimplementing cargo's target
discovery rules. `cargo metadata` reports every package target whose `kind`
contains `bench`, which covers all three ways a crate acquires one:

  - an explicit `[[bench]]` table in its manifest,
  - autodiscovery of `benches/*.rs` or `benches/*/main.rs`,
  - and the `autobenches = false` opt-out that suppresses the second.

An earlier version parsed `[[bench]]` tables out of the manifests directly. That
missed the autodiscovery case entirely, and autodiscovered benches are exactly
what the nightly must not silently skip — cargo's own answer cannot drift from
cargo's own rules.

Scope and failures:
  - Runs one `cargo metadata --no-deps`, which resolves the workspace without
    compiling anything.
  - Fails closed on any error, including an empty scope. An empty scope means no
    crate has a bench target, which is never the expected state, and the
    caller's fallback — an unscoped `cargo bench` — would silently reinstate the
    full-workspace cold compile this scoping exists to avoid.
  - A bench target gated behind `required-features` is reported here like any
    other; whether it builds is cargo's call at build time, not this script's.

Usage: `python3 scripts/derive-bench-scope.py`
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys


def _cargo_metadata(root: pathlib.Path) -> dict:
    """Run `cargo metadata` for the workspace, refusing loudly on any failure."""
    try:
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--manifest-path",
                str(root / "Cargo.toml"),
            ],
            capture_output=True,
            text=True,
            check=True,
            cwd=root,
        )
    except FileNotFoundError:
        print(
            "derive-bench-scope: `cargo` is not on PATH; cannot ask cargo which "
            "targets are benches",
            file=sys.stderr,
        )
        raise SystemExit(1) from None
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip().splitlines()
        print(
            "derive-bench-scope: `cargo metadata` failed; refusing to guess the "
            "bench scope",
            file=sys.stderr,
        )
        for line in detail[-5:]:
            print(f"  {line}", file=sys.stderr)
        raise SystemExit(1) from None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        print(
            f"derive-bench-scope: could not parse `cargo metadata` output: {error}",
            file=sys.stderr,
        )
        raise SystemExit(1) from None


def bench_packages(root: pathlib.Path) -> list[tuple[str, int]]:
    """Return `(package_name, bench_target_count)` for crates that have benches."""
    found: list[tuple[str, int]] = []
    for package in _cargo_metadata(root).get("packages", []):
        count = sum(1 for target in package["targets"] if "bench" in target["kind"])
        if count:
            found.append((package["name"], count))
    return sorted(found)


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    found = bench_packages(root)
    if not found:
        print(
            "derive-bench-scope: no workspace crate declares a bench target; "
            "refusing to return an empty scope",
            file=sys.stderr,
        )
        return 1
    for name, count in found:
        print(f"  {name}: {count} bench target(s)", file=sys.stderr)
    print(" ".join(f"-p {name}" for name, _ in found))
    return 0


if __name__ == "__main__":
    sys.exit(main())
