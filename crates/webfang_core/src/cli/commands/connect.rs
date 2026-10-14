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
//! - **Write**: splice a `[[providers]]` record into the user's config so the
//!   registry resolves it on the next run. The on-disk document is edited in
//!   place: an existing record with the same id is MERGED field by field (never
//!   replaced wholesale, never duplicated), and a new one is appended.
//! - **Never write config when the endpoint did not respond** — the error names
//!   what was missing (URL, model, DNS / connection / status / parse). A
//!   refusal for any other reason (a type-invalid config, an operator-owned
//!   record bound to a different endpoint, an uneditable `providers` shape)
//!   likewise leaves the file byte-for-byte identical.
//!
//! Auth is always `{"source":"none"}` for the connect flow: the subcommand does
//! not ask for credentials, and an anonymous endpoint is the only honest shape
//! a discovery tool can register. An endpoint that needs auth is declared by
//! hand afterwards, which keeps this side-effect minimal and auditable.
//!
//! ## Why a model is required
//!
//! `RemoteEmbeddingAdapter` refuses to build without a declared `model` (the
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
/// merged into (or appended to) `path`; on any failure nothing is written and
/// the message names what failed.
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
        // `build_embedding_provider` maps BOTH construction failures (bad
        // `--model`, an SSRF loopback rejection) and probe failures
        // (connection, status, parse) onto `CliExit::ConfigError`, with the
        // real cause in the message. The exit code is remapped to
        // `NetworkError` (69) because a `connect` that could not validate an
        // endpoint is, from the operator's seat, an endpoint that did not
        // answer.
        //
        // Known limitation: a construction failure that is NOT a network
        // problem (an HTTP 401 for a wrong model, or the loopback rejection)
        // also lands on 69. Distinguishing them needs `build_embedding_provider`
        // to surface the category separately; until then the message — not the
        // exit code — carries the cause.
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

/// Splice the provider record for `provider.id` into `path`.
///
/// **Idempotent by id, by MERGE.** `ProvidersConfig::find` returns the FIRST
/// match, so an append-only write would let a stale record silently shadow the
/// newly connected endpoint forever. A matching `[[providers]]` entry is
/// therefore merged field by field rather than replaced wholesale: a plain
/// replacement would silently downgrade a working provider to the anonymous,
/// loopback-forbidden shape `connect` builds. The merge's exact split is
/// [`merge_provider`]; a record whose operator-owned fields belong to a
/// DIFFERENT endpoint is refused instead of merged.
///
/// **The file is edited, never re-serialized.** The user's config holds far
/// more than providers (`format`, `concurrency`, `vault_path`,
/// `[[model_profiles]]`…), and `ProvidersConfig` models exactly one field.
/// Parsing to that struct and writing the round-trip back to disk would
/// silently DESTROY every key the shape does not know about. Instead the
/// on-disk TOML document is parsed into an editable tree, only the matching
/// `[[providers]]` entry is touched (or one appended), and every other byte —
/// keys, tables, ordering, comments — survives verbatim.
///
/// **What is guaranteed about a successful write.** The `providers` array is
/// well-formed TOML and matches the `ProvidersConfig` shape (the whole file is
/// deserialized as a gate before anything is written, so a record the run could
/// not resolve is refused rather than written through); the matching entry was
/// merged or appended and no duplicate id was created; the document was
/// rendered from the parsed tree, which preserves every untouched key, table
/// and comment — including the comments attached to the merged entry's own
/// refreshed keys. The write is atomic (temp file + rename), preserves the
/// file's permissions, and follows a symlinked config to its target.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidData`] — the existing file is not valid TOML,
/// does not match the `ProvidersConfig` shape, declares a `providers` value
/// that is neither an array of tables nor a convertible inline array, or holds
/// a record whose operator-owned fields belong to a different endpoint. Every
/// one of these is checked before the atomic write, so the file is left
/// byte-for-byte identical. [`std::io::Error`] otherwise — I/O failure creating
/// the parent directory, the temp file, or performing the rename.
fn write_provider(path: &Path, provider: &ProviderConfig) -> Result<(), std::io::Error> {
    // The `webfang/` parent directory does not exist on a first run — exactly
    // the case `connect` exists for. Create it before opening, or the write
    // fails AFTER the network probe already succeeded.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut document = if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        // The `DocumentMut` parse below only proves the file is WELL-FORMED
        // TOML; it accepts content the run cannot resolve. `ProvidersConfig` is
        // the shape `ConfigDefaults` actually deserializes, so a second, typed
        // pass rejects a record such as `base_url = 42` here — before any write
        // handle is opened, so a refusal leaves the file byte-identical.
        //
        // Without this gate connect reported success on a config that the very
        // next run resolved ZERO providers from (the documented `#[serde(
        // default)]` fallback at `cli/config.rs`): a loud refusal became a
        // false success plus a silently degraded run.
        toml::from_str::<ProvidersConfig>(&existing).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración en '{}' no coincide con el esquema de providers \
                     (no se toca el archivo para no corromperlo): {e}",
                    path.display()
                ),
            )
        })?;
        existing.parse::<toml_edit::DocumentMut>().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración existente en '{}' no es TOML válido: {e}",
                    path.display()
                ),
            )
        })?
    } else {
        toml_edit::DocumentMut::new()
    };

    upsert_provider(&mut document, provider)?;

    write_atomically(path, &document.to_string())
}

