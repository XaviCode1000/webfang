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
//!   replaced wholesale, never duplicated), and a new one is appended. The
//!   write is also safe against the two ways the config around it is
//!   destroyed: the temp file is created O_EXCL under a unique per-write name
//!   (a pre-planted symlink is refused, never truncated through), and the
//!   bytes are re-read after the splice so a config a concurrent `connect`
//!   rewrote meanwhile is refused instead of renamed over.
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
/// well-formed TOML and the whole file deserializes as the `ConfigDefaults`
/// document the run loads (that gate runs before anything is written, so
/// anything the next run could not load — a type-invalid provider record or a
/// mistyped key anywhere in the file — is refused rather than written
/// through); the matching entry was
/// merged or appended and no duplicate id was created; the document was
/// rendered from the parsed tree, which preserves every untouched key, table
/// and comment — including the comments attached to the merged entry's own
/// refreshed keys. The write is atomic (temp file + rename), preserves the
/// file's permissions, and follows a symlinked config to its target. It is
/// also exclusive in effect: the bytes on disk are re-read after the splice,
/// so a config another `connect` rewrote meanwhile is refused instead of
/// renamed over (see [`ensure_config_unchanged`]).
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidData`] — the existing file is not valid TOML,
/// does not match the full `ConfigDefaults` document the run loads, declares a
/// `providers` value
/// that is neither an array of tables nor a convertible inline array, or holds
/// a record whose operator-owned fields belong to a different endpoint. Every
/// one of these is checked before the atomic write, so the file is left
/// byte-for-byte identical. The same [`std::io::ErrorKind::InvalidData`] is
/// returned when the config changed on disk while this write held it — a
/// concurrent `connect` landed first, nothing is written, and the message
/// names the retry. [`std::io::Error`] otherwise — I/O failure creating
/// the parent directory, the temp file, or performing the rename.
fn write_provider(path: &Path, provider: &ProviderConfig) -> Result<(), std::io::Error> {
    // The `webfang/` parent directory does not exist on a first run — exactly
    // the case `connect` exists for. Create it before opening, or the write
    // fails AFTER the network probe already succeeded.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // The snapshot the splice is computed from, kept for the optimistic
    // concurrency check after the merge (`ensure_config_unchanged`).
    let mut original: Option<String> = None;
    let mut document = if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        // The `DocumentMut` parse below only proves the file is WELL-FORMED
        // TOML; it accepts content the run cannot resolve. The gate therefore
        // deserializes the SAME document the run loads — `ConfigDefaults` —
        // not just its `providers` slice: `ProvidersConfig` models exactly one
        // field, so a mistyped key anywhere else (`format = 42`, an invalid
        // `[[model_profiles.profiles]]` entry) passed the old gate, connect
        // printed success and wrote — and the next run degraded EVERYTHING to
        // defaults in silence through the documented `#[serde(default)]`
        // fallback at `cli/config.rs`. `ConfigDefaults` deliberately does NOT
        // deny unknown fields (an operator's unmodeled keys keep working, and
        // an empty file — a fresh install — parses), so the gate only refuses
        // what the run itself could not load. It runs before any write handle
        // is opened, so a refusal leaves the file byte-identical.
        toml::from_str::<crate::cli::config::ConfigDefaults>(&existing).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración en '{}' no coincide con el documento completo que \
                     el run carga (no se toca el archivo para no corromperlo): {e}",
                    path.display()
                ),
            )
        })?;
        let document = existing.parse::<toml_edit::DocumentMut>().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "la configuración existente en '{}' no es TOML válido: {e}",
                    path.display()
                ),
            )
        })?;
        original = Some(existing);
        document
    } else {
        toml_edit::DocumentMut::new()
    };

    upsert_provider(&mut document, provider)?;

    // Optimistic concurrency: the whole splice is a read-modify-write with no
    // lock on the config, so the bytes are re-read AFTER the merge and must
    // still be the ones the merge started from. A difference means a
    // concurrent `connect` landed first: renaming now would drop its record
    // while both printed success, so the write refuses and names the retry.
    commit_splice(path, &document, original.as_deref())
}

/// Persist the spliced document: refuse first if the file drifted, then write
/// atomically.
///
/// Extracted from [`write_provider`] so the wiring itself is testable: the
/// drift check and the write are two calls that must happen IN ORDER, and a
/// test that removes one while keeping the other green is exactly the
/// unfalsifiable coverage this function exists to prevent.
fn commit_splice(
    path: &Path,
    document: &toml_edit::DocumentMut,
    original: Option<&str>,
) -> Result<(), std::io::Error> {
    ensure_config_unchanged(path, original)?;
    write_atomically(path, &document.to_string())
}

