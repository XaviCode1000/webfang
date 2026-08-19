//! LLM structured extraction — configuration, SSRF gate, and application
//! orchestration (#789).

use std::collections::HashSet;
use std::sync::Arc;

use crate::application::scraper_service::enforce_robots_policy;
use crate::domain::credentials::ApiKey;
use crate::domain::http_port::HttpClientPort;
use crate::domain::llm_port::{ChatMessage, LlmPort, LlmRequest, LlmResponse};
use crate::domain::semantic_cleaner::SemanticCleaner;
use crate::domain::text_chunker::TextChunker;
use crate::error::{Result, ScraperError};
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
use crate::infrastructure::llm::validation::{validate_record, validate_schema};
use serde_json::Value;
use url::Url;

/// Approximate chars per token for the sub-chunk char budget.
const CHARS_PER_TOKEN: usize = 8;

/// Result envelope for one LLM extraction run (#789).
#[derive(Debug, Clone, PartialEq)]
pub struct LlmExtraction {
    /// Source URL the records were extracted from.
    pub url: String,
    /// Provider model used for the completions.
    pub model: String,
    /// Merged records, in first-seen order (deduplicated).
    pub records: Vec<Value>,
    /// Number of sub-chunks sent to the LLM (`== LlmPort` call count).
    pub chunks: usize,
    /// Total prompt tokens billed by the provider.
    pub input_tokens: u32,
    /// Total completion tokens billed by the provider.
    pub output_tokens: u32,
}

/// Application orchestrator for LLM structured extraction (#789): schema
/// gate → SSRF gate → robots gate → fetch → clean (raw HTML never reaches
/// the LLM) → chunk within char budget → one sequential completion per
/// sub-chunk → merge + dedupe → schema-validate → envelope. Portfolio
/// failures map onto existing [`ScraperError`] variants (zero new ones).
pub struct LlmExtractionService {
    http: Arc<dyn HttpClientPort>,
    cleaner: Arc<dyn SemanticCleaner>,
    chunker: Arc<dyn TextChunker>,
    robots: Option<Arc<RobotsFetcher>>,
    llm_port: Option<Arc<dyn LlmPort>>,
}

impl LlmExtractionService {
    /// Assemble the service from its ports. `llm_port` is `None` when no
    /// provider is configured — surfacing as an honest
    /// [`ScraperError::Config`] at `extract` time, never a silent no-op.
    pub fn new(
        http: Arc<dyn HttpClientPort>,
        cleaner: Arc<dyn SemanticCleaner>,
        chunker: Arc<dyn TextChunker>,
        robots: Option<Arc<RobotsFetcher>>,
        llm_port: Option<Arc<dyn LlmPort>>,
    ) -> Self {
        Self {
            http,
            cleaner,
            chunker,
            robots,
            llm_port,
        }
    }

