//! Checkpoint persistence for crawl state.
//!
//! Saves crawl progress (visited URLs, queue, page count) to disk using
//! JSON serialization via jzon-rs. Enables resuming interrupted crawls.

pub mod store;

pub use store::BincodeCheckpoint;
