# Feature: release-plz full App-token migration (slice 2)

## Objective

Complete the GitHub App migration: App token in the tag/release job too,
delete `dispatch-release` (native push trigger replaces it), and rewrite the
dispatch guard to assert the new wiring. Slice 1 (PR job only, #1805) proved
the App flow end-to-end via #1807.

## Problem

With `GITHUB_TOKEN`, bot-pushed tags never fire `release.yml` (`push: tags`),
so `dispatch-release` + sweep hand the tag over explicitly. With an App token
the push fires natively — keeping dispatch guarantees duplicate concurrent
`release.yml` runs per release (sweep resolves in seconds, builds take
minutes, no concurrency group).

## Why now

Slice 1 verified in production: App mint works (`client-id`, zero deprecation
warnings), App-opened/bot-updated Release PRs trigger CI alone (#1807).
The tag path is the remaining half.

## Scope

- `.github/workflows/release-plz.yml`: App-token step in `release-plz-release`
  (contents-only perms), delete `dispatch-release` job, rewrite header.
- `scripts/release-tag-trust.sh`: accept EITHER tagger
  (`github-actions[bot]` OR the App bot) + unchanged subject check.
  Rationale: the tagger identity for App-pushed tags is ambient-git-config
  dependent and unverifiable without a live push; the lib itself documents the
  check as misfeature guard, not auth boundary (ruleset + preflight are).
- `scripts/check_release_dispatch.sh`: assert App-token wiring + absence of
  dual hand-off instead of asserting `dispatch-release` exists.
- Update `test_release_{reconcile,squash_dispatch,provenance}.sh` assertions
  that encode the old actor, if they do (verify first, don't assume).
- `AGENTS.md` release-automation section: replace the suppression/dispatch
  narrative with the push-native one.

## Out of scope

- `release-reconcile.yml` daily backstop and `sweep-releases.sh`: stay.
- `cut-patch-tag.yml` support-line flow: stays on its tokens.
- No Rust code, no version bumps, no CHANGELOG.

## Constraints

- Conventional branch `fix/release-plz-app-full` → base `main` (topology).
- SHA-pinned actions only; `client-id` (canonical v3), never `app-id`.
- One linked issue with `status:approved` before the PR; PR carries single
  `type:chore`.

## Tasks

- [ ] T1: workflow edit (App step in release job, delete dispatch, header).
- [ ] T2: trust predicate disjunction + reconcile/sweep untouched.
- [ ] T3: guard rewrite (`check_release_dispatch.sh`) + affected test scripts.
- [ ] T4: AGENTS.md sync.
- [ ] T5: verify (YAML, actionlint, zizmor, guard green, release test scripts
    green) + work-unit commit. No push/PR from the writer.

## Acceptance criteria

- `release-plz.yml` has exactly one hand-off path (native push); no
  `dispatch-release` job; guard passes on the new wiring and fails closed on
  missing App-token step.
- Trust predicate accepts both automation taggers; old-tag history still
  trusted (no re-tagging of history).
- CI lane for the PR is green; live proof deferred to the next release cycle.

## Verification evidence

- T1–T4 implemented by bounded writer; T5 spot-checked by parent.
- `python3 yaml.safe_load(release-plz.yml)`: OK.
- `actionlint`: exit 0, no findings. `zizmor --no-online-audits`: no findings.
- `bash scripts/check_release_dispatch.sh`: PASS, all 32 steps ok; fail-closed
  probes verified (old wiring → exit 1; re-added dispatch → exit 1).
- `test_release_reconcile.sh`: PASS=26 FAIL=0.
  `test_release_squash_dispatch.sh`: PASS=24 FAIL=0.
  `test_release_provenance.sh`: PASS=70 FAIL=0.
- Predicate functional test (scratch repo): `github-actions[bot]` trusted,
  `release-plz-tu-repo[bot]` trusted, human/lightweight/missing untrusted.
- Parent follow-ups: stale slice-1 comment in PR job fixed; `verify-release-tag.sh`
  error text generalized to "automation tagger". Single open assumption: the
  exact App tagger login is unevidenced in-repo — confirm against the next
  live tag (v-next) and narrow the allowlist if it differs.
