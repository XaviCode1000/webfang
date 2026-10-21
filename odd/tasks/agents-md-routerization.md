# Feature: AGENTS.md as a router — pruning to Engram with a verified destination

**Issue:** #2206 (`docs: AGENTS.md as a router — pruning to Engram (1752 → 741 lines) and correction of ~26 false claims`) — requires the owner's `status:approved` before the PR is opened.
**Labels:** `type:docs`
**Branch:** `docs/agents-routerization` (worktree `~/Projects/Rust/webfang-worktrees/docs-agents-routerization`), base `origin/main` @ `7c391c1e`
**Delivery route:** `single-pr` (this repo does not admit stacked PRs — see AGENTS.md "No stacked PRs")
**Scope:** one repo file (`AGENTS.md`) + 8 new Engram observations. No production code change.

## Objective

Turn `AGENTS.md` from 1,752 lines / 130 KB into a **router** of ~740 lines: it keeps the
project rules and the policies the harness does not provide (including `codedb`/`codegraph` and
the worktree policy, by explicit owner decision), and sends **everything else** to Engram with
a queryable pointer. No content is deleted without a verified destination.

## Specs

- **S1** — The owner's request, verbatim: *"necesito un analisis profundo sobre el archivo @AGENTS.md , porque creo que no estamos siguiendo las mejores practicas relacionadas con este tipo de archivos, y porque creo que incluye mucha informacion irrelevante para agentes y lo mas peligroso informaciones falsas."*

- **S2** — The destination decision, verbatim: *"vamos a utilizar (A) y no mover nada a /docs solo engram y mencionar en agentes.md como consultar cada informacoin en engram. asi conservamos lo importante."* ⇒ **Engram is the only destination for the pruned content.** No new file is created under `docs/`. The existing `docs/` (`ssrf-layers.md`, `persistence-resume.md`, `error-classification-matrix.md`, `merge-queue-manual.md`, `release-tag-ruleset.md`, `ci-slo.md`, `docs/src/**`) are **left untouched** — they are already referenced by the repo, by scripts and by users.

