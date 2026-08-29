//! Shim — `CookieBridge` moved to [`crate::domain::cookie_bridge`] in ADR-0012
//! sub-slice 3.B-1a.
//!
//! The cookie jar is pure domain logic (it only touches the `Cookie` and
//! `FetchedPage` DTOs and has no transport or IO dependency), so its canonical
//! home is `domain`. This module survives only to keep the historical
//! `infrastructure::downloader::cookie_bridge` path resolving for callers that
//! have not been repointed yet — notably the public
//! `webfang_core::infrastructure::downloader::cookie_bridge::CookieBridge` path
//! used by `webfang_benchmark`, and the `super::cookie_bridge` references from
//! the sibling downloader modules.
//!
//! New code should import from [`crate::domain::cookie_bridge`] directly. This
//! shim can be deleted once the remaining external consumers are repointed.

pub use crate::domain::cookie_bridge::{CdpCookie, CookieBridge};

// Consumed by `chromiumoxide_downloader` via `super::cookie_bridge::domain_matches`.
pub(crate) use crate::domain::cookie_bridge::domain_matches;
