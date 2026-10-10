//! OpenAI-compatible wreq adapter for `POST {base_url}/embeddings` (#1462).
//!
//! Implements [`EmbeddingPort`] for
//! [`ProviderKind::OpenAiCompatible`](crate::domain::providers::ProviderKind)
//! so vault search works without the `ai` feature and with non-384d dims.
//! Default stays local ONNX; this adapter is strictly opt-in through the
//! embedding provider slot (`--embedding-provider` / default resolution).
//!
//! Wire contract (`fixtures/remote_embedding/`):
//! - request `{model, input}` to `{base_url}/embeddings`, con cabecera
//!   `Authorization: Bearer` **sólo si** la fuente configurada aporta
//!   credencial: `auth: {source: "none"}` envía la petición anónima, sin
//!   sintetizar ni una cabecera vacía;
//! - response vectors map 1:1 to inputs in order;
//! - tolerant parse (never `deny_unknown_fields`, `usage` is `Option` +
//!   `#[serde(default)]`); missing `data` / empty vectors are
//!   [`SemanticError::Inference`], never silence.
//!
//! Guard chain (fetch-chain stages 1→5 minus crawl pacing, #1462):
//! entry gate at construction (scheme + parameterized literal-IP check) →
//! per-request timeout/connect timeout → 10-hop redirect policy → SSRF at
//! socket dial (parameterized loopback permit) → retry classification
//! (timeout/429/5xx retry, 4xx terminal, last status on exhaustion) →
//! `read_body_capped` at 16 MiB.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::Instrument;
use url::Url;

use crate::domain::credentials::ApiKey;
use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::providers::ProviderConfig;
use crate::error::SemanticError;
use crate::infrastructure::llm::provider::ProviderInitError;

/// A boxed future for dyn-compatible async port methods.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Maximum remote-embedding response body (decompressed bytes), mirroring
/// `LLM_MAX_BODY_BYTES` (client.rs): no fetch path may read an unbounded
/// body.
const EMBEDDING_MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// Bounded attempts for one `embed_batch`: the initial try plus retries for
/// timeouts, 429s and 5xx. Terminal 4xx and builder errors never consume a
/// retry.
const EMBEDDING_MAX_ATTEMPTS: u32 = 4;

/// Exponential backoff base/cap for embedding retries (production defaults
/// shared with the hardened HTTP client).
const BACKOFF_BASE_MS: u64 = 1000;
const BACKOFF_MAX_MS: u64 = 10_000;

/// Ceiling for a server-requested `Retry-After` delay.
///
/// `Retry-After` is server-controlled input and **both** wire forms accept
/// unbounded magnitudes: `delay-seconds` is a `u64` (`Duration::from_secs`
/// never fails) and an HTTP-date can sit centuries out. Unclamped, one header
/// pins the adapter in `tokio::time::sleep` for `u64::MAX` seconds ≈ 584M
/// years, and the millisecond conversion then saturates instead of erroring —
/// a stall, not a crash, which is the harder failure to diagnose. Real
/// embedding endpoints ask for at most ~60s, so 60s honors every legitimate
/// instruction while bounding the total stall across
/// [`EMBEDDING_MAX_ATTEMPTS`] retries to ~4 minutes.
const RETRY_AFTER_MAX_MS: u64 = 60_000;

/// Wire request for OpenAI-compatible `POST /embeddings`.
#[derive(serde::Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

/// Wire response: only the fields this adapter consumes.
///
/// Tolerancia deliberada (fixtures de contrato en
/// `fixtures/remote_embedding/`): vendors add `id`, `created`,
/// `system_fingerprint` and per-datum extras — all ignored. `usage` is
/// `Option` + `#[serde(default)]` (vendors may send `null` or omit it);
/// `index` is `Option` (order is positional 1:1, never re-sorted).
#[derive(serde::Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
    #[serde(default)]
    usage: Option<EmbeddingUsage>,
}

/// One datum of the wire response.
///
/// `index` is parsed for contract tolerance but never used for ordering:
/// vectors map 1:1 to inputs positionally (count-checked in
/// [`parse_embedding_body`]).
#[derive(serde::Deserialize)]
struct EmbeddingDatum {
    #[allow(dead_code)]
    #[serde(default)]
    index: Option<u32>,
    embedding: Vec<f32>,
}

/// Token usage reported by the vendor (informational only).
#[derive(serde::Deserialize)]
struct EmbeddingUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

/// Remote embedding adapter: `EmbeddingPort` over `POST {base_url}/embeddings`.
///
/// The HTTP client always carries the parameterized SSRF guard
/// (`secure_client_with_loopback` with this provider's `allow_loopback`);
///
/// `dim` is set once by the startup probe ([`probe_dim`](Self::probe_dim)):
/// pin-verified when the config declares `embedding_dim`, adopted with an
/// info log otherwise. Before the probe runs, [`embedding_dim`](Self::embedding_dim)
/// reports the configured pin (or `0` when unpinned) — the adapter must be
/// probed at startup before serving, exactly like the completion provider is
/// validated before the first extraction.
pub struct RemoteEmbeddingAdapter {
    http: wreq::Client,
    base_url: Url,
    model: String,
    /// La credencial resuelta, o [`None`] cuando el provider declara
    /// `auth: {source: "none"}` — es decir, un endpoint anónimo por diseño.
    ///
    /// `Option` y no una `ApiKey` vacía: la ausencia de credencial se
    /// expresa como **ausencia de cabecera**, nunca como `Bearer ` — que un
    /// servidor puede rechazar como key inválida en vez de entender como
    /// petición anónima (invariante 2 de [`crate::domain::auth_source`]).
    api_key: Option<ApiKey>,
    /// Etiqueta de la fuente declarada (`"none"`, `"env"`, `"keyring"`,
    /// `"encrypted_file"`), para que un 401 pueda decir en el log si se
    /// mandó credencial o no. Nunca deriva del secreto.
    auth_source: &'static str,
    provider_id: String,
    pinned_dim: Option<usize>,
    dim: OnceLock<usize>,
}

