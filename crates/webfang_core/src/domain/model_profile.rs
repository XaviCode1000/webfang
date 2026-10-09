//! Versioned model profiles (ADR-0004 §"Perfiles de modelo").
//!
//! Domain-only: a declarative record plus pure resolution. No I/O, no HTTP,
//! no client wiring — the HTTP payload is built by the adapter that owns the
//! request ([`infrastructure::llm`](crate::domain)), and it reads
//! [`ModelProfile::dimensions`] from here.
//!
//! # Why own profiles and not models.dev
//!
//! `models.dev` publishes `limit.output`, which is **not** an embedding
//! dimension (it declares `3072` for Mistral where the real vector is `1024`),
//! it carries no batch limit, no query/passage prefixes and no Granite-embedding
//! entries at all. A profile built from it therefore lies silently. The
//! ADR's answer is our own versioned records, shipped in the repo next to
//! [`ProvidersConfig`](crate::domain::providers::ProvidersConfig) and adjusted
//! per operator in the user's config file.
//!
//! # What "verified" means here
//!
//! Every shipped profile is [`ProfileProvenance::RepoVerified`] and carries a
//! [`ModelProfile::citation`] pointing at the file in this repo that pins its
//! numbers. An override that changes the **audited identity** of the model
//! (`dim`, `matryoshka`) drops the profile to
//! [`ProfileProvenance::OperatorOverride`] / `verified: false` while keeping
//! the original citation for traceability. Budget fields (`batch_max`,
//! `chars_per_token`, `context`, the prefixes) are operator choices and do not
//! cost the profile its verification.
//!
//! Overrides only ever *patch* a shipped profile. An override naming an
//! unknown `id` cannot introduce a model, because every number of a new
//! profile would then be an invention — adding a model is a repo-side,
//! `schema_version`-pinned change.
//!
//! # Schema
//!
//! ```text
//! id → { schema_version, dim, batch_max, query_prefix, passage_prefix,
//!        context, chars_per_token, matryoshka }
//! ```
//!
//! plus the provenance triple ([`ModelProfile::verified`],
//! [`ModelProfile::provenance`], [`ModelProfile::citation`]) that the ADR's
//! "no silent values" rule requires. The shipped schema version is
//! [`PROFILE_SCHEMA_VERSION`]; the PR that changes this shape bumps it and
//! migrates the existing profiles in the same commit.

use serde::{Deserialize, Serialize};

/// Schema version of the profile records this build understands.
///
/// Pinned by ADR-0004 §"Perfiles de modelo" (Tramo D). A profile carrying any
/// other version is rejected rather than reinterpreted — a profile is a
/// record whose meaning a future build may change, so guessing is the one
/// thing that must not happen.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Unified embedding dimension of the vault schema.
///
/// Both Granite tiers produce 384-dimensional vectors after Matryoshka
/// truncation (`cache_config.rs:35` — "Both produce 384-dimensional embeddings
/// for unified storage schema", and `AiModel::output_dim()`), so 384 is the
/// only effective width this repo currently stores. It is also the target a
/// Matryoshka profile asks the remote runtime to truncate to.
pub const MATRYOSHKA_OUTPUT_DIM: usize = 384;

/// Id of the default 97M-parameter Granite embedding profile.
pub const GRANITE_97M_PROFILE_ID: &str = "granite-97m";

/// Id of the 311M-parameter Granite embedding profile (higher precision).
pub const GRANITE_311M_PROFILE_ID: &str = "granite-311m";

/// Repo source that pins the native dimensions and the Matryoshka behaviour of
/// both Granite tiers.
///
/// It cites the **literals**, not the doc comments above them: `embedding_dim`
/// returns `384` / `768` at `:80`/`:81`, and `output_dim` returns `384` at
/// `:88`. The function names are part of the citation on purpose — a citation
/// that names the symbol it pins cannot silently drift onto the neighbouring
/// comment when the file is edited, which is the one failure a provenance
/// record cannot afford.
///
/// It lives in the `webfang_ai` crate today; ADR-0004 step 6 deletes that
/// crate, at which point these literals become the primary source and the
/// citation migrates with them.
const CITATION_CACHE_CONFIG: &str =
    "crates/webfang_ai/src/infrastructure_ai/cache_config.rs:80,81 (AiModel::embedding_dim), :88 \
     (AiModel::output_dim)";

