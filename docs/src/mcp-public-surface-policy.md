# MCP Public Surface Policy

What may change in `webfang_mcp` without breaking somebody, and what requires a
release. This is the written decision for issue #1614; the code attributes and
the machine check below exist only because this document states the rules first.

The source of the findings is `AUDIT-MCP-API-CONTRACT-20260925-035505/` →
`BREAKING_CHANGE_RISKS.md` (BC-01 … BC-10).

---

## The one-sentence version

> A client written against the advertised `inputSchema` of release *N* must keep
> working against release *N+1* for any change classified **additive** below, and
> must be re-validated by the human for anything classified **breaking** — and
> every classification below is checked by a test, not by good intentions.

---

## 1. Three boundaries, not one

The audit speaks of "the public surface" as if it were a single thing. It is
three, with three different sets of consumers, and conflating them is what
produces both false alarms and false comfort.

| # | Boundary | Who consumes it | Stability promise |
|---|---|---|---|
| **A** | The **MCP wire contract**: the tool inventory and each tool's advertised `inputSchema` | MCP clients in other processes. The only boundary with consumers outside this workspace. | **Strong.** Classified below. |
| **B** | The **Rust surface of `webfang_mcp`**: `pub` items in `crates/webfang_mcp/src/mcp_server/` | This workspace only. See §2. | **Explicitly none.** No semver promise. |
| **C** | The **`webfang_core` port traits** (`domain::*_port`, `domain::*`, `application::crawler`) | Implementors. Today: this workspace only (see §2). | Stated per trait in §6. |

**BC-10 is answered by this table**, and the answer is not "we promise
backwards compatibility". The reviewed source establishes no semver promise,
and none is being invented here for a crate that is never published. What is
being promised is a narrower and checkable thing: **the wire contract is
versioned by `contract_version`, and the workspace-internal Rust surfaces are
declared unstable.**

### Why B carries no promise

`release-plz.toml` sets `publish = false` for every crate in the workspace and
AGENTS.md states distribution is **binaries-only**. No `webfang_mcp` type has an
external Rust consumer today, and `webfang_cli` does not depend on
`webfang_mcp` at all (its `mcp` feature forwards to `webfang_core/mcp`).

The consequence is the opposite of the comforting reading: because nothing
outside the workspace compiles against `webfang_mcp`, **a Rust-level change to
it cannot break a user**. Changes there are reviewed on their own merits, not
under a compatibility rubric. `#[non_exhaustive]` therefore buys a `webfang_mcp`
type nothing against users — it only constrains this repository. That is why
§4 stamps it in exactly three places and records a deliberate "no" everywhere
else.

---

## 2. Consumers, stated once

Verified in this tree at `2b35f6ee`:

- `webfang_cli` → `webfang_mcp`: **no dependency edge**.
- `webfang_mcp`'s `pub` items referenced outside the crate: only its own
  integration tests under `crates/webfang_mcp/tests/`.
- `crates/webfang_mcp/tests/*` are *separate crates*: they link `webfang_mcp` as
  an external dependency, so `#[non_exhaustive]` binds them exactly as it would
  bind a downstream user. This is not a technicality — it is the reason §4's
  list is short.

---

## 3. Compatibility classes (boundary A)

| Class | Meaning | Release |
|---|---|---|
| **S0 — stable** | No client's observable behaviour changes. Docs, descriptions, internal logging. | patch |
| **S1 — additive** | Every previously-valid request stays valid and behaves identically. New clients gain capability. | minor |
| **S2 — breaking** | Some previously-valid request, or some previously-served response, changes meaning or stops being accepted. | **major** |

### 3.1 The compatibility matrix

One row per dimension of a property line in the checked-in manifest
(`crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv`). "Tightens" always
means *fewer requests are accepted* or *more requests mean something different*.

