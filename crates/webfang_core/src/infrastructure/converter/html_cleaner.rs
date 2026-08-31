//! HTML boilerplate removal before Markdown conversion.
//!
//! This module is a **forwarding shim**. The single owner of `clean_html` is
//! [`crate::domain::html_cleaner`]; this path only re-exports it so the
//! historical `infrastructure::converter::html_cleaner` imports keep resolving
//! for existing consumers (`content_processing`, `converter::html_to_markdown`,
//! `webfang_mcp`, benches and the core test suite).
//!
//! It exists because the two modules were once independent copies of the same
//! algorithm, and the copy here had drifted: it emitted two `tracing` calls the
//! domain copy did not. That drift was invisible to the test suite, since no
//! assertion covered those log lines. Collapsing them removes the class of bug
//! (#1056).
//!
//! New code should import from [`crate::domain::html_cleaner`] directly.
//! `infrastructure → domain` is an inward dependency, so this is legal under
//! ADR-0010.

pub use crate::domain::html_cleaner::clean_html;
