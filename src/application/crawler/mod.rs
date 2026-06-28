//! Crawler module — crawling orchestration and result collection
//!
//! This module contains the crawler service and its supporting components.

pub mod collector;

pub use collector::{CrawlMessage, ResultsAdapter, ResultsCollector};
