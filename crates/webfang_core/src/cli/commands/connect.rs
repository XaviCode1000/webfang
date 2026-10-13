//! `webfang connect` — detect and validate a remote embedding endpoint, then
//! write its config so a later `webfang --embedding-provider <id>` (or the
//! vault-search path) resolves it with no further setup.
//!
//! Contract (issue #1933, ADR-0004 step 4):
//! - **Detect + validate**: build a real `RemoteEmbeddingAdapter` against the
//!   endpoint and run its startup `probe_dim`. The adapter POSTs a probe
//!   embedding and reads the served dimension; a pin that disagrees fails
//!   closed. This is the same preflight the scrape path runs, so a `connect`
//!   that succeeds is a `connect` whose provider will start.
//! - **Write**: append a `[[providers]]` record to the user's config so the
//!   registry resolves it on the next run.
//! - **Never write config when the endpoint did not respond** — the error names
//!   what was missing (URL, model, DNS / connection / status / parse).
//!
//! Auth is always `{"source":"none"}` for the connect flow: the subcommand does
//! not ask for credentials, and an anonymous endpoint is the only honest shape
//! a discovery tool can register. An endpoint that needs auth is declared by
//! hand afterwards, which keeps this side-effect minimal and auditable.
//!
//! ## Why a model is required
//!
//! [`RemoteEmbeddingAdapter`] refuses to build without a declared `model` (the
//! remote endpoint requires it on every request), so the probe cannot run
//! without one. `connect` therefore requires `--model`: guessing it would
//! register a provider that fails at first use, which is exactly the failure
//! mode this subcommand exists to prevent.

use std::fmt::Write as _;
use std::path::Path;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::cli::llm_wire;
use crate::cli::parse::parse_provider_id;
use crate::domain::auth_source::AuthSource;
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::providers::{Capability, ProviderConfig, ProviderKind, ProvidersConfig};

/// Run the connect flow for the endpoint `base_url` under the id `id`,
/// writing the resulting provider record to `path`.
///
/// Returns the exit the binary should use. On success the provider record is
/// appended to `path`; on any failure nothing is written and the message names
/// what failed.
pub async fn run(base_url: &url::Url, id: &str, model: &str, path: &Path) -> CliExit {
    let id = match parse_provider_id(id) {
        Ok(id) => id,
        Err(e) => return CliExit::UsageError(e),
    };

    let provider = ProviderConfig {
        id: id.clone(),
        display_name: display_for_id(&id),
        kind: ProviderKind::OpenAiCompatible,
        base_url: base_url.clone(),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        // The model is required: the adapter cannot build without it.
        model: Some(model.to_string()),
        // `connect` never locks a dimension: the startup probe adopts the
        // served width, and writing a pin here would turn a healthy endpoint
        // into a fail-closed mismatch the operator never asked for.
        embedding_dim: None,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
    };

    register(&provider, path).await
}

/// Validate `provider` against its endpoint, then write it to `path`.
///
/// Split from [`run`] so tests can drive the same validation/write path with a
/// provider whose `allow_loopback` is set (the guarded production client
/// rejects loopback; wiremock lives there).
async fn register(provider: &ProviderConfig, path: &Path) -> CliExit {
    if let Err(exit) = validate_endpoint(provider).await {
        return exit;
    }

    if let Err(e) = write_provider(path, provider) {
        return CliExit::ConfigError(format!(
            "no se pudo escribir la configuración en '{}': {e}",
            path.display()
        ));
    }

    println!(
        "Provider '{}' conectado y registrado en {}",
        provider.id,
        path.display()
    );
    CliExit::Success
}

/// Build the real adapter against `provider` and run its startup probe.
///
/// This is the one network hop the subcommand makes — and it is the SAME
/// preflight the scrape path runs (`llm_wire::build_embedding_provider`), so a
/// provider that validates here is one that will start there. A failure is
/// returned verbatim so the message names the missing piece (URL gate, model,
/// connection, status, parse).
async fn validate_endpoint(provider: &ProviderConfig) -> Result<(), CliExit> {
    let providers = ProvidersConfig {
        providers: vec![provider.clone()],
    };
    // `resolve_embedding_config` picks the explicit slot; we then build the
    // adapter (which probes). Building with a fresh `CrawlOptions` whose
    // embedding slot names this provider keeps the resolution unambiguous even
    // when the config already holds other providers.
    let opts = CrawlOptions {
        embedding_provider: Some(provider.id.clone()),
        ..CrawlOptions::default()
    };

    match llm_wire::build_embedding_provider(&opts, &providers, false).await {
        Ok(Some(_conn)) => Ok(()),
        Ok(None) => Err(CliExit::ConfigError(format!(
            "el provider '{}' no se resolvió como un endpoint remoto de embeddings: \
             el endpoint respondió pero no es OpenAI-compatible",
            provider.id
        ))),
        // `build_embedding_provider` maps construction/probe failures to
        // `ConfigError`; surface it as a network failure so the operator sees
        // "the endpoint did not answer" rather than "bad config".
        Err(CliExit::ConfigError(msg)) => Err(CliExit::NetworkError(msg)),
        Err(other) => Err(other),
    }
}

