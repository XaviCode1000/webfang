//! LLM port — async domain trait for chat completions.
//!
//! Defines the contract for server-side structured extraction against an
//! OpenAI-compatible `chat/completions` provider (#789). Concrete adapters
//! live in `crate::infrastructure::llm`. The trait uses `BoxFuture` for
//! dyn-compatibility, matching [`super::embedding_port`].
//!
//! # Design decisions
//!
//! - **Not sealed**: open for testing (mock clients) and future backends.
//! - **Always compiled**: no `#[cfg(feature = "ai")]` guard. The Container
//!   stores `Option<Arc<dyn LlmPort>>` which is `None` when the `ai`
//!   feature is disabled.

use std::future::Future;
use std::pin::Pin;

use crate::error::ScraperError;

/// A boxed future for dyn-compatible async traits.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A single chat message (`system` / `user` / `assistant`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Message role: `system`, `user`, or `assistant`.
    pub role: String,
    /// Message content — cleaned text only, never raw HTML (#789).
    pub content: String,
}

/// Request for one chat completion call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    /// Chat messages, in order (system prompt first).
    pub messages: Vec<ChatMessage>,
    /// Provider model identifier (e.g. `gpt-4o-mini`).
    pub model: String,
    /// Per-call output token budget for this completion.
    pub max_tokens: usize,
}

/// Result of one chat completion call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmResponse {
    /// Assistant message content (JSON text when `response_format=json_object`).
    pub content: String,
    /// Prompt tokens billed by the provider.
    pub input_tokens: u32,
    /// Completion tokens billed by the provider.
    pub output_tokens: u32,
}

/// Domain trait for LLM chat completions (structured extraction, #789).
///
/// # Errors
///
/// Returns [`ScraperError`] when:
/// - [`ScraperError::Http`]: provider returned a non-2xx status
/// - [`ScraperError::Network`]: transport failure
/// - [`ScraperError::Extraction`]: malformed or missing completion body
/// - [`ScraperError::Validation`]: output truncated (`finish_reason == "length"`)
pub trait LlmPort: Send + Sync {
    /// Send one completion request and return the assistant response.
    ///
    /// Adapters POST to `{base_url}/chat/completions` with
    /// `response_format = {"type": "json_object"}`.
    fn send_completion<'a>(
        &'a self,
        request: LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, ScraperError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic mock returning a fixed JSON completion.
    struct FakeLlm;

    impl LlmPort for FakeLlm {
        fn send_completion<'a>(
            &'a self,
            request: LlmRequest,
        ) -> BoxFuture<'a, Result<LlmResponse, ScraperError>> {
            Box::pin(async move {
                Ok(LlmResponse {
                    content: format!("{{\"model\":\"{}\"}}", request.model),
                    input_tokens: 1,
                    output_tokens: 2,
                })
            })
        }
    }

    #[tokio::test]
    async fn test_send_completion_roundtrip() {
        let llm = FakeLlm;
        let resp = llm
            .send_completion(LlmRequest {
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: "extract".to_string(),
                }],
                model: "stub-model".to_string(),
                max_tokens: 100,
            })
            .await
            .expect("fake completion succeeds");
        assert_eq!(resp.content, r#"{"model":"stub-model"}"#);
        assert_eq!(resp.input_tokens, 1);
        assert_eq!(resp.output_tokens, 2);
    }

    #[test]
    fn test_llm_port_is_object_safe() {
        fn assert_dyn_compatible(_: &dyn LlmPort) {}
        let llm = FakeLlm;
        assert_dyn_compatible(&llm);
    }
}
