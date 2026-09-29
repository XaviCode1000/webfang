#!/usr/bin/env python3
"""Emit the profile-bearing tables of a root Cargo.toml as canonical JSON.

`cargo metadata` exposes no profiles at all, so the workspace build contract
needs this second source. Only tables that can change a compiled artifact are
kept; anything cosmetic or inherited is left out, because a digest that moved on
irrelevant edits would re-key every seed for no reason.

Reads with tomllib, so it is a real TOML parse rather than a regex — a regex
would silently miss a nested `profile.dev.package."*"` override, which is exactly
the kind of declaration that changes a compiled artifact.
"""
import json
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - tomllib is stdlib from 3.11
    print(json.dumps({"profiles": "tomllib-unavailable"}))
    raise SystemExit(0)

path = sys.argv[1] if len(sys.argv) > 1 else "Cargo.toml"
try:
    with open(path, "rb") as fh:
        doc = tomllib.load(fh)
except FileNotFoundError:
    print(json.dumps({"profiles": "absent"}))
    raise SystemExit(0)
except Exception as exc:  # malformed manifest: say so rather than hashing noise
    print(json.dumps({"profiles": "unreadable", "error": type(exc).__name__}))
    raise SystemExit(0)

keep = {k: doc[k] for k in ("profile", "build-override") if k in doc}
if "workspace" in doc:
    ws = doc["workspace"]
    keep["workspace"] = {
        k: v for k, v in ws.items() if k in ("resolver", "default-members")
    }

print(json.dumps(keep, sort_keys=True, separators=(",", ":")))
