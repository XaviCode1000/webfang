//! Infrastructure layer — External implementations (HTTP, FS, converters)
//!
//! This layer contains the technical implementations of external concerns:
//! - HTTP client creation
//! - Web scraping (Readability, fallback)
//! - Content conversion (HTML to Markdown, syntax highlighting)
//! - File I/O (saving results, frontmatter generation)
//!
//! Following Clean Architecture: infrastructure depends on domain, not vice versa.

pub mod converter;
pub mod http;
pub mod output;
pub mod scraper;
