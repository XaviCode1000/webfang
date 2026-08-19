//! LLM port — async domain trait for chat completions (#789).
//!
//! OpenAI-compatible contract; concrete adapters live in
//! [`crate::infrastructure::llm`]. `BoxFuture` for dyn-compatibility,
//! matching [`super::embedding_port`]. Always compiled (no feature guard);
//! the Container stores `Option<Arc<dyn LlmPort>>` (`None` when `ai` off).

use std::future::Future;
use std::pin::Pin;

use crate::error::ScraperError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A single chat message (`system` / `user` / `assistant`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChatMessage {
    /// Message role.
    pub role: String,
    /// Message content — cleaned text only, never raw HTML (#789).
    pub content: String,
}

/// Request for one chat completion call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    /// Chat messages, in order (system prompt first).
    pub messages: Vec<ChatMessage>,
    /// Provider model identifier.
    pub model: String,
    /// Per-call output token budget.
    pub max_tokens: usize,
}

/// Result of one chat completion call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmResponse {
    /// Assistant message content.
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
/// [`ScraperError::Http`] non-2xx, [`ScraperError::Network`] transport,
/// [`ScraperError::Extraction`] malformed body, [`ScraperError::Validation`]
/// truncated output (`finish_reason == "length"`).
pub trait LlmPort: Send + Sync {
    /// Send one completion request (`response_format=json_object`).
    fn send_completion<'a>(
        &'a self,
        request: LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, ScraperError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLlm;

    impl LlmPort for FakeLlm {
        fn send_completion<'a>(
            &'a self,
            request: LlmRequest,
        ) -> BoxFuture<'a, Result<LlmResponse, ScraperError>> {
            Box::pin(async move {
                Ok(LlmResponse {
                    content: format!("{{\"m\":\"{}\"}}", request.model),
                    input_tokens: 1,
                    output_tokens: 2,
                })
            })
        }
    }

    #[tokio::test]
    async fn test_send_completion_roundtrip() {
        let resp = FakeLlm
            .send_completion(LlmRequest {
                messages: vec![],
                model: "stub".into(),
                max_tokens: 10,
            })
            .await
            .expect("fake succeeds");
        assert_eq!(resp.content, r#"{"m":"stub"}"#);
        assert_eq!((resp.input_tokens, resp.output_tokens), (1, 2));
    }

    #[test]
    fn test_llm_port_is_object_safe() {
        fn assert_dyn(_: &dyn LlmPort) {}
        assert_dyn(&FakeLlm);
    }
}
