//! LLM infrastructure — OpenAI-compatible chat/completions adapter (#789)
//! plus the opt-in remote embeddings adapter (#1462).

pub mod client;
pub mod provider;
pub mod remote_embedding;
pub mod validation;
