# Rustdoc module scope trap

Do not put an outer `///` doc comment on a `pub mod` whose file already
starts with inner `//!` module docs. The `///` line silently re-scopes
every short-form intra-doc link in the file, and the resulting
`unresolved link` error points at the parent `///` line instead of the
broken link.

## Quick path

1. If the module file has `//!` docs at the top, declare it with a bare
   `pub mod foo;` — no `///` line above it.
2. Inside the file, short-form links like ``[`is_forbidden_ip]`` keep
   resolving against the module itself.
3. Verify with `RUSTDOCFLAGS="-D warnings" cargo doc -p webfang_core --no-deps --all-features`.

## Details

| Topic | Decision |
|-------|----------|
| Minimal repro | `pub mod ssrf_guard;` (bare) lets ``[`is_forbidden_ip]`` in `crates/webfang_core/src/domain/ssrf_guard.rs` resolve to the item in that same file. Adding `/// SSRF guard — ...` above the `pub mod` line re-parents the doc scope, so the same short-form link no longer resolves. |
| Misleading span | With the `///` line present, `cargo doc` reports `unresolved link to is_forbidden_ip` but the diagnostic span points at the parent `///` line in `crates/webfang_core/src/domain/mod.rs`, not at any link in the module file. If a link error points at a `pub mod` doc line, suspect the scope trap before touching the links. |
| Chosen convention | No outer `///` on a `pub mod` whose file already carries `//!` module docs. The `//!` block is the single doc home; the parent keeps a bare declaration. |
| Why short-form links | Fully qualified links (`crate::domain::ssrf_guard::is_forbidden_ip`) survive either scope, but short-form links are the file-local norm — the fix here shortened 7 links in `crates/webfang_core/src/domain/ssrf_guard.rs` instead of keeping the scope-breaking `///` line. |
| Broken-today list | Only `crates/webfang_core/src/domain/ssrf_guard.rs` was broken (fixed in this change: `///` line dropped, links shortened). The other 24 outer-`///` sites repo-wide were surveyed and are healthy — plain prose with no scope-sensitive links, so no speculative reformatting. |

## Checklist

- [ ] New `pub mod` with a `//!`-documented file uses a bare declaration.
- [ ] A link error spanning a `pub mod` doc line is read as a scope problem first.
- [ ] Paths naming non-public items stay plain code spans, never link syntax.

## Next step

Slice 2 of the singleton batch (issue #1062); slice 1 was #1045. Run the
verify command above before touching module docs in `domain/`.