/// Repo source that pins the 32 768-token sequence length
/// (`webfang_ai::infrastructure_ai::tokenizer::DEFAULT_MAX_LENGTH` at `:40`).
const CITATION_TOKENIZER: &str = "crates/webfang_ai/src/infrastructure_ai/tokenizer.rs:40";

/// Repo source that pins the default characters-per-token ratio
/// (`domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN` at `:113`).
const CITATION_CHARS_PER_TOKEN: &str = "crates/webfang_core/src/domain/options_spec/ai.rs:113";

/// Ids of the shipped profiles, in declaration order.
const BUILTIN_IDS: [&str; 2] = [GRANITE_97M_PROFILE_ID, GRANITE_311M_PROFILE_ID];

/// Where a profile's numbers come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileProvenance {
    /// Every number is pinned by a file in this repo, cited in
    /// [`ModelProfile::citation`].
    RepoVerified,
    /// The operator changed the audited identity (`dim` / `matryoshka`) in
    /// their config file, so the profile is no longer repo-verified.
    OperatorOverride,
}

/// Failure modes of [`ModelProfileRegistry::resolve`].
///
/// Every message is Spanish: they reach the operator through CLI and MCP
/// surfaces, per the repo's error-stratification rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// No shipped profile carries the requested id.
    #[error("perfil de modelo '{profile_id}' desconocido; perfiles disponibles: {available}")]
    ProfileNotFound {
        /// The id that was requested.
        profile_id: String,
        /// Comma-separated list of the shipped ids.
        available: String,
    },
    /// The profile declares a `schema_version` this build does not understand.
    #[error(
        "el perfil '{profile_id}' declara schema_version {found} y esta versión de webfang \
         sólo entiende {expected}; actualizá webfang o quitá ese override"
    )]
    UnknownSchemaVersion {
        /// The id of the offending profile.
        profile_id: String,
        /// The version the profile declares.
        found: u32,
        /// The version this build understands.
        expected: u32,
    },
    /// A field carries a value the profile cannot be used with.
    #[error("el perfil '{profile_id}' tiene '{field}' inválido: {reason}")]
    InvalidField {
        /// The id of the offending profile.
        profile_id: String,
        /// Name of the offending field, verbatim as in the schema.
        field: &'static str,
        /// Why the value cannot be used.
        reason: String,
    },
}

/// One embedding model, as this build knows it.
///
/// Every field of the ADR schema is present and non-optional except the two
/// prefix slots and [`ModelProfile::batch_max`], which are `Option` because
/// **no repo-verified value exists for them**: sending a prefix the model was
/// not trained for, or a batch cap nobody can source, is exactly the silent
/// invention the ADR rejects. `None` means "this repo declares nothing", and
/// callers supply their own bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Stable identifier (`"granite-97m"`, …).
    pub id: String,
    /// Schema version this record was written against
    /// ([`PROFILE_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Native embedding dimension the model emits, **before** any Matryoshka
    /// truncation. This is not necessarily what lands in the vault — see
    /// [`Self::output_dim`].
    pub dim: usize,
    /// Maximum batch size per embeddings request, if declared. `None` = no cap
    /// is declared by this profile.
    pub batch_max: Option<usize>,
    /// Prefix prepended to a query-side input, if the model requires one.
    pub query_prefix: Option<String>,
    /// Prefix prepended to a passage-side input, if the model requires one.
    pub passage_prefix: Option<String>,
    /// Maximum sequence length in tokens.
    pub context: usize,
    /// Characters per token, the ratio the chunk budget is derived from.
    pub chars_per_token: f32,
    /// Whether the model exposes the Matryoshka representation-learning
    /// property, i.e. whether a narrower vector can be asked for.
    pub matryoshka: bool,
    /// Whether the model's identity (`dim` + `matryoshka`) is the one this
    /// repo verifies. `false` means the operator overrode an audited value.
    pub verified: bool,
    /// Class of the currently effective values.
    pub provenance: ProfileProvenance,
    /// Where the shipped values are pinned in this repo, as `path:line`.
    pub citation: String,
}

