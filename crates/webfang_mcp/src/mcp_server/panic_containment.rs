//! Panic containment that answers with the CALLER'S JSON-RPC id (#1646).
//!
//! # Why this module exists
//!
//! #1611 (F2) put a `CatchPanicLayer` on the MCP router so a panic anywhere
//! on the HTTP request path becomes a JSON-RPC `-32603` document instead of
//! tower-http's empty 500. That mapping hardcoded `"id": null`, because by the
//! time the outer layer runs no body has been read and no envelope is in
//! scope. The result is exactly the symptom the containment exists to remove:
//! an agent that sent `"id": 7` receives a `-32603` carrying `"id": null`, has
//! nothing to correlate it with, and cannot tell whether its own call is still
//! outstanding.
//!
//! So the recoverable case gets its own layer, mounted INSIDE every gate
//! (see `server::build_mcp_router_with_service`): it reads the POST body,
//! recovers the id out of the JSON-RPC envelope, and installs a `catch_unwind`
//! around the nested `/mcp` service only. A panic at or below that service is
//! then answered with the caller's own id.
//!
//! # The id rules (published in `docs/src/mcp-error-contract.md`)
//!
//! | [`IdSource`] | when | echoed id |
//! | :--- | :--- | :--- |
//! | `Envelope` | object body with an `id` member | the parsed `id` value |
//! | `Notification` | single request with no `id` member | `null` |
//! | `NullId` | single request that sent `"id": null` | `null` |
//! | `Batch` | body is a top-level array | `null` |
//! | `NotARequest` | non-JSON / empty / scalar / non-POST / unreadable | `null` |
//! | `ScanWindow` | envelope did not close inside [`ID_SCAN_LIMIT`] | `null` |
//!
//! Every one of those answers is still `-32603` over HTTP 500: a `null` id
//! never means the mapping failed, and none of them ever leaks the panic
//! payload to the client (the payload goes to the trace only — see
//! `super::render_panic_payload`).
//!
//! The one case this module deliberately does NOT improve is a panic raised
//! ABOVE it (auth, rate limiting, timeout, body limit). There, no body has
//! been read at all, so the id is `null` **by an explicit documented rule**
//! rather than by a silent fallback, and the outer `CatchPanicLayer` answers.
//! Mounting this layer inside those gates is deliberate: an unauthenticated
//! flood can never make the server buffer a request body.

use std::any::Any;
use std::panic::AssertUnwindSafe;

use axum::body::{Body, BodyDataStream, Bytes};
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::{FutureExt, StreamExt};
use serde_json::Value;

/// JSON-RPC `Internal error` code (JSON-RPC 2.0 spec) — what a contained panic
/// maps to.
pub(crate) const JSONRPC_INTERNAL_ERROR: i64 = -32603;

/// User-facing (Spanish) body of a contained panic. Fixed text: the panic
/// payload may embed request data, so it goes to the trace, never to the client.
pub(crate) const PANIC_HTTP_ERROR: &str =
    "Error interno del servidor MCP contenido. La petición falló de forma inesperada.";

/// Upper bound on how many body bytes are pulled while looking for the id.
///
/// A JSON-RPC envelope closes within its first few hundred bytes, so the scan
/// stops long before this in practice; the bound exists so a body that is NOT
/// a JSON object (a 10 MB upload, a chunked stream that never ends) cannot turn
/// id recovery into an unbounded read. Everything past the limit is left in the
/// stream and replayed untouched — see [`capture_request_id`].
const ID_SCAN_LIMIT: usize = 64 * 1024;

/// Where the id in a [`RecoveredRequestId`] came from.
///
/// Every variant is a stated rule, not a best-effort guess: a `null` id in a
/// contained-panic response is always explainable by exactly one of them, and
/// the variant is recorded on the `jsonrpc.id_source` span field so an
/// operator can tell "the client sent a batch" from "the body was garbage"
/// without reading a line of code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IdSource {
    /// Recovered from the request envelope: the object carried an `id` member.
    Envelope,
    /// A single request with no `id` member — a JSON-RPC notification.
    Notification,
    /// A single request that explicitly sent `"id": null`.
    NullId,
    /// The body is a JSON-RPC batch (a top-level array). Batches are not
    /// recovered from: the error is not attributable to one member, and
    /// rmcp rejects the shape upstream anyway.
    Batch,
    /// The body is not a JSON object — non-JSON, empty, a bare scalar, a
    /// non-POST request, or a body that could not be read at all.
    NotARequest,
    /// The envelope did not close inside [`ID_SCAN_LIMIT`], so the body was
    /// not fully inspected and no claim about the id may be made.
    ScanWindow,
}

