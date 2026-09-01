//! OpenAI-compatible wreq adapter for `POST {base_url}/chat/completions` (#789).
//!
//! Wire shape: `response_format = {"type": "json_object"}`, `temperature = 0.0`.
//! Error mapping reuses the existing [`ScraperError`] chain (zero new
//! variants): 429 → `Http{429}` (TransientBackoff), ≥500 → `Http{status}`
//! (TransientRetriable), transport → `Network`, malformed body / missing
//! choices → `Extraction`, `finish_reason == "length"` → `Validation`.

use crate::domain::credentials::ApiKey;
use crate::domain::llm_port::{ChatMessage, LlmPort, LlmRequest, LlmResponse};
use crate::error::{Result, ScraperError};
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;

/// Wire request for OpenAI-compatible `POST /chat/completions`.
#[derive(serde::Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    response_format: serde_json::Value,
    temperature: f32,
    max_tokens: usize,
}

/// Wire response: only the fields this client consumes.
#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// OpenAI-compatible chat/completions adapter (covers OpenAI / Ollama / vLLM).
///
/// The client carries no SSRF logic of its own: `new` applies the
/// domain-owned [`crate::domain::ssrf_guard::SsrfGuard`] port (#703), while
/// the application-layer
/// [`crate::application::llm_extraction::ssrf_gate`] validates the base URL
/// before any request is sent. Unit tests call the client directly against
/// wiremock (loopback) by design.
pub struct OpenAiLlmClient {
    client: wreq::Client,
    base_url: Url,
    api_key: ApiKey,
}

impl OpenAiLlmClient {
    /// Build an adapter for `POST {base_url}/chat/completions`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] if the wreq client cannot be built.
    pub fn new(base_url: Url, api_key: ApiKey) -> Result<Self> {
        let builder = wreq::Client::builder()
            .emulation(wreq_util::Profile::Chrome145)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10));
        // SSRF guard (#703) applied through the domain `SsrfGuard` port:
        // literal-IP redirect guard + connect-time validating resolver that
        // re-validates every DNS answer. Entry-level `ssrf_gate` stays as
        // fast-fail typed UX; this is defense in depth.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            .map_err(|e| ScraperError::Config(format!("no se pudo crear el cliente LLM: {e}")))?;
        Ok(Self {
            client,
            base_url,
            api_key,
        })
    }
}

impl LlmPort for OpenAiLlmClient {
    fn send_completion<'a>(
        &'a self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + 'a>> {
        Box::pin(async move {
            let endpoint = format!(
                "{}/chat/completions",
                self.base_url.as_str().trim_end_matches('/')
            );

            let wire = CompletionRequest {
                model: &request.model,
                messages: &request.messages,
                response_format: json!({ "type": "json_object" }),
                temperature: 0.0,
                max_tokens: request.max_tokens,
            };

            let response = self
                .client
                .post(endpoint)
                .bearer_auth(self.api_key.expose_secret())
                .json(&wire)
                .send()
                .await
                .map_err(ScraperError::from)?;

            let status = response.status().as_u16();
            if !response.status().is_success() {
                return Err(ScraperError::Http {
                    status,
                    url: self.base_url.to_string(),
                });
            }

            let body = response.text().await.map_err(ScraperError::from)?;
            let parsed: CompletionResponse = serde_json::from_str(&body).map_err(|e| {
                ScraperError::Extraction(format!(
                    "el proveedor LLM devolvió un cuerpo inválido: {e}"
                ))
            })?;

            let choice = parsed.choices.first().ok_or_else(|| {
                ScraperError::Extraction("el proveedor LLM no devolvió choices".to_string())
            })?;

            if choice.finish_reason.as_deref() == Some("length") {
                return Err(ScraperError::Validation(
                    "la salida del LLM fue truncada por el límite de tokens (finish_reason=length)"
                        .to_string(),
                ));
            }

            let (input_tokens, output_tokens) = parsed
                .usage
                .map(|u| (u.prompt_tokens, u.completion_tokens))
                .unwrap_or((0, 0));

            Ok(LlmResponse {
                content: choice.message.content.clone(),
                input_tokens,
                output_tokens,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorClass;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const VALID_BODY: &str = r#"{"choices":[{"message":{"content":"{\"items\":[]}"}}],
        "usage":{"prompt_tokens":11,"completion_tokens":7}}"#;

    fn test_request() -> LlmRequest {
        LlmRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "extract".into(),
            }],
            model: "stub-model".into(),
            max_tokens: 64,
        }
    }

    fn client_for(server: &MockServer) -> OpenAiLlmClient {
        OpenAiLlmClient::new(
            Url::parse(&server.uri()).expect("mock uri parses"),
            ApiKey::new("sk-test"),
        )
        .expect("client builds")
    }

    async fn mount(server: &MockServer, status: u16, body: &str) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(server)
            .await;
    }

