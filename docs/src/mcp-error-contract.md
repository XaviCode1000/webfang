# MCP error contract — which channel carries which failure

**Status:** published decision table for issue #1613. Describes the server as it stands on top
of commit `ccdef614` **plus the uncommitted working-tree changes that converted the last 16
reason-less rejection sites**. Commit `ccdef614` is the state **after code slices 2–5 landed**
(EC-02 … EC-08) and closed EC-08's machine-readable half everywhere it was originally scoped;
the working tree closes it everywhere else. This page states what the server **does**, not what
it should do; a future change that invalidates a row must change this page in the same commit.

**Evidence class: mixed, and the split is deliberate.** Rows A1, A2, A3, A6, A7, A8, A9
and C1 are **observed over a real transport** — they are asserted on the wire in
`tests/mcp_error_channel_mapping_test.rs` (A1, A2, A3),
`tests/mcp_ssrf_error_class_test.rs` (A6–A9) and
`tests/mcp_validation_reason_code_test.rs` (C1 and the `data` shape). Every other row is
still **statically derived**: read out of the source, not exercised end-to-end. Rows A4, A5,
A10–A19, B1–B8, C2–C4 and T1–T9 are in that second group. (A5 is the only SSRF row not
covered end-to-end: `no_host` is unreachable from outside, which is exactly what its row
says.)