/// Splice `provider` into the `providers` array of `document`, in place.
///
/// An existing entry with the same id is merged field by field (never
/// replaced wholesale); see [`merge_provider`] for the exact split. A plain
/// overwrite would silently downgrade a working provider to the anonymous,
/// loopback-forbidden shape `connect` builds.
fn upsert_provider(
    document: &mut toml_edit::DocumentMut,
    provider: &ProviderConfig,
) -> Result<(), std::io::Error> {
    let entry = toml_edit::ser::to_document(provider)
        .map_err(|e| {
            std::io::Error::other(format!(
                "no se pudo serializar el provider '{}': {e}",
                provider.id
            ))
        })?
        .as_table()
        .clone();

    // `providers` missing, wrong type, or empty: start a fresh array of
    // tables. `entry` is an inline-shaped table from serde; `to_string` is
    // what turns it into a `[[providers]]` table, so store the serialized
    // form and let the writer render it.
    let array = ensure_providers_array(document)?;

    // Locate by id, then merge OR append in one pass without holding a
    // mutable borrow across both arms. `iter_mut().find` hands back the
    // mutable table directly — no index round-trip, no `expect()` — and the
    // append path is taken only when the find yielded nothing, so the two
    // borrows never overlap.
    let matched = array
        .iter_mut()
        .find(|table| table.get("id").and_then(|id| id.as_str()) == Some(provider.id.as_str()));
    if let Some(existing) = matched {
        merge_provider(existing, &entry)?;
        return Ok(());
    }
    array.push(entry);

    Ok(())
}

/// Borrow (creating if needed) the `providers` array-of-tables.
fn ensure_providers_array(
    document: &mut toml_edit::DocumentMut,
) -> Result<&mut toml_edit::ArrayOfTables, std::io::Error> {
    if !document.contains_key("providers") {
        document["providers"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    if document["providers"].as_array_of_tables_mut().is_none() {
        convert_inline_providers(document)?;
    }
    document["providers"]
        .as_array_of_tables_mut()
        .ok_or_else(providers_shape_error)
}

/// Rewrite an inline `providers = [ { … } ]` as a `[[providers]]` array of
/// tables so the rest of the write can use one shape.
///
/// The inline array is legal TOML, deserializes through `ProvidersConfig {
/// providers: Vec<ProviderConfig> }` and is accepted by the run, so refusing
/// it would be a narrowing nothing required. `Item::into_array_of_tables`
/// does the conversion for a non-empty all-inline-table array; an EMPTY
/// `providers = []` is the same declaration as no providers at all, and
/// `into_array_of_tables` declines it, so a fresh array-of-tables is built
/// there. Everything else keeps the loud refusal.
fn convert_inline_providers(document: &mut toml_edit::DocumentMut) -> Result<(), std::io::Error> {
    use toml_edit::{ArrayOfTables, Item, Value};

    let Some((key, item)) = document
        .get_key_value("providers")
        .map(|(k, v)| (k.clone(), v.clone()))
    else {
        return Err(providers_shape_error());
    };
    let key_prefix = key.leaf_decor().prefix().cloned();

    let mut item = match item.into_array_of_tables() {
        Ok(array_of_tables) => Item::ArrayOfTables(array_of_tables),
        Err(Item::Value(Value::Array(array))) if array.is_empty() => {
            Item::ArrayOfTables(ArrayOfTables::new())
        },
        Err(_) => return Err(providers_shape_error()),
    };

    // A `# comment` the operator wrote above `providers = [...]` lives in the
    // KEY's decor prefix. An array-of-tables renders its header with DEFAULT
    // key decor (`encode_key_path(..., DEFAULT_KEY_PATH_DECOR)`), so the key is
    // re-inserted with `fmt()`'d decor — carrying the original would splice the
    // comment INTO the header and emit `[[# comment\nproviders ]]`, a file
    // nothing can parse. The first converted table's OWN decor prefix renders
    // immediately before that header (`table.decor.prefix_encode` in
    // `visit_table`), which is the faithful home for the text. An empty array
    // has no table to carry it and is about to receive the appended record
    // anyway, so nothing is emitted for it.
    if let Some(prefix) = key_prefix {
        if let Some(first) = item
            .as_array_of_tables_mut()
            .and_then(|array_of_tables| array_of_tables.iter_mut().next())
        {
            first.decor_mut().set_prefix(prefix);
        }
    }

    document.insert("providers", item);
    Ok(())
}

/// The single loud refusal for a `providers` key this code cannot edit.
fn providers_shape_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "'providers' existe en la configuración pero no es un array de tablas \
         ([[providers]]) ni un array de tablas en línea; no se toca el archivo \
         para no corromperlo",
    )
}

