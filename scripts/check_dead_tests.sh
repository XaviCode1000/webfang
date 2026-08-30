#!/bin/bash
set -euo pipefail

# Detect test files that aren't discovered by cargo (dead tests).
#
# Source of truth: `cargo metadata`. Cargo tells us exactly which test targets
# it builds and where each one's entry file lives. A .rs file in any crate's
# tests/ tree that is NOT under any registered test target's entry (or its
# `mod` / trybuild subtree) is dead — it never gets compiled and never runs.
#
# This regression slipped `tests/infrastructure/` (84 dead tests, 1493 lines)
# through #1008/#1014 because the previous guard was a no-op (`[ -d "tests" ]`
# short-circuits in a virtual workspace). Ground the guard in cargo's own
# discovery so it can't slip through again.
#
# See:
#   * AGENTS.md §"Workspace structure" — root tests/ is dead.
#   * crates/webfang_core/Cargo.toml `[[test]]` entries — the explicit-wire
#     pattern for subdirectory test targets (e.g. `compile_fail/page_state.rs`).
#   * scripts/check_dependency_direction.sh — sibling policy guard.

DEAD=0

# ─────────────────────────────────────────────────────────────────────────────
# Collect the set of .rs files reachable from any cargo-registered test
# target, expressed as paths RELATIVE to the repo root (one per line).
#
# Reachability rules:
#   1. The entry itself (cargo metadata `src_path`).
#   2. Files reachable transitively via `mod foo;` declarations, which resolve
#      to either `<dir>/foo.rs` or `<dir>/foo/mod.rs`. Each submodule file
#      can declare more submodules, so we recurse.
#   3. Files matched by `trybuild::TestCases::compile_fail("...glob...")` calls
#      in any reachable file — these are test INPUTS consumed by trybuild,
#      not Rust modules, so cargo doesn't compile them but they still belong
#      to a live test target. Globs resolve relative to the crate root.
# ─────────────────────────────────────────────────────────────────────────────
REPO_ROOT="$(pwd)"
REACHABLE="$(REPO_ROOT="$REPO_ROOT" python3 - <<'PY'
import json, os, re, subprocess, sys
from pathlib import Path

repo_root = Path(os.environ["REPO_ROOT"]).resolve()
raw = subprocess.run(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"],
    check=True, capture_output=True, text=True,
).stdout
meta = json.loads(raw)

MOD_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;",
    re.MULTILINE,
)
COMPILE_FAIL_RE = re.compile(r'compile_fail\(\s*"(?P<g>[^"]+)"\s*\)')

entries: dict[Path, Path] = {}  # entry → crate root
for pkg in meta["packages"]:
    crate_root = Path(pkg["manifest_path"]).parent.resolve()
    for t in pkg["targets"]:
        if t["kind"] == ["test"]:
            entries[Path(t["src_path"]).resolve()] = crate_root

reachable: set[Path] = set()

def visit(rs_path: Path) -> None:
    rp = rs_path.resolve()
    if rp in reachable:
        return
    reachable.add(rp)
    if not rp.is_file():
        return
    try:
        text = rp.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return

    rs_dir = rp.parent
    for m in MOD_RE.finditer(text):
        name = m.group(1)
        for candidate in (rs_dir / f"{name}.rs", rs_dir / name / "mod.rs"):
            if candidate.is_file():
                visit(candidate)
                break

    crate_root = entries.get(rp)
    if crate_root is not None:
        for gm in COMPILE_FAIL_RE.finditer(text):
            for match_path in crate_root.glob(gm.group("g")):
                if match_path.is_file():
                    reachable.add(match_path.resolve())

for e in entries:
    visit(e)

out = []
for p in sorted(reachable):
    try:
        rel = p.relative_to(repo_root)
    except ValueError:
        continue
    out.append(str(rel))
print("\n".join(out))
PY
)"

if [ -z "$REACHABLE" ]; then
  echo "ERROR: cargo metadata returned zero test targets — cannot cross-check."
  exit 2
fi

# ─────────────────────────────────────────────────────────────────────────────
# Mode 1 — root tests/ has no .rs files (workspace is virtual, no [package])
# ─────────────────────────────────────────────────────────────────────────────
if [ -d "tests" ]; then
  ROOT_RS="$(find tests -type f -name '*.rs' 2>/dev/null || true)"
  if [ -n "$ROOT_RS" ]; then
    echo "ERROR: Root tests/ has .rs files — workspace root is virtual (no [package]), so these never compile."
    echo "Move them under crates/<crate>/tests/ or wire them with [[test]] entries."
    echo "$ROOT_RS" | sed 's/^/  /'
    DEAD=$((DEAD+1))
  fi
fi

# ─────────────────────────────────────────────────────────────────────────────
# Mode 2 — any .rs under crates/*/tests/ not in REACHABLE is dead.
# ─────────────────────────────────────────────────────────────────────────────
while IFS= read -r -d '' rs_file; do
  # Skip support directories (#[path]-included, not registered as test targets):
  # common/ is for shared test helpers, fixtures/ holds static test data,
  # snapshots/ stores insta baseline files.
  case "$rs_file" in
    */tests/common/*|*/tests/fixtures/*|*/tests/snapshots/*) continue ;;
  esac

  if ! grep -qxF "$rs_file" <<< "$REACHABLE"; then
    echo "ERROR: Dead test file (not under any cargo-registered test target): $rs_file"
    DEAD=$((DEAD+1))
  fi
done < <(find crates -path "*/tests/*" -name "*.rs" -print0 2>/dev/null)

# ─────────────────────────────────────────────────────────────────────────────
# Verdict
# ─────────────────────────────────────────────────────────────────────────────
if [ $DEAD -gt 0 ]; then
  echo "FAILED: $DEAD dead test issue(s) found"
  exit 1
fi

echo "OK: No dead tests detected"
exit 0