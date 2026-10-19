//! Model-profile contract (ADR-0004 §"Perfiles de modelo", unit A3).
//!
//! These tests are the *acceptance surface* of the profile unit, and they are
//! deliberately written against the public API only:
//!
//! - a profile resolves by `id` with repo defaults layered under the user's
//!   config-file overrides, and an unknown `id` fails with a **Spanish** error;
//! - `schema_version` is always present, and a profile declaring a version this
//!   build does not understand is rejected instead of being used silently;
//! - the two Granite profiles carry the values this repo can actually verify
//!   (the ADR-0004 profile authority, and the budget constants in
//!   `options_spec/ai.rs`), with the citation attached, and the native-768 vs
//!   effective-384 Matryoshka distinction is modelled rather than flattened;
//! - `dimensions` (the OpenAI-compatible request field) is omitted for a
//!   non-Matryoshka profile — vLLM rejects it with a 400 — and carries the
//!   effective dimension for a Matryoshka one;
//! - no value may come from models.dev.

use webfang_core::domain::model_profile::{
    builtin_profiles, ModelProfileOverride, ModelProfileRegistry, ModelProfilesConfig,
    ProfileError, ProfileProvenance, GRANITE_311M_PROFILE_ID, GRANITE_97M_PROFILE_ID,
    MATRYOSHKA_OUTPUT_DIM, PROFILE_SCHEMA_VERSION,
};

/// Registry with no user overrides — the shipped defaults.
fn defaults() -> ModelProfileRegistry {
    ModelProfileRegistry::new(ModelProfilesConfig::default())
}

/// Registry with one user override, as the config file would carry it.
fn with_override(override_: ModelProfileOverride) -> ModelProfileRegistry {
    ModelProfileRegistry::new(ModelProfilesConfig {
        profiles: vec![override_],
    })
}

// ---------------------------------------------------------------------------
// Repo-verified Granite values
// ---------------------------------------------------------------------------

#[test]
fn granite_97m_carries_the_values_verifiable_in_this_repo() {
    let profile = defaults()
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("granite-97m is a builtin profile");

    assert_eq!(profile.id, "granite-97m");
    // The ADR-0004 profile authority — Granite97M => 384, no truncation.
    assert_eq!(profile.dim, 384);
    assert!(!profile.matryoshka);
    // The same authority — output_dim() => 384 for both tiers.
    assert_eq!(profile.output_dim(), 384);
    // options_spec/ai.rs:158 — the 32 768-token sequence length.
    assert_eq!(profile.context, 32_768);
}

#[test]
fn granite_311m_keeps_native_768_distinct_from_effective_384() {
    let profile = defaults()
        .resolve(GRANITE_311M_PROFILE_ID)
        .expect("granite-311m is a builtin profile");

    // The distinction must be modelled, not flattened: `dim` is the model's
    // native width, `output_dim` is what the vault stores.
    assert_eq!(
        profile.dim, 768,
        "the ADR-0004 profile authority — Granite311M => 768"
    );
    assert!(profile.matryoshka, "the same authority — truncated to 384d");
    assert_eq!(profile.output_dim(), MATRYOSHKA_OUTPUT_DIM);
    assert_eq!(profile.output_dim(), 384);
    assert_eq!(profile.context, 32_768);
}

#[test]
fn every_builtin_profile_cites_its_repo_provenance_and_is_verified() {
    let profiles = builtin_profiles();
    assert_eq!(
        profiles.len(),
        2,
        "only the two repo-verified Granite tiers"
    );

    for profile in &profiles {
        assert_eq!(
            profile.schema_version, PROFILE_SCHEMA_VERSION,
            "{} must pin the schema version explicitly",
            profile.id
        );
        assert!(
            profile.verified,
            "{} is repo-verified and must not claim otherwise",
            profile.id
        );
        assert_eq!(profile.provenance, ProfileProvenance::RepoVerified);
        assert!(
            profile
                .citation
                .contains("docs/adr/0004-onnx-local-removal.md"),
            "{} must cite the repo source of its dimension, got: {}",
            profile.id,
            profile.citation
        );
        assert!(
            !profile.citation.to_lowercase().contains("models.dev"),
            "models.dev is not a source for `dim` (limit.output != dimension); \
             got: {}",
            profile.citation
        );
    }
}