/// Merge the probed `incoming` record into an existing `[[providers]]` table.
///
/// Three groups, keyed on what `connect` is authoritative about:
///
/// 1. **Probe-owned** — `kind`, `base_url`, `model`, `display_name` — always
///    refreshed: they describe the endpoint the probe just validated, and that
///    is the whole point of re-running `connect`.
/// 2. **Operator-owned** — `auth`, `embedding_dim`, `allow_loopback`,
///    `network_policy` — never overwritten, and carried over only when the
///    record already points at the SAME endpoint `connect` just probed (see
///    [`same_endpoint`]). Preservation happens by NOT TOUCHING the key, so a
///    field `connect` cannot express is kept: `to_document` skips `None`, so
///    `embedding_dim` is never in `incoming` and the operator's pin survives by
///    absence, not by any insert rule. The keys `connect` DOES build —
///    `auth`, `allow_loopback`, `network_policy` — are filled in on absence.
/// 3. **Additive** — `capabilities` — the operator may have widened an
///    existing provider to serve more than embeddings, so the incoming
///    embedding capability is unioned in rather than replacing the list.
fn merge_provider(
    existing: &mut toml_edit::Table,
    incoming: &toml_edit::Table,
) -> Result<(), std::io::Error> {
    // A record declaring operator-owned fields describes ONE host: its
    // credentials, its LAN allowlist and its pinned dim are only meaningful
    // against the `base_url` they were written for. `connect` accepts an
    // explicit `--id`, so without this guard one host's keyring entry and
    // allowlist would be shipped to a different host while `register` printed
    // "conectado y registrado" — and the stale `embedding_dim` would fail the
    // next run closed. Checked BEFORE any mutation, so a refusal leaves nothing
    // half-merged and nothing written.
    if carries_operator_fields(existing) && !same_endpoint(existing, incoming) {
        return Err(cross_endpoint_error(existing, incoming));
    }

    // Group 1: probe-owned, always refreshed (decor carried over).
    for key in ["display_name", "base_url", "model", "kind"] {
        refresh_key(existing, key, incoming);
    }

    // Group 2: operator-owned, filled in only on absence.
    for key in ["auth", "embedding_dim", "allow_loopback", "network_policy"] {
        if !existing.contains_key(key) {
            if let Some(value) = incoming.get(key) {
                existing.insert(key, value.clone());
            }
        }
    }

    // Group 3: capabilities are unioned, never replaced.
    if let Some(value) = incoming.get("capabilities") {
        merge_capabilities(existing, value);
    }
    Ok(())
}

