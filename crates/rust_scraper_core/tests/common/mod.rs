//! Shared test fixtures and helpers for rust_scraper integration tests.
//!
//! Provides reusable test data generators, mock servers, and temporary
//! directory helpers. Consumed by integration tests across the workspace.
//!
//! # Usage
//!
//! ```ignore
//! mod common;
//! use common::{sample_html, sample_sitemap, TestHttpServer};
//! ```

pub mod fixtures;

pub use fixtures::*;
