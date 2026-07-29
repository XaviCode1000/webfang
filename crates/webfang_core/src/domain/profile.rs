//! TLS/H2 profile name resolution.
//!
//! Single source of truth for mapping user-facing profile names (e.g. the
//! `--h2-profile` CLI value) onto [`wreq_util::Profile`]. Both the strict
//! page-fetch path ([`HttpClientConfig::profile_from_name`](super::HttpClientConfig::profile_from_name))
//! and the best-effort asset path (`cli::orchestrator::parse_asset_h2_profile`)
//! delegate here.

use wreq_util::Profile;

/// Resolve a profile name to a [`Profile`].
///
/// Accepts the exact variant name of the [`Profile`] enum (e.g. `"Chrome120"`,
/// `"SafariIos18_1_1"`). Matching is case-sensitive and covers the full catalog:
/// the lookup is driven by [`Profile::VARIANTS`], so profiles added by future
/// `wreq-util` upgrades are accepted automatically.
///
/// Returns `None` for unknown names; each caller decides its failure policy
/// (hard error for page-fetch, warn + fallback for assets).
#[must_use]
pub fn profile_from_name(name: &str) -> Option<Profile> {
    Profile::VARIANTS
        .iter()
        .copied()
        .find(|profile| profile_name(*profile) == name)
}

/// All valid profile names, for user-facing error messages.
#[must_use]
pub fn valid_profile_names() -> Vec<String> {
    Profile::VARIANTS
        .iter()
        .copied()
        .map(profile_name)
        .collect()
}

/// The user-facing name of a profile (its exact enum variant name).
fn profile_name(profile: Profile) -> String {
    format!("{profile:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips_through_its_debug_name() {
        for profile in Profile::VARIANTS {
            let name = format!("{profile:?}");
            assert_eq!(
                profile_from_name(&name),
                Some(*profile),
                "Debug name '{name}' must resolve back to its variant"
            );
        }
    }

    #[test]
    fn resolves_known_profiles_across_families() {
        assert_eq!(profile_from_name("Chrome120"), Some(Profile::Chrome120));
        assert_eq!(profile_from_name("Chrome131"), Some(Profile::Chrome131));
        assert_eq!(profile_from_name("Chrome145"), Some(Profile::Chrome145));
        assert_eq!(profile_from_name("Firefox135"), Some(Profile::Firefox135));
        assert_eq!(
            profile_from_name("SafariIos18_1_1"),
            Some(Profile::SafariIos18_1_1)
        );
        assert_eq!(profile_from_name("OkHttp5"), Some(Profile::OkHttp5));
    }

    #[test]
    fn unknown_names_return_none() {
        assert_eq!(profile_from_name("NetscapeNavigator"), None);
        assert_eq!(profile_from_name(""), None);
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(profile_from_name("chrome120"), None);
    }

    #[test]
    fn valid_profile_names_covers_full_catalog() {
        let names = valid_profile_names();
        assert_eq!(names.len(), Profile::VARIANTS.len());
        assert!(names.contains(&"Chrome145".to_owned()));
    }
}