| Change to a tool's advertised schema | Class | Note |
|---|---|---|
| A new tool is registered | **S1** | The inventory is append-only within a major. |
| A tool is removed or renamed | **S2** | Includes moving it behind `cfg(feature)`. |
| A tool's `description` changes | **S0** | Prose, except where it makes a behavioural claim — see §7. |
| A **Rust doc comment** on a params type or enum changes | **S0 in intent, wire in fact** | See §3.3. schemars emits the doc comment into the advertised `$defs.<Type>.description`, so it is a wire string, not documentation. |
| A property is added and is **not** in `required` | **S1** | |
| A property is added and **is** in `required` | **S2** | Existing requests omit it. |
| A property is removed | **S2** | Even if the handler still tolerates it: `deny_unknown_fields` is on every params struct, so it becomes a rejection. |
| A property's `type` changes | **S2** | |
| Nullability **widens** (accepts `null` where it did not) | **S1** | Strictly more requests accepted; the new requests previously errored. |
| Nullability **tightens** (`null` no longer accepted) | **S2** | |
| `default` is added, and the runtime already applied exactly that value | **S1** | Pure advertisement of existing behaviour. |
| `default` changes, and the runtime changed with it | **S2** | **A silent behaviour change for a client that omitted the field.** |
| `default` changes in the schema only | **S0 defect, not S1** | This is drift, not a change. It is a bug in whichever side moved alone; see §5. |
| A bound **widens** (`minimum` lowers, `maximum` raises) | **S1** | |
| A bound **tightens** | **S2** | |
| An enum gains a variant | **S1** | Announce it. A client that matches exhaustively over JSON strings must add a wildcard. |
| An enum loses a variant | **S2** | |
| `additionalProperties: false` is removed | **S2** | Widens acceptance; `deny_unknown_fields` is a deliberate control. |

### 3.2 Response payloads

Response payloads are **out of scope for versioning in this issue** (audit
slice 4, BC-08). Three channels already exist and must be used instead of a
fourth shape: see [`mcp-error-contract.md`](mcp-error-contract.md). For success
payloads the promise this policy makes is narrow and honest: **the envelope is
stable, the body is not versioned.** Handlers that emit `Content::text` with
ad-hoc JSON are not covered by the manifest and must not be described as
covered.

### 3.3 A doc comment is not a doc change

Discovered while implementing this issue, and worth stating before somebody
learns it the expensive way: **schemars copies a type's Rust doc comment into the
advertised schema** as `$defs.<Type>.description`. Writing a thorough doc
comment on a type named by a params struct therefore changes what an MCP client
reads.

The first draft of `#[non_exhaustive]` on `SnapshotFormatParams` carried its
rationale in the doc comment and failed `mcp_advertised_schema_snapshot_test`
with a multi-line description diff. The rationale now lives in a `//` comment
directly above the doc comment, and the doc comment is frozen.

Rule: **a type that appears in an advertised schema keeps its doc comment
byte-stable.** Put the reasoning next to it, not in it. Types that never reach a
schema (`Outcome`, `DefaultOverride`) are unconstrained.

---

## 4. `#[non_exhaustive]` — where, and why not everywhere

`#[non_exhaustive]` is **not** free. Adding it to a type that external code
matches exhaustively, or constructs with a struct literal, is itself a breaking
change for those consumers — the standard library's own
`std::collections::hash_map::Entry` addition was a breaking change for exactly
this reason.

### 4.1 Applied

