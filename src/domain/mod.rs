//! Domain layer — Core business entities (puro, sin frameworks)
//!
//! Following Clean Architecture: no dependencies on infrastructure.
//! This layer contains the business logic that doesn't depend on external frameworks.

pub mod entities;
pub mod value_objects;

pub use entities::{DownloadedAsset, ScrapedContent};
pub use value_objects::ValidUrl;