- **S3** — The instruction about SDD, verbatim: *"1- sdd ya esta deprecado eso eliminalo por completo."* ⇒ the `SDD planning phases | sdd-*` row of the Skill Routing Matrix and the `Gate 0 freeze — RETIRED` block are deleted; the `## 🚧 Sprint 0 — StateStore resume contract (sdd/stabilization-sprint0-baseline)` heading is rewritten without the `sdd/` reference. **The historical observations with `topic_key` `sdd/*` in Engram are not deleted** (obs #1131, #1193, #1194, #1206 hold valid, referenced design): what is deprecated is the *method*, not the knowledge.

- **S4** — The owner's correction, verbatim: *"manten codedb + codegraph y nuestra politica de worktree en egents.md el resto aplica tus recomendaciones."* ⇒ **two blocks are kept whole in `AGENTS.md`**: (a) the `🔬 Intelligence Stack` section (codedb/codegraph routing + gates + absolute paths in worktrees) and (b) the **worktree policy** (rules, placement, bootstrap, identity protocol, cleanup, seeds, shared resources, namespace, contamination, immutable history). They are kept *condensed* — the incident post-mortems go, the rules stay.

- **S5** — Before pruning, **every outgoing block has a verified destination**: 8 new Engram observations plus the 8 pre-existing ones that cover worktrees. Nothing is deleted without a destination.

- **S6** — The file **renders**: fence balance verified by script (it had 67 fences, an odd number ⇒ 1,221 lines of prose rendered as code and 9 of 13 H2 were not headings). The new file has paired fences and all its H2 are headings.

- **S7** — **Zero line-number citations** in the file. The document's own rule demands it (*"a line-number citation in this file is invalidated by any edit above it"*) and it had ~40. It cites the **symbol** (`WreqDownloader::fetch_inner`, `RateLimiterConfig::new`), not the line. File paths are kept.

- **S8** — **Zero mutable state tables** copied into prose. The support-line state lives in `.github/support.json` and is read with `jq`; the tool count lives in `crates/webfang_mcp/tests/stdio_handshake.rs`; the dependency edges are derived from `cargo metadata`. The document **links** those sources, it does not replicate them.

- **S9** — The ~26 false claims detected by the 2026-10-21 audit are **corrected or removed**. The verifiably false ones to correct or delete: support.json declares 4.0 STABLE / 3.0 MAINTENANCE (the file said 2.12/2.11/2.10); `CrawlCheckpoint` **is** wired to `--resume`; the AI shims **no longer exist** (`--ai-model` and `--max-tokens` get clap's "unexpected argument" error, exit 64; the envs `WEBFANG_AI_MODEL_ID`/`AI_MODEL_ID`/`WEBFANG_MAX_TOKENS`/`WEBFANG_AI_ENGINE` are silently ignored, with no notice) and the real surface is `--threshold`/`--max-chars`/`--offline`; `InfraError` has 15 variants (not 13) and `DomainError` 7 (correct); the downloader lives in `infrastructure/downloader/` (not `infrastructure/crawler/`); `infrastructure/mcp_server/mod.rs` **does not exist** (the real one is `crates/webfang_mcp/src/mcp_server/mod.rs`, 443 lines) and the actually-large files are `application/crawler/engine.rs` (3,219), `cli/preflight.rs` (3,192), `infrastructure/http/waf_engine.rs` (2,570), `mcp_server/handlers/scraping.rs` (2,513), `src/error.rs` (2,158); the `cli → mcp` edge **does not exist**; the CLI code lives in `crates/webfang_core/src/cli/` (`webfang_cli/` is a single `main.rs`); the real branch regex includes `hotfix|release|support`; `test_support_drift.sh` has 29 cases (not 50); `.cargo/config.toml` **exists**; `[profile.dev.build-override]` **does not exist** (it is a planned experiment); `ExportState.version` is a `StateVersion` type-state; local git is 2.56; the `type:testing` label exists; the job keys are `release-plz-pr`/`release-plz-release` (not `update`); `.envrc` is in `.gitignore`; the `codedb`/`codegraph`/`sdd-*` skills **do not exist** (the CLIs do: `codedb 0.2.5849`, `codegraph 1.6.2`).

- **S10** — The Engram lookup table is **verified**: every row names the query that was run with `mem_search` and the observations it returns. No pointer to something that was not checked.

- **S11** — The `💾 Engram` section **grows** instead of shrinking: it is the map of the project memory. It loses the one-off historical cleanup ("27 stale sessions closed on 2026-09-26") and gains the lookup mechanism.

- **S12** — The file opens with **runnable commands** (best practice #1: commands go on top, not at the bottom) and with the Engram pointer table, so an agent knows where to look without walking 1,700 lines.

- **S13** — **The file describes the CURRENT state, not the history.** No "we used to do this", no "we had X installed": if something is no longer there, it is **not mentioned at all** — neither to explain its absence, nor to warn that it is expected. The current state is what matters. Direct consequence: the AI shims, the `webfang_ai` crate, the local ONNX tree, the `git-hygiene.timer` and the OpenTelemetry backend **do not appear**, because they do not exist; and the perishable snapshots ("the biggest files in the repo today are X with N lines") become a rule plus the command that lists them. This rule applies to the normative file: the `## Log` of a feature doc and the Engram observations do keep history, that is their contract.

- **S14** — **Every repository-facing artifact is in English.** The owner requirement: *"estas mezclando ingles con espanol, solo ingles"*. The router, this feature document and the issue/PR bodies are English; the conversation with the owner stays in Spanish. The verbatim owner quotes inside this document stay in Spanish, because the spec format requires quoting the user's exact words.

## Tasks

| ID | Spec | Description | Route | Commit |
|---|---|---|---|---|
| T1 | S5 | Write the new Engram observations (destination for the pruned content) | inline (obs #2306, #2307, #2308, #2309, #2311, #2312, #2313, #2316) | — (memory) |
| T2 | S1-S4, S6-S9, S13, S14 | Rewrite `AGENTS.md` as a router | inline (parent) | 1 |
| T3 | S6, S7, S8, S10, S11 | Verify: paired fences, zero line-number citations, zero state tables, Engram queries resolve, facts correct | inline + independent verifier | — |
| T4 | S1 | Create issue + PR (`Closes #N`, label `type:docs`, base `main`) | inline | — |
| T5 | — | Phase 2 (out of scope, needs a separate decision): fail-closed guard `scripts/check_agents_doc.sh` in `repo-guards` (fences + fact asserts + size budget) | — | — |

## Log

- **L1 (2026-10-21)** — Original request, verbatim: *"necesito un analisis profundo sobre el archivo @AGENTS.md , porque creo que no estamos siguiendo las mejores practicas relacionadas con este tipo de archivos, y porque creo que incluye mucha informacion irrelevante para agentes y lo mas peligroso informaciones falsas."*

- **L2 (2026-10-21)** — The owner asks about the source: *"en engram esta guardad informacion como se crean y las mejores practicas para este tipo de archivos,"* ⇒ there is `obs #368` (`research/webfang/agents-md-best-practices`, 2026-08-23: 11 best practices from 2 sources + a prior audit of the same file) and `obs #370` (`decision/webfang/knowledge-architecture`, the owner's binding decision: methodology does NOT go in AGENTS.md; the repo holds only project-specific rules).

- **L3 (2026-10-21)** — Audit result: 67 fences (odd) ⇒ 1,221 lines (70%) of prose rendered as code; 9 of 13 H2 not headings; ~26 false claims; the file tripled since the August audit (688 → 1,752 lines) without the L2 decision being applied. Detail in `obs #368` rev 2, `#2302`, `#2303`.

- **L4 (2026-10-21)** — Destination decision, verbatim: *"vamos a utilizar (A) y no mover nada a /docs solo engram y mencionar en agentes.md como consultar cada informacoin en engram. asi conservamos lo importante."* + note 1: *"sdd ya esta deprecado eso eliminalo por completo"* + note 2: *"necesito que me digas antes creando una lista que vamos a aliminar y conservar y porque. para decidir antes de podarlo"*.

- **L5 (2026-10-21)** — The keep/delete list was delivered and approved with one correction, verbatim: *"manten codedb + codegraph y nuestra politica de worktree en egents.md el resto aplica tus recomendaciones. adelante"* ⇒ S4. The 4 decisions (DB) are applied: observations first and visible; 6 lines for RDD; workstation profile with `scope=personal`; guard in a separate phase (T5).

- **L6 (2026-10-21)** — Checked against `origin/main` @ `7c391c1e` (post-#2184): the AI shims **no longer exist** (obs #849, T2 of the ADR-0004 campaign). `crates/webfang_core/src/cli/args/ai.rs` pins it in the `the_retired_shims_are_not_flags_anymore` and `the_retired_shim_env_vars_are_ignored` tests: *"The retired shims are GONE (issue #2119): they are not flags any more, so clap answers with its standard 'unexpected argument' error"* and *"The retired env doors are IGNORED, not rejected"*. `ONNX_DEPRECATION_NOTICE` does not exist in the tree. The real AI surface is `--threshold` / `--max-chars` / `--offline`. AGENTS.md (1,752 lines, 67 fences, 131,568 bytes) did not change between `726921b9` and `7c391c1e` except for one line.

- **L7 (2026-10-21)** — Implementation decision: the guard (T5) stays **out** of this PR — it gets its own PR with its own harness and must not block the pruning. Pruning and guard are sequential slices, not stacked.

- **L8 (2026-10-21)** — **The isolation guard caught me.** I wrote the worktree's `.envrc` with a literal, unsubstituted `$TREE` (I had escaped the `$` in the heredoc), and `scripts/ci_fast_gate.sh` rejected it with `reason=unregistered-target` — exactly the failure documented in `obs #2205` and in the AGENTS.md bootstrap section itself. Correction: `$TREE` is substituted when the file is written; only `$HOME` stays literal. **The repo's own policy detected my error**: that is the argument for the phase-2 guard (T5).

- **L9 (2026-10-21)** — Independent verifier (8 checks) over the result. **2 BLOCKER**: (a) the error stratification section was **inverted**: `DomainError`/`InfraError` converge **towards** `ScraperError` via `From` (they do not stack), `CliError` is independent, `HttpError` is a **domain** enum, `WafError` is not an error type (it is a `FetchBehavior` variant) and `ParseError` **does not exist**; (b) 4 durable rules without a destination: "commit frequently", the cloud-verification rules (`gh run watch` timeout, ≥180 s + retry, a timed-out push → verify with `git ls-remote`), the **main-push gate** and the `core.hooksPath` wiring ("never match a gate against git's human-facing prose"). 5 SHOULD-FIX: the claim "no head prefix may target another working branch" was false (`fix/*` → `release/X.Y` and `hotfix/*` → `support/X.Y` are admitted); `.atl/skill-registry.md` is gitignored and absent from a fresh clone; `src/error.rs` is 2,158 lines (~2,200); the Sprint 0/StateStore section was deleted instead of rewritten (spec S3); "Skills of the repo" labelled 6 user-scope skills as repo-scope. 3 NIT: `RateLimiterConfig::new` attribution (already fixed earlier, the verifier read the previous version), issue TBD, branch 1 commit behind main.

- **L10 (2026-10-21)** — Corrections applied: the error diagram rewritten as a funnel with the four real `From` impls; the 4 durable rules recovered (one back into AGENTS.md, three into obs **#2316** `project/webfang/delivery-gates-and-ci-verification`, with a verified query); the topology claim narrowed to feature head prefixes; the note that `.atl/skill-registry.md` is generated per tree; `src/error.rs` to ~2,200; the skill scopes corrected; the Sprint 0 section replaced by a pointer to `docs/persistence-resume.md` + obs #216/#1341 (the old text was false: `version` is a `StateVersion` type-state and the `CURRENT_VERSION` worth 2 lives in `record_store.rs`). Every correction was verified with CodeDB before being applied, not taken from the report.

- **L11 (2026-10-21)** — `origin/main` advanced 2 commits during the work; rebase applied. Those commits did not touch AGENTS.md, so the rebase was clean.

- **L12 (2026-10-21)** — The owner stated it explicitly: *"utiliza codedb siempre"*. Recorded as `obs #2315` (`preference/webfang/use-codedb-always`). In practice it caught 2 wrong symbol attributions (`until_ready_or_cancel_observed` and `RateLimiterConfig` were attributed to the call-site module instead of the definition module) that grep had missed.

- **L13 (2026-10-21)** — **Slice limit (answer to the pre-commit hook warning).** The hook warned: *"Commit slice: 2374 authored changed lines in 1 commit ... Past the ~400-line slice"*. The delivery strategy of this feature is **a single work-unit**: the deliverable is *one* file (`AGENTS.md`) rewritten, plus its feature doc and its mirror. Splitting a router rewrite into partial commits would produce commits where the file **does not render** (fences unbalanced halfway) or where the Engram pointers reference content that does not exist yet — that is, commits in a state that is neither old nor new. The diff is −1,614/+760 on a single file, which is the mechanical consequence of pruning, not evidence of uncontrolled scope. **The slice limit is re-established here**: the next slice is the guard `scripts/check_agents_doc.sh` (T5, phase 2), which gets its own PR and does fit comfortably in the ~400-line budget.

- **L14 (2026-10-21)** — The worktree identity proxy reported MISMATCH on the first commit: the directory was `agents-md-routerization` and the branch `docs/agents-routerization` (the proxy requires `docs-agents-routerization`). The commit was **not** excepted: since it was local and unpushed, it was resolved **structurally** with `git worktree remove` + `git worktree add` under the correct name (the route the repo policy prescribes; `git worktree move` is forbidden). The commit survived the re-add and the proxy now passes. Lesson: the proxy is not bureaucracy — the directory is the branch-naming convention, and a silent deviation breaks the correspondence that makes a repo with many worktrees readable.

- **L15 (2026-10-21)** — The owner established a baseline rule for the file: *"no menciones informacion irrelevante, por ejemplo antes hacia esto o usabamos en el pasado ..., teniamos instalado ..., si algo ya no esta no se menciona en absuluto, lo que importa en este archivo es la actualidad estado actual"*. Recorded as `obs #2321` (`preference/webfang/agents-md-current-state-only`) and as spec **S13**. Applied to this PR's AGENTS.md: the mentions of the retired AI shims, the `webfang_ai` crate, the local ONNX tree, the `git-hygiene.timer` and the OpenTelemetry backend were removed; the references to historical issues/PRs were removed (the Engram pointers stay — they are current state); and the "biggest files in the repo today" snapshot became a rule plus the command that lists them. The file drops from 752 to 740 lines / 44.8 KB.

- **L16 (2026-10-21)** — The owner required all repository-facing artifacts in English only: *"estas mezclando ingles con espanol, solo ingles"*. The router, this feature document and the issue/PR bodies were translated from Spanish to English; no fact, path, query or identifier changed. The conversation with the owner stays in Spanish. The delegated translation worker stalled on its first call and delivered nothing, so the parent did the `AGENTS.md` translation itself and re-aligned the error diagram and restored the `//` comment marker it had broken. The owner quotes inside this document stay in Spanish, because the spec format requires quoting the exact words.