**Slug evidence is narrower than row evidence, and the two must not be conflated.** The
wire-level slug assertions cover exactly three validation slugs (`unsupported_scheme`,
`too_long`, `out_of_range` in `tests/mcp_validation_reason_code_test.rs`) plus three SSRF
slugs (`forbidden_ip_literal`, `forbidden_ip_resolved`, `dns_resolution_failed` in
`tests/mcp_ssrf_error_class_test.rs`). The A4.x and A10–A14 slugs are **not** transport-observed:
A4.x is pinned by the in-crate unit test
`path_gate::every_rejection_carries_field_and_reason_slug`, which samples every branch of
`confine`, and the A10–A14 slugs by construction — the reason-less constructor those sites
used to call has been deleted, and no bare-string `data` producer remains anywhere in the
crate (see the [appendix](#appendix--construction-site-inventory)). Nothing here is inferred
from a *hypothetical* transport any more; see
[Unsettled rows](#unsettled-rows) for the one question that changed evidence class in the
other direction and the one deliberate non-action.

## Quick path for an agent consumer

```text
HTTP status != 200                      → Channel 0. Transport refused the request. Fix the request shape.
JSON-RPC "error" member present         → Channel A. The call never ran. Fix the arguments.
result.isError == true                  → Channel B. The call ran and failed. The text is the reason.
result without isError                  → Channel C. The call ran. Read the body: it may be a
                                          diagnostic, a partial result, or a success.
```

Two things that table does not say, and that bite:

- **A malformed argument does not always reach Channel A.** A *deserialization* rejection
  (rows A1, A2) arrives as `isError: true` — Channel B. Only a rejection raised by the
  handler body (row A3) is a JSON-RPC `-32602`. The type is the same on both routes; the
  *wire shape* is what differs. See [A1–A3](#a1a3--argument-deserialization-before-the-handler-body).
- **Channels A and B overlap textually.** A `isError: true` body can contain the literal
  string `-32602`. Never discriminate on the code appearing in the text — see
  [Channel overlap](#channel-overlap--the-text-is-not-the-discriminator).

"Retriable" in this table means: **repeating the identical call can succeed without the
caller changing anything.** Anything else is a caller change, not a retry.

## The three channels (plus the one before them)

| Channel | Wire shape | HTTP | Carries | Code |
| :--- | :--- | :--- | :--- | :--- |
| **0** — transport | bare status, usually no JSON-RPC body (T9 is the exception) | 401/406/408/413/415/422/429/500 | a request that never reached a tool | none (T9 adds a `-32603` body) |
| **A** — protocol error | `{"error": {"code": …, "message": …, "data": …}}` | 200 | the call was unroutable, or its arguments were refused **by the handler body** | `-32601` / `-32602` / `-32603` |
| **B** — tool error | `result.isError = true` | 200 | the tool ran and the job failed — *or* its arguments never deserialized | none |
| **C** — success-shaped diagnostic | `result`, no `isError` | 200 | the tool ran; the body describes a non-success outcome | none |

Channel A is the only channel with a machine-readable `code` **and** a machine-readable
`data.reason` slug (see [the reason taxonomy](#the-reason-slug-taxonomy--the-machine-readable-contract)).
Channel B carries **no code**: the only machine-readable signal is the boolean, and the
reason is free text. Channel C has the same gap, which is the whole reason it is listed
separately — see row C1.

## Channel overlap — the text is not the discriminator

> **A Channel B body can literally contain `-32602`.** Two independent facts combine:
>
> 1. `McpError::invalid_params(msg, data)` builds an `rmcp::ErrorData`, and `ErrorData`'s
>    `Display` renders `"{code}: {message}({data})"` — the numeric code is part of the
>    *string form* (`rmcp-1.8.0/src/error.rs:8-16`).
> 2. On the deserialization route, rmcp wraps any serde error as
>    `"failed to deserialize parameters: {error}"`, so that rendered `ErrorData` — code
>    and `data` included — is what lands in the `isError: true` body.
>
> An A1/A2 rejection therefore reads, in the tool text, roughly
> `failed to deserialize parameters: -32602: …({"field":"url","reason":"malformed"})`
> while carrying **no** `error` member at all. The only reliable discriminator is the
> wire shape: **is there a top-level `error` member, or is `result.isError` true?** Branch
> on that, never on a code appearing in the text.
>
> This is also why the *reason slug* is the machine-readable half on this route: it is
> readable in the text (rmcp's `Display` appends the `data` JSON verbatim) even though it
> is not delivered as structured data. Parsing it out of prose is a last resort, not the
> contract — rows A1/A2 give no `data` a client can index.

---

## Channel A — JSON-RPC protocol errors

Emitted when a tool function returns `Err(McpError)`; `McpHandler::call_tool`
(`mcp_server/mod.rs:359-382`) forwards that `Err` unchanged to rmcp.

Two things about this channel are not obvious from the table, and both were settled by
issue #1613 rather than assumed:

- It is **not** the channel every argument rejection takes. Only a rejection the *handler
  body* raises is Channel A — settled by observation in slice 3.
- `error.data` is **not** a single shape. There are **two** live shapes, listed in
  [the reason taxonomy](#the-reason-slug-taxonomy--the-machine-readable-contract): an object
  carrying both `field` and `reason`, and an object carrying `reason` alone. **Every
  argument refusal carries a slug** — the pre-EC-08 bare-string shape and the reason-less
  shape are both gone. The only Channel A rows still left with no `data` at all are the
  `-32603` server faults (A15, A16), which never had a caller mistake to describe.

### A1–A3 · argument deserialization (before the handler body)

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :--- | :--- | :--- | :-: |
| A1 | URL argument is not http(s), is oversize, or carries embedded credentials | **none — `result.isError = true`**, no `error` member (observed) | No | Reshape the URL. Never retry unchanged. | `mcp_server/params.rs:81-99` (`McpUrl::try_from`); observed in `tests/mcp_error_channel_mapping_test.rs:99-127` |
| A2 | Unknown JSON key in the argument object (`deny_unknown_fields`) | **none — `result.isError = true`**, no `error` member (observed) | No | Drop the unknown key. | `mcp_server/params.rs:7` (per-struct attribute, first at `:316`); observed in `tests/mcp_error_channel_mapping_test.rs:139-166` |
| A3 | In-handler `params.validate()?` semantic rejection | `-32602` (observed) | No | Fix the named field; `data.field` names it. | every `validate()` in `mcp_server/params.rs` (e.g. `461-475`); observed in `tests/mcp_error_channel_mapping_test.rs:196-217` |

A1 and A3 produce the **same Rust error type** (`McpError::invalid_params` *is*
`rmcp::ErrorData`) but travel **different routes** and land on **different channels**: A1
is raised inside rmcp's `Parameters<P>` deserialization and arrives as
`result.isError = true`; A3 is returned from the tool function and arrives as
`error.code = -32602` with no `result` member. That was the open question under
[Unsettled rows](#unsettled-rows); it is now **settled by test**, and the answer is that
they do **not** reach the client as the same code.

The mechanism is worth knowing, because it is why `McpUrl::try_from` does not have to fake
anything. rmcp 1.8.0's `Parameters<P>` extractor wraps *any* serde error with its own
`"failed to deserialize parameters: "` prefix
(`rmcp-1.8.0/src/handler/server/tool.rs:181-196`), and `into_tool_argument_error`
downgrades to `isError: true` only when the code is `INVALID_PARAMS` **and** that prefix is
present (`rmcp-1.8.0/src/handler/server/router/tool.rs:144-156`). A handler-raised
rejection never carries the prefix, so it is never downgraded. One shared Rust type, two
routes, two channels.

A3 covers every branch below. They share one constructor
(`validation::invalid_params_with_reason`), so they share one channel, one code, and one
`data` shape — the split is by *condition*, not by envelope.

| # | A3 sub-condition | Field | `data.reason` | Retriable | Agent should | Source |
| :-: | :--- | :--- | :--- | :--- | :--- | :-: |
| A3.1 | URL empty, > 8192 B, unparseable, or non-http(s) scheme | `url` / varies | `empty` / `too_long` / `malformed` / `unsupported_scheme` | No | Resend a valid absolute http(s) URL. | `mcp_server/validation.rs:137-174` |
| A3.2 | Path empty, > 1024 B, absolute, drive-lettered, `..`-bearing, or a rejected path component | varies | `empty` / `too_long` / `path_not_allowed` | No | Send a relative, traversal-free path. | `mcp_server/validation.rs:176-237`, `333-374` |
| A3.3 | HTML / markdown / content blob > 1 MiB | varies | `too_long` | No | Chunk the input. | `mcp_server/validation.rs:41`, `376-393` |
| A3.4 | `max_pages` outside 1..=100 000, `max_depth` outside 0..=10 | `max_pages` / `max_depth` | `out_of_range` | No | Clamp to the advertised cap. | `mcp_server/params.rs:152-183` |
| A3.5 | `urls` empty or over cap, `concurrency` out of range | `urls` / `concurrency` | `empty` / `out_of_range` | No | Fix the batch. | `mcp_server/params.rs:461-485`, `mcp_server/validation.rs:555-618` |
| A3.6 | `js_strategy` not a known strategy | `js_strategy` | `not_in_allowed_set` | No | Use `static` / `hybrid` / `full`. | `mcp_server/params.rs:526-532` |
| A3.7 | `checkpoint_dir` present but blank | `checkpoint_dir` | `empty` | No | Omit it or name a real directory. | `mcp_server/params.rs:533-540` |
| A3.8 | Enum-ish string not in the allowed set | varies | `not_in_allowed_set` | No | Use a listed value; the message enumerates them. | `mcp_server/validation.rs:751-762` |
| A3.9 | Filename not a single safe component (traversal, separator, reserved name, Windows-invalid char) | `filename` | `empty` / `too_long` / `path_not_allowed` | No | Send one flat component. | `mcp_server/validation.rs:620-733`, `287-331` |

Since slice 5, `error.data` on every row above is the structured object
`{"field": "<field>", "reason": "<slug>"}`. Before that it was a bare JSON **string**
holding only the field tag, so a client could tell which field was wrong but never why,
and had to parse prose. The field survived the restructure — it is now `data.field` — and
`data.reason` is the new machine-readable half. Both are contract: see
[the reason taxonomy](#the-reason-slug-taxonomy--the-machine-readable-contract).

### A4 · path confinement (root-of-trust)

| # | Condition | Code | `data.reason` | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :--- | :-: |
| A4.1 | Empty path | `-32602` | `empty` | No | Name a directory. | `mcp_server/path_gate.rs:281-286` via `mcp_server/state.rs:332, 352` |
| A4.2 | Path over `MAX_PATH_LEN` (1024 B) | `-32602` | `too_long` | No | Shorten the path. | `mcp_server/path_gate.rs:288-293` |
| A4.3 | Rooted-but-not-absolute form (`C:foo`, `\foo`) | `-32602` | `path_not_allowed` | No | Use a genuinely absolute or genuinely relative path. | `mcp_server/path_gate.rs:296-303` |
| A4.4 | `..` traversal component | `-32602` | `path_not_allowed` | No | Send a traversal-free path. | `mcp_server/path_gate.rs:308-313` |
| A4.5 | Absolute path with no export roots configured | `-32602` | `path_not_allowed` | No | Use a relative path, or tell the operator to set `--export-roots`. | `mcp_server/path_gate.rs:321-334` via `mcp_server/state.rs:332, 352` |
| A4.6 | Absolute path outside every configured export root | `-32602` | `path_not_allowed` | No | Re-target under a configured root. | `mcp_server/path_gate.rs:340-348` |

The gate is fail-closed: with zero roots configured, every absolute path is refused
(`mcp_server/path_gate.rs:321`). Relative paths are allowed and resolve against the
server's CWD (`mcp_server/path_gate.rs:315-319`).

All six branches now emit the full `{"field": "output_dir" | "checkpoint_dir", "reason":
<slug>}` envelope — there is no reason-less shape left. Only A4.1 and A4.2 differ from the
shared `path_not_allowed`: their remedy is *fill it in* and *shorten it*, not *use a
different path*, and a slug that sends the agent down the wrong remedy is worse than no
slug at all. The four `path_not_allowed` branches are deliberately **not** split further —
"use a safe path inside an allowed root" is one instruction for all of them, and the
message carries which rule fired. Only the `data` payload moved: the gate's control flow,
messages, `-32602` code and allow/deny decisions are byte-for-byte unchanged.

### A5–A9 · SSRF entry pre-check

All five branches come from one function, `validate_url_no_ssrf`
(`mcp_server/ssrf.rs:96-187`). Since slice 2 they are **separated into three classes**,
and the split is expressed twice: in the JSON-RPC code, and in a `data.reason` slug.

| # | Condition | Class | Code | `data.reason` | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: | :--- | :-: |
| A5 | URL has no host | caller input | `-32602` | `no_host` | No | Unreachable in practice — `McpUrl` always carries a host. | `mcp_server/ssrf.rs:118-123` |
| A6 | Literal IP is in a forbidden range (loopback / private / link-local / CGNAT / ULA) | policy | `-32602` | `forbidden_ip_literal` | No | Stop. Retargeting is the only fix. | `mcp_server/ssrf.rs:130-142` |
| A7 | DNS resolver returned an error | **infrastructure** | `-32603` | `dns_resolution_failed` | **Yes** | Retry — the argument is fine, the server's resolver is not. | `mcp_server/ssrf.rs:148-159` |
| A8 | DNS returned an empty answer set | **infrastructure** | `-32603` | `dns_no_addresses` | **Yes** | Retry — same class as A7. | `mcp_server/ssrf.rs:161-170` |
| A9 | A resolved address is in a forbidden range | policy | `-32602` | `forbidden_ip_resolved` | No | Stop. This is a policy refusal, not a transient. | `mcp_server/ssrf.rs:172-185` |

**A7 and A8 changed class in slice 2.** They used to be `-32602` with `data: None`,
identical to a caller aiming at internal infrastructure — the only way to tell a
transient DNS outage from an SSRF attempt was to parse Spanish message text. They are now
`-32603 internal_error`: the argument is correct and the server's resolver is not, which
is the same fault class this crate already reports that way (rows A15, A16). Retriability
follows the code: **the two DNS rows are the only retriable rows in Channel A.**

Policy and caller input still *share* `-32602` on purpose — the knob-matrix suite pins the
loopback refusal at exactly that code, and the SSRF knob documentation advertises the
shape, so changing it would break pinned probes for no gain. That is why `data.reason` is
load-bearing here and not decoration: **the slug, not the code, is what separates the
classes.** The five SSRF slugs are a separate, closed set from the seven validation slugs,
and they carry no `field` (an SSRF refusal is not a bad field); see
[the reason taxonomy](#the-reason-slug-taxonomy--the-machine-readable-contract).

This pre-check is not the enforcement point. Every scrape client is independently guarded
at connect time by `webfang_core::domain::ssrf_guard` (literal-IP redirect policy +
validating resolver), so A6/A9 are fast-fail UX and defense in depth. See
`docs/ssrf-layers.md` for the full layer matrix.

### A10–A17 · handler-local protocol errors

| # | Condition | Code | `data.reason` | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :--- | :-: |
| A10 | `content_format` / `pipeline_format` not a known export format | `-32602` | `not_in_allowed_set` | No | Use `jsonl` / `vector` / `auto`. | `mcp_server/handlers/export.rs:196-202`, `409-415` |
| A11 | `filename` rejected by the sanitizer, or unparseable inside the synthetic export URL | `-32602` | `path_not_allowed` | No | Send one flat component. | `mcp_server/handlers/export.rs:209-215`, `234-240`, `324-331`, `362-369`, `469-475` |
| A12 | Synthetic export URL for the filename fails the hardened URL gate | `-32602` | `malformed` | No | Reshape the filename. | `mcp_server/handlers/export.rs:244-257` |
| A13 | `js_strategy` failed to parse **in the handler** (post-validate path) | `-32602` | `not_in_allowed_set` | No | Use a known strategy. | `mcp_server/handlers/scraping.rs:457-463` |
| A14 | Explicit `sitemap_url` fails the hardened URL re-wrap | `-32602` | `unsupported_scheme` | No | Reshape the sitemap URL. | `mcp_server/handlers/scraping.rs:599-610` |
| A15 | Per-category semaphore closed or poisoned | `-32603` | none — `data` is `null` | No | Report it; the server is unhealthy. | `mcp_server/macros.rs:16` |
| A16 | Record conversion failed: `DocumentChunk::validate()` or metadata serialization | `-32603` | none — `data` is `null` | No | Report it; the payload was not produced. | `mcp_server/handlers/scraping.rs:106, 109, 212, 215, 1121, 1127` |
| A17 | SSRF pre-check failure on the axtree tool (re-raised unchanged) | as A5–A9 | as A5–A9 | as A5–A9 | As A5–A9 — read `data.reason`, the code is the SSRF channel's. | `mcp_server/handlers/axtree.rs:66-74` |

A15, A16 and the two SSRF DNS branches (A7, A8) are the only `-32603` values this crate
constructs for a tool call. They are server faults, not caller mistakes, and must not be
retried — which is also why they are the only Channel A rows with **no** `data` at all:
there is nothing for the caller to branch on. A17 is a *forwarding* site, not a construction:
it re-raises whatever `validate_url_no_ssrf` produced, so its code and slug are inherited.

**Two slug choices in this table are not mechanical**, and both are deliberate. A12 reports
`malformed`, not `unsupported_scheme`: the scheme there is *server-built*
(`https://webfang.local/<filename>`), so `unsupported_scheme` would describe a URL the
caller never sent and hand it unusable advice; `try_from_url` also collapses "unparseable"
and "not http(s)" into one error, exactly as the `McpUrl` boundary does. A14 is the mirror
image and reports `unsupported_scheme`: there the value was *already* parsed by `McpUrl`, so
the scheme allow-list is the only reachable cause and the precise slug is the true one.

### A18–A19 · produced by rmcp, not by this crate

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| A18 | Unknown JSON-RPC method (e.g. a tool name that is not registered) | `-32601` | No | Call a name from `tools/list`. | rmcp; not constructed here — `mcp_server/server.rs:166-177`, pinned by `tests/mcp_transport_contract_test.rs:138` |
| A19 | A panic escapes the HTTP request path | `-32603` body, HTTP 500 | No | Report it. The body echoes the JSON-RPC `id` the client sent whenever one can be recovered (see the rules below). The tool-level panic path (B1) is the one that keeps the session alive. | `mcp_server/panic_containment.rs` (`jsonrpc_internal_error_response`), outermost backstop in `mcp_server/server.rs` (`jsonrpc_panic_response`) |

#### Which `id` a contained panic echoes

The `-32603` body always carries an `id`, and what it carries is a stated
rule, not a best effort. Both containment layers share one response builder, so
the shape never depends on which layer caught the panic.

| `id` echoed | when | rule id |
| :--- | :--- | :--- |
| the parsed `id` member | POST body is an object with an `id` member | `envelope` |
| `null` | the object closed with no `id` member (a JSON-RPC notification) | `notification` |
| `null` | the object sent `"id": null` explicitly | `null_id` |
| `null` | the body is a top-level array (a JSON-RPC batch) | `batch` |
| `null` | non-JSON, empty, a bare scalar, a non-POST request, or an unreadable body | `not_a_request` |
| `null` | the envelope did not close inside the 64 KiB scan window | `scan_window` |

A `null` id is recorded on the `jsonrpc.id_source` span field
(`mcp_jsonrpc_panic_containment`), so "the client sent a batch" is
distinguishable from "the body was garbage" in the trace. `null` never means
the mapping failed: the code is `-32603` either way. Source of truth:
`crates/webfang_mcp/src/mcp_server/panic_containment.rs`.

---

## The reason-slug taxonomy — the machine-readable contract

`data.reason` is the half of this contract an agent branches on. Before slice 5, Channel A
told a caller *which field* was wrong and nothing about *why*, so every consumer parsed
prose. Since EC-08 the `data` member is an object and `reason` is one of a **closed set of
stable slugs**, declared as a contract in `mcp_server/validation.rs:64-99`. There is
deliberately **no** reason-less constructor left in the crate: the
`invalid_params(field, msg)` escape hatch that `path_gate.rs` and the handler-inline sites
used to call was deleted once nothing called it any more, and a caller that cannot name a
slug must now pick the closest one (`mcp_server/validation.rs:101-104`). That is what makes
the taxonomy exhaustive — a consumer branching on `data.reason` can rely on it being there.

### The seven validation slugs

| Slug | Means | What the caller should do |
| :--- | :--- | :--- |
| `empty` | a required value is empty | fill it in |
| `too_long` | the value exceeds a byte cap | shorten it |
| `malformed` | present but unparseable | fix its shape |
| `unsupported_scheme` | the URL scheme is not `http`/`https` | use `http` or `https` |
| `path_not_allowed` | any structural path rejection — the filename rules (absolute, UNC prefix, drive letter, `..`, non-flat filename, Windows reserved name, Windows-invalid character, trailing `.`/space) **and** the export-root confinement rules in `path_gate.rs` (rooted-not-absolute, `..`, absolute with no roots, absolute outside every root) | use a safe relative path and a plain filename |
| `out_of_range` | a numeric value is outside its inclusive bounds | clamp it |
| `not_in_allowed_set` | an enum-ish value is not in the allowed list | pick a listed value |

**Coarse is deliberate, and the set is load-bearing.** A slug's job is to tell the agent
what to *do*, not to restate the message; seven stable values beat forty brittle ones.
Downstream tooling branches on these strings, so **do not rename one, add an eighth, or
remap a condition to a different slug without changing this page in the same change.**
`path_not_allowed` deliberately collapses those twelve rules into one slug, because
their remedy is one instruction — use a safe path — and the message carries the specifics.

### The SSRF slugs

A separate, disjoint set — the SSRF pre-check classifies a *target*, not a request field,
so it carries no `field` at all:

| Slug | Class | Code |
| :--- | :--- | :-: |
| `no_host` | caller input | `-32602` |
| `forbidden_ip_literal` | policy | `-32602` |
| `forbidden_ip_resolved` | policy | `-32602` |
| `dns_resolution_failed` | infrastructure | `-32603` |
| `dns_no_addresses` | infrastructure | `-32603` |

### The `data` shapes a client must tolerate

| Shape | Emitted by | Reader |
| :--- | :--- | :--- |
| `{"field": "<field>", "reason": "<slug>"}` | every field-bearing rejection: the 44-branch `validation.rs` funnel, the six inline bounds in `params.rs` (A1, A3.x), the six `path_gate.rs` branches (A4.x), and the ten handler-inline sites in `handlers/export.rs` / `handlers/scraping.rs` (A10–A14) | index `field` and `reason` |
| `{"reason": "<slug>"}` — no `field` | `ssrf.rs`, 3 branches (A5, A6, A9) | index `reason`; `field` is legitimately absent |

That is the whole of what the server produces. A client may still meet a bare string or a
`data` with no `reason` **in the wild**, because `error.data` is sender-defined and
`validation::reason_of` deliberately tolerates all three cases (`data` absent, a non-object,
or an object with no `reason` key) rather than panicking
(`mcp_server/validation.rs:122-124`, pinned by
`reason_of_tolerates_a_missing_or_foreign_data_payload`). Keep that tolerance in the reader;
do not document either as a shape this server can emit.

### `validate_url`'s parallel channel

The one success-shaped diagnostic that reports a rejection reuses the same slug under a
**different key**, because it is not a JSON-RPC error and has no `data`: the body is
`{"valid": false, "reason": <message only>, "reason_code": <slug>}` (row C1). `reason` is
now the message alone; it used to be `ErrorData::to_string()`, which leaked the numeric
code and a raw `data` blob into a human-facing field. `reason_code` carries the slug, so
one taxonomy covers both channels. The `valid: true` field set is unchanged.

---

## Message language — the ratified (inconsistent) contract

The repo-wide rule is "user-facing errors in Spanish, internal logs in English". The MCP
error channels **do not follow it uniformly**, and that is now a documented state rather
than an accident to be papered over.

| Channel | Language as it actually is |
| :--- | :--- |
| A — structural / validation | **English** in `validation.rs` (every `require_*` message) and in `path_gate.rs`; **Spanish** in one deliberate island, `validation::filename_component_error` (`mcp_server/validation.rs:291-323` — all six of its messages), and also in `ssrf.rs`, `handlers/export.rs`, and the `js_strategy` / `checkpoint_dir` / `sitemap_url` messages in `params.rs` and `handlers/scraping.rs` |
| B — operational | **Spanish** by intent, with a few English literals surviving: `"no valid URLs provided"` (`mcp_server/handlers/scraping.rs:264`) and the three `"HTTP error: …"` messages (`mcp_server/handlers/scraping.rs:780-783`, `852`, `1019`). Messages that forward an upstream `Error::to_string()` are language-dependent on their source. |
| C — success-shaped | mixed: `"no vault detected"` and `"not an SPA - sufficient content found"` are English, and `validate_url`'s failure reason is whatever `require_http_url` produced — also English. |

**Unifying this was deliberately not done in issue #1613.** The reason is that it would
buy nothing for a programmatic consumer: `data.reason` makes the *branch* language-free,
and the message text is explicitly **not** part of the contract. Rewriting the message
strings behind these rows would churn the diff of a contract change, put the language split
in the path of existing message-matching probes, and change nothing a client can rely on.
(How many strings that is was deliberately not counted: an exact figure would be a
snapshot, and this page is about a contract, not a census.)

**This is a deviation from the repo-wide convention, recorded here so a maintainer can
revisit it.** The honest framing: the convention says these strings are *user-facing*, and
they are read by an autonomous agent rather than a person, so "user-facing" is arguably the
wrong lens — but that is an argument, not a decision, and the decision belongs to the
maintainer. If it is revisited, the taxonomy section is the anchor: only the *slugs* are
frozen, so translating a message is a compatible change while renaming a slug is not.

---

## Channel B — `isError: true` tool results

Every row here is built by `provenance::neutralized_error`
(`mcp_server/provenance.rs:275-294`) or by the `honest_error` wrappers
(`mcp_server/handlers/ai.rs:38-42`, `mcp_server/handlers/axtree.rs:25-28`). Text is
neutralized (ANSI + C0/DEL stripped, fence sentinels escaped) and capped, but **no
provenance envelope and no code is added** — the reason is the text and nothing else.

Slice 4 moved four conditions *into* this channel from Channel C: three
serialization-failure classes that used to return a placeholder body as a success (now
B8.1–B8.3) and the inverted Obsidian severity (now B7.3). The rule they restore is the one
`render_metrics` already followed: **if the tool could not produce the answer it promised,
the answer is `isError: true` — never a success whose body happens to read like a
failure.**

### B1 · contained panic

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| B1 | A tool body panicked; the panic was caught at the dispatch boundary | none | **Yes, with a caveat** | Retry, but know that containment restores the *transport*, not the side effects — a tool that panicked after writing an export or charging a rate-limit token will duplicate that work. | `mcp_server/mod.rs:359-380` (containment + rationale), client text at `mcp_server/mod.rs:268-270` |

### B2 · site policy denial (robots.txt / WAF)

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B2.1 | robots.txt rules forbid the URL | none | No | Stop, or retry with the tool's own opt-out (`ignore_robots`) if the caller has permission. | `mcp_server/handlers/scraping.rs:1154-1171` (`robots_denied_response`), used by `mcp_server/handlers/scraping.rs:759-771` (`discover_urls`), `mcp_server/handlers/scraping.rs:957-970` (`detect_spa`), and the inline gate at `mcp_server/handlers/scraping.rs:74-84`; `mcp_server/handlers/ai.rs:122-124` |
| B2.2 | The policy guard refused (guard condition, not a rules denial) | none | No | Treat as a policy refusal. | `mcp_server/state.rs:375-390` |
| B2.3 | Live scrape inside the export pipeline denied | none | No | As B2.1; the message is prefixed with the target URL. | `mcp_server/handlers/export.rs:428-435` |

### B3 · scrape / crawl operational failure

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B3.1 | `scrape_url` — fetch, Readability extraction, or content sink failed | none | Depends on the class in the text | Read the text; a transient network fault and a hard WAF block look the same here. | `mcp_server/handlers/scraping.rs:121-130` |
| B3.2 | `scrape_with_options` — same | none | Same | Same. | `mcp_server/handlers/scraping.rs:227-236` |
| B3.3 | `scrape_batch` — whole-batch failure | none | Yes if transient | Retry the batch; per-URL failures inside a *successful* batch are rows, not errors (C3). | `mcp_server/handlers/scraping.rs:380-398` |
| B3.4 | `crawl_site` — engine run failed | none | Yes if transient | Retry the crawl. | `mcp_server/handlers/scraping.rs:534-544` |
| B3.5 | `crawl_with_sitemap` — discovery failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:713-731` |
| B3.6 | `crawl_with_sitemap` — sitemap session run failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:694-711` |
| B3.7 | `discover_urls` / `discover_sitemap` — link extraction or sitemap read failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:830-840`, `mcp_server/handlers/scraping.rs:915-925` |
| B3.8 | `download_assets` — asset fetch or write failed | none | Yes if transient | Retry; already-downloaded assets are content-addressed. | `mcp_server/handlers/assets.rs:88` |
| B3.9 | `extract_links` — base-URL resolution failed | none | No | Fix the input HTML or base URL. | `mcp_server/handlers/content.rs:87` |
| B3.10 | `url_to_file_path` — `OutputPath::from_url` failed | none | No | Fix the URL. | `mcp_server/handlers/url_utils.rs:210` |
| B3.11 | `crawl_site` / `crawl_with_sitemap` — the run's extracted records exceed the session result budget (64 MiB default), so **nothing** was retained for export (#1611, F7) | none | Yes, with a smaller run | Re-run with fewer `max_pages`, or crawl per section. The text names the budget, the measured size, and how many records were kept vs dropped. | `mcp_server/handlers/scraping.rs` (`within_session_budget`, `session_budget_message`, `store_session_results`) |

The per-class retry policy these texts inherit is
`docs/error-classification-matrix.md`. The MCP channel
carries no exit code, so the class is not machine-readable here — the agent must parse
the text.

### B4 · direct-fetch HTTP failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B4.1 | `discover_urls` — non-2xx status (a real failure, deliberately not an empty link set) | none | 4xx no / 5xx yes | Read the status in the text. | `mcp_server/handlers/scraping.rs:776-791` |
| B4.2 | `discover_urls` — transport failure | none | Yes | Retry. | `mcp_server/handlers/scraping.rs:844-853` |
| B4.3 | `detect_spa` — transport failure | none | Yes | Retry. | `mcp_server/handlers/scraping.rs:1010-1020` |
| B4.4 | `semantic_cleaner` — page fetch failed | none | Yes if transient | Retry. | `mcp_server/handlers/ai.rs:134-143` |
| B4.5 | `get_accessibility_snapshot` — snapshot fetch failed (both formats) | none | Yes if transient | Retry. | `mcp_server/handlers/axtree.rs:136-147`, `168-180` |

### B5 · export failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B5.1 | No session results to export (no crawl ran, or its extraction produced nothing) | none | No | Run a crawl first. This is a precondition, not a fault. | `mcp_server/handlers/export.rs:115-121` |
| B5.2 | Blocking-pool join for the session snapshot failed | none | Yes | Retry. | `mcp_server/handlers/export.rs:136-142` |
| B5.3 | `export_file` with empty / whitespace-only content | none | No | Send real content. | `mcp_server/handlers/export.rs:186-190` |
| B5.4 | `export_file` — document chunk validation failed | none | No | Fix the content. | `mcp_server/handlers/export.rs:269-275` |
| B5.5 | Exporter construction failed | none | Depends | Read the text. | `mcp_server/handlers/export.rs:278-284` |
| B5.6 | Export write failed (all four export tools) | none | Depends | Read the text. | `mcp_server/handlers/export.rs:57-61`, `294-298` |
| B5.7 | `process_export_pipeline` — live scrape failed | none | Depends | Read the text; it names the URL. | `mcp_server/handlers/export.rs:444-449` |

### B6 · feature gating and missing ports

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B6.1 | AI compiled but not enabled, or the model is still warming up | none | No (or retry after warmup) | Restart with `--enable-ai`, or wait for the model to load and retry. | `mcp_server/handlers/ai.rs:48-58` (`ai_unavailable`), used at `mcp_server/handlers/ai.rs:129`, `mcp_server/handlers/ai.rs:196-200`, `mcp_server/handlers/ai.rs:207` |
| B6.2 | AI not compiled into the binary | none | No | Rebuild with `--features ai`. | same helper, `mcp_server/handlers/ai.rs:54-57` |
| B6.3 | Vault note repository not configured | none | No | Operator must configure persistence. | `mcp_server/handlers/ai.rs:201-204` |
| B6.4 | `chromium` not compiled (axtree tool) | none | No | Rebuild with `--features chromium`. | `mcp_server/handlers/axtree.rs:185-197` |
| B6.5 | Semantic cleaner / inference / embedding failed | none | No | Report it; this is a server-side AI fault. | `mcp_server/handlers/ai.rs:144-147`, `270` |
| B6.6 | Response serialization failed (AI + axtree) | none | No | Report it; the payload could not be built. | `mcp_server/handlers/ai.rs:161-168`, `263-268`; `mcp_server/handlers/axtree.rs:121-133` |

### B7 · local integration failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B7.1 | `build_obsidian_uri` / `open_in_obsidian` — vault name or file path rejected | none | No | Fix the vault name or note path. | `mcp_server/handlers/obsidian.rs:72-77`, `99-104` |
| B7.2 | `open_in_obsidian` — the dispatch command could not be started at all (`Err`) | none | Yes | Retry; Obsidian may not be installed. | `mcp_server/handlers/obsidian.rs:137` |
| B7.3 | `open_in_obsidian` — the URI handler reported `DispatchStatus::HandlerFailed` (the OS handler exited non-zero) | none | Yes | Retry after installing/registering the handler. **Moved here from Channel C in slice 4**: the requested action did not happen, so it is a tool error. The `⚠️` marker that used to fake the severity inside a success body is gone — the envelope carries it now. | `mcp_server/handlers/obsidian.rs:134-136` (in `dispatch_outcome_to_result`, `mcp_server/handlers/obsidian.rs:126-139`) |
| B7.4 | `get_scrape_metrics` — no scrape has been recorded in this process | none | No | Run a scrape first. Precondition, not a fault. | `mcp_server/handlers/security.rs:204-208` |
| B7.5 | `get_scrape_metrics` — snapshot serialization failed | none | No | Report it. This is the `render_metrics` precedent the four B8 sites now follow. | `mcp_server/handlers/security.rs:209-213` |
| B7.6 | `scrape_batch` — every input URL was rejected before the run | none | No | Fix the URLs. | `mcp_server/handlers/scraping.rs:263-265` |

### B8 · reply serialization failed (server-side)

Added in slice 4. Each of these four sites used to do
`to_string_pretty(..).unwrap_or_else(|_| "failed to serialize")` and return that literal as
a **successful** result — a lost report that was indistinguishable from a real one, which
is the single shape an agent cannot detect. They are ordinary Channel B rows now.

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B8.1 | `download_assets` — the asset list could not be serialized | none | No | Report it; the download report was lost. Do **not** treat any body as authoritative. | `mcp_server/handlers/assets.rs:113-125` (`assets_tool_result`) |
| B8.2 | `extract_links` — the link list could not be serialized | none | No | Report it; the link list was lost. | `mcp_server/handlers/content.rs:234-246` (`links_tool_result`) |
| B8.3 | `discover_urls` / `discover_sitemap` — the URL list could not be serialized | none | No | Report it; the link list was lost. | `mcp_server/handlers/scraping.rs:1239-1256` (`discovered_urls_tool_result`) |

The old placeholder body cannot be produced any more — a client no longer needs to
special-case the literal string `"failed to serialize"`. These rows are **not** `-32603`:
the enclosing `match` already returns a `CallToolResult` directly, and `?`-propagating
would have split one function's failures across two channels. Retrying will not help; the
payload types' `Serialize` impls are total, so these branches are defensive.

---

## Channel C — success-shaped diagnostics

A `result` with **no** `isError` and no `error` member. The call succeeded; the body
describes something that is not a plain success. This is the channel an agent cannot
distinguish from a real success by shape alone — that is why it is enumerated rather than
folded into B.

Slice 4 shrank this channel from six conditions to the four below, and the IDs were
renumbered: the old C2 (the Obsidian severity inversion) became B7.3, and the old C3 (the
`"failed to serialize"` placeholder) was eliminated outright — its four sites are now
B8.1–B8.3. Old C4/C5/C6 are therefore new C2/C3/C4. **What remains here is a deliberate
diagnostic, not a leak**: every row below is a tool that succeeded at the only thing it
actually does, and reports a negative finding in the body.

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: | :-: |
| C1 | `validate_url` reports an unusable URL as `{"valid": false, "reason": …, "reason_code": …}` | none | No | Parse the JSON and branch on `valid` — never on `isError`. This is the only tool whose failure is a *success* by design (issue #590, bug #7). Since slice 5 its `reason` is the message alone and the slug moved to its own `reason_code` field; the tool's `description` now states the success-shaped contract outright. | `mcp_server/handlers/url_utils.rs:73-82`; success twin at `mcp_server/handlers/url_utils.rs:58-71` |
| C2 | A run completed with per-item failures (`failed_url` records; `errors` / `error_breakdown` fields) | none | Per item | Read the per-item records; the run itself is a success. | `mcp_server/handlers/scraping.rs:1129-1142` (failure records), returned through `mcp_server/handlers/scraping.rs:372-379`; crawl summary at `mcp_server/handlers/scraping.rs:519-534` |
| C3 | `detect_obsidian_vault` found no vault (`"no vault detected"`) | none | No | A negative *result*, not a failure. | `mcp_server/handlers/obsidian.rs:53-55` |
| C4 | `detect_spa` found sufficient content (`"not an SPA - sufficient content found"`) | none | No | A negative *result*, not a failure. | `mcp_server/handlers/scraping.rs:1005-1007` |

> **The old C3 row is gone for good.** It promised that a body equal to the literal
> `"failed to serialize"` meant a lost report. Slice 4 removed the four sites that could
> produce it, so no tool can emit that body any more. A client still special-casing the
> string is guarding against something the server no longer does — the equivalent failure
> now arrives as an ordinary `isError: true` result (B8.1–B8.3).

---

## Channel 0 — transport refusals (no JSON-RPC exists yet)

These never reach a tool. The client sees an HTTP status and, in most cases, no
JSON-RPC body at all. Every row here is **statically derived** — these statuses are
produced by middleware, not by a tool, and no test in this crate observes them over a
live listener.

| # | Condition | Status | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| T1 | Missing / malformed / wrong bearer token | `401` | No | Fix the `Authorization` header. | `mcp_server/auth.rs:38-60` |
| T2 | Rate-limit quota exceeded | `429` | Yes, after backoff | Slow down. | `mcp_server/server.rs:298-312` |
| T3 | Session-admission cap exceeded | `429` | Yes, after the window | Slow down. Bounded by `--max-sessions` / `--session-cap-window-secs`. | `mcp_server/server.rs:629-677` |
| T4 | Request took longer than `--request-timeout-secs` | `408` | Yes | Retry. | `mcp_server/server.rs:222-224` |
| T5 | Body over `--body-limit-bytes` | `413` | No | Shrink the payload. | `mcp_server/server.rs:225` |
| T6 | `Accept` missing both `application/json` and `text/event-stream` | `406` | No | Fix the request headers. | rmcp `StreamableHttpService`; matrix at `mcp_server/server.rs:166-173` |
| T7 | `Content-Type` not `application/json`, or the body is not JSON-RPC 2.0 | `415` | No | Fix the request. | rmcp; same matrix |
| T8 | Non-`initialize` request with no `mcp-session-id` | `422` | No | Complete the handshake first. | rmcp; same matrix |
| T9 | Panic on the HTTP request path | `500` + JSON-RPC `-32603` body | No | Report it. The body echoes the JSON-RPC `id` when one is recoverable; see the id rules above. | `mcp_server/panic_containment.rs` |

Layer order matters for reading this: the session cap is the **innermost** gate, so a
request rejected `401` or shed `429` upstream never consumes a session slot
(`mcp_server/server.rs:205-215`).

---

## Unsettled rows

**EC-02 — SETTLED by test, not by reasoning.** The question was whether A1/A2 (raised
inside rmcp's argument deserialization) and A3 (raised by the handler body) reach the
client as the same JSON-RPC code, given that they share one Rust error type
(`McpError::invalid_params` *is* `rmcp::ErrorData`). They do not, and
`crates/webfang_mcp/tests/mcp_error_channel_mapping_test.rs` now records **both**
`error.code` and `result.isError` for all three routes against a live server:

| Route | Observed | Test |
| :--- | :--- | :--- |
| `McpUrl` deserialization (`scrape_url` with `file:///etc/passwd`) | `result.isError = true`, **no** `error` member | `mcp_error_channel_mapping_test.rs:99-127` |
| unknown field (`scrape_with_options` with `typo_field`) | `result.isError = true`, **no** `error` member | `mcp_error_channel_mapping_test.rs:139-166` |
| handler `params.validate()?` (`crawl_site` with `max_depth: 11`) | `error.code = -32602`, **no** `result` member | `mcp_error_channel_mapping_test.rs:196-217` |

The mechanism is rmcp's own message prefix, described in
[A1–A3](#a1a3--argument-deserialization-before-the-handler-body). No production code
changed to settle it. The older hedged helper
`tests/params_rejection_test.rs::assert_url_argument_rejected`, which "accepts EITHER
rejection shape", is now strictly weaker than the new suite and is kept only because it
also asserts the reason text; it is no longer the evidence for these rows.

**Rows the code slices changed — all landed:**

| Finding | Row affected | Slice | Status |
| :--- | :--- | :-: | :--- |
| EC-02 — the rmcp argument-rejection mapping | A1, A2, A3 | 3 | **Landed** — settled by test, rows rewritten above |
| EC-03 — SSRF policy and DNS failures shared `invalid_params` | A7, A8 | 2 | **Landed** — `-32603` + `data.reason`; classes separated |
| EC-04 — the third channel was undocumented | C1 | 5 | **Landed** — `reason_code` slug + an explicit success-shaped `description` |
| EC-06 — a serialization failure returned success with a placeholder | was C3 | 4 | **Landed** — C3 deleted; the four sites are now B8.1–B8.3 |
| EC-07 — `DispatchStatus::HandlerFailed` was returned as tool success | was C2 | 4 | **Landed** — moved to B7.3, `⚠️` marker dropped |
| EC-08 — validation text was English while operational errors were Spanish; no machine-readable reason | A3.x, A4.x, A10–A14, B*, C1 | 5 | **Landed** on the machine-readable half — `data.reason` is now present on **every argument refusal the server raises** (the `-32603` server faults, A15/A16, carry no `data` and never did). The 16 sites slice 5 left behind (6 in `path_gate.rs`, 10 handler-inline in `handlers/export.rs` / `handlers/scraping.rs`) were converted, and the reason-less constructor they called was deleted, so both shapes that survived slice 5 are gone; see [A4](#a4--path-confinement-root-of-trust), [A10–A17](#a10a17--handler-local-protocol-errors) and [the shape table](#the-data-shapes-a-client-must-tolerate). The **language** half was **deliberately not done**, and remains open: see [Message language](#message-language--the-ratified-inconsistent-contract). |

**EC-05 — a deliberate, justified non-action, not an oversight.** The seven panic-capable
serialization `expect` sites are unchanged by design: `mcp_server/handlers/content.rs:209`,
`mcp_server/handlers/scraping.rs:532`, `691`, `1002`, and
`mcp_server/handlers/url_utils.rs:70`, `80`, `207` (all
`expect("serializing JSON to a string cannot fail")`). Each carries an explicit
`#[allow(clippy::expect_used)]` under a crate-wide `deny`; each operand is a
`serde_json::Value`, a total `Serialize`; and a panic there is contained into
`isError: true` at the dispatch boundary (row B1) without killing the session. The correct
remedy — a proven-total serialization boundary or an explicit `internal_error` mapping —
would be a behavioural change, not a refactor, and the issue explicitly warns that blind
replacement is churn. Recorded here so the next reader does not re-open it as a bug.

**Overlapping, not duplicated here:** EC-08's *message-language* half overlaps #1604,
which normalizes operational error event text to structured fields. #1604 does not own the
message-language or machine-readable-reason halves; those are decided on this page.

---

## Appendix — construction-site inventory

Counted in `crates/webfang_mcp/src/mcp_server/` in the **working tree** — that is, commit
`ccdef614` *plus the uncommitted* reason-slug changes described under
[EC-08](#unsettled-rows) — production code only (test modules and doc comments excluded).
`ccdef614` is the base, not a clean checkout: `validation.rs`, `path_gate.rs`,
`handlers/export.rs` and `handlers/scraping.rs` are all dirty in this tree.

**How the counts were derived** — each row is a `grep` for one construction form,
restricted to the `mcp_server/` tree, with comment-only lines dropped by a `///` / `//!`
filter. Two counts deserve a note rather than a bare number:

- `provenance::neutralized_error` is counted as **production call sites**, so the
  `fn neutralized_error` definition and every `#[cfg(test)]` call are excluded. That leaves
  37, of which 2 are the `honest_error` wrapper *bodies* and 4 are the mapper helpers
  slice 4 introduced; the 31 remaining are the same direct handler-body calls the previous
  revision of this table counted (that figure reproduces exactly at `763714b6`, which is
  the cross-check that the method is stable).
- Channel C has **no** `neutralized_error`/`CallToolResult::error` sites at all, and its
  `local_text` sites are only 5. The old "10 construction sites across 6 conditions" is
  not reproducible by any single grep; the honest number is 5 `local_text` sites across 3
  standalone conditions, plus C2, which is a *field inside* a success payload rather than
  a construction of one.

| Channel | Construction form | Sites | Accounted for |
| :--- | :--- | :-: | :--- |
| A | `McpError::invalid_params` called directly | **4** — the one surviving factory (`mcp_server/validation.rs:110`) and the SSRF channel (`mcp_server/ssrf.rs:122, 135, 178`) | A5, A6, A9 |
| A | `validation::invalid_params_with_reason` branches (the validation funnel) | **44** — all in `mcp_server/validation.rs` (lines 139–756) | A3.1–A3.9 |
| A | `validation::invalid_params_with_reason` call sites in `params.rs` | **6** — `mcp_server/params.rs:97, 155, 176, 463, 527, 536` | A1, A3.4, A3.5, A3.6, A3.7 |
| A | `validation::invalid_params_with_reason` call sites in `path_gate.rs` | **6** — `mcp_server/path_gate.rs:282, 289, 298, 309, 327, 345` | A4.1–A4.6 |
| A | `validation::invalid_params_with_reason` call sites in `handlers/export.rs` | **8** — `mcp_server/handlers/export.rs:197, 210, 235, 252, 326, 364, 410, 470` | A10–A12 |
| A | `validation::invalid_params_with_reason` call sites in `handlers/scraping.rs` | **2** — `mcp_server/handlers/scraping.rs:458, 605` | A13, A14 |
| A | `McpError::internal_error` | **9** — `mcp_server/handlers/scraping.rs:106, 109, 212, 215, 1121, 1127`; `mcp_server/macros.rs:16`; `mcp_server/ssrf.rs:154, 166` | A7, A8, A15, A16 |
| A | re-raised `Err(e)` — *forwarding, not a construction* | **1** — `mcp_server/handlers/axtree.rs:74` | A17 |
| B | `provenance::neutralized_error` called directly | **37** — 31 in handler bodies + 2 `honest_error` wrapper bodies (`handlers/ai.rs:41`, `handlers/axtree.rs:27`) + 4 in the slice-4 mapper helpers (`handlers/assets.rs:120`, `handlers/content.rs:241`, `handlers/scraping.rs:1251`, `handlers/obsidian.rs:134`) | B1–B8 |
| B | `honest_error` wrapper *definitions* | **2** — `mcp_server/handlers/ai.rs:40`, `mcp_server/handlers/axtree.rs:26` | (definitions only) |
| B | calls through those wrappers | **14** — `mcp_server/handlers/ai.rs:124, 129, 142, 146, 164, 197, 202, 207, 265, 270`; `mcp_server/handlers/axtree.rs:130, 144, 176, 194` | B2.1, B4.4, B4.5, B6.1–B6.6 |
| B | direct `CallToolResult::error` | **1** — `mcp_server/mod.rs:377` | B1 |
| C | `provenance::local_text` on a non-success outcome | **5** — `mcp_server/handlers/url_utils.rs:68, 78`; `mcp_server/handlers/obsidian.rs:54, 55`; `mcp_server/handlers/scraping.rs:1005` | C1, C3, C4 |
| C | per-item failure records *inside* a success payload (not a standalone construction) | **2 payloads** — `mcp_server/handlers/scraping.rs:372`, `519` | C2 |
| 0 | HTTP status producers **in this crate** | **6** — `mcp_server/auth.rs:59`; `mcp_server/server.rs:223, 225, 279, 311, 672` | T1–T5, T9 |
| 0 | HTTP status producers owned by rmcp's `StreamableHttpService` | **3** — not constructed here; enumerated in the matrix at `mcp_server/server.rs:166-173` | T6–T8 |

Totals: **136** error-producing construction sites on the JSON-RPC channels
(79 on A, 52 on B, 5 on C), plus **1** forwarding site and **6** in-crate HTTP status
producers. Every one is accounted for by a row above; **0** unclassified. A is
4 + 44 + 6 + 6 + 8 + 2 + 9; B is 37 + 14 + 1.

Two scope statements, so the totals are not read as more than they are:

- These are *sites*, not *conditions*. The eight `invalid_params_with_reason` calls in
  `handlers/export.rs` collapse into three rows (A10–A12), because several sites report
  the same condition for different tools; the reverse also holds, with 44 funnel branches
  collapsing into nine A3.x rows.
- A1's construction site is the `McpUrl::try_from` branch at `params.rs:97`, but it is
  **not** a Channel A construction — it travels on Channel B. It appears in this inventory
  only because that is where its Rust error is defined; its row (A1) is a Channel B row,
  and no `error` member is emitted for it at all.