/// Build the guarded HTTP client for a remote embedding provider.
///
/// Same timeouts as the completion path (60s request / 10s connect); the
/// SSRF guard is the parameterized variant carrying this provider's
/// `allow_loopback` — never the process registry.
///
/// # Errors
///
/// [`ProviderInitError::HttpClient`] if the builder fails.
fn embedding_http_client(allow_loopback: bool) -> Result<wreq::Client, ProviderInitError> {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Profile::Chrome145)
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10));
    crate::domain::ssrf_guard::ssrf_guard()
        .secure_client_with_loopback(builder, allow_loopback)
        .build()
        .map_err(|e| ProviderInitError::HttpClient(e.to_string()))
}

/// Entry gate for the remote endpoint (fetch-chain stage 1): http(s) scheme
/// plus the parameterized literal-IP check.
///
/// Runs at construction so a forbidden `base_url` fails fast at startup,
/// before any socket opens. Hostnames pass here — they are enforced per
/// connection by the validating resolver on the guarded client.
///
/// # Errors
///
/// [`ProviderInitError::InvalidBaseUrl`] when the scheme is not http(s) or
/// the host is a forbidden literal (loopback without the config-file
/// `allow_loopback` opt-in included).
fn entry_gate(config: &ProviderConfig) -> Result<(), ProviderInitError> {
    let invalid = |msg: String| ProviderInitError::InvalidBaseUrl {
        provider_id: config.id.clone(),
        source: crate::error::ScraperError::Config(msg),
    };
    let scheme = config.base_url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(invalid(format!(
            "esquema '{scheme}' no permitido para embeddings remotos (solo http/https)"
        )));
    }
    let host = config.base_url.host_str().unwrap_or("");
    if crate::domain::ssrf_guard::is_forbidden_literal_host_with(host, config.allow_loopback) {
        return Err(invalid(format!(
            "URL de embeddings bloqueada por SSRF: el host '{host}' pertenece a una red \
             interna; para endpoints locales activá `allow_loopback` en el archivo de configuración"
        )));
    }
    Ok(())
}

/// Retry delay in milliseconds before attempt `attempt` (1-based): the max
/// of `Retry-After` and exponential backoff from the 1s base (capped at
/// 10s), mirroring the hardened HTTP client convention.
///
/// Pure so the policy is unit-testable without sleeping. A
/// [`Duration::ZERO`] `Retry-After` (past HTTP-date, or an explicit `0`)
/// carries no instruction, so the exponential floor stands alone.
///
/// Total by construction: the result is at most
/// [`RETRY_AFTER_MAX_MS`], because a `retry_after` argument is expected to be
/// pre-clamped by [`retry_after_delay`] and is clamped again here. The second
/// clamp is dead code for every current caller, and that is the point — this
/// function is the last thing standing between a hostile header and a
/// `tokio::time::sleep`, so its fallback must be bounded even if a future
/// caller forgets to bound its input. `u64::MAX` milliseconds would be ~584M
/// years of sleep, i.e. an indefinite hang that looks like a stuck process.
fn backoff_delay_ms(attempt: u32, retry_after: Option<Duration>) -> u64 {
    let exponential = BACKOFF_BASE_MS
        .saturating_mul(2u64.pow(attempt.saturating_sub(1)))
        .min(BACKOFF_MAX_MS);
    match retry_after {
        Some(delay) if !delay.is_zero() => exponential.max(
            u64::try_from(delay.as_millis())
                .unwrap_or(u64::MAX)
                .min(RETRY_AFTER_MAX_MS),
        ),
        _ => exponential,
    }
}

/// Clamp one requested `Retry-After` delay to [`RETRY_AFTER_MAX_MS`].
///
/// The clamp is **not** silent: an oversized instruction is a server asking
/// for more than the adapter will ever grant, which an operator triaging a
/// stall needs to see. Only a request strictly above the ceiling warns — an
/// absent, empty, unparsable, `"0"` or past-date header carries no oversized
/// instruction and reaches here untouched.
fn clamp_retry_after(requested: Duration, raw: &str) -> Duration {
    let ceiling = Duration::from_millis(RETRY_AFTER_MAX_MS);
    if requested <= ceiling {
        return requested;
    }
    tracing::warn!(
        raw = %raw,
        // `as_millis` is a u128; saturate rather than truncate a hostile value.
        requested_ms = u64::try_from(requested.as_millis()).unwrap_or(u64::MAX),
        honored_ms = RETRY_AFTER_MAX_MS,
        "remote embedding Retry-After exceeds the honored ceiling; clamped"
    );
    ceiling
}

/// `Retry-After` header value → the delay the server asked for, in either
/// wire form RFC 9110 §10.2.3 allows.
///
/// Both forms are honored: `delay-seconds` (`"120"`) and `HTTP-date`
/// (`"Sun, 06 Nov 1994 08:49:37 GMT"`) — each bounded by
/// [`RETRY_AFTER_MAX_MS`], as described below.
///
/// `now` is a **parameter**, not a `Utc::now()` call inside: that is what
/// makes the HTTP-date branch assertable with exact values from the in-crate
/// test module — no clock mock, no sleeping. The conversion is pure, so it
/// does not belong on the public surface; what the crate's users observe is
/// the retry behavior, pinned through the public port.
///
/// [`None`] when the header is absent, empty, or parses in neither form;
/// the caller then falls back to exponential backoff (and says so in a
/// `warn!` — the discard is never silent).
///
/// A date at or before `now` yields [`Duration::ZERO`] ("retry now"),
/// clamped explicitly: a stale date must never become a negative
/// duration.
///
/// Any honored delay is clamped to [`RETRY_AFTER_MAX_MS`] here, not at the
/// call site, so both wire forms and the unit tests share one bound.
///
/// # Observability
///
/// The oversized case warns **from inside this function**
/// ([`clamp_retry_after`]) rather than being handed back for the caller to
/// warn about. Two reasons, both about this function's signature: it returns
/// a bare `Option<Duration>`, so a clamp is only detectable here *before* the
/// requested magnitude is discarded — afterwards `RETRY_AFTER_MAX_MS` is
/// indistinguishable from a legitimate `Retry-After: 60`, and a caller-side
/// comparison would double-warn on every honest header that happens to sit on
/// the boundary. The call site keeps its own `warn!` for the case this one
/// cannot cover (unparsable → `None`), carrying the `provider_id`; this
/// warning carries the raw value and both magnitudes, which is what identifies
/// the hostile header in a trace.
fn retry_after_delay(header: Option<&str>, now: DateTime<Utc>) -> Option<Duration> {
    let raw = header?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(clamp_retry_after(Duration::from_secs(secs), raw));
    }
    // RFC 9110 §10.2.3 names three HTTP-date forms. Verified empirically
    // against chrono 0.4.45: `parse_from_rfc2822` accepts IMF-fixdate
    // (the one servers actually send, and the regression this unit
    // fixes); neither obsolete form — asctime nor RFC 850 — parses. An
    // unsupported form degrades to exponential backoff behind the
    // explicit `warn!` above: never silent, never a wrong sleep.
    let target = DateTime::parse_from_rfc2822(raw).ok()?;
    let target = target.with_timezone(&Utc);
    if target <= now {
        return Some(Duration::ZERO);
    }
    // A delta too large for `to_std` (well past the year-9999 case, which
    // chrono converts fine at ~2.5e11 s) is still an *oversized*
    // instruction, not a retry-now one: saturate to `Duration::MAX` and let
    // the shared clamp decide. The previous `unwrap_or(Duration::ZERO)`
    // turned such a date into an immediate retry — the opposite of what the
    // server asked for.
    let requested = (target - now).to_std().unwrap_or(Duration::MAX);
    Some(clamp_retry_after(requested, raw))
}