    /// SSRF choke-point wiring proof (#1060): the client `OpenAiLlmClient::new`
    /// actually builds must carry the guard obtained from the `SsrfGuard` port.
    /// `localhost` resolves to loopback through getaddrinfo with no network
    /// dependency, so the connect attempt must be rejected by the validating
    /// resolver. (The wiremock tests above pass because their base URL is an
    /// IP *literal*, which wreq resolves without consulting a custom resolver.)
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn llm_client_enforces_ssrf_guard_from_the_port() {
        // Env hermeticity (#926): the escape hatch is captured at client-build
        // time, so clearing it must be serialized against siblings that set it.
        // `EnvGuard` holds the shared process-env lock and restores on drop.
        let _env = webfang_test_utils::EnvGuard::clean(&[
            crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
        ]);
        let client = OpenAiLlmClient::new(
            Url::parse("http://localhost:9/").expect("loopback url parses"),
            ApiKey::new("sk-test"),
        )
        .expect("client builds");

        let err = client
            .client
            .get("http://localhost:9/")
            .send()
            .await
            .expect_err("hostname resolving to loopback must fail at connect");
        assert!(
            format!("{err:?}").contains("ForbiddenResolutionError"),
            "failure must come from the SSRF resolver, not the network: {err:?}"
        );
    }

    #[tokio::test]
    async fn valid_json_response_returns_completion() {
        let server = MockServer::start().await;
        mount(&server, 200, VALID_BODY).await;
        let result = client_for(&server)
            .send_completion(test_request())
            .await
            .expect("200 + valid JSON succeeds");
        assert_eq!(result.content, r#"{"items":[]}"#);
        assert_eq!(result.input_tokens, 11);
        assert_eq!(result.output_tokens, 7);
    }

    #[tokio::test]
    async fn malformed_body_maps_to_extraction_error() {
        let server = MockServer::start().await;
        mount(&server, 200, "<html>not json</html>").await;
        let err = client_for(&server)
            .send_completion(test_request())
            .await
            .expect_err("non-JSON body must fail");
        assert!(
            matches!(err, ScraperError::Extraction(_)),
            "malformed body must be Extraction, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn missing_choices_maps_to_extraction_error() {
        let server = MockServer::start().await;
        mount(
            &server,
            200,
            r#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        )
        .await;
        let err = client_for(&server)
            .send_completion(test_request())
            .await
            .expect_err("empty choices must fail");
        assert!(
            matches!(err, ScraperError::Extraction(_)),
            "empty choices must be Extraction, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn rate_limited_maps_to_transient_backoff() {
        let server = MockServer::start().await;
        mount(&server, 429, "{}").await;
        let err = client_for(&server)
            .send_completion(test_request())
            .await
            .expect_err("429 must fail");
        assert!(
            matches!(&err, ScraperError::Http { status: 429, .. }),
            "429 must be Http{{429}}, got: {err:?}"
        );
        assert_eq!(err.classify(), ErrorClass::TransientBackoff);
    }

    #[tokio::test]
    async fn server_error_maps_to_transient_retriable() {
        let server = MockServer::start().await;
        mount(&server, 503, "service down").await;
        let err = client_for(&server)
            .send_completion(test_request())
            .await
            .expect_err("503 must fail");
        assert!(
            matches!(&err, ScraperError::Http { status: 503, .. }),
            "503 must be Http{{503}}, got: {err:?}"
        );
        assert_eq!(err.classify(), ErrorClass::TransientRetriable);
    }

    #[tokio::test]
    async fn length_finish_reason_maps_to_validation_error() {
        let server = MockServer::start().await;
        let body = r#"{"choices":[{"message":{"content":"{\"items\":[]}"},
            "finish_reason":"length"}],"usage":{"prompt_tokens":1,"completion_tokens":64}}"#;
        mount(&server, 200, body).await;
        let err = client_for(&server)
            .send_completion(test_request())
            .await
            .expect_err("truncated output must fail");
        assert!(
            matches!(err, ScraperError::Validation(_)),
            "finish_reason=length must be Validation, got: {err:?}"
        );
    }
}