/// A citation that names the repo is not enough: it must name **the symbol it
/// pins**, or it can drift onto a neighbouring doc comment without anything
/// failing.
///
/// Regression pin (history): the shipped citation used to say
/// `cache_config.rs:32,33,35,73-76`, and `:73-76` is the doc comment above
/// `embedding_dim` — the literals were at `:80`, `:81` and `:88`. The old
/// assertion (`contains("cache_config.rs")`) could not tell those two apart,
/// so a comment-only citation passed.
///
/// What this measures NOW is the same property, on the authority that
/// survives ADR-0004 paso 6: the citation is *self-locating* — it names the
/// symbols whose literals back `dim` and `output_dim`, and the recovery tag
/// where those literals live — AND it does not point into the deleted crate,
/// which would be a dead link in the very output the citation exists to make
/// auditable.
#[test]
fn the_dimension_citation_names_the_symbols_and_not_a_deleted_path() {
    for profile in builtin_profiles() {
        assert!(
            profile.citation.contains("AiModel::embedding_dim"),
            "{} must cite the function whose literals back `dim`, got: {}",
            profile.id,
            profile.citation
        );
        assert!(
            profile.citation.contains("AiModel::output_dim"),
            "{} must cite the function whose literal backs `output_dim`, got: {}",
            profile.id,
            profile.citation
        );
        assert!(
            profile.citation.contains("onnx-tree/pre-step5"),
            "{} must name the tag where the deleted literals are recoverable, \
             got: {}",
            profile.id,
            profile.citation
        );
        assert!(
            !profile.citation.contains("crates/webfang_ai"),
            "the citation must not point into the deleted crate — a dead path \
             cannot be audited; got: {}",
            profile.citation
        );
    }
}

/// The verified anchors, pinned so a future edit cannot move the citation
/// silently.
///
/// Verified by hand against the two files this repo still owns:
///
/// ```text
/// docs/adr/0004-onnx-local-removal.md:87-89                     §Perfiles de modelo
/// crates/webfang_core/src/domain/options_spec/ai.rs:148         DEFAULT_CHARS_PER_TOKEN = 3.0
/// crates/webfang_core/src/domain/options_spec/ai.rs:158         DEFAULT_MAX_CHARS = 32_768 * 3
/// ```
///
/// plus the recovery tag `onnx-tree/pre-step5` (commit `38ab6a31`), which
/// `git rev-parse onnx-tree/pre-step5` resolves to the last `main` commit
/// with the local inference stack intact.
///
/// What this deliberately does **not** do: read the deleted implementation and
/// check that each cited line really contains the digit the profile claims.
/// That file no longer exists in this tree, so the check could only run
/// against a tag — and its natural "fix" (deleting the assertion) would
/// quietly remove the coverage. Anchor-to-literal correspondence stays one
/// `sed -n` away, done by hand on whichever commit changes a citation; these
/// tests guard the weaker, durable invariant instead of pretending to the
/// stronger one.
#[test]
fn citation_keeps_the_hand_verified_line_numbers() {
    let citation = &builtin_profiles()[0].citation;
    for fragment in [
        "docs/adr/0004-onnx-local-removal.md:87-89",
        "onnx-tree/pre-step5",
        "38ab6a31",
        "options_spec/ai.rs:158",
        "options_spec/ai.rs:148",
    ] {
        assert!(
            citation.contains(fragment),
            "citation must keep the verified fragment `{fragment}`, got: {citation}"
        );
    }
}

#[test]
fn builtin_ids_are_exactly_the_documented_pair() {
    assert_eq!(
        defaults().ids(),
        vec![GRANITE_97M_PROFILE_ID, GRANITE_311M_PROFILE_ID]
    );
}

// ---------------------------------------------------------------------------
// `dimensions` — omitted unless Matryoshka (ADR defensive rule)
// ---------------------------------------------------------------------------

#[test]
fn dimensions_is_omitted_for_a_non_matryoshka_profile() {
    let profile = defaults()
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("granite-97m is a builtin profile");

    // vLLM rejects an explicit `dimensions` on a non-Matryoshka model with a
    // 400, so the field must be absent from the request — not 384.
    assert_eq!(profile.dimensions(), None);
    // The dimension is still known, it is just not *sent*.
    assert_eq!(profile.dim, 384);
}

#[test]
fn dimensions_is_the_effective_dim_for_a_matryoshka_profile() {
    let profile = defaults()
        .resolve(GRANITE_311M_PROFILE_ID)
        .expect("granite-311m is a builtin profile");

    assert_eq!(
        profile.dimensions(),
        Some(MATRYOSHKA_OUTPUT_DIM),
        "a Matryoshka profile asks the runtime to truncate 768 → 384"
    );
    assert_ne!(
        profile.dimensions(),
        Some(profile.dim),
        "the requested dimension is the truncated one, never the native one"
    );
}