impl IdSource {
    /// Stable lowercase slug, recorded on the `jsonrpc.id_source` span field.
    ///
    /// Stable because it is what operators grep for in the JSONL trace and
    /// what the error contract documents; renaming one is a contract change,
    /// not a refactor.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Envelope => "envelope",
            Self::Notification => "notification",
            Self::NullId => "null_id",
            Self::Batch => "batch",
            Self::NotARequest => "not_a_request",
            Self::ScanWindow => "scan_window",
        }
    }
}

/// The id to echo in a contained-panic response, plus why it is what it is.
#[derive(Clone, Debug)]
pub(crate) struct RecoveredRequestId {
    /// The id to echo. `Value::Null` whenever `source` is not
    /// [`IdSource::Envelope`].
    id: Value,
    /// Which rule produced `id`.
    source: IdSource,
}

impl RecoveredRequestId {
    /// The id nobody could recover — always `null`, always with a stated
    /// reason. This is the constructor [`jsonrpc_panic_response`] uses for a
    /// panic raised above the body-reading layer.
    pub(crate) fn unrecoverable(source: IdSource) -> Self {
        Self {
            id: Value::Null,
            source,
        }
    }
}

/// Map a panic raised at or below the nested `/mcp` service to a JSON-RPC
/// `-32603` error body (HTTP 500, `application/json`) that echoes `recovered`.
///
/// Shared by the two containment layers on purpose: the body, the status, the
/// code and the message are ONE shape, and the only thing that varies is which
/// id gets echoed. A `-32603` that changed shape depending on which layer
/// caught the panic would be a worse contract than the null id ever was.
pub(crate) fn jsonrpc_internal_error_response(
    recovered: RecoveredRequestId,
    payload: &(dyn Any + Send),
) -> Response {
    tracing::error!(
        panic.payload = %super::render_panic_payload(payload),
        jsonrpc.id = %recovered.id,
        jsonrpc.id_source = %recovered.source.as_str(),
        "MCP request panicked — mapped to a JSON-RPC -32603 response"
    );

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": recovered.id,
        "error": {
            "code": JSONRPC_INTERNAL_ERROR,
            "message": PANIC_HTTP_ERROR,
        },
    });

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Contain panics raised at or below the nested `/mcp` service, answering with
/// the caller's own JSON-RPC id.
///
/// # Which panic this catches
///
/// Anything raised while the nested service future is being polled, plus
/// anything raised synchronously by the polling of the layers directly around
/// it. It does NOT catch a panic raised above this layer — auth, rate
/// limiting, the timeout and the body limit have already answered by the time
/// this runs (it is mounted innermost; see
/// `server::build_mcp_router_with_service`), so for those the outer
/// `CatchPanicLayer` is the backstop and the id is `null` **by the documented
/// rule** in this module's docs, not by a silent fallback.
///
/// That ordering is a security property, not a preference: a request rejected
/// `401` or shed `429` never reaches this layer, so an unauthenticated flood
/// can never make the server buffer a request body.
///
/// The recovered id is published as a request extension so downstream layers
/// and tool handlers can log or answer with it without re-parsing a body that
/// this layer has already consumed.
#[tracing::instrument(
    name = "mcp_jsonrpc_panic_containment",
    skip_all,
    fields(
        jsonrpc.id = tracing::field::Empty,
        jsonrpc.id_source = tracing::field::Empty
    )
)]
pub(crate) async fn jsonrpc_panic_containment(request: Request, next: Next) -> Response {
    let (mut request, recovered) = capture_request_id(request).await;

    let span = tracing::Span::current();
    span.record("jsonrpc.id", tracing::field::display(&recovered.id));
    span.record("jsonrpc.id_source", recovered.source.as_str());

    request.extensions_mut().insert(recovered.clone());

    // `AssertUnwindSafe` is the honest annotation, and it is the one this
    // crate already uses at the tool-dispatch boundary: the guarded future is
    // a tower/axum service future whose unwind safety the compiler cannot
    // prove, and the invariant relied on is the one this function establishes
    // — a caught panic is logged and reported, never resumed.
    match AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(payload) => jsonrpc_internal_error_response(recovered, payload.as_ref()),
    }
}