    /// Run the extraction pipeline for one URL against one JSON schema.
    ///
    /// # Errors
    ///
    /// Missing port → `Config`; invalid schema, over-budget chunk, or
    /// schema-violating record → `Validation`; robots denial → `WafBlocked`;
    /// non-2xx page → `Http`; malformed LLM output → `Extraction`.
    pub async fn extract(
        &self,
        url: &Url,
        schema: &Value,
        model: &str,
        config: &LlmConfig,
    ) -> Result<LlmExtraction> {
        let llm = self
            .llm_port
            .clone()
            .ok_or_else(|| ScraperError::Config("no hay proveedor LLM configurado".to_string()))?;

        // [1]-[3] Gates before any I/O.
        validate_schema(schema)
            .map_err(|e| ScraperError::Validation(format!("esquema inválido: {e}")))?;
        ssrf_gate(&config.base_url)?; // [2]
        enforce_robots_policy(url, self.robots.as_deref(), false).await?; // [3] fail-open (#697)

        // [4]-[5] Fetch and clean — raw HTML never reaches the LLM.
        let html = self.fetch_page(url).await?;
        let chunks = self.cleaner.clean(url.as_str(), &html).await?;

        // [6] Chunk within the char budget; an over-budget sub-chunk fails
        // BEFORE any LLM call is sent.
        let char_budget = config.max_tokens.saturating_mul(CHARS_PER_TOKEN);
        let sub_chunks = self.build_sub_chunks(&chunks, char_budget)?;

        // [7]-[8] One completion per sub-chunk (sequential, single attempt),
        // merging records in order with dedupe.
        let mut records: Vec<Value> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let (mut input_tokens, mut output_tokens) = (0_u32, 0_u32);
        for sub_chunk in &sub_chunks {
            let response = llm
                .send_completion(build_request(sub_chunk, schema, model, config.max_tokens))
                .await?;
            input_tokens = input_tokens.saturating_add(response.input_tokens);
            output_tokens = output_tokens.saturating_add(response.output_tokens);
            merge_records(&mut records, &mut seen, response)?;
        }

        // [9] Schema-validate every merged record — never a silent null.
        for record in &records {
            validate_record(record, schema).map_err(|e| {
                ScraperError::Validation(format!("la salida del LLM no cumple el esquema: {e}"))
            })?;
        }

        // [10] Envelope.
        Ok(LlmExtraction {
            url: url.to_string(),
            model: model.to_string(),
            records,
            chunks: sub_chunks.len(),
            input_tokens,
            output_tokens,
        })
    }

    async fn fetch_page(&self, url: &Url) -> Result<String> {
        let response = self.http.get(url.as_str()).await.map_err(|e| {
            crate::application::error_mapping::scraper_error_from_http(e, url.as_str())
        })?;
        if !(200..300).contains(&response.status) {
            return Err(ScraperError::http(response.status, url.as_str()));
        }
        Ok(response.body)
    }

    fn build_sub_chunks(
        &self,
        chunks: &[crate::domain::DocumentChunk],
        char_budget: usize,
    ) -> Result<Vec<String>> {
        let mut sub_chunks = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            for sub_chunk in self.chunker.chunk_text(&chunk.content)? {
                if sub_chunk.len() > char_budget {
                    return Err(ScraperError::Validation(format!(
                        "el fragmento {index} supera el presupuesto ({char_budget} caracteres)"
                    )));
                }
                sub_chunks.push(sub_chunk);
            }
        }
        Ok(sub_chunks)
    }
}

/// Build one completion request: system prompt (schema + instruction) plus
/// the cleaned sub-chunk as user content — never raw HTML.
fn build_request(sub_chunk: &str, schema: &Value, model: &str, max_tokens: usize) -> LlmRequest {
    LlmRequest {
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: format!(
                    "You are a data extraction engine. Extract every record that validates \
                    this JSON schema: {schema}. Respond ONLY with JSON — an object, or an \
                    array of objects conforming to the schema."
                ),
            },
            ChatMessage {
                role: "user".into(),
                content: sub_chunk.to_string(),
            },
        ],
        model: model.to_string(),
        max_tokens,
    }
}

/// Merge one LLM response into `records` — ordered union, first-seen dedupe.
///
/// # Errors
///
/// [`ScraperError::Extraction`] when the content is not valid JSON, or JSON
/// that is neither an object nor an array.
fn merge_records(
    records: &mut Vec<Value>,
    seen: &mut HashSet<String>,
    response: LlmResponse,
) -> Result<()> {
    let parsed: Value = serde_json::from_str(&response.content).map_err(|e| {
        ScraperError::Extraction(format!("la respuesta del LLM no es JSON válido: {e}"))
    })?;
    let batch: Vec<Value> = match parsed {
        Value::Array(items) => items,
        object @ Value::Object(_) => vec![object],
        other => {
            return Err(ScraperError::Extraction(format!(
                "la respuesta del LLM no es un objeto ni un array JSON: {other}"
            )));
        },
    };
    for record in batch {
        if seen.insert(record.to_string()) {
            records.push(record);
        }
    }
    Ok(())
}