#[test]
fn dimensions_follows_the_override_not_the_builtin() {
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_311M_PROFILE_ID.to_string(),
        matryoshka: Some(false),
        ..ModelProfileOverride::default()
    });
    let profile = registry
        .resolve(GRANITE_311M_PROFILE_ID)
        .expect("override patches a known profile");

    assert_eq!(profile.dimensions(), None);
    assert_eq!(profile.output_dim(), profile.dim);
}

// ---------------------------------------------------------------------------
// Resolution: defaults + overrides, Spanish failures
// ---------------------------------------------------------------------------

#[test]
fn resolve_applies_the_user_override_over_the_repo_default() {
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_97M_PROFILE_ID.to_string(),
        chars_per_token: Some(4.0),
        ..ModelProfileOverride::default()
    });
    let profile = registry
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("override patches a known profile");

    assert_eq!(profile.chars_per_token, 4.0);
    assert_eq!(profile.dim, 384, "untouched fields keep the repo default");
    assert_eq!(profile.context, 32_768);
}

#[test]
fn max_chars_is_derived_from_context_and_chars_per_token() {
    let profile = defaults()
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("granite-97m is a builtin profile");

    // options_spec/ai.rs:113 — DEFAULT_CHARS_PER_TOKEN = 3.0.
    assert_eq!(profile.chars_per_token, 3.0);
    assert_eq!(profile.max_chars(), 98_304);
}

#[test]
fn resolve_unknown_id_fails_with_a_spanish_error() {
    let err = defaults()
        .resolve("mistral-7b")
        .expect_err("an id outside the repo-verified set must not resolve");

    assert!(matches!(err, ProfileError::ProfileNotFound { .. }), "{err}");
    let message = err.to_string();
    assert!(
        message.contains("desconocido"),
        "user-facing error must be Spanish, got: {message}"
    );
    assert!(
        message.contains("granite-97m") && message.contains("granite-311m"),
        "the error must list the available profiles, got: {message}"
    );
}

#[test]
fn an_override_cannot_invent_a_new_profile() {
    let registry = with_override(ModelProfileOverride {
        id: "mistral-7b".to_string(),
        dim: Some(1024),
        ..ModelProfileOverride::default()
    });

    let err = registry
        .resolve("mistral-7b")
        .expect_err("no value may be invented by an override");
    assert!(matches!(err, ProfileError::ProfileNotFound { .. }), "{err}");
}

#[test]
fn override_of_an_unknown_id_is_rejected_even_for_a_known_lookup() {
    // The override names an id that does not exist; resolving a *different*
    // id still works, and the dangling override never silently becomes a
    // profile.
    let registry = with_override(ModelProfileOverride {
        id: "text-embedding-3-small".to_string(),
        dim: Some(1536),
        ..ModelProfileOverride::default()
    });

    assert!(registry
        .resolve("text-embedding-3-small")
        .is_err_and(|err| matches!(err, ProfileError::ProfileNotFound { .. })));
    assert!(registry.resolve(GRANITE_97M_PROFILE_ID).is_ok());
}

#[test]
fn invalid_chars_per_token_fails_with_a_spanish_error() {
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_97M_PROFILE_ID.to_string(),
        chars_per_token: Some(0.0),
        ..ModelProfileOverride::default()
    });

    let err = registry
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect_err("a zero ratio would reject every chunk");
    assert!(matches!(err, ProfileError::InvalidField { .. }), "{err}");
    let message = err.to_string();
    assert!(
        message.contains("chars_per_token") && message.contains("inválido"),
        "the error must name the offending field, in Spanish, got: {message}"
    );
}

#[test]
fn zero_dim_and_zero_context_are_rejected() {
    for override_ in [
        ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            dim: Some(0),
            ..ModelProfileOverride::default()
        },
        ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            context: Some(0),
            ..ModelProfileOverride::default()
        },
        ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            batch_max: Some(0),
            ..ModelProfileOverride::default()
        },
    ] {
        let registry = with_override(override_);
        let err = registry
            .resolve(GRANITE_97M_PROFILE_ID)
            .expect_err("a zero-valued field is never a usable profile");
        assert!(matches!(err, ProfileError::InvalidField { .. }), "{err}");
    }
}

// ---------------------------------------------------------------------------
// schema_version — pinned, and never silently tolerated
// ---------------------------------------------------------------------------

#[test]
fn schema_version_is_explicit_on_every_builtin() {
    assert_eq!(PROFILE_SCHEMA_VERSION, 1);
    for profile in builtin_profiles() {
        assert_eq!(profile.schema_version, PROFILE_SCHEMA_VERSION);
    }
}