/// Read the POST body far enough to recover the JSON-RPC id, then hand the
/// request on with its body intact.
///
/// Non-POST requests are returned untouched and unrecovered: rmcp uses `GET`
/// for the SSE stream and `DELETE` to terminate a session, and neither has a
/// JSON-RPC envelope, so consuming a body there would be pure damage.
///
/// The body is REBUILT, never taken. Everything read is replayed as the first
/// frames of a new body and the unread remainder is chained behind it, so the
/// inner service observes the same bytes it would have without this layer. A
/// read error (what `RequestBodyLimitLayer` produces when the body is too
/// large) is re-emitted FIRST, so the inner service sees the failure exactly
/// where it would have seen it.
async fn capture_request_id(request: Request) -> (Request, RecoveredRequestId) {
    if request.method() != Method::POST {
        return (
            request,
            RecoveredRequestId::unrecoverable(IdSource::NotARequest),
        );
    }

    let (parts, body) = request.into_parts();
    let mut remainder = body.into_data_stream();

    let mut prefix: Vec<u8> = Vec::new();
    let mut failure: Option<axum::Error> = None;
    while prefix.len() < ID_SCAN_LIMIT {
        match remainder.next().await {
            Some(Ok(chunk)) => prefix.extend_from_slice(&chunk),
            Some(Err(error)) => {
                failure = Some(error);
                break;
            },
            None => break,
        }
    }

    let (id, source) = scan_envelope_id(&prefix);
    let request = Request::from_parts(parts, rebuild_body(prefix, failure, remainder));

    (request, RecoveredRequestId { id, source })
}

/// Re-assemble the body from the consumed `prefix` and the unread `remainder`.
fn rebuild_body(prefix: Vec<u8>, failure: Option<axum::Error>, remainder: BodyDataStream) -> Body {
    let mut head: Vec<Result<Bytes, axum::Error>> = Vec::with_capacity(2);
    // The error goes FIRST, ahead of the bytes that preceded it: ordering it
    // after them would let a body-limit failure surface late, and the inner
    // service is entitled to see the failure where it would have seen it.
    if let Some(error) = failure {
        head.push(Err(error));
    }
    head.push(Ok(Bytes::from(prefix)));

    Body::from_stream(futures::stream::iter(head).chain(remainder))
}

/// Advance past JSON whitespace (space, tab, CR, LF).
fn skip_ws(input: &[u8], mut index: usize) -> usize {
    while matches!(input.get(index), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        index += 1;
    }
    index
}

/// Scan a `"`-delimited string token starting at `start` (which must be the
/// opening quote).
///
/// Returns the index just past the closing quote and the token's INNER bytes.
/// Escapes are consumed as pairs, so a `\"` never closes the token; the inner
/// slice keeps the backslash, which is what makes an escaped key compare
/// unequal to the plain key it would decode to.
fn scan_string(input: &[u8], start: usize) -> Option<(usize, &[u8])> {
    let mut index = start + 1;
    loop {
        match input.get(index) {
            None => return None,
            Some(b'\\') => index += 2,
            Some(b'"') => return Some((index + 1, &input[start + 1..index])),
            Some(_) => index += 1,
        }
    }
}