/// Derive a readable label from the id `connect` generated.
///
/// `connect-f82b` → `Connect F82b`; `acme-inc-99` → `Acme Inc 99`. Purely
/// cosmetic: the id is what the registry matches on, and the label can be
/// edited by hand afterwards without changing behaviour.
fn display_for_id(id: &str) -> String {
    id.split('-')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = String::new();
                    let _ = write!(s, "{}{}", first.to_ascii_uppercase(), chars.as_str());
                    s
                },
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append the provider record to `path` as valid TOML.
///
/// The written shape is the generic connect form (ADR-0004 "forma B"):
/// anonymous, embedding-only, no pinned dimension, no allowlist. It round-trips
/// through `ConfigDefaults` as-is, so the next run resolves it.
fn write_provider(path: &Path, provider: &ProviderConfig) -> Result<(), std::io::Error> {
    use std::io::Write as _;

    // A fresh config file is valid TOML with no tables; appending a
    // `[[providers]]` array-of-tables is valid on top of it. We do NOT seed a
    // placeholder table — an empty file is already valid.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    // Escape the two fields that can carry a quote; the URL and id are
    // validated upstream and cannot.
    let display = provider
        .display_name
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let base_url = provider.base_url.as_str();
    let model = provider
        .model
        .as_deref()
        .unwrap_or_default()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    let mut record = String::new();
    let _ = write!(
        record,
        "\n[[providers]]\n  id = \"{}\"\n  display_name = \"{}\"\n  kind = \"open_ai_compatible\"\n  base_url = \"{}\"\n  auth = {{ source = \"none\" }}\n  capabilities = [\"embedding\"]\n  model = \"{}\"\n",
        provider.id, display, base_url, model
    );

    file.write_all(record.as_bytes())?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use wiremock::matchers::{method, path as path_matcher};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A wiremock server answering `POST /embeddings` with a 3-wide vector.
    async fn serve_embeddings() -> MockServer {
        let server = MockServer::start().await;
        let body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.0,0.1,0.2]}],"model":"nomic-embed-text","usage":{"prompt_tokens":0,"total_tokens":0}}"#;
        Mock::given(method("POST"))
            .and(path_matcher("/embeddings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .append_header("content-type", "application/json"),
            )
            .mount(&server)
            .await;
        server
    }

    fn temp_config_path() -> PathBuf {
        let dir = tempfile::tempdir().expect("temp dir");
        dir.path().join("webfang-connect.toml")
    }

    #[test]
    fn display_for_id_derives_a_readable_label() {
        assert_eq!(display_for_id("connect-f82b"), "Connect F82b");
        assert_eq!(display_for_id("acme-inc-99"), "Acme Inc 99");
        assert_eq!(display_for_id("connect-x"), "Connect X");
    }

    #[test]
    fn write_provider_appends_a_parseable_record() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let provider = ProviderConfig {
            id: "connect-a1b2".into(),
            display_name: "Connect A1b2".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };

        write_provider(&path, &provider).expect("append record");

        // The file must parse back through the SAME loader the run uses.
        let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
            .expect("written config must parse");
        assert_eq!(loaded.providers.len(), 1);
        let written = &loaded.providers[0];
        assert_eq!(written.id, "connect-a1b2");
        assert_eq!(written.model.as_deref(), Some("nomic-embed-text"));
        assert!(written.has_capability(Capability::Embedding));
    }

    #[tokio::test]
    async fn connect_registers_a_provider_that_answers_the_probe() {
        let server = serve_embeddings().await;
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");

        // The guarded production client rejects loopback, so this exercises the
        // same validation/write path with `allow_loopback` set — the operator's
        // own provider would carry `false` and dial a public host.
        let provider = ProviderConfig {
            id: "connect-a1b2".into(),
            display_name: "Connect A1b2".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: server.uri().parse().expect("mock uri"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".into()),
            embedding_dim: None,
            allow_loopback: true,
            network_policy: NetworkPolicy::default(),
        };

        let exit = register(&provider, &path).await;
        assert!(matches!(exit, CliExit::Success), "got {exit:?}");

        let loaded =
            crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config parses");
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].id, "connect-a1b2");
    }

    #[tokio::test]
    async fn connect_rejects_a_dead_endpoint_and_writes_nothing() {
        // RFC 5737 TEST-NET-1, port 1: nothing listens there.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");

        let exit = run(
            &url::Url::parse("http://192.0.2.1:1/").expect("url"),
            "connect-dead",
            "nomic-embed-text",
            &path,
        )
        .await;

        assert!(matches!(exit, CliExit::NetworkError(_)), "got {exit:?}");
        assert!(
            !path.exists()
                || std::fs::read_to_string(&path)
                    .unwrap_or_default()
                    .is_empty(),
            "no provider record may be written when the endpoint does not answer"
        );
    }

    #[tokio::test]
    async fn connect_rejects_an_illegal_provider_id_before_touching_the_network() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");

        let exit = run(
            &url::Url::parse("http://192.0.2.1:1/").expect("url"),
            "has space",
            "nomic-embed-text",
            &path,
        )
        .await;

        assert!(matches!(exit, CliExit::UsageError(_)), "got {exit:?}");
        assert!(!path.exists(), "an illegal id must not write anything");
    }
}
