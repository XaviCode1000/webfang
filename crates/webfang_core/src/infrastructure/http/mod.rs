//! HTTP client infrastructure
//!
//! Re-exports the application layer HTTP client creation.
//! This module exists for architectural consistency.

pub use crate::application::http_client::create_http_client;

pub mod waf_engine;
