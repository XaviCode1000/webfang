//! Scraping implementations
//!
//! Contains the actual scraping logic:
//! - Readability algorithm wrapper
//! - Fallback text extraction
//!
//! DOM pre-pruning and author extraction moved to `domain::scraper_port`
//! (ADR-0012 sub-slice 3.D).
//!
//! Selector repair lives here on both tiers: Tier 1 is lexical
//! (`dom_inspector::DefaultDomInspector`), Tier 2 is embedding-backed
//! (`embedding_semantic_inspector::EmbeddingSemanticInspector`, ADR-0004).

pub mod dom_inspector;
pub mod embedding_semantic_inspector;
pub mod fallback;
pub mod readability;