#[test]
fn a_profile_with_an_unknown_schema_version_is_not_used_silently() {
    for declared in [0_u32, 2, 99] {
        let registry = with_override(ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            schema_version: Some(declared),
            ..ModelProfileOverride::default()
        });

        let err = registry
            .resolve(GRANITE_97M_PROFILE_ID)
            .expect_err("an unknown schema version must never be served");
        match &err {
            ProfileError::UnknownSchemaVersion {
                found, expected, ..
            } => {
                assert_eq!(*found, declared);
                assert_eq!(*expected, PROFILE_SCHEMA_VERSION);
            },
            other => panic!("expected UnknownSchemaVersion, got: {other}"),
        }
        assert!(
            err.to_string().contains("schema_version"),
            "the error must name the schema version, got: {err}"
        );
    }
}

#[test]
fn a_matching_schema_version_resolves_normally() {
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_97M_PROFILE_ID.to_string(),
        schema_version: Some(PROFILE_SCHEMA_VERSION),
        ..ModelProfileOverride::default()
    });
    assert!(registry.resolve(GRANITE_97M_PROFILE_ID).is_ok());
}

// ---------------------------------------------------------------------------
// Provenance: an override that changes the audited values loses its seal
// ---------------------------------------------------------------------------

#[test]
fn overriding_the_dimension_drops_verification() {
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_97M_PROFILE_ID.to_string(),
        dim: Some(512),
        ..ModelProfileOverride::default()
    });
    let profile = registry
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("override patches a known profile");

    assert_eq!(profile.dim, 512);
    assert!(!profile.verified, "512d is not a value this repo verifies");
    assert_eq!(profile.provenance, ProfileProvenance::OperatorOverride);
    assert!(
        profile
            .citation
            .contains("docs/adr/0004-onnx-local-removal.md"),
        "the original citation is kept for traceability, got: {}",
        profile.citation
    );
}

#[test]
fn tuning_the_budget_keeps_verification() {
    // `chars_per_token`, `batch_max`, `context` and the prefixes are operator
    // budgets, not the audited model identity.
    let registry = with_override(ModelProfileOverride {
        id: GRANITE_97M_PROFILE_ID.to_string(),
        chars_per_token: Some(2.5),
        batch_max: Some(16),
        query_prefix: Some("query: ".to_string()),
        ..ModelProfileOverride::default()
    });
    let profile = registry
        .resolve(GRANITE_97M_PROFILE_ID)
        .expect("override patches a known profile");

    assert!(profile.verified);
    assert_eq!(profile.provenance, ProfileProvenance::RepoVerified);
    assert_eq!(profile.batch_max, Some(16));
    assert_eq!(profile.query_prefix.as_deref(), Some("query: "));
}

// ---------------------------------------------------------------------------
// Config-file surface (user overrides land in the user's config file)
// ---------------------------------------------------------------------------

#[test]
fn overrides_survive_the_config_file_round_trip() {
    let json = r#"{
        "profiles": [
            {
                "id": "granite-311m",
                "batch_max": 32,
                "chars_per_token": 4.0,
                "schema_version": 1
            }
        ]
    }"#;
    let parsed: ModelProfilesConfig = serde_json::from_str(json).expect("override block parses");

    let registry = ModelProfileRegistry::new(parsed);
    let profile = registry
        .resolve(GRANITE_311M_PROFILE_ID)
        .expect("override from the config file applies");
    assert_eq!(profile.batch_max, Some(32));
    assert_eq!(profile.chars_per_token, 4.0);
    assert_eq!(profile.dim, 768, "the repo default survives the round trip");
    assert!(profile.verified);
}

#[test]
fn an_absent_override_block_resolves_to_the_shipped_defaults() {
    let parsed: ModelProfilesConfig = serde_json::from_str("{}").expect("empty block parses");
    assert!(parsed.is_empty());

    let registry = ModelProfileRegistry::new(parsed);
    assert_eq!(
        registry
            .resolve(GRANITE_97M_PROFILE_ID)
            .expect("builtin")
            .dim,
        384
    );
}

#[test]
fn all_resolves_every_profile_or_fails_loudly() {
    let profiles = defaults().all().expect("shipped defaults are valid");
    assert_eq!(profiles.len(), 2);

    let broken = ModelProfileRegistry::new(ModelProfilesConfig {
        profiles: vec![ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            dim: Some(0),
            ..ModelProfileOverride::default()
        }],
    });
    assert!(
        broken.all().is_err(),
        "`all` must not drop a broken profile silently"
    );
}