/// Overwrite one probe-owned key, carrying the operator's decor onto the
/// incoming value.
///
/// `Table::insert` on an Occupied entry calls `entry.key_mut().fmt()` and
/// `mem::replace`s the item, which drops BOTH the key decor (a `# comment`
/// line above the key) and the value decor (a trailing `# comment`). A plain
/// rewrite therefore deletes hand-written notes inside the provider block on
/// every re-`connect`. So read the decor that is actually on the old entry,
/// apply it to the incoming value, and re-insert under the ORIGINAL key.
fn refresh_key(existing: &mut toml_edit::Table, key: &str, incoming: &toml_edit::Table) {
    let Some(value) = incoming.get(key) else {
        return;
    };
    let Some((existing_key, existing_item)) = existing
        .get_key_value(key)
        .map(|(k, v)| (k.clone(), v.clone()))
    else {
        // Vacant: there is no old decor to carry, and a new key renders with
        // the default decor.
        existing.insert(key, value.clone());
        return;
    };

    let mut item = value.clone();
    if let Some(old) = existing_item.as_value() {
        if let Some(new) = item.as_value_mut() {
            *new.decor_mut() = old.decor().clone();
        }
    }
    existing.insert_formatted(&existing_key, item);
}

/// Whether the existing record declares any operator-owned field a merge would
/// have to carry across to the probed endpoint.
fn carries_operator_fields(existing: &toml_edit::Table) -> bool {
    ["auth", "embedding_dim", "allow_loopback", "network_policy"]
        .iter()
        .any(|key| existing.contains_key(key))
}

/// Whether the existing record points at the endpoint `connect` just probed.
///
/// An absent or unreadable `base_url` is NOT evidence of the probed endpoint,
/// and this predicate fails closed on it.
fn same_endpoint(existing: &toml_edit::Table, incoming: &toml_edit::Table) -> bool {
    match (
        existing.get("base_url").and_then(toml_edit::Item::as_str),
        incoming.get("base_url").and_then(toml_edit::Item::as_str),
    ) {
        (Some(existing_url), Some(incoming_url)) => existing_url == incoming_url,
        _ => false,
    }
}

/// The refusal for "these credentials belong to a different host".
fn cross_endpoint_error(
    existing: &toml_edit::Table,
    incoming: &toml_edit::Table,
) -> std::io::Error {
    const UNREADABLE: &str = "(sin base_url legible)";
    let id = incoming
        .get("id")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(UNREADABLE);
    let existing_url = existing
        .get("base_url")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(UNREADABLE);
    let incoming_url = incoming
        .get("base_url")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(UNREADABLE);

    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "el provider '{id}' ya está registrado contra '{existing_url}' y el endpoint \
             conectado es '{incoming_url}': no se arrastran credenciales ni políticas de \
             red entre hosts distintos (no se toca el archivo). Usá un `--id` distinto, \
             o editá o borrá a mano el registro existente"
        ),
    )
}

/// Union an `incoming` capabilities array into `existing`'s, preserving order
/// and without duplicating an entry the operator already had.
fn merge_capabilities(existing: &mut toml_edit::Table, incoming: &toml_edit::Item) {
    let Some(incoming_caps) = incoming.as_array() else {
        return;
    };
    // Ensure the target is an array; if the operator wrote a non-array, leave
    // their value untouched rather than silently reshaping it.
    if existing.get("capabilities").is_some() && existing["capabilities"].as_array().is_none() {
        return;
    }
    let target = existing.entry("capabilities").or_insert_with(|| {
        toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new()))
    });
    let Some(target) = target.as_array_mut() else {
        return;
    };
    for cap in incoming_caps {
        if !target.iter().any(|c| c.as_str() == cap.as_str()) {
            target.push(cap.clone());
        }
    }
}

/// Write `contents` to `path` atomically: temp file in the same directory,
/// then rename over the target. A same-filesystem rename is atomic, so a
/// PROCESS crash (panic, kill, failed write) leaves either the old file or
/// the new one — never a truncated config. A POWER crash is NOT covered:
/// neither the temp file nor the parent directory is fsynced, so after power
/// loss the rename may be durable while the new contents are not.
///
/// The rename replaces the destination INODE and does not follow symlinks, so
/// two things are corrected before the rename: a symlinked config is resolved
/// to its target (otherwise a stow/chezmoi link is silently replaced by a
/// regular file and later hand-edits stop landing in the dotfiles tree), and
/// the temp file inherits the target's permissions (otherwise an operator who
/// restricted `config.toml` to `0600` silently gets `0644`).
fn write_atomically(path: &Path, contents: &str) -> Result<(), std::io::Error> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.toml");
    let temp = parent.join(format!(".{file_name}.connect-{}.tmp", std::process::id()));

    // The temp is cleaned up on BOTH error arms: leaving it behind turns a
    // failed connect into a stray file full of the operator's config.
    if let Err(e) = write_temp(&temp, &target, contents) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    match std::fs::rename(&temp, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        },
    }
}