impl ModelProfile {
    /// The `dimensions` value for an OpenAI-compatible embeddings request.
    ///
    /// `None` means **omit the field**: vLLM (and several OpenAI-compatible
    /// runtimes) reject an explicit `dimensions` on a non-Matryoshka model
    /// with a 400, so sending the native width there is a hard failure, not a
    /// default. A Matryoshka profile gets the effective width
    /// ([`MATRYOSHKA_OUTPUT_DIM`]) — the truncated one, never
    /// [`Self::dim`].
    #[must_use]
    pub fn dimensions(&self) -> Option<usize> {
        self.matryoshka.then_some(MATRYOSHKA_OUTPUT_DIM)
    }

    /// Dimension the vectors actually carry into the vault.
    ///
    /// The truncated width for a Matryoshka profile, the native
    /// [`Self::dim`] otherwise — the distinction ADR-0004 §"Migración de
    /// vaults" depends on, kept explicit instead of flattened into one field.
    #[must_use]
    pub fn output_dim(&self) -> usize {
        self.dimensions().unwrap_or(self.dim)
    }

    /// Character budget derived from `context × chars_per_token`.
    ///
    /// This is the per-profile input for the chunk budget (ADR-0004 Tramo D):
    /// steps 2 and 3 consume it instead of the hard-coded ratio
    /// ([`DEFAULT_CHARS_PER_TOKEN`](crate::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN)).
    /// Rounds rather than truncates, and saturates on cast, so a large
    /// `context` can neither lose a character nor wrap.
    #[must_use]
    pub fn max_chars(&self) -> usize {
        (self.context as f64 * f64::from(self.chars_per_token)).round() as usize
    }

    /// Apply a user override, producing the effective profile.
    ///
    /// Only the audited fields (`dim`, `matryoshka`) drop `verified`; see the
    /// module docs for why.
    #[must_use]
    pub fn with_override(&self, over: &ModelProfileOverride) -> Self {
        let mut merged = self.clone();
        if let Some(version) = over.schema_version {
            merged.schema_version = version;
        }
        if let Some(dim) = over.dim {
            merged.dim = dim;
        }
        if let Some(batch_max) = over.batch_max {
            merged.batch_max = Some(batch_max);
        }
        if let Some(prefix) = over.query_prefix.clone() {
            merged.query_prefix = Some(prefix);
        }
        if let Some(prefix) = over.passage_prefix.clone() {
            merged.passage_prefix = Some(prefix);
        }
        if let Some(context) = over.context {
            merged.context = context;
        }
        if let Some(ratio) = over.chars_per_token {
            merged.chars_per_token = ratio;
        }
        if let Some(matryoshka) = over.matryoshka {
            merged.matryoshka = matryoshka;
        }
        if over.dim.is_some() || over.matryoshka.is_some() {
            merged.verified = false;
            merged.provenance = ProfileProvenance::OperatorOverride;
        }
        merged
    }

    /// Whether the profile is usable as-is.
    ///
    /// Checks the schema version first: a record this build cannot interpret
    /// must not be reinterpreted field by field.
    ///
    /// # Errors
    ///
    /// [`ProfileError::UnknownSchemaVersion`] for an unknown schema version,
    /// [`ProfileError::InvalidField`] for a zero dimension, zero context, zero
    /// batch cap or a non-positive / non-finite `chars_per_token`.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnknownSchemaVersion {
                profile_id: self.id.clone(),
                found: self.schema_version,
                expected: PROFILE_SCHEMA_VERSION,
            });
        }
        let invalid = |field: &'static str, reason: &str| ProfileError::InvalidField {
            profile_id: self.id.clone(),
            field,
            reason: reason.to_string(),
        };
        if self.dim == 0 {
            return Err(invalid(
                "dim",
                "una dimensión 0 no se puede indexar en ningún vault",
            ));
        }
        if self.context == 0 {
            return Err(invalid(
                "context",
                "un contexto 0 rechaza cualquier entrada, incluso una sola",
            ));
        }
        if self.batch_max.is_some_and(|max| max == 0) {
            return Err(invalid("batch_max", "el tope debe ser >= 1"));
        }
        if !self.chars_per_token.is_finite() || self.chars_per_token <= 0.0 {
            return Err(invalid("chars_per_token", "la razón debe ser finita y > 0"));
        }
        Ok(())
    }
}