/// Pure wire parse: response body → one vector per input, in order.
///
/// Tolerant by contract (unknown fields ignored); honest on degenerate
/// bodies — invalid JSON, missing `data`, empty `data`, an empty vector, or
/// a vector count that does not match the input count are all
/// [`SemanticError::Inference`]. The count check keeps chunk↔vector
/// alignment from silently shifting when a vendor truncates.
fn parse_embedding_body(
    body: &str,
    expected_inputs: usize,
) -> Result<Vec<Vec<f32>>, SemanticError> {
    let parsed: EmbeddingResponse = serde_json::from_str(body).map_err(|e| {
        SemanticError::Inference(format!(
            "el endpoint de embeddings devolvió un cuerpo inválido: {e}"
        ))
    })?;
    if let Some(usage) = &parsed.usage {
        tracing::debug!(
            prompt_tokens = usage.prompt_tokens,
            total_tokens = usage.total_tokens,
            "remote embedding usage"
        );
    }
    if parsed.data.is_empty() {
        return Err(SemanticError::Inference(
            "el endpoint de embeddings devolvió `data` vacío".to_string(),
        ));
    }
    if parsed.data.len() != expected_inputs {
        return Err(SemanticError::Inference(format!(
            "el endpoint de embeddings devolvió {} vectores para {expected_inputs} textos",
            parsed.data.len()
        )));
    }
    let mut vecs = Vec::with_capacity(parsed.data.len());
    for datum in &parsed.data {
        if datum.embedding.is_empty() {
            return Err(SemanticError::Inference(
                "el endpoint de embeddings devolvió un vector vacío".to_string(),
            ));
        }
        vecs.push(datum.embedding.clone());
    }
    Ok(vecs)
}