/// Whether the bytes on disk no longer match the snapshot the splice was
/// computed from.
///
/// Byte-exact on purpose: a whitespace-only difference still means another
/// writer touched the document, and merging into a snapshot that is no
/// longer what is on disk is exactly the lost update this guards.
fn config_drifted(original: &str, current_on_disk: &str) -> bool {
    original != current_on_disk
}

/// Fail closed when the config changed on disk since `original` was read.
///
/// `write_provider` is a read-modify-write with no portable advisory lock
/// on the config: two concurrent `connect` runs carrying different
/// providers would both splice into the same snapshot and race to rename,
/// and the loser's provider would vanish while BOTH printed "conectado y
/// registrado". The conflict is therefore detected optimistically — the
/// bytes are re-read after the splice and must still be the ones the
/// splice started from. A difference (including a file that appeared or
/// disappeared between the read and the write) means another writer landed
/// first: this run refuses, writes nothing, and names the retry.
///
/// The check narrows the window to the microseconds between the re-read
/// and the rename; closing it completely needs a lock on the config, which
/// this write does not take.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidData`] when the bytes drifted, the file
/// vanished, or it became unreadable.
fn ensure_config_unchanged(path: &Path, original: Option<&str>) -> Result<(), std::io::Error> {
    let current = std::fs::read_to_string(path).ok();
    match (original, current) {
        (Some(original), Some(current)) if !config_drifted(original, &current) => Ok(()),
        // Absent when read, absent now: a fresh first connect.
        (None, None) => Ok(()),
        _ => Err(config_drift_error(path)),
    }
}

/// The refusal for "another writer changed the config while connect held it".
fn config_drift_error(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "la configuración '{}' cambió en disco mientras connect la escribía: \
             no se toca el archivo para no perder el registro del otro connect; \
             ejecutá connect de nuevo",
            path.display()
        ),
    )
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

/// Monotonic counter backing the connect temp-name suffix.
///
/// `pid` alone is not unique inside a multi-threaded process, and two
/// writers a nanosecond apart can derive the same name from a clock. A
/// per-process monotonic counter makes every temp name unique for the
/// lifetime of the process; the pid keeps concurrent PROCESSES disjoint.
static TMP_NONCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Attempts to mint a temp name before failing loud on a persistent
/// pre-plant (see [`create_temp`]).
const TEMP_CREATE_ATTEMPTS: usize = 3;

/// Mint the temp path for one write: `.{file_name}.connect-{pid}-{nonce}.tmp`
/// next to the target.
///
/// The dot prefix keeps the temp out of a plain listing, and the pid +
/// monotonic nonce make the name unguessable to a would-be planter while
/// still obvious to the GC sweep (the strict `.{file_name}.connect-`
/// prefix). The nonce is what makes the name UNIQUE: pid alone collides
/// between two writes in one multi-threaded process, and the collision is
/// silent — one write renames the temp away and the other's rename fails
/// with ENOENT.
fn temp_path(dir: &Path, file_name: &str) -> std::path::PathBuf {
    let nonce = TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!(
        ".{file_name}.connect-{}-{nonce}.tmp",
        std::process::id()
    ))
}

/// Create the temp file, retrying with a fresh name when the candidate was
/// pre-planted. Returns the temp path actually used.
///
/// `next_temp` mints the candidate name; production passes [`temp_path`],
/// tests pass a deterministic sequence so the retry arm is exercised
/// without predicting the process counter.
///
/// O_EXCL rejection (`create_new`, see [`write_temp`]) is the pre-plant
/// signal: someone with write access to the config directory left a file —
/// typically a symlink — at the exact name this write was about to use,
/// hoping the writer would truncate through it (CWE-377). That never
/// happens here: the candidate is abandoned, `next_temp` mints the next
/// name, and the write lands there. The retry is bounded by
/// [`TEMP_CREATE_ATTEMPTS`]; a collision that survives every attempt is
/// reported loud rather than downgraded to a truncating open.
///
/// A pre-planted candidate is NOT removed on the way out: the path belongs
/// to whoever created it, and unlinking it would only race that writer.
/// Only a temp this call actually created is cleaned up.
fn create_temp(
    mut next_temp: impl FnMut() -> std::path::PathBuf,
    target: &Path,
    contents: &str,
) -> Result<std::path::PathBuf, std::io::Error> {
    let mut temp = next_temp();
    let mut attempts_left = TEMP_CREATE_ATTEMPTS;
    loop {
        match write_temp(&temp, target, contents) {
            Ok(()) => return Ok(temp),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts_left > 1 => {
                attempts_left -= 1;
                temp = next_temp();
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    let _ = std::fs::remove_file(&temp);
                }
                return Err(e);
            },
        }
    }
}

