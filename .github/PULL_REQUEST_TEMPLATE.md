## Linked Issue

<!-- REQUIRED — use one of: Closes #N / Fixes #N / Resolves #N -->
Closes #

## PR Type

<!-- Check exactly ONE, then add the matching label -->
- [ ] Bug fix (`type:bug`)
- [ ] New feature (`type:feature`)
- [ ] Documentation only (`type:docs`)
- [ ] Code refactoring (`type:refactor`)
- [ ] Maintenance / tooling (`type:chore`)
- [ ] Breaking change (`type:breaking-change`)

## Summary

<!-- 1-3 bullet points: what does this PR do and why -->
-

## Changes

| File | Change |
|------|--------|
| `path/to/file` | what changed |

## Test Plan

<!-- Adapt to the change. For Rust work, use the exact CI gates (see AGENTS.md — a bare clippy passes locally but CI fails). -->
- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` clean
- [ ] `cargo nextest run` passes
- [ ] Manually verified the affected functionality

## Contributor Checklist

- [ ] Linked an approved issue (`status:approved`) with `Closes/Fixes/Resolves #N`
- [ ] Branch name matches `type/description` (e.g. `fix/parser-crash`) — CI rejects others
- [ ] Added exactly one `type:*` label
- [ ] Conventional commit format (`type(scope): description`)
- [ ] No `Co-Authored-By` / AI attribution trailers
- [ ] Docs updated if behavior changed
- [ ] Defensive error paths annotated with `// LCOV_EXCL_*` markers per docs/src/testing.md (issue #527)