/// A per-operator patch of a shipped profile, read from the user's config
/// file.
///
/// Every field is optional: absent means "keep the repo default". `query_prefix`
/// / `passage_prefix` cannot be *cleared* by an override (absent and absent are
/// the same thing in serde) — that is accepted, since a shipped profile with a
/// prefix is not one this repo ships today.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelProfileOverride {
    /// Id of the shipped profile this patch applies to. Must exist in the repo.
    pub id: String,
    /// Schema version the operator wrote this override against.
    pub schema_version: Option<u32>,
    /// Replacement native dimension.
    pub dim: Option<usize>,
    /// Replacement per-request batch cap.
    pub batch_max: Option<usize>,
    /// Replacement query-side prefix.
    pub query_prefix: Option<String>,
    /// Replacement passage-side prefix.
    pub passage_prefix: Option<String>,
    /// Replacement sequence length in tokens.
    pub context: Option<usize>,
    /// Replacement characters-per-token ratio.
    pub chars_per_token: Option<f32>,
    /// Replacement Matryoshka flag.
    pub matryoshka: Option<bool>,
}

/// The `[[model_profiles]]` block of the user's config file.
///
/// Mirrors [`ProvidersConfig`](crate::domain::providers::ProvidersConfig):
/// declarative, ordered, and empty by default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelProfilesConfig {
    /// One patch per shipped profile, in config order.
    pub profiles: Vec<ModelProfileOverride>,
}

impl ModelProfilesConfig {
    /// Number of declared overrides.
    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Whether no override is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Look up the override for `profile_id`.
    #[must_use]
    pub fn find(&self, profile_id: &str) -> Option<&ModelProfileOverride> {
        self.profiles.iter().find(|o| o.id == profile_id)
    }
}

/// The repo's shipped profiles, in declaration order.
///
/// Only models this repo can verify are here. `granite-97m` is 384d native
/// with no truncation; `granite-311m` is 768d native truncated to 384d via
/// Matryoshka, and both land 384d in the vault.
#[must_use]
pub fn builtin_profiles() -> Vec<ModelProfile> {
    let citation =
        format!("{CITATION_CACHE_CONFIG}; {CITATION_TOKENIZER}; {CITATION_CHARS_PER_TOKEN}");
    vec![
        ModelProfile {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            schema_version: PROFILE_SCHEMA_VERSION,
            dim: 384,
            // No repo-verified per-request cap: the ONNX path never declared
            // one, and inventing a number for a remote runtime is the failure
            // mode this whole unit exists to prevent.
            batch_max: None,
            query_prefix: None,
            passage_prefix: None,
            context: 32_768,
            chars_per_token: crate::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            matryoshka: false,
            verified: true,
            provenance: ProfileProvenance::RepoVerified,
            citation: citation.clone(),
        },
        ModelProfile {
            id: GRANITE_311M_PROFILE_ID.to_string(),
            schema_version: PROFILE_SCHEMA_VERSION,
            dim: 768,
            batch_max: None,
            query_prefix: None,
            passage_prefix: None,
            context: 32_768,
            chars_per_token: crate::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN,
            matryoshka: true,
            verified: true,
            provenance: ProfileProvenance::RepoVerified,
            citation,
        },
    ]
}

/// Resolver over the shipped profiles plus the operator's overrides.
///
/// Same shape as [`ProviderRegistry`](crate::domain::providers::ProviderRegistry)
/// — declarative config plus a typed lookup — so the two config blocks read
/// the same way in a user's config file.
#[derive(Debug, Clone, Default)]
pub struct ModelProfileRegistry {
    overrides: ModelProfilesConfig,
}