/// Skip one JSON value starting at `start`, returning the index just past it.
///
/// Nested containers and strings are tracked, so a `,` or a `}` that belongs
/// to the enclosing object (or that sits inside a string value) is never
/// mistaken for the end of this value. The two closing cases differ on
/// purpose: a brace at depth 0 belongs to the ENCLOSING object and is left for
/// the caller to read as the member separator, while the brace that closes a
/// nested container is part of the value and is consumed. Getting that
/// distinction wrong is what made a trailing `"id": 7` scan as `7}` and be
/// reported as unrecoverable.
///
/// `None` means the input ran out.
fn skip_value(input: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    let mut depth = 0usize;
    loop {
        match input.get(index)? {
            b'"' => {
                index = scan_string(input, index)?.0;
                if depth == 0 {
                    return Some(index);
                }
            },
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            },
            b'}' | b']' => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            },
            b',' if depth == 0 => return Some(index),
            _ => index += 1,
        }
    }
}

/// Recover the `id` member of a JSON-RPC envelope from the front of a body.
///
/// This is a byte scanner, not a parser, and the distinction is the whole
/// point: `serde_json::from_slice` on a TRUNCATED prefix would either fail
/// (fine) or — worse — succeed on a shorter document than the one that was sent
/// and report a confident answer about a body this layer never fully read.
/// Everything the scanner accepts, it accepts because the bytes said so.
///
/// A key is compared as raw bytes against `b"id"`, so a key written with an
/// escape (`"i\u0064"`) is NOT taken as the `id` member: it may decode to
/// `id`, and guessing would be worse than reporting a notification.
fn scan_envelope_id(input: &[u8]) -> (Value, IdSource) {
    let start = skip_ws(input, 0);
    match input.get(start) {
        Some(b'{') => {},
        Some(b'[') => return (Value::Null, IdSource::Batch),
        _ => return (Value::Null, IdSource::NotARequest),
    }

    let mut index = start + 1;
    loop {
        index = skip_ws(input, index);
        match input.get(index) {
            // A `}` here is an object that closed before any member was read
            // (`{}`): same rule as an object whose members are all non-`id`.
            Some(b'}') => return (Value::Null, IdSource::Notification),
            Some(b'"') => {},
            _ => return (Value::Null, IdSource::ScanWindow),
        }

        let Some((after_key, key)) = scan_string(input, index) else {
            return (Value::Null, IdSource::ScanWindow);
        };
        index = skip_ws(input, after_key);
        if input.get(index) != Some(&b':') {
            return (Value::Null, IdSource::ScanWindow);
        }

        index = skip_ws(input, index + 1);
        let Some(value_end) = skip_value(input, index) else {
            return (Value::Null, IdSource::ScanWindow);
        };

        if key == b"id" {
            return match serde_json::from_slice::<Value>(&input[index..value_end]) {
                Ok(Value::Null) => (Value::Null, IdSource::NullId),
                Ok(id) => (id, IdSource::Envelope),
                Err(_) => (Value::Null, IdSource::NotARequest),
            };
        }

        index = skip_ws(input, value_end);
        match input.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return (Value::Null, IdSource::Notification),
            _ => return (Value::Null, IdSource::ScanWindow),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One row of the published id rules: a body, and what must come back.
    struct Case {
        /// What the case pins (used in the failure message).
        name: &'static str,
        /// The body bytes handed to the scanner.
        body: &'static str,
        /// The id that must be echoed.
        expected_id: Value,
        /// The rule that must be recorded.
        expected_source: IdSource,
    }

    /// Table-driven over [`scan_envelope_id`]. The envelope cases are the ones
    /// the bug is about (id present, possibly not first, possibly behind
    /// braces and strings); the rest pin that a `null` id is always a STATED
    /// rule and never an accident of the scan running out of bytes.
    #[test]
    fn scan_envelope_id_follows_the_published_rules() {
        let cases = [
            Case {
                name: "id first",
                body: r#"{"id":7,"method":"tools/list"}"#,
                expected_id: json!(7),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "id last",
                body: r#"{"jsonrpc":"2.0","method":"tools/list","id":"pc4-probe-7"}"#,
                expected_id: json!("pc4-probe-7"),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "id last, after a string value",
                body: r#"{"jsonrpc":"2.0","method":"tools/list","id":14}"#,
                expected_id: json!(14),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "an object value, with the id after it",
                body: r#"{"params":{"a":1},"id":11}"#,
                expected_id: json!(11),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "an id recovered before the body was truncated",
                body: r#"{"jsonrpc":"2.0","id":7,"method":"to"#,
                expected_id: json!(7),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "nested object and array before the id",
                body: r#"{"jsonrpc":"2.0","params":{"a":{"b":[1,2]},"c":null},"id":11}"#,
                expected_id: json!(11),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "a string value that merely contains an id fragment",
                body: r#"{"params":{"note":"the \"id\" is 99"},"id":12}"#,
                expected_id: json!(12),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "a key containing an escape is not the id key",
                body: r#"{"a\"b":1,"id":13}"#,
                expected_id: json!(13),
                expected_source: IdSource::Envelope,
            },
            Case {
                name: "explicit null id",
                body: r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#,
                expected_id: Value::Null,
                expected_source: IdSource::NullId,
            },
            Case {
                name: "missing id member",
                body: r#"{"jsonrpc":"2.0","method":"notifications/x"}"#,
                expected_id: Value::Null,
                expected_source: IdSource::Notification,
            },
            Case {
                name: "empty object",
                body: "{}",
                expected_id: Value::Null,
                expected_source: IdSource::Notification,
            },
            Case {
                name: "batch",
                body: r#"[{"id":1},{"id":2}]"#,
                expected_id: Value::Null,
                expected_source: IdSource::Batch,
            },
            Case {
                name: "not JSON at all",
                body: "not json at all",
                expected_id: Value::Null,
                expected_source: IdSource::NotARequest,
            },
            Case {
                name: "empty body",
                body: "",
                expected_id: Value::Null,
                expected_source: IdSource::NotARequest,
            },
        ];

        for case in cases {
            let (id, source) = scan_envelope_id(case.body.as_bytes());
            assert_eq!(
                (source, id),
                (case.expected_source, case.expected_id),
                "case {:?} ({})",
                case.name,
                case.body
            );
        }
    }

    /// A body that stops mid-envelope must NOT be reported as a completed
    /// object: the scanner has no right to answer for bytes it never saw, and
    /// `serde_json::from_slice` on the same prefix would have failed too.
    #[test]
    fn truncated_envelope_reports_the_scan_window() {
        let prefixes: [&[u8]; 3] = [
            r#"{"jsonrpc":"2.0","id"#.as_bytes(),
            r#"{"jsonrpc":"2.0","method":"tools/list""#.as_bytes(),
            r#"{"a":{"b":1}"#.as_bytes(),
        ];
        for prefix in prefixes {
            let (id, source) = scan_envelope_id(prefix);
            assert_eq!(
                (source, id),
                (IdSource::ScanWindow, Value::Null),
                "prefix {prefix:?} must not be reported as a completed envelope"
            );
        }
    }

    /// A body larger than the scan window is bounded, not read to the end: the
    /// id that does appear near the front is still recovered, and a body whose
    /// id only appears after the limit is reported as `ScanWindow` rather than
    /// guessed. The second case is handed exactly the prefix
    /// `capture_request_id` would have collected.
    #[test]
    fn scan_is_bounded_by_the_window() {
        let padding = "x".repeat(ID_SCAN_LIMIT);
        let inside = format!(r#"{{"id":15,"method":"{padding}"}}"#);
        assert_eq!(
            scan_envelope_id(inside.as_bytes()),
            (json!(15), IdSource::Envelope)
        );

        let outside = format!(r#"{{"method":"{padding}","id":16}}"#);
        assert_eq!(
            scan_envelope_id(&outside.as_bytes()[..ID_SCAN_LIMIT]),
            (Value::Null, IdSource::ScanWindow),
            "an id past the window must not be guessed at"
        );
    }

    /// The slugs are a published contract (error-contract doc + trace field),
    /// so they are pinned literally rather than derived from a `Debug` print.
    #[test]
    fn id_source_slugs_are_stable() {
        assert_eq!(IdSource::Envelope.as_str(), "envelope");
        assert_eq!(IdSource::Notification.as_str(), "notification");
        assert_eq!(IdSource::NullId.as_str(), "null_id");
        assert_eq!(IdSource::Batch.as_str(), "batch");
        assert_eq!(IdSource::NotARequest.as_str(), "not_a_request");
        assert_eq!(IdSource::ScanWindow.as_str(), "scan_window");
    }
}