/// Best-effort sweep of stale connect temps beside the freshly written
/// config.
///
/// A connect killed between create and rename leaves its temp behind — a
/// full copy of the operator's config. The next successful write reclaims
/// every sibling sharing the strict `.{file_name}.connect-` prefix,
/// whatever pid or nonce left it (the pre-fix `.{file}.connect-{pid}.tmp`
/// names included). Sweeping NEVER fails the operation it follows: the
/// config is already renamed into place, a temp is never read as config,
/// and the subcommands run without a subscriber, so a GC problem must not
/// turn a successful connect into an error. A live sibling's temp may lose
/// the race; that writer's rename then fails loud — the loss is never
/// silent.
fn gc_connect_temps(parent: &Path, prefix: &str, current: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(prefix) || entry.path().as_path() == current {
            continue;
        }
        tracing::debug!(file = %name, "GC stale connect temp file");
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Write `contents` to `path` atomically: O_EXCL temp file in the same
/// directory, then rename over the target. A same-filesystem rename is
/// atomic, so a PROCESS crash (panic, kill, failed write) leaves either the
/// old file or the new one — never a truncated config. A POWER crash is
/// NOT covered: neither the temp file nor the parent directory is fsynced,
/// so after power loss the rename may be durable while the new contents
/// are not.
///
/// The rename replaces the destination INODE and does not follow symlinks, so
/// two things are corrected before the rename: a symlinked config is resolved
/// to its target (otherwise a stow/chezmoi link is silently replaced by a
/// regular file and later hand-edits stop landing in the dotfiles tree), and
/// the temp file inherits the target's permissions (otherwise an operator who
/// restricted `config.toml` to `0600` silently gets `0644`).
///
/// The temp name is unique per process AND per write (pid + monotonic
/// nonce, see [`temp_path`]) and is created with O_EXCL: a pre-planted
/// name — a symlink left by someone with write access to the config
/// directory — is refused, never truncated into, and the write retries
/// [`TEMP_CREATE_ATTEMPTS`] times on a fresh name before failing loud (see
/// [`create_temp`]). After a successful rename the stale `.connect-` temps
/// left by killed runs are swept best-effort (see [`gc_connect_temps`]).
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

    // The temp is cleaned up on BOTH error arms: leaving it behind turns a
    // failed connect into a stray file full of the operator's config. A
    // pre-planted candidate is the exception — it is not ours to remove
    // (see `create_temp`).
    let temp = create_temp(|| temp_path(parent, file_name), &target, contents)?;
    match std::fs::rename(&temp, &target) {
        Ok(()) => {
            gc_connect_temps(parent, &format!(".{file_name}.connect-"), &temp);
            Ok(())
        },
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        },
    }
}