/// Configuration for one LLM extraction run.
///
/// `Debug` is safe: [`ApiKey`] prints `[REDACTED]` — the key value never
/// appears in logs or traces.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// OpenAI-compatible endpoint base (`POST {base_url}/chat/completions`).
    pub base_url: Url,
    /// Provider API key — `Authorization: Bearer` only, never logged.
    pub api_key: ApiKey,
    /// Per-completion output token budget; sub-chunk char budget is
    /// `max_tokens * 8` (≈8 chars/token estimate).
    pub max_tokens: usize,
}

impl LlmConfig {
    /// Build the config from the `LLM_API_KEY` environment variable.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] when the base URL is malformed or
    /// `LLM_API_KEY` is missing/empty.
    pub fn from_env(base_url: &str, max_tokens: usize) -> Result<Self> {
        let base_url = Url::parse(base_url)
            .map_err(|e| ScraperError::Config(format!("URL base del LLM inválida: {e}")))?;
        match std::env::var("LLM_API_KEY") {
            Ok(key) if !key.is_empty() => Ok(Self {
                base_url,
                api_key: ApiKey::new(key),
                max_tokens,
            }),
            _ => Err(ScraperError::Config(
                "LLM_API_KEY no está definida: configure la clave del proveedor LLM".into(),
            )),
        }
    }
}