| Type | File | Why it is safe here |
|---|---|---|
| `SnapshotFormatParams` | `params.rs` (BC-02) | It is *deserialized*, never constructed: `McpUrl` sets the precedent (private field, `TryFrom<String>` only). No test crate constructs it. The two in-crate exhaustive matches in `handlers/axtree.rs` stay legal — `non_exhaustive` only binds outside the defining crate. |
| `DefaultOverride` | `schema_bridge.rs` | A bridge vocabulary already extended once in place (`SetBounds`, #1294), i.e. it demonstrably grows. Only `apply_default_overrides` matches it, in-crate. |
| `Outcome` | `metrics.rs` | A metrics bucket whose set has already grown with each new outcome concept (admission control, #1611). Only in-crate. |

### 4.2 Deliberately NOT applied

**The 16 public params structs (BC-03).** Not an oversight, and not a
half-measure — a decision with three reasons:

1. **It would break the only consumers that exist.** `crates/webfang_mcp/tests/`
   build these structs with struct literals in 16 places
   (`mcp_params_validation_test.rs`). Adding the attribute turns every one of
   them into a compile error, and the fix is 16 constructor builders that buy
   nothing: no user compiles against this crate (§2).
2. **The compatibility they threaten is already protected by the right
   mechanism.** The wire surface of these structs is the advertised schema, and
   that is pinned byte-for-byte by `mcp_advertised_schema_snapshot_test`
   (#1612) and semantically by `options_spec_parity_test`. The gap `#[non_exhaustive]`
   would close — "someone constructs this struct from outside" — is empty.
3. **The attribute encodes the wrong axis.** These structs grow by *field*, and
   their `required`-ness is a wire property that already has a machine check.
   An attribute that constrains struct literals would not notice a new
   *required* field, which is the actual S2 risk.

If this crate is ever published, revisit 4.2 with constructors — not before.

**`Origin` (`provenance.rs`).** Its three variants are a complete semantic
partition ("where does this text come from"), not a growing vocabulary. Adding a
variant would change provenance labelling semantics, which is a correctness
matter under §7, not a compatibility lever.

---

## 5. Defaults, nullability and bounds (BC-09)

These are compatibility-bearing, and the audit's "no-op schema fields may
activate on a later build" is the sharp form of the risk: a field that is
inert today and *accidentally* becomes live is a behaviour change with no code
review signal.

Three rules, each machine-checked:

1. **An advertised default must equal the applied default.** Enforced for
   OptionsSpec-backed properties by `options_spec_parity_test`
   (`overridden_defaults_advertise_runtime_effective_values`) and for
   MCP-only properties by `mcp_advertised_defaults_test` (#1294 NS-04). The
   bridge's `DefaultOverride::{Set, Unset, SetBounds}` is the single sanctioned
   way to make them differ *deliberately*, and each override must name a real
   property — an override naming an unknown property warns and is skipped, so a
   renamed field cannot leave a stale override behind silently.
2. **A nullability union in the schema must match serde's acceptance.**
   `promote_spec_to_nullable` reconciles an `Option<T>` field's derived
   `["<inner>", "null"]` with the spec rendering; a mismatch is an S2 defect.
3. **A bound must equal the bound the validator enforces**, taken from the same
   constant. `concurrency` advertised `minimum: 0` while the validator rejected
   0 — the class of drift this rule exists for.

The manifest in §8 pins the advertised side of all three for **every**
registered tool, not only the bridged seven.

---

## 6. Port traits (BC-04, BC-05, BC-06)

Adding a **required** method to a public trait is source-breaking for every
existing implementor. What determines the class is whether the trait is sealed:

| Trait | Sealed? | Adding a required method |
|---|---|---|
| `CheckpointStore` (`application/crawler/checkpoint.rs`) | **Yes** — `mod private { pub trait Sealed {} }`, private module | **S1.** No external implementor exists. This is the reference pattern. |
| `SemanticCleaner` (`domain/semantic_cleaner.rs`) | **Yes** — same pattern. `pub mod private` rather than `mod private`, which is a *visibility* difference with **no** sealing effect: the bound `private::Sealed` is what seals it, and it is unsatisfiable outside the crate. | **S1.** |
| `NoteRepository`, `VaultNoteReader` | No | **S2** for any downstream implementor. |
| `TextChunker`, `EmbeddingPort` | No | **S2** for any downstream implementor. |

**BC-05 is settled**: `SemanticCleaner` **is** sealed. The audit's "Needs
verification" is closed by reading the bound, not by a compile-fail fixture —
which would have required a `trybuild` dev-dependency this issue may not add.
The residual risk is a future contributor changing `pub mod private` to
`pub(crate) mod private`; that still seals, and a blanket `private::Sealed`
bound is the thing to protect. Adding the sealing compile-fail fixture remains
open and is recorded here as future work rather than silently skipped.

**The migration rule** for BC-04: an unsealed port trait that acquires a new
capability should first gain a **defaulted** method (S1), and only take a
required one (S2) behind a major. Sealing first makes every later method S1.

---

## 7. Always-listed tools (BC-07)

Verified in this tree: `handlers::ai::build_router` and
`handlers::axtree::build_router` are called unconditionally from
`build_tool_router`, with no `#[cfg]` on the function or the call. So the tool
inventory is **identical across feature builds**, and that is a deliberate
strength the audit correctly told us not to regress.

The client-facing consequence, which is the part BC-07 left open:

- **A tool is never absent.** Availability depends on features and runtime, not
  on registration.
- **An unavailable tool answers with a result, not an absence.** Handlers route
  through `honest_error`, producing `isError: true` with Spanish content text and
  a reason slug — never a protocol error, never a false success.
- Therefore: **gating a registration behind `cfg(feature)`, or dropping a tool
  from the always-listed inventory, is S2.** The audit's "verify the client
  policy before changing registration" is answered here, and the answer is
  "do not".

The one exception is `test_probe`, registered only under an env gate. It is a
zero-argument probe for `#1646` panic containment, not a product tool; it is
excluded from the manifest for that reason, and the exclusion is stated here
rather than left implicit.

---

## 8. What is checked, and by what

Policy nobody can check drifts; this section exists so the coverage claim is
auditable rather than asserted.

| Rule | Enforced by | Runs in CI |
|---|---|---|
| Every registered tool's inventory (names + count) | `public_surface_policy_test::tool_inventory_is_pinned` | yes |
| Per-property `type` / nullability / `default` / `minimum` / `maximum` / `required` | `public_surface_policy_test::advertised_property_matrix_is_pinned` | yes |
| Advertised default == applied default (bridged + MCP-only) | `options_spec_parity_test`, `mcp_advertised_defaults_test` (#1612, #1294) | yes |
| Bridged schema byte-for-byte | `mcp_advertised_schema_snapshot_test` (#1612) | yes |
| A shape change **moved `contract_version`** | `scripts/check_mcp_public_surface.sh` | **no** — local gate, see below |

### 8.1 `contract_version`

A single integer in `crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv`.
It starts at **1**, and:

- **Any S2 change bumps it**, and requires a major release.
- **Any S1 change that alters a compatibility-bearing row** bumps it, and
  requires a minor release.
- **S0 changes do not bump it.**

### 8.2 The honest gap

The manifest test compares the **live** surface against the **committed**
fixture, so it fails in CI the moment a shape moves and the fixture is not
updated. It cannot know *whether the author bumped `contract_version`*, because
both live in the same file.

`scripts/check_mcp_public_surface.sh` closes that half. It diffs the fixture
against its committed state at the merge-base with `main`, classifies every
changed row per §3.1, and **exits non-zero when the surface moved and
`contract_version` did not**.

It is wired into `scripts/ci_fast_gate.sh` (the local pre-push gate). It is
**not** wired into `.github/workflows/ci.yml`, which this issue is forbidden to
edit. So the enforcement split is: *deep check in CI, version-marker check
locally*. That is weaker than a CI-enforced gate and is stated rather than
glossed. The follow-up is a two-line addition to `ci.yml`'s `repo-guards` job,
which is a one-PR change once this stack merges.

---

## 9. Per-finding disposition

| id | finding | disposition |
|---|---|---|
| BC-01 | Export names not aligned (`export_format` ↔ wire `format`) | **Root cause owned by #1612.** Here: recorded as the canonical case for §3.1 — renaming either side is S2. Not re-fixed. |
| BC-02 | `SnapshotFormatParams` exhaustive | **Fixed** — `#[non_exhaustive]`, §4.1. |
| BC-03 | 16 params structs unannotated | **Decided: no attribute**, with reasons, §4.2. |
| BC-04 | Unsealed public port traits | **Policy declared**, §6. Migration rule stated. Sealing not performed — see §10. |
| BC-05 | `SemanticCleaner` sealing unverified | **Settled: it is sealed.** §6. Compile-fail fixture remains open. |
| BC-06 | `CheckpointStore` sealed, no method-addition policy | **Policy declared**, §6. Confirmed as the reference pattern. |
| BC-07 | AI/chromium always registered | **Client policy declared**, §7. Verified ungated. |
| BC-08 | Output payloads untyped/unversioned | **Deferred** — the manifest covers input schemas only, and §3.2 says so. Follow-up recorded. |
| BC-09 | Defaults / nullability / bounds | **Rules + machine check**, §5. |
| BC-10 | No semver promise stated | **Declared**, §1 — narrowed to three boundaries, with B and C explicitly promised nothing. |

---

## 10. Explicitly not done

- **No re-fix of the schema drift** (BC-01). `#1612` owns it and it is
  delivered. Nothing here changes a wire name, a field name, or an advertised
  schema.
- **No sealing of the BC-04 traits.** It is a `webfang_core` change with a wide
  blast radius across `ai`, `mcp`, `cli`, `benchmark` and `test_utils`, and
  three sibling issues branch off this branch. The policy above states what
  sealing would buy; the migration is its own change.
- **No output-DTO versioning** (BC-08). Recorded as slice 4.
- **No compile-fail fixture** for BC-05 — it needs a dev-dependency this issue
  may not add. The bound was read instead, which settles the same question.

---

## 11. When you change something on this surface

1. Classify the change with §3.1 before writing it.
2. Run `cargo nextest run -p webfang_mcp`. A red manifest test is not a chore to
   re-record — it is the answer to "did I classify this right?".
3. If a row changed, run `scripts/check_mcp_public_surface.sh --record`, then
   read its diff. It prints the class of every row that moved.
4. If any row moved, bump `contract_version` and put the release requirement in
   the commit body.
5. If a row moved and you classified it **S0**, you are wrong about one of
   them — go back to §3.1.