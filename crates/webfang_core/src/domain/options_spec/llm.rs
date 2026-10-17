//! LLM provider flag group: `--extract-with-llm` + provider selection.
//!
//! Separate from the `ai` group on purpose. The `ai` cargo feature is the
//! **local ONNX** stack (`InferencePool`, granite models); the LLM provider
//! is a **remote** port (`LlmPort` / `OpenAiCompatibleProvider`) that lives in
//! `webfang_core` unconditionally. Gating these flags behind `ai` would hide
//! them on builds that can still talk to a remote provider.
//!
//! The `--extract-with-llm` flag is the trigger for the Container contract in
//! `docs/architecture/ai-providers-design.md` §8b: the Container stays permissive
//! (`llm_port()` is `None` without a provider), and the **binary** validates
//! `llm_port().is_some()` at startup whenever this flag is set. That
//! validation is the expiry of the §8b DEBE — it ships with this flag, never
//! after it.

use super::{DefaultValue, OptionSpec, ValueKind};

/// `--extract-with-llm` (env `WEBFANG_EXTRACT_WITH_LLM`, bool SetTrue).
///
/// Requests structured extraction through the configured remote LLM provider.
/// When set, startup fails fast (exit 78) if no completion provider is
/// configured — a daemon that reports OK and then fails on the first
/// invocation is exactly the failure the §8b contract exists to prevent.
pub const EXTRACT_WITH_LLM: OptionSpec = OptionSpec {
    id: "extract_with_llm",
    value_name: "EXTRACT_WITH_LLM",
    long: "extract-with-llm",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_EXTRACT_WITH_LLM"),
    default: Some(DefaultValue::Bool(false)),
    help: "Run structured extraction through the configured LLM provider (requires a provider with the `completion` capability; fails at startup otherwise)",
    heading: Some("LLM Extraction"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--llm-provider <LLM_PROVIDER>` (env `WEBFANG_LLM_PROVIDER`).
///
/// Selects which configured provider serves `--extract-with-llm`. Absent,
/// the first provider declaring the `completion` capability wins (config
/// order) — the same rule as
/// [`crate::domain::providers::ProviderRegistry::resolve_default_completion`].
pub const LLM_PROVIDER: OptionSpec = OptionSpec {
    id: "llm_provider",
    value_name: "LLM_PROVIDER",
    long: "llm-provider",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_LLM_PROVIDER"),
    default: None,
    help: "Provider id to use for LLM extraction (default: first provider declaring the `completion` capability)",
    heading: Some("LLM Extraction"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--embedding-provider <EMBEDDING_PROVIDER>` (env `WEBFANG_EMBEDDING_PROVIDER`).
///
/// Selects which configured provider serves the embedding slot (vault search
/// query/chunk vectorization, #1462). Absent, the first provider declaring
/// the `embedding` capability wins (config order) — the same rule as
/// [`crate::domain::providers::ProviderRegistry::resolve_default_embedding`].
/// A `local_onnx` selection (or no embedding provider at all) serves the
/// local pool adapter with no network probe; an `open_ai_compatible`
/// selection builds the remote adapter and verifies the served dimension at
/// startup (exit 78 on mismatch).
pub const EMBEDDING_PROVIDER: OptionSpec = OptionSpec {
    id: "embedding_provider",
    value_name: "EMBEDDING_PROVIDER",
    long: "embedding-provider",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_EMBEDDING_PROVIDER"),
    default: None,
    help: "Provider id to use for embeddings (default: first provider declaring the `embedding` capability; local pool when unset or local)",
    heading: Some("LLM Extraction"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--tier2-embedding-provider <TIER2_EMBEDDING_PROVIDER>` (env `WEBFANG_TIER2_EMBEDDING_PROVIDER`).
///
/// Selects which configured provider serves the Tier 2 semantic inspector
/// embedding slot (selector repair, ADR-0004). This flag is **separate** from
/// `--embedding-provider` on purpose: vault search and Tier 2 repair are
/// distinct privacy domains, so the vault slot's default resolution (first
/// provider declaring `embedding`, config order —
/// [`crate::domain::providers::ProviderRegistry::resolve_default_embedding`])
/// deliberately does NOT apply here. A user enabling remote embeddings for
/// vault search does NOT implicitly enable remote embeddings for DOM fragment
/// inspection (issue #1977, #1981).
/// Absent (or an explicit `local_onnx` selection) serves the local pool `--ai`
/// builds, with no network probe and no config lookup; an
/// `open_ai_compatible` selection requires `--tier2-consent` (exit 78 without
/// it) and verifies the served dimension at startup.
pub const TIER2_EMBEDDING_PROVIDER: OptionSpec = OptionSpec {
    id: "tier2_embedding_provider",
    value_name: "TIER2_EMBEDDING_PROVIDER",
    long: "tier2-embedding-provider",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_TIER2_EMBEDDING_PROVIDER"),
    default: None,
    help: "Provider id to use for Tier 2 semantic inspector embeddings (default: local pool; the vault-search provider is never adopted — an explicit remote id requires --tier2-consent)",
    heading: Some("Tier 2 Embeddings"),
    kind: ValueKind::Text,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// `--tier2-consent` (env `WEBFANG_TIER2_CONSENT`, bool SetTrue).
///
/// Explicit consent for Tier 2 remote embeddings egress. When
/// `--tier2-embedding-provider` resolves to a remote endpoint (not
/// `local_onnx`), this flag MUST be set, otherwise startup fails with
/// exit 78. This ensures the operator explicitly authorizes DOM fragment
/// egress to a third-party endpoint (issue #1977, ADR-0004 §Egress).
/// A hash of `provider_id|base_url` is logged at consent time for
/// auditability.
pub const TIER2_CONSENT: OptionSpec = OptionSpec {
    id: "tier2_consent",
    value_name: "TIER2_CONSENT",
    long: "tier2-consent",
    short: None,
    aliases: &[],
    env: Some("WEBFANG_TIER2_CONSENT"),
    default: Some(DefaultValue::Bool(false)),
    help: "Explicit consent for Tier 2 remote embeddings egress (required when --tier2-embedding-provider is remote; hash of provider|endpoint logged for audit)",
    heading: Some("Tier 2 Embeddings"),
    kind: ValueKind::Bool,
    visible_aliases: &[],
    nullable: false,
    description_override: None,
    feature_gate: None,
    value_delimiter: None,
};

/// All LLM-group options, in `LlmArgs` field-declaration order.
pub const GROUP: &[OptionSpec] = &[
    EXTRACT_WITH_LLM,
    LLM_PROVIDER,
    EMBEDDING_PROVIDER,
    TIER2_EMBEDDING_PROVIDER,
    TIER2_CONSENT,
];