impl RemoteEmbeddingAdapter {
    /// Build the adapter resolving the credential ONCE (startup).
    ///
    /// The entry gate runs first (forbidden `base_url` fails here), then the
    /// credential resolves, then the parameterized guarded client is built.
    ///
    /// # Errors
    ///
    /// [`ProviderInitError`] when the entry gate rejects the URL, the
    /// credential does not resolve, the model is undeclared, or the shared
    /// client cannot be built.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
        let http = embedding_http_client(config.allow_loopback)?;
        Self::with_http(config, http)
    }

    /// Variant with an already-built HTTP client (shared process client or
    /// wiremock in tests).
    ///
    /// # SSRF contract
    ///
    /// The caller owns the injected client's protection: production passes
    /// the client from [`new`](Self::new) (parameterized guard applied);
    /// tests inject a plain client against loopback wiremock.
    ///
    /// # Errors
    ///
    /// [`ProviderInitError::InvalidBaseUrl`] / `AuthFailed` — the entry gate
    /// and the credential resolution happen here, NOT on the first call.
    pub fn with_http(
        config: ProviderConfig,
        http: wreq::Client,
    ) -> Result<Self, ProviderInitError> {
        entry_gate(&config)?;
        let auth_source = config.auth.label();
        let secret = config
            .auth
            .resolve()
            .map_err(|source| ProviderInitError::AuthFailed {
                provider_id: config.id.clone(),
                source,
            })?;
        let model = config
            .model
            .clone()
            .ok_or_else(|| ProviderInitError::InvalidBaseUrl {
                provider_id: config.id.clone(),
                source: crate::error::ScraperError::Config(format!(
                "el provider '{}' no declara `model`: el endpoint remoto lo exige en cada request",
                config.id
            )),
            })?;
        Ok(Self {
            http,
            base_url: config.base_url.clone(),
            model,
            api_key: secret,
            auth_source,
            provider_id: config.id.clone(),
            pinned_dim: config.embedding_dim,
            dim: OnceLock::new(),
        })
    }

    /// Provider identifier (startup diagnostics and error context).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.provider_id
    }

    /// Startup probe: pin-or-adopt the served dimension.
    ///
    /// POSTs `["probe"]` and reads the actual dim off the first vector: a
    /// declared `embedding_dim` that disagrees fails closed (mixed dims
    /// silently break cosine ranking — no auto-migration); unpinned adopts
    /// with an info log and serves it from here on.
    ///
    /// Call once at startup before serving; the binary maps failures to
    /// exit-78 `ConfigError`.
    ///
    /// # Errors
    ///
    /// [`SemanticError::Inference`] when the probe request fails, the body
    /// is degenerate, or a declared pin disagrees with the served dim.
    pub async fn probe_dim(&self) -> Result<usize, SemanticError> {
        let probe = vec!["probe".to_string()];
        let actual = self
            .post_embeddings(&probe)
            .await?
            .into_iter()
            .next()
            .map(|vec| vec.len())
            .unwrap_or(0);
        if let Some(pinned) = self.pinned_dim {
            if actual != pinned {
                return Err(SemanticError::Inference(format!(
                    "dimensión declarada {pinned} != remota {actual} para provider '{}'",
                    self.provider_id
                )));
            }
        } else {
            tracing::info!(
                provider_id = %self.provider_id,
                dim = actual,
                "remote embedding dim adopted from startup probe"
            );
        }
        let _ = self.dim.set(actual);
        Ok(actual)
    }

    /// The dimension this adapter serves: probed once available, else the
    /// configured pin, else `0` (cold adapter — probe at startup before
    /// serving).
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
        self.dim.get().copied().or(self.pinned_dim).unwrap_or(0)
    }

    /// POST one batch with the bounded retry classification: timeouts, 429
    /// (honoring `max(Retry-After, backoff)`) and 5xx retry with backoff;
    /// other 4xx and builder errors are terminal as-is; exhaustion reports
    /// the last observed status, never a hardcoded one.
    async fn post_embeddings(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SemanticError> {
        let endpoint = format!(
            "{}/embeddings",
            self.base_url.as_str().trim_end_matches('/')
        );
        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            // Invariante 1: `auth: None` no manda cabecera. La rama `Some`
            // es la única que puede firmarla, así que no hay forma de que una
            // credencial llegue a una petición declarada anónima — ni siquiera
            // si el entorno tiene una key válida.
            let mut pending = self.http.post(&endpoint);
            if let Some(key) = &self.api_key {
                pending = pending.bearer_auth(key.expose_secret());
            }
            match pending.json(&request).send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if response.status().is_success() {
                        return self.read_and_parse(response, texts.len()).await;
                    }
                    if status == 429 || (500..=599).contains(&status) {
                        if attempt >= EMBEDDING_MAX_ATTEMPTS {
                            return Err(last_status_error(&self.provider_id, status));
                        }
                        let raw_retry_after = response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok());
                        let retry_after = retry_after_delay(raw_retry_after, Utc::now());
                        // A header we could not honor is a server
                        // instruction we are about to ignore — say so,
                        // with the raw value, instead of silently
                        // downgrading to exponential backoff. An
                        // *oversized* header is reported one level down,
                        // inside `retry_after_delay`, which is the only
                        // place that still knows the requested magnitude.
                        if let Some(raw) = raw_retry_after {
                            if retry_after.is_none() {
                                tracing::warn!(
                                    provider_id = %self.provider_id,
                                    status,
                                    raw = %raw,
                                    reason = "neither delay-seconds nor a parsable HTTP-date",
                                    "remote embedding retry-after discarded; falling back to exponential backoff"
                                );
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(backoff_delay_ms(
                            attempt,
                            retry_after,
                        )))
                        .await;
                        continue;
                    }
                    // Un 401 se explica según lo que realmente se mandó. Sin
                    // credencial no es "credencial inválida": mandarle a
                    // rotar una key que nunca existió es el falso positivo
                    // que esta fila evita.
                    if status == 401 {
                        return Err(self.unauthorized_error(status));
                    }
                    return Err(last_status_error(&self.provider_id, status));
                },
                Err(e) if e.is_timeout() => {
                    if attempt >= EMBEDDING_MAX_ATTEMPTS {
                        return Err(SemanticError::Inference(format!(
                            "el endpoint de embeddings '{}' agotó el tiempo de espera",
                            self.provider_id
                        )));
                    }
                    tokio::time::sleep(Duration::from_millis(backoff_delay_ms(attempt, None)))
                        .await;
                },
                Err(e) => {
                    return Err(SemanticError::Inference(format!(
                        "error de transporte contra el endpoint de embeddings '{}': {e}",
                        self.provider_id
                    )));
                },
            }
        }
    }

    /// A 401 reportado según si la petición llevaba credencial.
    ///
    /// Sigue siendo [`SemanticError::Inference`] (mismo tipo que cualquier
    /// otro fallo remoto — no se introduce una variante nueva: `error.rs` está
    /// fuera de las superficies de esta unidad). Lo que cambia es el texto en
    /// español y el campo estructurado `auth_source` del log, que es lo que
    /// permite a un operador —o a un matcher programático— distinguir los dos
    /// casos sin parsear el estado HTTP.
    ///
    /// El secreto nunca aparece: `auth_source` es la etiqueta de la fuente
    /// declarada, no la credencial.
    fn unauthorized_error(&self, status: u16) -> SemanticError {
        if self.api_key.is_none() {
            tracing::warn!(
                provider_id = %self.provider_id,
                status,
                auth_source = self.auth_source,
                credential_sent = false,
                "remote embedding endpoint rejected an anonymous request"
            );
            return SemanticError::Inference(format!(
                "el endpoint de embeddings '{}' devolvió {status} a una petición sin credencial \
                 (auth source '{}'): el servidor rechazó la petición anónima",
                self.provider_id, self.auth_source
            ));
        }
        tracing::warn!(
            provider_id = %self.provider_id,
            status,
            auth_source = self.auth_source,
            credential_sent = true,
            "remote embedding endpoint rejected the configured credential"
        );
        SemanticError::Inference(format!(
            "el endpoint de embeddings '{}' devolvió {status}: rechazó la credencial configurada \
             (auth source '{}')",
            self.provider_id, self.auth_source
        ))
    }

    /// Bounded body read (16 MiB cap) plus the tolerant wire parse, with the
    /// 1:1 input↔vector count check.
    async fn read_and_parse(
        &self,
        response: wreq::Response,
        expected_inputs: usize,
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let body = crate::domain::body_cap::read_body_capped(response, EMBEDDING_MAX_BODY_BYTES)
            .await
            .map_err(|e| {
                if matches!(e, crate::domain::http_error::HttpError::BodyTooLarge { .. }) {
                    return SemanticError::Inference(format!(
                        "la respuesta del endpoint de embeddings '{}' excede 16 MiB",
                        self.provider_id
                    ));
                }
                SemanticError::Inference(format!(
                    "error al leer la respuesta del endpoint de embeddings '{}': {e}",
                    self.provider_id
                ))
            })?;
        parse_embedding_body(&body, expected_inputs)
    }
}

/// Terminal/retries-exhausted status error naming the last observed status.
fn last_status_error(provider_id: &str, status: u16) -> SemanticError {
    SemanticError::Inference(format!(
        "el endpoint de embeddings '{provider_id}' devolvió el estado {status}"
    ))
}