/// Create `temp` carrying the permissions the finished config must have, then
/// write `contents` into it.
///
/// A NEW config is created `0600` from the start: even though `AuthSource` has
/// no inline-secret variant today, the file carries keyring account names and
/// LAN allowlists, and the repo already treats `0600` as the convention for
/// credential-bearing files. An EXISTING config keeps whatever mode the
/// operator chose, copied onto the temp before the rename.
fn write_temp(temp: &Path, target: &Path, contents: &str) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(temp)?;
    std::io::Write::write_all(&mut file, contents.as_bytes())?;
    // Closed before the chmod: on Windows an open handle blocks it.
    drop(file);

    if let Ok(metadata) = std::fs::metadata(target) {
        std::fs::set_permissions(temp, metadata.permissions())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A first run has no `webfang/` directory yet — the write must create it
    /// rather than fail after the endpoint probe already succeeded.
    #[test]
    fn write_provider_creates_a_missing_parent_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("webfang").join("config.toml");
        let provider = ProviderConfig {
            id: "connect-fresh".into(),
            display_name: "Connect Fresh".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };

        write_provider(&path, &provider).expect("the parent dir is created on demand");
        assert!(path.exists());
    }

    /// Re-registering the same id must replace the record, never append a
    /// second one: `ProvidersConfig::find` is first-match, so a duplicate
    /// would let the stale endpoint shadow the newly connected one forever.
    #[test]
    fn write_provider_replaces_an_existing_id_instead_of_duplicating() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let mut provider = ProviderConfig {
            id: "connect-a1b2".into(),
            display_name: "Connect A1b2".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://old.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };

        write_provider(&path, &provider).expect("first write");
        // Same endpoint, refreshed model: the id is what must stay unique, and
        // a different endpoint under the same id is now refused outright (see
        // `write_provider_refuses_to_carry_operator_fields_across_endpoints`).
        provider.model = Some("nomic-embed-text-v2".into());
        write_provider(&path, &provider).expect("second write");

        let loaded =
            crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
        assert_eq!(
            loaded.providers.len(),
            1,
            "re-registering the same id must not append a duplicate"
        );
        assert_eq!(
            loaded.providers[0].model.as_deref(),
            Some("nomic-embed-text-v2"),
            "the record must be the one just connected"
        );
    }

    /// THE regression test for the data-loss bug: a real config holds far more
    /// than `[[providers]]`, and `ProvidersConfig` models exactly one field.
    /// Writing a provider must leave every other key, table and comment
    /// byte-for-byte intact, or the operator loses their whole config on the
    /// first `connect`.
    #[test]
    fn write_provider_preserves_unrelated_config_keys_and_comments() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let original = "\
# Configuración de webfang — no borrar a mano
format = \"markdown\"
concurrency = \"auto\"
max_pages = 25
vault_path = \"/home/op/vault\"
log_level = \"debug\"

[[model_profiles.profiles]]
id = \"granite-97m\"
chars_per_token = 4.0
matryoshka = true