/// SSRF gate for the LLM base URL (#703): scheme allow-list http/https plus
/// literal-host rejection via [`crate::infrastructure::ssrf::is_forbidden_ip`]
/// (loopback / private / link-local / CGNAT / IPv6-ULA). A blocked URL never
/// produces a request.
///
/// Tests against wiremock (127.0.0.1) may set `WEBFANG_DISABLE_SSRF=1`.
/// Hostname-level DNS validation lives at the MCP entry point (slice B,
/// `validate_url_no_ssrf` precedent) alongside the client-side redirect guard.
///
/// # Errors
///
/// Returns [`ScraperError::Config`] when the scheme is not http(s) or the
/// host is a forbidden literal IP.
pub fn ssrf_gate(url: &Url) -> Result<()> {
    if std::env::var("WEBFANG_DISABLE_SSRF").is_ok() {
        return Ok(());
    }
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ScraperError::Config(format!(
            "esquema '{scheme}' no permitido para el LLM (solo http/https)"
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ScraperError::Config("el URL del LLM no tiene host".to_string()))?;
    if crate::infrastructure::ssrf::is_forbidden_literal_host(host) {
        return Err(ScraperError::Config(format!(
            "URL del LLM bloqueada por SSRF: el host '{host}' pertenece a una red interna (#703)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_gate_blocks_forbidden_internal_ips() {
        for host in [
            "127.0.0.1",
            "169.254.169.254",
            "10.0.0.1",
            "100.64.1.1",
            "[fc00::1]",
        ] {
            let url = Url::parse(&format!("http://{host}/v1")).expect("literal parses");
            let err = ssrf_gate(&url).expect_err("forbidden host must be blocked");
            assert!(
                matches!(err, ScraperError::Config(_)),
                "host {host}: {err:?}"
            );
        }
    }

    #[test]
    fn ssrf_gate_allows_public_ip() {
        let url = Url::parse("https://8.8.8.8/v1").expect("literal parses");
        ssrf_gate(&url).expect("public host must pass");
    }

    #[test]
    fn ssrf_gate_rejects_non_http_scheme() {
        let url = Url::parse("ftp://8.8.8.8/v1").expect("parses");
        assert!(matches!(
            ssrf_gate(&url).expect_err("ftp must fail"),
            ScraperError::Config(_)
        ));
    }

    #[test]
    fn ssrf_gate_bypass_env_for_tests() {
        std::env::set_var("WEBFANG_DISABLE_SSRF", "1");
        let url = Url::parse("http://127.0.0.1:9/v1").expect("parses");
        let ok = ssrf_gate(&url).is_ok();
        std::env::remove_var("WEBFANG_DISABLE_SSRF");
        assert!(ok);
    }

    #[test]
    fn from_env_missing_key_is_config_error() {
        std::env::remove_var("LLM_API_KEY");
        let err = LlmConfig::from_env("https://8.8.8.8/v1", 100).expect_err("missing key fails");
        assert!(matches!(err, ScraperError::Config(_)));
    }

    #[test]
    fn from_env_redacts_key_in_debug() {
        std::env::set_var("LLM_API_KEY", "sk-super-secret");
        let cfg = LlmConfig::from_env("https://8.8.8.8/v1", 100).expect("key present");
        std::env::remove_var("LLM_API_KEY");
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("[REDACTED]"), "debug must redact: {dbg}");
        assert!(
            !dbg.contains("sk-super-secret"),
            "debug leaked secret: {dbg}"
        );
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::domain::http_error::HttpResult;
    use crate::domain::http_port::{HttpClientPort, HttpResponse};
    use crate::domain::llm_port::{LlmPort, LlmRequest, LlmResponse};
    use crate::domain::text_chunker::TextChunker;
    use crate::domain::DocumentChunk;
    use crate::error::SemanticError;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    fn extraction_schema() -> Value {
        json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"}}})
    }

    /// max_tokens=100 → char budget 800 (≈8 chars/token).
    fn test_config() -> LlmConfig {
        LlmConfig {
            base_url: Url::parse("https://203.0.113.7/v1").expect("TEST-NET base parses"),
            api_key: ApiKey::new("sk-test"),
            max_tokens: 100,
        }
    }

    fn target_url() -> Url {
        Url::parse("https://e.test/page").expect("target parses")
    }

    fn chunk_with(content: &str) -> DocumentChunk {
        DocumentChunk::new(uuid::Uuid::new_v4(), "https://e.test/page", "Page", content)
    }

    struct FakeHttp {
        status: u16,
        body: String,
    }

    impl HttpClientPort for FakeHttp {
        fn get(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = HttpResult<HttpResponse>> + Send + '_>> {
            let response = HttpResponse {
                status: self.status,
                body: self.body.clone(),
                headers: HashMap::new(),
            };
            Box::pin(async move { Ok(response) })
        }
    }

    struct FakeCleaner {
        chunks: Vec<DocumentChunk>,
    }

    impl crate::domain::semantic_cleaner::private::Sealed for FakeCleaner {}

    impl crate::domain::semantic_cleaner::SemanticCleaner for FakeCleaner {
        fn clean<'a>(
            &'a self,
            _url: &'a str,
            _html: &'a str,
        ) -> Pin<
            Box<
                dyn Future<Output = std::result::Result<Vec<DocumentChunk>, SemanticError>>
                    + Send
                    + 'a,
            >,
        > {
            let chunks = self.chunks.clone();
            Box::pin(async move { Ok(chunks) })
        }

        fn max_tokens(&self) -> usize {
            512
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    struct IdentityChunker;

    impl TextChunker for IdentityChunker {
        fn chunk_text(&self, text: &str) -> std::result::Result<Vec<String>, SemanticError> {
            if text.is_empty() {
                return Err(SemanticError::Tokenize("empty text".into()));
            }
            Ok(vec![text.to_string()])
        }
    }

    /// Counting LLM double: one canned `content` per call ordinal.
    #[derive(Default)]
    struct FakeLlm {
        calls: Mutex<usize>,
        bodies: Mutex<Vec<String>>,
        contents: Vec<String>,
    }

    impl FakeLlm {
        fn new(contents: &[&str]) -> Self {
            Self {
                contents: contents.iter().map(|c| (*c).to_string()).collect(),
                ..Default::default()
            }
        }

        fn call_count(&self) -> usize {
            *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn request_bodies(&self) -> Vec<String> {
            self.bodies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl LlmPort for FakeLlm {
        fn send_completion<'a>(
            &'a self,
            request: LlmRequest,
        ) -> Pin<
            Box<
                dyn Future<Output = std::result::Result<LlmResponse, crate::error::ScraperError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                let index = {
                    let mut calls = self
                        .calls
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *calls += 1;
                    *calls - 1
                };
                self.bodies
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(
                        json!({
                            "messages": request.messages,
                            "model": request.model,
                            "max_tokens": request.max_tokens,
                        })
                        .to_string(),
                    );
                let content = self.contents.get(index).cloned().unwrap_or_default();
                Ok(LlmResponse {
                    content,
                    input_tokens: 11,
                    output_tokens: 7,
                })
            })
        }
    }

    fn service(cleaner_chunks: Vec<DocumentChunk>, llm: Arc<FakeLlm>) -> LlmExtractionService {
        LlmExtractionService::new(
            Arc::new(FakeHttp {
                status: 200,
                body: "<html><body><p>página</p></body></html>".into(),
            }),
            Arc::new(FakeCleaner {
                chunks: cleaner_chunks,
            }),
            Arc::new(IdentityChunker),
            None,
            Some(llm),
        )
    }

    #[tokio::test]
    async fn exactly_one_llm_call_per_sub_chunk() {
        let llm = Arc::new(FakeLlm::new(&[
            r#"[{"name":"a"}]"#,
            r#"[{"name":"b"}]"#,
            r#"[{"name":"c"}]"#,
        ]));
        let svc = service(
            vec![
                chunk_with("Texto uno."),
                chunk_with("Texto dos."),
                chunk_with("Texto tres."),
            ],
            Arc::clone(&llm),
        );

        svc.extract(
            &target_url(),
            &extraction_schema(),
            "test-model",
            &test_config(),
        )
        .await
        .expect("extraction succeeds");
        assert_eq!(llm.call_count(), 3, "cost must be O(chunks)");
    }

    #[tokio::test]
    async fn over_budget_sub_chunk_errors_before_any_llm_call() {
        let llm = Arc::new(FakeLlm::new(&[]));
        let svc = service(vec![chunk_with(&"x".repeat(801))], Arc::clone(&llm));

        let err = svc
            .extract(
                &target_url(),
                &extraction_schema(),
                "test-model",
                &test_config(),
            )
            .await
            .expect_err("801 chars > 800 budget");
        assert!(
            matches!(err, ScraperError::Validation(_)),
            "budget breach must be Validation, got: {err:?}"
        );
        assert_eq!(llm.call_count(), 0, "no LLM call may be sent");
    }

    #[tokio::test]
    async fn request_bodies_never_carry_html_tags() {
        let llm = Arc::new(FakeLlm::new(&[r#"[{"name":"a"}]"#]));
        let svc = service(
            vec![chunk_with("Texto limpio sin etiquetas.")],
            Arc::clone(&llm),
        );

        svc.extract(
            &target_url(),
            &extraction_schema(),
            "test-model",
            &test_config(),
        )
        .await
        .expect("extraction succeeds");
        for body in llm.request_bodies() {
            assert!(!body.contains('<'), "raw HTML reached the LLM: {body}");
        }
    }

    #[tokio::test]
    async fn absent_llm_port_is_config_error() {
        let svc = LlmExtractionService::new(
            Arc::new(FakeHttp {
                status: 200,
                body: String::new(),
            }),
            Arc::new(FakeCleaner { chunks: Vec::new() }),
            Arc::new(IdentityChunker),
            None,
            None,
        );
        let err = svc
            .extract(&target_url(), &extraction_schema(), "m", &test_config())
            .await
            .expect_err("missing port must fail");
        assert!(matches!(err, ScraperError::Config(_)), "got: {err:?}");
    }
}