impl EmbeddingPort for RemoteEmbeddingAdapter {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        let span = tracing::debug_span!(
            "embed",
            provider_id = %self.provider_id,
            model = %self.model,
            dim = self.embedding_dim()
        );
        Box::pin(
            async move {
                let batch = self
                    .embed_batch(std::slice::from_ref(&text.to_owned()))
                    .await?;
                batch.into_iter().next().ok_or_else(|| {
                    SemanticError::Inference(
                        "el endpoint de embeddings devolvió `data` vacío".to_string(),
                    )
                })
            }
            .instrument(span),
        )
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        let span = tracing::debug_span!(
            "embed_batch",
            provider_id = %self.provider_id,
            model = %self.model,
            count = texts.len(),
            dim = self.embedding_dim()
        );
        Box::pin(
            async move {
                if texts.is_empty() {
                    return Ok(Vec::new());
                }
                self.post_embeddings(texts).await
            }
            .instrument(span),
        )
    }

    fn embedding_dim(&self) -> usize {
        self.embedding_dim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::auth_source::AuthSource;
    use crate::domain::providers::{Capability, ProviderKind};

    /// Regla de actualización de fixtures (patrón `fixtures/llm`): los
    /// fixtures viven fuera del código; si un vendor deriva el wire shape,
    /// el fixture se actualiza primero en commit separado con justificación.
    fn load_fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/remote_embedding/{name}"));
        std::fs::read_to_string(&path).expect("fixture legible")
    }

    fn config_with(auth: AuthSource) -> ProviderConfig {
        ProviderConfig {
            id: "test-remote".to_string(),
            display_name: "Test Remote".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse("https://api.example.com/v1").expect("url"),
            auth,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".to_string()),
            embedding_dim: None,
            allow_loopback: false,
        }
    }

    fn adapter_for(
        server: &wiremock::MockServer,
        modifier: impl FnOnce(&mut ProviderConfig),
    ) -> RemoteEmbeddingAdapter {
        let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let mut cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        });
        cfg.base_url = Url::parse(&server.uri()).expect("mock uri parses");
        cfg.allow_loopback = true;
        modifier(&mut cfg);
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let adapter = RemoteEmbeddingAdapter::with_http(cfg, client).expect("test adapter builds");
        // `with_http` resolves the credential in the constructor: dropping
        // the env guard here proves later calls never re-resolve.
        drop(env);
        adapter
    }

    // --- Pure parse layer (no network) ---

    #[test]
    fn parse_happy_fixture_maps_vectors_one_to_one_in_order() {
        let vecs = parse_embedding_body(&load_fixture("embeddings_happy.json"), 2)
            .expect("happy fixture parses");
        assert_eq!(vecs.len(), 2, "one vector per input, in order");
        assert_eq!(vecs[0], vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
        assert_eq!(vecs[1], vec![0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1]);
    }

    #[test]
    fn parse_tolerant_fixture_ignores_unknown_fields() {
        let vecs = parse_embedding_body(&load_fixture("embeddings_tolerant.json"), 1)
            .expect("vendor extras must be ignored");
        assert_eq!(vecs.len(), 1);
        assert_eq!(vecs[0].len(), 8, "non-384d dims pass through");
        assert!((vecs[0][0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_empty_data_is_semantic_error() {
        let err = parse_embedding_body(&load_fixture("embeddings_empty.json"), 1)
            .expect_err("empty data must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "empty data must be Inference, got: {err}"
        );
    }

    #[test]
    fn parse_count_mismatch_is_semantic_error() {
        let err = parse_embedding_body(&load_fixture("embeddings_happy.json"), 1)
            .expect_err("2 vectors for 1 input must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "count mismatch must be Inference, got: {err}"
        );
        assert!(
            err.to_string().contains('2'),
            "mismatch must name the served count, got: {err}"
        );
    }

    #[test]
    fn parse_empty_vector_is_semantic_error() {
        let err = parse_embedding_body(r#"{"data": [{"index": 0, "embedding": []}]}"#, 1)
            .expect_err("empty vector must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "empty vector must be Inference, got: {err}"
        );
    }

    #[test]
    fn parse_invalid_json_is_semantic_error() {
        let err = parse_embedding_body("not json", 1).expect_err("invalid body must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "invalid body must be Inference, got: {err}"
        );
    }

    // --- Backoff policy (no sleeps) ---

    #[test]
    fn backoff_honors_max_of_retry_after_and_exponential() {
        // Retry-After: 3s → 3000ms beats exponential attempt 2 (2000ms).
        assert_eq!(backoff_delay_ms(2, Some(Duration::from_secs(3))), 3000);
        // Retry-After: 1s → 1000ms loses to exponential attempt 3 (4000ms).
        assert_eq!(backoff_delay_ms(3, Some(Duration::from_secs(1))), 4000);
        // Absent/zero → pure exponential from the 1000ms base.
        assert_eq!(backoff_delay_ms(1, None), 1000);
        assert_eq!(backoff_delay_ms(2, None), 2000);
        assert_eq!(backoff_delay_ms(2, Some(Duration::ZERO)), 2000);
    }

    // --- Retry-After wire forms (RFC 9110 §10.2.3) ---
    //
    // `now` is injected, so these are exact value assertions rather than
    // "roughly a minute" timing guesses.

    /// The single instant every `Retry-After` test measures against:
    /// `1994-11-06T08:49:37Z`, RFC 9110's own example date — so the header
    /// values below are readably "60s away".
    fn utc_fixture() -> DateTime<Utc> {
        DateTime::parse_from_rfc2822("Sun, 06 Nov 1994 08:49:37 GMT")
            .expect("RFC 2822 fixture parses")
            .with_timezone(&Utc)
    }

    #[test]
    fn retry_after_delay_seconds_form_is_honored() {
        let now = utc_fixture();
        // "120" parses as a plain 120s instruction, but it is above the 60s
        // ceiling, so it is honored *bounded*: this row now pins the clamp,
        // not a silent cap (see the hostile-header test below for the
        // under-ceiling counterpart that must stay untouched).
        assert_eq!(
            retry_after_delay(Some("120"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
            "a 120s delay-seconds is parsed, then clamped to the honored ceiling"
        );
        // Surrounding whitespace is legal header framing.
        assert_eq!(
            retry_after_delay(Some(" 7 "), now),
            Some(Duration::from_secs(7))
        );
    }

    /// A `delay-seconds` header is an unbounded `u64`, and
    /// `Duration::from_secs` accepts all of it — so `u64::MAX` used to become
    /// a `Duration` whose millisecond conversion saturates, and the adapter
    /// slept for ~584M years instead of failing or retrying (#1956,
    /// `R3-overflow-saturation`). Pure, so this asserts in microseconds.
    #[test]
    fn retry_after_hostile_delay_seconds_is_clamped_not_slept() {
        let now = utc_fixture();
        assert_eq!(
            retry_after_delay(Some("18446744073709551615"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
            "u64::MAX seconds must clamp to the honored ceiling, never become a sleep"
        );
        // End to end through the sleep computation: the value handed to
        // `tokio::time::sleep` is bounded even though nothing sleeps here.
        assert_eq!(
            backoff_delay_ms(1, retry_after_delay(Some("18446744073709551615"), now)),
            RETRY_AFTER_MAX_MS,
            "the ms handed to tokio::time::sleep must be bounded by the ceiling"
        );
        // Just above the ceiling behaves the same way.
        assert_eq!(
            retry_after_delay(Some("61"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
            "61s is one second past the ceiling and must be clamped"
        );
    }

    /// The same class of hostile input through the other wire form: an
    /// HTTP-date far enough out that its delta does not even fit in a
    /// `Duration` (~7973 years), which the old `to_std().unwrap_or(ZERO)`
    /// silently answered as "retry immediately".
    #[test]
    fn retry_after_far_future_http_date_is_clamped_to_the_ceiling() {
        let now = utc_fixture();
        assert_eq!(
            retry_after_delay(Some("Fri, 31 Dec 9999 23:59:59 GMT"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
            "a year-9999 date must clamp to the ceiling, not saturate or sleep"
        );
        // A date inside the representable range but past the ceiling takes
        // the same path, so the bound does not depend on the overflow edge.
        assert_eq!(
            retry_after_delay(Some("Sun, 06 Nov 1994 08:51:37 GMT"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS)),
            "a 120s HTTP-date is over the ceiling and must clamp"
        );
    }

    /// The counterweight to the two tests above: the ceiling must bound
    /// hostile input **without** quietly capping normal operation. Every value
    /// here is honored exactly, to the millisecond.
    #[test]
    fn retry_after_under_the_ceiling_is_honored_exactly() {
        let now = utc_fixture();
        assert_eq!(
            retry_after_delay(Some("30"), now),
            Some(Duration::from_secs(30)),
            "a legitimate 30s delay-seconds must not be clamped"
        );
        assert_eq!(
            retry_after_delay(Some("Sun, 06 Nov 1994 08:50:07 GMT"), now),
            Some(Duration::from_secs(30)),
            "a legitimate 30s HTTP-date must not be clamped"
        );
        // The boundary is inclusive: exactly the ceiling is honored, so the
        // clamp can never shave a millisecond off a compliant server.
        assert_eq!(
            retry_after_delay(Some("60"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS))
        );
        assert_eq!(
            retry_after_delay(Some("Sun, 06 Nov 1994 08:50:37 GMT"), now),
            Some(Duration::from_millis(RETRY_AFTER_MAX_MS))
        );
        // And it still flows into the sleep as asked, beating the exponential
        // floor (1000ms) exactly as a 30s instruction should.
        assert_eq!(
            backoff_delay_ms(1, retry_after_delay(Some("30"), now)),
            30_000
        );
    }

    #[test]
    fn retry_after_http_date_form_is_honored() {
        // The bug this pins: the IMF-fixdate form used to fail a
        // `.parse::<u64>()`, be discarded silently, and downgrade a
        // server's explicit wait to pure exponential backoff.
        let now = utc_fixture();
        assert_eq!(
            retry_after_delay(Some("Sun, 06 Nov 1994 08:50:37 GMT"), now),
            Some(Duration::from_secs(60)),
            "IMF-fixdate must convert to a 60s delay against the injected now"
        );
        // Measured limitation (chrono 0.4.45): neither obsolete HTTP-date form
        // parses, so each degrades to exponential backoff behind the
        // caller's warn!. Pinned so a chrono upgrade that starts accepting
        // them shows up as a deliberate change, not silent drift.
        assert_eq!(
            retry_after_delay(Some("Sunday, 06-Nov-94 08:50:37 GMT"), now),
            None,
            "RFC 850 is not parsed by chrono 0.4.45; it must degrade, not panic"
        );
        assert_eq!(
            retry_after_delay(Some("Sun Nov  6 08:50:37 1994"), now),
            None,
            "asctime is not parsed by chrono 0.4.45; it must degrade, not panic"
        );
    }

    #[test]
    fn retry_after_past_date_clamps_to_zero_never_negative() {
        let now = utc_fixture();
        let past = "Sun, 06 Nov 1994 08:49:37 GMT";
        assert_eq!(
            retry_after_delay(Some(past), now),
            Some(Duration::ZERO),
            "a stale date means retry-now and must not underflow"
        );
        // Exactly-now is also "not in the future".
        assert_eq!(
            retry_after_delay(Some(past), now),
            Some(Duration::ZERO),
            "a date equal to now clamps the same way"
        );
        // And the zero delay leaves the exponential floor standing alone.
        assert_eq!(
            backoff_delay_ms(1, retry_after_delay(Some(past), now)),
            1000
        );
    }

    #[test]
    fn retry_after_absent_empty_or_garbage_is_none() {
        let now = utc_fixture();
        assert_eq!(retry_after_delay(None, now), None, "absent header");
        assert_eq!(retry_after_delay(Some(""), now), None, "empty header");
        assert_eq!(retry_after_delay(Some("   "), now), None, "blank header");
        assert_eq!(retry_after_delay(Some("pronto"), now), None, "garbage");
        assert_eq!(
            retry_after_delay(Some("-5"), now),
            None,
            "negative delay-seconds is not a valid Retry-After"
        );
        assert_eq!(
            retry_after_delay(Some("Sun, 99 Xxx 1994 08:49:37 GMT"), now),
            None,
            "an unparsable date falls back rather than panicking"
        );
        // None means "exponential only", never "sleep zero".
        assert_eq!(
            backoff_delay_ms(2, retry_after_delay(Some("pronto"), now)),
            2000
        );
    }

    // --- Construction contract ---

    #[test]
    fn missing_credential_fails_at_construction_not_first_call() {
        // Hermetic: clean the var — a leaked value from a sibling would flip
        // this to success (nextest isolates, plain cargo test does not).
        let _guard = webfang_test_utils::EnvGuard::clean(&["WEBFANG_TEST_REMOTE_MISSING_VAR"]);
        let cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_MISSING_VAR".to_string(),
        });
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
            Err(e) => e,
            Ok(_) => panic!("debe fallar en construcción"),
        };
        assert!(err.to_string().contains("test-remote"), "{err}");
        assert!(matches!(err, ProviderInitError::AuthFailed { .. }));
    }

    #[test]
    fn missing_model_fails_at_construction() {
        // The key must resolve (constructor checks auth before model), so it
        // is set explicitly — never inherited from a sibling's guard.
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let mut cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        });
        cfg.model = None;
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
            Err(e) => e,
            Ok(_) => panic!("model ausente debe fallar en construcción"),
        };
        assert!(err.to_string().contains("test-remote"), "{err}");
        assert!(matches!(err, ProviderInitError::InvalidBaseUrl { .. }));
    }

    #[test]
    fn loopback_base_url_rejected_without_opt_in() {
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let mut cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        });
        cfg.base_url = Url::parse("http://127.0.0.1:9/v1").expect("url");
        cfg.allow_loopback = false;
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
            Err(e) => e,
            Ok(_) => panic!("loopback sin opt-in debe fallar en construcción"),
        };
        assert!(
            matches!(err, ProviderInitError::InvalidBaseUrl { .. }),
            "loopback sin opt-in debe ser InvalidBaseUrl, got: {err}"
        );
    }

    #[test]
    fn loopback_base_url_constructs_with_opt_in() {
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let mut cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        });
        cfg.base_url = Url::parse("http://127.0.0.1:9/v1").expect("url");
        cfg.allow_loopback = true;
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let adapter =
            RemoteEmbeddingAdapter::with_http(cfg, client).expect("loopback con opt-in construye");
        assert_eq!(adapter.id(), "test-remote");
    }

    #[test]
    fn cold_adapter_reports_pin_or_zero_before_probe() {
        let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let client = wreq::Client::builder()
            .build()
            .expect("plain test client builds");
        let pinned = RemoteEmbeddingAdapter::with_http(
            {
                let mut cfg = config_with(AuthSource::Env {
                    var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
                });
                cfg.embedding_dim = Some(1536);
                cfg
            },
            client.clone(),
        )
        .expect("pinned adapter builds");
        assert_eq!(pinned.embedding_dim(), 1536);
        let unpinned = RemoteEmbeddingAdapter::with_http(
            config_with(AuthSource::Env {
                var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
            }),
            client,
        )
        .expect("unpinned adapter builds");
        assert_eq!(unpinned.embedding_dim(), 0);
        drop(env);
    }

    // --- Wire behavior against wiremock (loopback literals bypass the
    // custom resolver, so the plain injected client needs no env hatch) ---

    async fn mount_embeddings(
        server: &wiremock::MockServer,
        status: u16,
        body: &str,
    ) -> wiremock::MockGuard {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount_as_scoped(server)
            .await
    }

    #[tokio::test]
    async fn embed_batch_happy_path_serves_fixture_vectors_in_order() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard = mount_embeddings(&server, 200, &load_fixture("embeddings_happy.json")).await;
        let adapter = adapter_for(&server, |_| {});

        let vecs = adapter
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .expect("happy path serves");
        assert_eq!(vecs.len(), 2);
        assert_eq!(vecs[0].len(), 8, "non-384d dims pass through");
        assert_eq!(adapter.embedding_dim(), 0, "no probe ran yet");
    }

    #[tokio::test]
    async fn embed_delegates_to_batch_for_single_text() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard =
            mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
        let adapter = adapter_for(&server, |_| {});

        let vec = adapter.embed("hello").await.expect("single embed serves");
        assert_eq!(vec.len(), 8);
    }

    #[tokio::test]
    async fn embed_batch_empty_input_short_circuits_without_request() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        // No mock mounted: any request would 404. Empty input must never dial.
        let adapter = adapter_for(&server, |_| {});

        let empty: Vec<String> = vec![];
        let vecs = adapter
            .embed_batch(&empty)
            .await
            .expect("empty input short-circuits");
        assert!(vecs.is_empty());
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            0,
            "empty input must not send any request"
        );
    }

    #[tokio::test]
    async fn wire_shape_matches_contract_golden() {
        use std::sync::{Arc, Mutex};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

        struct Capture(Arc<Mutex<Option<serde_json::Value>>>);
        impl Respond for Capture {
            fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
                *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(serde_json::from_slice(&request.body).expect("body es JSON"));
                // Single-vector body: the golden posts one input, and the
                // 1:1 count check holds the wire honest.
                ResponseTemplate::new(200).set_body_string(load_fixture("embeddings_tolerant.json"))
            }
        }

        let server = MockServer::start().await;
        let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(Capture(captured.clone()))
            .mount(&server)
            .await;

        let adapter = adapter_for(&server, |_| {});
        adapter
            .embed_batch(&["hello world".to_string()])
            .await
            .expect("200 + valid JSON succeeds");

        let body = captured
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .expect("el mock debió capturar el body");
        let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/remote_embedding/embed_request.golden.json");
        let golden: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&golden_path).expect("golden legible"))
                .expect("golden es JSON válido");
        assert_eq!(
            body, golden,
            "wire shape cambió: actualizá el golden en commit separado con justificación"
        );
    }

    #[tokio::test]
    async fn status_429_then_success_recovers() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let happy = load_fixture("embeddings_tolerant.json");
        // Mount the one-shot 429 FIRST: wiremock matches in mount order, so
        // the first request hits the 429 and the retry falls through to the
        // blanket 200 mounted second.
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "0")
                    .set_body_string("lento"),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&happy))
            .mount(&server)
            .await;
        let adapter = adapter_for(&server, |_| {});

        let vecs = adapter
            .embed_batch(&["a".to_string()])
            .await
            .expect("429 + Retry-After: 0 recovers on retry");
        assert_eq!(vecs.len(), 1);
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            2,
            "one 429 plus one recovery attempt"
        );
    }

    #[tokio::test]
    async fn status_500_exhausts_and_reports_last_status() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard = mount_embeddings(&server, 500, "caído").await;
        let adapter = adapter_for(&server, |_| {});

        let err = adapter
            .embed_batch(&["a".to_string()])
            .await
            .expect_err("persistent 500 must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "exhaustion must be Inference, got: {err}"
        );
        assert!(
            err.to_string().contains("500"),
            "exhaustion must report the last status, got: {err}"
        );
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            EMBEDDING_MAX_ATTEMPTS as usize,
            "retries must spend the full attempt budget"
        );
    }

    #[tokio::test]
    async fn status_400_is_terminal_without_retry() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard = mount_embeddings(&server, 400, "mala petición").await;
        let adapter = adapter_for(&server, |_| {});

        let err = adapter
            .embed_batch(&["a".to_string()])
            .await
            .expect_err("400 must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "terminal 4xx must be Inference, got: {err}"
        );
        assert!(
            err.to_string().contains("400"),
            "terminal 4xx reports its status as-is, got: {err}"
        );
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            1,
            "terminal 4xx must not retry"
        );
    }

    #[tokio::test]
    async fn oversize_body_maps_to_semantic_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let huge = "x".repeat((EMBEDDING_MAX_BODY_BYTES + 1) as usize);
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_string(&huge))
            .mount(&server)
            .await;
        let adapter = adapter_for(&server, |_| {});

        let err = adapter
            .embed_batch(&["a".to_string()])
            .await
            .expect_err("body past 16 MiB must fail");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "oversize body must be Inference, got: {err}"
        );
        assert!(
            err.to_string().contains("16 MiB"),
            "oversize error must name the cap, got: {err}"
        );
    }

    // Multi-thread runtime: the stalled responder below blocks one worker
    // for 500ms, and the 150ms client deadline must fire on another one —
    // on a current-thread runtime the stall would park the timer itself.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timeout_is_retried_and_recovery_is_served() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

        // First request stalls past the client deadline; later ones serve.
        struct StallOnce {
            stalled: AtomicBool,
            body: String,
        }
        impl Respond for StallOnce {
            fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
                if !self.stalled.swap(true, Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(500));
                }
                ResponseTemplate::new(200).set_body_string(self.body.clone())
            }
        }

        let server = MockServer::start().await;
        let responder = Arc::new(StallOnce {
            stalled: AtomicBool::new(false),
            body: load_fixture("embeddings_tolerant.json"),
        });
        struct Share(Arc<StallOnce>);
        impl Respond for Share {
            fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
                self.0.respond(request)
            }
        }
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(Share(responder))
            .mount(&server)
            .await;

        // Tiny-deadline client injected via with_http (multi-thread runtime:
        // the stalled worker must not park the timeout timer).
        let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
        let mut cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        });
        cfg.base_url = Url::parse(&server.uri()).expect("mock uri parses");
        cfg.allow_loopback = true;
        let client = wreq::Client::builder()
            .timeout(Duration::from_millis(150))
            .build()
            .expect("deadline client builds");
        let adapter = RemoteEmbeddingAdapter::with_http(cfg, client).expect("test adapter builds");
        drop(env);

        let vecs = adapter
            .embed_batch(&["a".to_string()])
            .await
            .expect("timeout must retry and serve the recovery");
        assert_eq!(vecs.len(), 1);
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            2,
            "one timed-out attempt plus one recovery"
        );
    }

    // --- Body-read error mapping ---
    //
    // ADR-0004's original "truncado de stream" row is not expressible
    // here: the adapter has no streaming API (it uses
    // `read_body_capped`), and wiremock has no primitive to abort a body
    // mid-stream. What IS observable is the error mapping for a body
    // read that fails — the arm that a mid-stream abort would land in.

    #[tokio::test]
    async fn body_read_error_maps_to_spanish_inference_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // `respond_with` cannot abort a body mid-stream in wiremock; a
        // `Content-Length` that overshoots the delivered payload is the
        // closest reachable shape — the client sees a short read.
        let short = r#"{"data":[{"index":0,"embedding":[0.1,0.2]}]}"#;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "4096")
                    .set_body_string(short),
            )
            .mount(&server)
            .await;
        let adapter = adapter_for(&server, |_| {});

        // Either the short read surfaces as the body-read error arm or the
        // complete body parses; both are legitimate, and the assertion
        // pins that a failure is NEVER silent and never panics.
        match adapter.embed_batch(&["a".to_string()]).await {
            Ok(vecs) => assert_eq!(vecs.len(), 1, "a complete body must still parse"),
            Err(err) => {
                assert!(
                    matches!(err, SemanticError::Inference(_)),
                    "a body-read failure must be Inference, got: {err}"
                );
                let rendered = err.to_string();
                assert!(
                    rendered.contains("embeddings"),
                    "the error must name the endpoint, got: {rendered}"
                );
            },
        }
    }

    // --- Startup probe: pin-or-adopt ---

    #[tokio::test]
    async fn probe_adopts_dim_when_unpinned() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        // Single-vector body: the probe sends exactly one input, and the
        // 1:1 count check holds the wire honest.
        let _guard =
            mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
        let adapter = adapter_for(&server, |_| {});

        let dim = adapter.probe_dim().await.expect("probe serves 8d");
        assert_eq!(dim, 8);
        assert_eq!(adapter.embedding_dim(), 8, "adopted dim is served");
    }

    #[tokio::test]
    async fn probe_matching_pin_continues() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard =
            mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
        let adapter = adapter_for(&server, |cfg| cfg.embedding_dim = Some(8));

        let dim = adapter.probe_dim().await.expect("matching pin continues");
        assert_eq!(dim, 8);
    }

    #[tokio::test]
    async fn probe_pin_mismatch_fails_closed() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _guard =
            mount_embeddings(&server, 200, &load_fixture("embeddings_tolerant.json")).await;
        let adapter = adapter_for(&server, |cfg| cfg.embedding_dim = Some(7));

        let err = adapter.probe_dim().await.expect_err("pin mismatch fails");
        assert!(
            matches!(err, SemanticError::Inference(_)),
            "pin mismatch must be Inference, got: {err}"
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains('7') && rendered.contains('8'),
            "mismatch must name declared and actual dims, got: {rendered}"
        );
    }
}