[[providers]]
id = \"openai\"
display_name = \"OpenAI\"
kind = \"open_ai_compatible\"
base_url = \"https://api.openai.com/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"text-embedding-3-small\"
allow_loopback = false
";
        std::fs::write(&path, original).expect("seed config");

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

        let written = std::fs::read_to_string(&path).expect("read back");

        // Every unrelated top-level key survives.
        for key in [
            "format = \"markdown\"",
            "concurrency = \"auto\"",
            "max_pages = 25",
            "vault_path = \"/home/op/vault\"",
            "log_level = \"debug\"",
        ] {
            assert!(
                written.contains(key),
                "lost unrelated key: {key}\n---\n{written}"
            );
        }
        // The comment survives (a typed round-trip drops it entirely).
        assert!(
            written.contains("# Configuración de webfang — no borrar a mano"),
            "the operator's comment was destroyed\n---\n{written}"
        );
        // The unrelated `[[model_profiles]]` table survives.
        assert!(
            written.contains("[[model_profiles.profiles]]")
                && written.contains("matryoshka = true"),
            "lost the model_profiles table\n---\n{written}"
        );
        // The pre-existing provider AND the new one are both present.
        assert!(
            written.contains("id = \"openai\""),
            "lost the existing provider"
        );
        assert!(
            written.contains("id = \"connect-a1b2\""),
            "lost the new provider"
        );

        // And the file still loads through the real loader.
        let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
            .expect("written config must still parse");
        assert_eq!(loaded.providers.len(), 2, "both providers must survive");
    }

    /// Reconnecting the SAME endpoint must NOT clobber hand-tuned operator
    /// fields: an existing provider with credentials / loopback / a pinned dim
    /// would be silently downgraded to the anonymous, loopback-forbidden shape
    /// `connect` builds.
    ///
    /// The endpoint has to be identical — see
    /// [`write_provider_refuses_to_carry_operator_fields_across_endpoints`] for
    /// the refusal that guards the other direction.
    #[test]
    fn write_provider_preserves_operator_fields_when_reconnecting_the_same_endpoint() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let seeded = "\
[[providers]]
id = \"connect-a1b2\"
display_name = \"Viejo\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"keyring\", service = \"webfang\", account = \"op\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
embedding_dim = 768
allow_loopback = true
network_policy = { mode = \"allowlist\", allow_cidrs = [\"10.0.0.0/8\"], allow_hosts = [\"vllm.lan\"] }
";
        std::fs::write(&path, seeded).expect("seed config");

        // What `connect` would produce: anonymous, loopback forbidden, no dim.
        let provider = ProviderConfig {
            id: "connect-a1b2".into(),
            display_name: "Connect A1b2".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text-v2".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };
        write_provider(&path, &provider).expect("merge record");

        let loaded =
            crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
        assert_eq!(loaded.providers.len(), 1, "must not duplicate the id");
        let written = &loaded.providers[0];

        // Refreshed by the probe.
        assert_eq!(written.base_url.as_str(), "https://api.example.test/v1");
        assert_eq!(written.display_name, "Connect A1b2");
        assert_eq!(written.model.as_deref(), Some("nomic-embed-text-v2"));

        // Preserved operator intent.
        assert!(written.allow_loopback, "loopback opt-in was clobbered");
        assert_eq!(
            written.embedding_dim,
            Some(768),
            "the operator's pinned dim was clobbered"
        );
        assert!(
            !matches!(written.auth, AuthSource::None),
            "credentials were clobbered by the anonymous connect shape"
        );
        assert_eq!(
            written.network_policy.allow_hosts,
            vec!["vllm.lan".to_string()],
            "the operator's LAN allowlist was clobbered"
        );
    }

    /// THE cross-endpoint credential bug: `connect` accepts an explicit
    /// `--id`, so an operator can point an id that already carries one host's
    /// credentials and LAN allowlist at a completely different host. Carrying
    /// the operator-owned fields across would ship `old.host`'s keyring entry
    /// to `new.host` while `register` prints "conectado y registrado". The
    /// write fails closed and the file is untouched.
    #[test]
    fn write_provider_refuses_to_carry_operator_fields_across_endpoints() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let seeded = "\
[[providers]]
id = \"openai\"
display_name = \"OpenAI viejo\"
kind = \"open_ai_compatible\"
base_url = \"https://old.host/v1\"
auth = { source = \"keyring\", service = \"webfang\", account = \"op\" }
capabilities = [\"embedding\"]
model = \"text-embedding-3-small\"
embedding_dim = 1024
network_policy = { mode = \"allowlist\", allow_cidrs = [\"10.0.0.0/8\"], allow_hosts = [\"old.host\"] }
";
        std::fs::write(&path, seeded).expect("seed config");
        let before = std::fs::read(&path).expect("read seed");

        let provider = ProviderConfig {
            id: "openai".into(),
            display_name: "OpenAI".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://new.host/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("text-embedding-3-small".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };

        let err = write_provider(&path, &provider)
            .expect_err("credentials must never cross to another endpoint");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let message = err.to_string();
        assert!(message.contains("openai"), "must name the id: {message}");
        assert!(
            message.contains("https://old.host/v1") && message.contains("https://new.host/v1"),
            "must name both endpoints: {message}"
        );
        assert!(
            message.contains("--id"),
            "must tell the operator how to proceed: {message}"
        );

        assert_eq!(
            before,
            std::fs::read(&path).expect("read back"),
            "a refused merge must leave the config byte-for-byte identical"
        );
    }

    /// Comments the operator wrote INSIDE the provider block are part of their
    /// hand-tuning, and `Table::insert` on an occupied entry drops both the key
    /// decor (a preceding `# comment` line) and the value decor (a trailing
    /// `# comment`). The merge must carry both onto the refreshed value.
    #[test]
    fn write_provider_preserves_comments_inside_the_merged_provider_block() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let seeded = "\
[[providers]]
id = \"connect-a1b2\"
# etiqueta que el operador eligió a mano
display_name = \"Viejo\"
# backend validado contra el servidor de embeddings
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"  # no cambiar sin avisar al equipo
auth = { source = \"none\" }
capabilities = [\"embedding\"]
# modelo aprobado por Ticket WEB-482
model = \"nomic-embed-text\"
";
        std::fs::write(&path, seeded).expect("seed config");

        let provider = ProviderConfig {
            id: "connect-a1b2".into(),
            display_name: "Connect A1b2".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text-v2".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        };
        write_provider(&path, &provider).expect("merge record");

        let written = std::fs::read_to_string(&path).expect("read back");
        for comment in [
            "# etiqueta que el operador eligió a mano",
            "# backend validado contra el servidor de embeddings",
            "# no cambiar sin avisar al equipo",
            "# modelo aprobado por Ticket WEB-482",
        ] {
            assert!(
                written.contains(comment),
                "the merge destroyed a comment inside the provider block: {comment}\n---\n{written}"
            );
        }
    }

    /// A config that is well-formed TOML but type-invalid against the
    /// `ProvidersConfig` shape (`base_url = 42`) used to sail through connect
    /// untouched: `write_provider` reported success, printed "conectado y
    /// registrado", and every later run silently resolved ZERO providers via
    /// the documented fallback. A loud refusal is the only honest outcome.
    #[test]
    fn write_provider_refuses_a_config_whose_providers_are_type_invalid() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let seeded = "\
[[providers]]
id = \"connect-a1b2\"
display_name = \"Connect A1b2\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"

[[providers]]
id = \"otro\"
display_name = \"Otro\"
kind = \"open_ai_compatible\"
base_url = 42
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
        std::fs::write(&path, seeded).expect("seed config");
        let before = std::fs::read(&path).expect("read seed");

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

        let err = write_provider(&path, &provider)
            .expect_err("a type-invalid config must not be written through");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let message = err.to_string();
        assert!(
            message.contains("config.toml"),
            "the error must name the file: {message}"
        );

        assert_eq!(
            before,
            std::fs::read(&path).expect("read back"),
            "a refused write must leave the config byte-for-byte identical"
        );
    }

    /// `providers = [ { … } ]` is legal TOML, deserializes through
    /// `ProvidersConfig { providers: Vec<ProviderConfig> }` and is accepted by
    /// the run — so `connect` must edit it, not refuse it.
    ///
    /// Note the seed carries EVERY required `ProviderConfig` field: the A1
    /// validation gate runs first, and `display_name` / `kind` / `auth` have no
    /// default, so a partial inline record is refused before it is ever
    /// converted.
    #[test]
    fn write_provider_edits_an_inline_providers_array() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let seeded = "\
format = \"markdown\"
# declarados en línea; connect los reescribe como [[providers]]
providers = [ { id = \"old\", display_name = \"Old\", kind = \"open_ai_compatible\", \
base_url = \"https://old.example.test/v1\", auth = { source = \"none\" }, \
model = \"m\", capabilities = [\"embedding\"] } ]
";
        std::fs::write(&path, seeded).expect("seed config");

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
        write_provider(&path, &provider).expect("an inline array is editable");

        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(
            written.contains("format = \"markdown\""),
            "an unrelated top-level key was lost\n---\n{written}"
        );
        assert!(
            !written.contains("providers = ["),
            "the inline array should have been converted to [[providers]]\n---\n{written}"
        );
        assert!(
            written.contains("# declarados en línea; connect los reescribe como [[providers]]"),
            "the conversion dropped the comment above the `providers` key\n---\n{written}"
        );

        let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
            .expect("written config must parse through the real loader");
        assert_eq!(loaded.providers.len(), 2, "both providers must survive");
        assert_eq!(loaded.providers[0].id, "old");
        assert_eq!(loaded.providers[1].id, "connect-a1b2");
    }

    /// A config an operator restricted to `0600` (the repo's convention for
    /// credential-bearing files) must not silently become `0644` because
    /// `rename(2)` replaces the destination inode.
    #[cfg(unix)]
    #[test]
    fn write_provider_preserves_the_existing_config_file_mode() {
        use std::os::unix::fs::PermissionsExt;

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
        write_provider(&path, &provider).expect("first write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the config like an operator would");

        write_provider(&path, &provider).expect("second write");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the atomic rewrite downgraded the operator's 0600 config"
        );
    }

    /// The permission COPY is its own obligation, and a `0600` assertion cannot
    /// see it: the temp is created `0600`, so asserting `0600` passes whether the
    /// writer copied the operator's mode or just kept the default. An operator
    /// running a wider mode — group-readable `0640` in a shared setup — would be
    /// silently narrowed to `0600` with every test still green.
    #[cfg(unix)]
    #[test]
    fn write_provider_preserves_a_config_mode_wider_than_the_temp_default() {
        use std::os::unix::fs::PermissionsExt;

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
        write_provider(&path, &provider).expect("first write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .expect("widen the config the way a group-shared setup would");

        write_provider(&path, &provider).expect("second write");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o640,
            "the atomic rewrite narrowed an operator's 0640 config to the 0600 temp default"
        );
    }

    /// A brand-new config is created `0600` from the start: even though
    /// `AuthSource` has no inline-secret variant today, the file carries
    /// keyring account names and LAN allowlists.
    #[cfg(unix)]
    #[test]
    fn write_provider_creates_a_new_config_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

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

        write_provider(&path, &provider).expect("first write");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a newly created credential-bearing config must be created 0600"
        );
    }

    /// `rename(2)` does not follow symlinks, so a config managed by stow /
    /// chezmoi was being replaced by a regular file, silently detaching the
    /// link from the operator's dotfiles tree.
    #[cfg(unix)]
    #[test]
    fn write_provider_writes_through_a_symlinked_config() {
        let dir = tempfile::tempdir().expect("temp dir");
        let managed = dir.path().join("dotfiles").join("webfang-config.toml");
        std::fs::create_dir_all(managed.parent().expect("parent")).expect("make dotfiles dir");
        std::fs::write(&managed, "format = \"markdown\"\n").expect("seed the managed file");
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&managed, &link).expect("symlink the config");

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
        write_provider(&link, &provider).expect("write through the symlink");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("lstat")
                .file_type()
                .is_symlink(),
            "the atomic rewrite replaced the operator's symlink with a regular file"
        );
        let resolved = std::fs::read_to_string(&managed).expect("read the resolved target");
        assert!(
            resolved.contains("id = \"connect-a1b2\"") && resolved.contains("format ="),
            "the new content must land in the resolved target\n---\n{resolved}"
        );
    }

    /// A failed write must not leave its temp file behind: the temp holds the
    /// full config text, so a stale `.config.toml.connect-<pid>.tmp` is a copy
    /// of the operator's configuration sitting next to it — and a second write
    /// to the same pid would overwrite it, since the name is pid-derived.
    #[test]
    fn write_atomically_leaves_no_temp_file_when_the_rename_fails() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A non-empty directory at the destination makes rename(2) fail only
        // after the temp has already been written.
        let target = dir.path().join("config.toml");
        std::fs::create_dir(&target).expect("make a directory where a file is expected");
        std::fs::write(target.join("occupied"), b"x").expect("make it non-empty");

        write_atomically(&target, "id = \"connect-a1b2\"\n")
            .expect_err("renaming a file over a non-empty directory must fail");

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .expect("list the parent directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".config.toml") && name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "a failed atomic write left its temp file behind: {leftovers:?}"
        );
    }

    /// Reconnecting must not narrow an operator's widened `capabilities`:
    /// `connect` only knows about embeddings, but the provider may have been
    /// hand-extended, and replacing the list would drop the extra capability.
    #[test]
    fn write_provider_unions_capabilities_instead_of_replacing_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[providers]]\nid = \"connect-a1b2\"\ndisplay_name = \"Wide\"\n\
             kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
             auth = { source = \"none\" }\ncapabilities = [\"embedding\", \"completion\"]\n\
             model = \"nomic-embed-text\"\nallow_loopback = false\n",
        )
        .expect("seed config");

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
        write_provider(&path, &provider).expect("merge record");

        let loaded =
            crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
        let written = &loaded.providers[0];
        assert!(
            written.has_capability(Capability::Embedding),
            "embedding must remain"
        );
        assert!(
            written.has_capability(Capability::Completion),
            "the operator's extra capability was dropped by the merge"
        );
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