impl ModelProfileRegistry {
    /// Wrap the operator's overrides.
    #[must_use]
    pub fn new(overrides: ModelProfilesConfig) -> Self {
        Self { overrides }
    }

    /// The wrapped overrides.
    #[must_use]
    pub fn overrides(&self) -> &ModelProfilesConfig {
        &self.overrides
    }

    /// Ids of the shipped profiles, in declaration order.
    #[must_use]
    pub fn ids(&self) -> Vec<&'static str> {
        BUILTIN_IDS.to_vec()
    }

    /// The shipped profile for `profile_id`, before any override.
    #[must_use]
    pub fn builtin(&self, profile_id: &str) -> Option<ModelProfile> {
        builtin_profiles().into_iter().find(|p| p.id == profile_id)
    }

    /// Resolve the effective profile for `profile_id`: shipped defaults under
    /// the operator's override, validated.
    ///
    /// # Errors
    ///
    /// [`ProfileError::ProfileNotFound`] when the id is not shipped (an
    /// override alone can never introduce a model),
    /// [`ProfileError::UnknownSchemaVersion`] when the effective profile
    /// declares a schema version this build does not understand,
    /// [`ProfileError::InvalidField`] when a field is unusable.
    pub fn resolve(&self, profile_id: &str) -> Result<ModelProfile, ProfileError> {
        let base = self
            .builtin(profile_id)
            .ok_or_else(|| ProfileError::ProfileNotFound {
                profile_id: profile_id.to_string(),
                available: self.ids().join(", "),
            })?;
        let effective = match self.overrides.find(profile_id) {
            Some(over) => base.with_override(over),
            None => base,
        };
        effective.validate()?;
        Ok(effective)
    }

    /// Resolve every shipped profile.
    ///
    /// # Errors
    ///
    /// The first [`ProfileError`] encountered — one broken override must not
    /// be dropped from a listing that promises the whole set.
    pub fn all(&self) -> Result<Vec<ModelProfile>, ProfileError> {
        self.ids().into_iter().map(|id| self.resolve(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granite_97m() -> ModelProfile {
        builtin_profiles()
            .into_iter()
            .find(|p| p.id == GRANITE_97M_PROFILE_ID)
            .expect("builtin profile")
    }

    #[test]
    fn builtin_profiles_are_valid_and_explicitly_versioned() {
        for profile in builtin_profiles() {
            assert_eq!(profile.schema_version, PROFILE_SCHEMA_VERSION);
            profile
                .validate()
                .unwrap_or_else(|e| panic!("shipped profile must be valid: {e}"));
            assert!(profile.verified);
            assert_eq!(profile.provenance, ProfileProvenance::RepoVerified);
            assert!(!profile.citation.is_empty(), "provenance must be cited");
        }
    }

    #[test]
    fn override_config_parses_from_a_json_block_and_is_empty_by_default() {
        let parsed: ModelProfilesConfig =
            serde_json::from_str(r#"{"profiles": [{"id": "granite-97m", "batch_max": 8}]}"#)
                .expect("override block parses");
        assert_eq!(parsed.len(), 1);
        assert!(parsed
            .find("granite-97m")
            .is_some_and(|o| o.batch_max == Some(8)));
        assert!(ModelProfilesConfig::default().is_empty());
    }

    #[test]
    fn an_override_leaves_untouched_fields_alone() {
        let base = granite_97m();
        let patched = base.with_override(&ModelProfileOverride {
            id: base.id.clone(),
            chars_per_token: Some(2.0),
            ..ModelProfileOverride::default()
        });
        assert_eq!(patched.chars_per_token, 2.0);
        assert_eq!(patched.dim, base.dim);
        assert_eq!(patched.context, base.context);
        assert!(patched.verified);
    }

    #[test]
    fn validate_rejects_an_unpinned_or_zeroed_profile() {
        let mut profile = granite_97m();
        profile.schema_version = PROFILE_SCHEMA_VERSION + 1;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::UnknownSchemaVersion { .. })
        ));

        let mut profile = granite_97m();
        profile.dim = 0;
        assert!(matches!(
            profile.validate(),
            Err(ProfileError::InvalidField { field: "dim", .. })
        ));
    }
}