/// Create `temp` carrying the permissions the finished config must have, then
/// write `contents` into it.
///
/// The temp is created with `create_new` (O_EXCL): if anything already
/// exists at `temp` — a pre-planted file or symlink — the open fails with
/// [`std::io::ErrorKind::AlreadyExists`] and NOTHING is written, so a
/// symlink plant cannot redirect this write at an arbitrary victim file
/// (CWE-377). `truncate` is deliberately absent: O_EXCL guarantees a
/// brand-new file, so there is never anything to truncate, and keeping the
/// flag would suggest the code truncates a file it refuses to touch.
/// `write` is required alongside `create_new` — O_EXCL without an access
/// mode is EINVAL.
///
/// A NEW config is created `0600` from the start: even though `AuthSource` has
/// no inline-secret variant today, the file carries keyring account names and
/// LAN allowlists, and the repo already treats `0600` as the convention for
/// credential-bearing files. An EXISTING config keeps whatever mode the
/// operator chose, copied onto the temp before the rename.
fn write_temp(temp: &Path, target: &Path, contents: &str) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
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

    /// Canonical embedding provider for arrange blocks: the anonymous,
    /// loopback-forbidden, dim-unpinned shape `connect` builds. Tests that
    /// need another id, endpoint, model or loopback flag mutate the returned
    /// value — the arrange stays one line either way.
    fn test_provider(id: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.into(),
            display_name: display_for_id(id),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".into()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        }
    }

    /// Fresh `config.toml` path inside a temp dir. The dir is returned so it
    /// outlives the test body.
    fn temp_config() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    /// Seed `path` with `contents` for merge/gate tests.
    fn seed_config(path: &Path, contents: &str) {
        std::fs::write(path, contents).expect("seed config");
    }

    /// Provider pointed at the mock embeddings server, with the loopback
    /// opt-in the guarded production client requires for it.
    fn loopback_provider(server: &MockServer, id: &str) -> ProviderConfig {
        let mut provider = test_provider(id);
        provider.base_url = server.uri().parse().expect("mock uri");
        provider.allow_loopback = true;
        provider
    }

    /// First write of the canonical provider for the file-mode tests: every
    /// mode test starts from the same written record, then chmods it and
    /// rewrites.
    #[cfg(unix)]
    fn first_write_canonical() -> (tempfile::TempDir, std::path::PathBuf, ProviderConfig) {
        let (dir, path) = temp_config();
        let provider = test_provider("connect-a1b2");
        write_provider(&path, &provider).expect("first write");
        (dir, path, provider)
    }

    /// Second write plus the mode assertion shared by the file-mode tests:
    /// the atomic rewrite must keep the operator's mode, wide or narrow.
    #[cfg(unix)]
    fn assert_second_write_keeps_mode(
        path: &Path,
        provider: &ProviderConfig,
        mode: u32,
        context: &str,
    ) {
        use std::os::unix::fs::PermissionsExt;
        write_provider(path, provider).expect("second write");
        let actual = std::fs::metadata(path).expect("stat").permissions().mode();
        assert_eq!(actual & 0o777, mode, "{context}");
    }

    /// Names of the entries directly inside `dir` that satisfy `keep`, for
    /// the temp-sweep assertions (a failed write leaves none; a success
    /// sweeps stale siblings).
    fn dir_file_names_matching(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("list the parent directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| keep(name))
            .collect()
    }

    /// Config path plus a pre-planted temp candidate for the symlink-safety
    /// tests.
    fn planted_temp() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let (dir, target) = temp_config();
        let planted = dir.path().join(".config.toml.connect-planted.tmp");
        std::fs::write(&planted, b"PLANT\n").expect("seed the plant");
        (dir, target, planted)
    }

    #[test]
    fn display_for_id_derives_a_readable_label() {
        assert_eq!(display_for_id("connect-f82b"), "Connect F82b");
        assert_eq!(display_for_id("acme-inc-99"), "Acme Inc 99");
        assert_eq!(display_for_id("connect-x"), "Connect X");
    }

    #[test]
    fn write_provider_appends_a_parseable_record() {
        let (_dir, path) = temp_config();
        let provider = test_provider("connect-a1b2");

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
        let provider = test_provider("connect-fresh");

        write_provider(&path, &provider).expect("the parent dir is created on demand");
        assert!(path.exists());
    }

    /// Re-registering the same id must replace the record, never append a
    /// second one: `ProvidersConfig::find` is first-match, so a duplicate
    /// would let the stale endpoint shadow the newly connected one forever.
    #[test]
    fn write_provider_replaces_an_existing_id_instead_of_duplicating() {
        let (_dir, path) = temp_config();
        let mut provider = test_provider("connect-a1b2");
        provider.base_url = url::Url::parse("https://old.example.test/v1").expect("url");

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
        let (_dir, path) = temp_config();
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
        seed_config(&path, original);

        let provider = test_provider("connect-a1b2");
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
        let (_dir, path) = temp_config();
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
        seed_config(&path, seeded);

        // What `connect` would produce: anonymous, loopback forbidden, no dim.
        let mut provider = test_provider("connect-a1b2");
        provider.model = Some("nomic-embed-text-v2".into());
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
        let (_dir, path) = temp_config();
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
        seed_config(&path, seeded);
        let before = std::fs::read(&path).expect("read seed");

        let mut provider = test_provider("openai");
        provider.display_name = "OpenAI".into();
        provider.base_url = url::Url::parse("https://new.host/v1").expect("url");
        provider.model = Some("text-embedding-3-small".into());

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
        let (_dir, path) = temp_config();
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
        seed_config(&path, seeded);

        let mut provider = test_provider("connect-a1b2");
        provider.model = Some("nomic-embed-text-v2".into());
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
        let (_dir, path) = temp_config();
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
        seed_config(&path, seeded);
        let before = std::fs::read(&path).expect("read seed");

        let provider = test_provider("connect-a1b2");

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

    /// A valid `[[providers]]` block PLUS a mistyped unrelated top-level key
    /// (`format = 42`) used to pass the gate: `ProvidersConfig` models one
    /// field and ignores the rest, so connect printed success and wrote — and
    /// the next run degraded EVERYTHING to defaults in silence. The gate must
    /// validate the same document the run loads, so the whole flow (probe OK,
    /// write refused) ends in `CliExit::ConfigError` with the file untouched.
    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn connect_refuses_a_config_with_a_mistyped_unrelated_top_level_key() {
        let server = serve_embeddings().await;
        let (_dir, path) = temp_config();
        let seeded = "\
format = 42

[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
        seed_config(&path, seeded);
        let before = std::fs::read(&path).expect("read seed");

        let provider = loopback_provider(&server, "connect-a1b2");

        // The probe answers; only the pre-write gate can refuse. The incoming
        // id differs from the seeded one on purpose: the merge /
        // cross-endpoint guards would refuse on their own and mask the gate.
        let exit = register(&provider, &path).await;
        match exit {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("documento completo"),
                    "the refusal must name the full-document gate: {msg}"
                );
                assert!(
                    msg.contains("config.toml"),
                    "the refusal must name the file: {msg}"
                );
            },
            other => panic!("a mistyped config key must be a ConfigError, got {other:?}"),
        }

        assert_eq!(
            before,
            std::fs::read(&path).expect("read back"),
            "a refused write must leave the config byte-for-byte identical"
        );
    }

    /// A valid `[[providers]]` block PLUS an invalid `[[model_profiles]]
    /// .profiles]]` override (`dim = "no-numérico"`) is the same defect one
    /// table further away: the run loads `model_profiles` through
    /// `ConfigDefaults`, so a value it cannot deserialize must refuse the
    /// write, not pass a providers-only gate (also closes R4-3).
    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn connect_refuses_a_config_with_an_invalid_model_profiles_override() {
        let server = serve_embeddings().await;
        let (_dir, path) = temp_config();
        let seeded = "\
[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"

[[model_profiles.profiles]]
id = \"granite-97m\"
dim = \"no-numérico\"
";
        seed_config(&path, seeded);
        let before = std::fs::read(&path).expect("read seed");

        let provider = loopback_provider(&server, "connect-a1b2");

        // The incoming id differs from the seeded one on purpose: the merge /
        // cross-endpoint guards would refuse on their own and mask the gate.
        let exit = register(&provider, &path).await;
        assert!(
            matches!(exit, CliExit::ConfigError(_)),
            "an invalid model_profiles override must be a ConfigError, got {exit:?}"
        );

        assert_eq!(
            before,
            std::fs::read(&path).expect("read back"),
            "a refused write must leave the config byte-for-byte identical"
        );
    }

    /// The permission the full-document gate buys its honesty with: keys the
    /// `ConfigDefaults` struct does NOT model — an `[obsidian]` table, a
    /// future key — but that are well typed must keep passing, or the gate
    /// narrows the config language connect is allowed to edit. `format` is
    /// modeled; `[obsidian]` (the struct models the flat `obsidian_*` fields,
    /// not a table) is not. Both must survive the write.
    #[test]
    fn write_provider_accepts_well_typed_keys_the_config_does_not_model() {
        let (_dir, path) = temp_config();
        let seeded = "\
format = \"json\"

# Sección que ConfigDefaults no modela — debe sobrevivir igual.
[obsidian]
vault = \"/home/op/notes\"

[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
        seed_config(&path, seeded);

        let provider = test_provider("connect-a1b2");
        write_provider(&path, &provider).expect("well-typed unmodeled keys must pass the gate");

        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(
            written.contains("format = \"json\""),
            "the modeled key was lost\n---\n{written}"
        );
        assert!(
            written.contains("[obsidian]") && written.contains("vault = \"/home/op/notes\""),
            "an unmodeled but well-typed section was refused or lost\n---\n{written}"
        );

        let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
            .expect("written config must parse through the real loader");
        assert_eq!(loaded.providers.len(), 2, "both providers must survive");
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
        let (_dir, path) = temp_config();
        let seeded = "\
format = \"markdown\"
# declarados en línea; connect los reescribe como [[providers]]
providers = [ { id = \"old\", display_name = \"Old\", kind = \"open_ai_compatible\", \
base_url = \"https://old.example.test/v1\", auth = { source = \"none\" }, \
model = \"m\", capabilities = [\"embedding\"] } ]
";
        seed_config(&path, seeded);

        let provider = test_provider("connect-a1b2");
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

        let (_dir, path, provider) = first_write_canonical();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("restrict the config like an operator would");

        assert_second_write_keeps_mode(
            &path,
            &provider,
            0o600,
            "the atomic rewrite downgraded the operator's 0600 config",
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

        let (_dir, path, provider) = first_write_canonical();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .expect("widen the config the way a group-shared setup would");

        assert_second_write_keeps_mode(
            &path,
            &provider,
            0o640,
            "the atomic rewrite narrowed an operator's 0640 config to the 0600 temp default",
        );
    }

    /// A brand-new config is created `0600` from the start: even though
    /// `AuthSource` has no inline-secret variant today, the file carries
    /// keyring account names and LAN allowlists.
    #[cfg(unix)]
    #[test]
    fn write_provider_creates_a_new_config_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, path, _provider) = first_write_canonical();

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

        let provider = test_provider("connect-a1b2");
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
    /// of the operator's configuration sitting next to it. The name is now
    /// unique per write (pid + nonce), so a later write cannot even overwrite
    /// it — only the failure cleanup below reclaims it. The retry arm never
    /// fires here: nothing pre-planted the name, so the first candidate wins
    /// and the rename failure removes it.
    #[test]
    fn write_atomically_leaves_no_temp_file_when_the_rename_fails() {
        let (dir, target) = temp_config();
        // A non-empty directory at the destination makes rename(2) fail only
        // after the temp has already been written.
        std::fs::create_dir(&target).expect("make a directory where a file is expected");
        std::fs::write(target.join("occupied"), b"x").expect("make it non-empty");

        write_atomically(&target, "id = \"connect-a1b2\"\n")
            .expect_err("renaming a file over a non-empty directory must fail");

        let leftovers: Vec<String> = dir_file_names_matching(dir.path(), |name| {
            name.starts_with(".config.toml") && name.ends_with(".tmp")
        });
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
        let (_dir, path) = temp_config();
        seed_config(
            &path,
            "[[providers]]\nid = \"connect-a1b2\"\ndisplay_name = \"Wide\"\n\
             kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
             auth = { source = \"none\" }\ncapabilities = [\"embedding\", \"completion\"]\n\
             model = \"nomic-embed-text\"\nallow_loopback = false\n",
        );

        let provider = test_provider("connect-a1b2");
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

    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn connect_registers_a_provider_that_answers_the_probe() {
        let server = serve_embeddings().await;
        let (_dir, path) = temp_config();

        // The guarded production client rejects loopback, so this exercises the
        // same validation/write path with `allow_loopback` set — the operator's
        // own provider would carry `false` and dial a public host.
        let provider = loopback_provider(&server, "connect-a1b2");

        let exit = register(&provider, &path).await;
        assert!(matches!(exit, CliExit::Success), "got {exit:?}");

        let loaded =
            crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config parses");
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].id, "connect-a1b2");
    }

    #[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn connect_rejects_a_dead_endpoint_and_writes_nothing() {
        // RFC 5737 TEST-NET-1, port 1: nothing listens there.
        let (_dir, path) = temp_config();

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
        let (_dir, path) = temp_config();

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

    // ---------------------------------------------------------------------
    // R1-1: the temp file of the atomic write must not be plantable through
    // a symlink (CWE-377). Old shape: `.{file}.connect-{pid}.tmp`, opened
    // `write + create + truncate` — a local writer in the config directory
    // pre-planted that exact name with a symlink and connect truncated the
    // victim through it.
    // ---------------------------------------------------------------------

    /// THE CWE-377 regression: a symlink planted at the temp path is never
    /// followed or truncated through. The open fails with `AlreadyExists`
    /// (O_EXCL) and the victim file is left byte-for-byte intact.
    #[cfg(unix)]
    #[test]
    fn write_temp_refuses_a_preplanted_symlink_without_touching_the_victim() {
        let (dir, target) = temp_config();
        let temp = dir.path().join(".config.toml.connect-1-0.tmp");
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"VICTIM\n").expect("seed the victim");
        std::os::unix::fs::symlink(&victim, &temp).expect("plant the symlink");

        let err = write_temp(&temp, &target, "id = \"connect-a1b2\"\n")
            .expect_err("a pre-planted temp must be refused, never truncated into");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(&victim).expect("the victim must still be readable"),
            "VICTIM\n",
            "the write followed the planted symlink and destroyed the victim"
        );
        assert!(!target.exists(), "nothing may be written through the plant");
    }

    /// The name carries pid + monotonic nonce, so two writes in the SAME
    /// process (same pid) can never collide on one temp — the old pid-only
    /// name let one write rename the temp away under the other, whose
    /// rename then failed ENOENT.
    #[test]
    fn temp_path_is_unique_per_write_within_one_process() {
        let dir = tempfile::tempdir().expect("temp dir");
        let first = temp_path(dir.path(), "config.toml");
        let second = temp_path(dir.path(), "config.toml");

        assert_ne!(first, second, "two writes must never share a temp name");
        let expected_prefix = format!(".config.toml.connect-{}-", std::process::id());
        for name in [&first, &second] {
            let name = name
                .file_name()
                .expect("a temp path has a file name")
                .to_string_lossy()
                .into_owned();
            assert!(
                name.starts_with(&expected_prefix),
                "the temp must name the process and a nonce: {name}"
            );
            assert!(name.ends_with(".tmp"), "the temp keeps its suffix: {name}");
        }
        assert_eq!(
            first.parent(),
            second.parent(),
            "the temp must stay beside its target for the rename to be atomic"
        );
    }

    /// A pre-planted candidate is abandoned for the NEXT name: the write
    /// lands in a different temp (never through the plant, which keeps its
    /// bytes — unlinking it would only race its owner).
    #[test]
    fn create_temp_retries_with_a_fresh_name_when_the_candidate_is_planted() {
        let (dir, target, planted) = planted_temp();

        let mut served = 0_usize;
        let used = create_temp(
            || {
                served += 1;
                if served == 1 {
                    planted.clone()
                } else {
                    dir.path().join(format!(".fresh-{served}.tmp"))
                }
            },
            &target,
            "id = \"connect-a1b2\"\n",
        )
        .expect("the retry must land on a fresh name");

        assert_ne!(used, planted, "the write must not use the planted temp");
        assert_eq!(
            std::fs::read_to_string(&used).expect("the fresh temp is written"),
            "id = \"connect-a1b2\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(&planted).expect("the plant survives"),
            "PLANT\n",
            "a pre-planted candidate is neither truncated into nor removed"
        );
    }

    /// The retry is BOUNDED: a name planted under every candidate fails
    /// loud with `AlreadyExists` instead of silently downgrading to the
    /// old truncating open.
    #[test]
    fn create_temp_fails_loud_when_every_candidate_is_planted() {
        let (_dir, target, planted) = planted_temp();

        let mut attempts = 0_usize;
        let err = create_temp(
            || {
                attempts += 1;
                planted.clone()
            },
            &target,
            "id = \"connect-a1b2\"\n",
        )
        .expect_err("a plant under every candidate must fail loud");

        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            attempts, TEMP_CREATE_ATTEMPTS,
            "the retry must be bounded, not unbounded"
        );
        assert_eq!(
            std::fs::read_to_string(&planted).expect("the plant survives"),
            "PLANT\n",
            "even the loud failure must not remove or truncate the plant"
        );
    }

    /// A temp left by a killed run (here a foreign pid, in the pre-fix
    /// name shape) is a full copy of the operator's config: the next
    /// successful write sweeps it best-effort, and no connect temp
    /// survives the write.
    #[test]
    fn a_successful_write_sweeps_stale_connect_temps() {
        let (dir, path) = temp_config();
        let stale = dir.path().join(".config.toml.connect-999-abc.tmp");
        std::fs::write(&stale, b"leftover config copy\n").expect("seed a stale temp");
        let provider = test_provider("connect-a1b2");

        write_provider(&path, &provider).expect("write");

        assert!(!stale.exists(), "a stale sibling temp must be swept");
        let leftovers: Vec<String> =
            dir_file_names_matching(dir.path(), |name| name.starts_with(".config.toml.connect-"));
        assert!(
            leftovers.is_empty(),
            "no connect temp may survive a successful write: {leftovers:?}"
        );
        assert!(
            std::fs::read_to_string(&path)
                .expect("read the written config")
                .contains("id = \"connect-a1b2\""),
            "the sweep must not have eaten the config itself"
        );
    }

    // ---------------------------------------------------------------------
    // R4-1: two concurrent connects must never lose the loser's record
    // (read-modify-write with no lock). The bytes are re-read after the
    // splice and a drift refuses the write instead of renaming over the
    // other writer's record.
    // ---------------------------------------------------------------------

    #[test]
    fn config_drifted_is_a_byte_exact_comparison() {
        assert!(
            !config_drifted("a = 1\n", "a = 1\n"),
            "identical bytes never drift"
        );
        assert!(
            config_drifted("a = 1\n", "a = 2\n"),
            "different content is drift"
        );
        assert!(
            config_drifted("a = 1\n", "a = 1\n# otro escritor añadió una nota\n"),
            "a whitespace-only difference still means another writer landed"
        );
    }

    #[test]
    fn ensure_config_unchanged_accepts_unchanged_bytes() {
        let (_dir, path) = temp_config();
        seed_config(&path, "format = \"markdown\"\n");
        let original = std::fs::read_to_string(&path).expect("read the snapshot");

        ensure_config_unchanged(&path, Some(&original))
            .expect("identical bytes must never block the write");
    }

    /// THE lost-update regression: while connect holds its snapshot, another
    /// writer lands. The re-read differs, so the write refuses loud — the
    /// alternative is renaming a stale splice over the other writer's
    /// record while both printed success.
    #[test]
    fn ensure_config_unchanged_refuses_a_config_that_changed_on_disk() {
        let (_dir, path) = temp_config();
        seed_config(&path, "format = \"markdown\"\n");
        let original = std::fs::read_to_string(&path).expect("read the snapshot");
        std::fs::write(&path, "format = \"json\"\n# el otro connect\n")
            .expect("the other writer lands");

        let err = ensure_config_unchanged(&path, Some(&original))
            .expect_err("a config that changed on disk must refuse the write");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let message = err.to_string();
        assert!(
            message.contains("cambió en disco"),
            "the refusal must name the drift: {message}"
        );
        assert!(
            message.contains("connect de nuevo"),
            "the refusal must name the retry: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "format = \"json\"\n# el otro connect\n",
            "the refusal must leave the other writer's bytes untouched"
        );
    }

    /// The first-run race: the config was absent when connect read it and a
    /// concurrent connect created it before the rename — renaming now would
    /// clobber that record.
    #[test]
    fn ensure_config_unchanged_refuses_a_config_that_appeared_concurrently() {
        let (_dir, path) = temp_config();
        seed_config(&path, "format = \"markdown\"\n");

        let err = ensure_config_unchanged(&path, None)
            .expect_err("a config that appeared concurrently must refuse the write");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn ensure_config_unchanged_accepts_an_absent_config_that_stays_absent() {
        let (_dir, path) = temp_config();

        ensure_config_unchanged(&path, None)
            .expect("absent when read and absent now is a fresh first connect");
    }

    /// A config deleted between the read and the rename is drift too:
    /// renaming into its place would silently replace whatever landed
    /// there by then.
    #[test]
    fn ensure_config_unchanged_refuses_a_config_that_disappeared() {
        let (_dir, path) = temp_config();
        seed_config(&path, "format = \"markdown\"\n");
        let original = std::fs::read_to_string(&path).expect("read the snapshot");
        std::fs::remove_file(&path).expect("the other writer deletes it");

        let err = ensure_config_unchanged(&path, Some(&original))
            .expect_err("a config that disappeared must refuse the write");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// THE wiring regression: `write_provider` must REFUSE when the document
    /// drifted, not merely own a drift-checker. Removing the
    /// `ensure_config_unchanged` call from the commit path leaves every other
    /// test green while the lost update ships — this test pins the ordering
    /// itself. The drifted bytes are the OTHER writer's: they must survive
    /// byte-for-byte, and no record of ours may be appended over them.
    #[test]
    fn commit_splice_refuses_to_rename_over_a_config_that_drifted() {
        let (_dir, path) = temp_config();
        seed_config(&path, "format = \"json\"\n# el otro connect gano\n");
        let stale_snapshot = "format = \"markdown\"\n".to_string();
        let spliced = stale_snapshot
            .parse::<toml_edit::DocumentMut>()
            .expect("a snapshot parses");

        let err = commit_splice(&path, &spliced, Some(&stale_snapshot))
            .expect_err("a drifted config must never be renamed over");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("connect de nuevo"),
            "the refusal must name the retry: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "format = \"json\"\n# el otro connect gano\n",
            "the other writer's config must survive the refusal untouched"
        );
    }

    /// The positive half of the wiring: an unchanged config commits, so the
    /// refusal above cannot be satisfied by a commit path that always fails.
    #[test]
    fn commit_splice_writes_an_unchanged_config() {
        let (_dir, path) = temp_config();
        let original = "format = \"markdown\"\n";
        seed_config(&path, original);
        let spliced = original
            .parse::<toml_edit::DocumentMut>()
            .expect("a snapshot parses");

        commit_splice(&path, &spliced, Some(original)).expect("an unchanged config commits");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "format = \"markdown\"\n",
            "a no-drift commit writes exactly the spliced document"
        );
    }
}
