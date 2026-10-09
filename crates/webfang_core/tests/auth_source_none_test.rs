//! B1 — `AuthSource::None`: el endpoint sin credencial.
//!
//! ## Por qué es un test de integración y no unitario
//!
//! La mitad de estas filas es sobre *configuración*: que `{"source": "none"}`
//! se pueda expressing en el fichero de providers es justamente la afirmación
//! de que `domain/providers.rs` no necesita desdoblar variantes. Eso se
//! comprueba desde fuera del crate, sobre el tipo público, en la crate de test
//! — un test unitario dentro de `webfang_core` podría "probarla" contra el
//! mismo código que la define y no probaría nada del contrato de cara al
//! usuario.
//!
//! ## Qué fijan estas filas (ADR-0004 §Condición de revisión)
//!
//! 1. `None` construye el adapter remoto sin error de credencial.
//! 2. La petición NO lleva cabecera `Authorization`.
//! 3. Invariante de seguridad: con `None` **y** una variable de entorno de key
//!    válida presente, la petición sigue **sin** `Authorization` — es el caso
//!    "caer a otra fuente", que antes de esta unidad no cubría nadie.
//! 4. Un 401 se distingue según se mandara credencial o no, para que un 401 de
//!    provider anónimo no se lea ni se reporte como "credencial inválida".
//! 5. `Env { var }` no seteada sigue fallando, con el mismo mensaje.
//! 6. Loopback sigue permitido con el opt-in de config.

#![allow(clippy::disallowed_methods)] // via EnvGuard, el dueño sancionado de mutación de env

use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::llm_wire::build_llm_provider;
use webfang_core::domain::auth_source::{AuthError, AuthSource};
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind, ProvidersConfig};
use webfang_core::domain::EmbeddingPort;
use webfang_core::error::SemanticError;
use webfang_core::infrastructure::llm::provider::{OpenAiCompatibleProvider, ProviderInitError};
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
use webfang_core::CliExit;
use webfang_test_utils::EnvGuard;

/// Un vector 8d: los fixtures del repo usan esta misma forma.
const ONE_VECTOR_8D: &str = r#"{"data": [{"index": 0,
    "embedding": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]}]}"#;

/// Ruta del endpoint de embeddings. Nombrada para que el mock y las aserciones
/// no puedan desincronizarse por una cadena repetida en dos sitios.
const EMBEDDINGS_PATH: &str = "/embeddings";

/// Un `ProviderConfig` de embeddings apuntando al wiremock, con la fuente de
/// auth que cada fila necesita.
fn config_for(server: &wiremock::MockServer, auth: AuthSource) -> ProviderConfig {
    ProviderConfig {
        id: "anon-endpoint".to_string(),
        display_name: "Anonymous Endpoint".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: url::Url::parse(&server.uri()).expect("mock uri parses"),
        auth,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        embedding_dim: None,
        // Loopback es el wiremock; el opt-in de config es lo que lo permite.
        allow_loopback: true,
    }
}

async fn mount_embeddings(
    server: &wiremock::MockServer,
    status: u16,
    body: &str,
) -> wiremock::MockGuard {
    let response = wiremock::ResponseTemplate::new(status).set_body_string(body);
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(EMBEDDINGS_PATH))
        .respond_with(response)
        .mount_as_scoped(server)
        .await
}

/// Un `ProviderConfig` de embeddings para las filas que sólo observan la
/// construcción (no hacen ninguna petición): no necesitan un servidor real,
/// y una base URL pública basta porque la fila no llega a abrir un socket.
fn config_without_server(auth: AuthSource) -> ProviderConfig {
    ProviderConfig {
        id: "anon-endpoint".to_string(),
        display_name: "Anonymous Endpoint".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: url::Url::parse("https://api.example.com/v1").expect("url"),
        auth,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        embedding_dim: None,
        allow_loopback: false,
    }
}

/// La cabecera `Authorization` tal como la vio el servidor, si la hubo.
async fn recorded_authorization(server: &wiremock::MockServer) -> Vec<Option<String>> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|req| {
            req.headers
                .get("authorization")
                .map(|v| v.to_str().unwrap_or_default().to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. La expresión desde el fichero de configuración
// ---------------------------------------------------------------------------

/// `AuthSource` es `#[serde(tag = "source")]`, así que `{"source": "none"}`
/// tiene que bastar. Esta fila es la prueba de que `domain/providers.rs`
/// (fuera de las superficies de esta unidad, y deliberadamente sin tocar)
/// declara `pub auth: AuthSource` y no desdobla variantes: si alguien lo
/// cambiara para "soportar" `None`, esta fila documentaría el punto exacto.
#[test]
fn providers_config_parses_source_none_from_the_config_file() {
    let raw = r#"{
      "providers": [
        {
          "id": "local-embed",
          "display_name": "Embeddings locales",
          "kind": "open_ai_compatible",
          "base_url": "http://127.0.0.1:11434/v1",
          "auth": { "source": "none" },
          "capabilities": ["embedding"],
          "model": "nomic-embed-text",
          "allow_loopback": true
        }
      ]
    }"#;
    let parsed: ProvidersConfig =
        serde_json::from_str(raw).expect("config con auth none debe parsear");
    assert_eq!(parsed.len(), 1);
    let provider = &parsed.providers[0];
    assert!(
        matches!(provider.auth, AuthSource::None),
        "auth debe deserializar a la variante None, no a otra: {:?}",
        provider.auth
    );
}

/// Round-trip: el wizard escribe la config y la relee sin perder el sentido.
#[test]
fn source_none_round_trips_through_serialization() {
    let source = AuthSource::None;
    let json = serde_json::to_string(&source).expect("serializa");
    assert_eq!(json, r#"{"source":"none"}"#);
    let back: AuthSource = serde_json::from_str(&json).expect("reparsea");
    assert!(matches!(back, AuthSource::None));
}

/// `{"source": "none"}` no es un alias de "sin `auth`": el campo sigue siendo
/// obligatorio y explícito. Un provider sin `auth` no parsea, que es la
/// diferencia entre "endpoint anónimo declarado" y "config incompleta".
#[test]
fn omitting_auth_still_fails_rather_than_implying_none() {
    let raw = r#"{
      "providers": [
        {
          "id": "no-auth",
          "display_name": "Sin auth",
          "kind": "open_ai_compatible",
          "base_url": "http://127.0.0.1:11434/v1",
          "capabilities": ["embedding"],
          "model": "m"
        }
      ]
    }"#;
    let parsed: Result<ProvidersConfig, _> = serde_json::from_str(raw);
    assert!(
        parsed.is_err(),
        "omitir `auth` debe fallar: `None` es explícito, no un default implícito"
    );
}

// ---------------------------------------------------------------------------
// 2-3. Construcción sin credencial + la petición NO lleva Authorization
// ---------------------------------------------------------------------------

/// AC1: `auth: None` construye el adapter sin error de credencial.
#[tokio::test]
async fn none_auth_builds_the_adapter_without_a_credential_error() {
    let server = wiremock::MockServer::start().await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = match RemoteEmbeddingAdapter::new(cfg) {
        Ok(a) => a,
        Err(e) => panic!("auth none debe construir el adapter, falló: {e}"),
    };
    assert_eq!(adapter.id(), "anon-endpoint");
}

/// AC2 + AC3: lo que esta fila prueba es que la petición **no lleva
/// cabecera `Authorization`** — se lee en el servidor lo que llegó, no una
/// inspección del código.
///
/// La variable `WEBFANG_TEST_NONE_TRAP_KEY` es **decorativa** a propósito:
/// `AuthSource::None` no nombra ninguna variable de entorno, así que
/// setearla no prueba por sí sola que no haya fallback. Queda como red de
/// contención para el caso concreto de una regresión que cayera a *esa*
/// variable; no es —ni pretende ser— prueba de la garantía general de
/// "no fallback". La garantía general la sostiene la aserción observable de
/// abajo, junto con el control positivo `env_auth_still_sends_the_bearer_header`
/// (que demuestra que el helper sí detecta un bearer cuando existe).
#[tokio::test]
async fn none_auth_request_carries_no_authorization_header() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_NONE_TRAP_KEY", "sk-must-not-be-used")]);
    let _mock = mount_embeddings(&server, 200, ONE_VECTOR_8D).await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter con auth none");

    let vectors = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect("el endpoint anónimo responde 200");
    assert_eq!(vectors.len(), 1);

    let seen = recorded_authorization(&server).await;
    assert_eq!(seen.len(), 1, "debe haberse registrado una petición");
    assert!(
        seen[0].is_none(),
        "auth None NUNCA debe sintetizar cabecera Authorization; se vio: {:?}",
        seen[0]
    );
}

/// Control positivo del anterior: con `Env { var }` resuelta, la cabecera SÍ
/// viaja. Sin esta fila, "no hay Authorization" podría pasar por un cliente
/// que nunca manda auth en absoluto.
#[tokio::test]
async fn env_auth_still_sends_the_bearer_header() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_ENV_BEARER_KEY", "sk-expected")]);
    let _mock = mount_embeddings(&server, 200, ONE_VECTOR_8D).await;
    let cfg = config_for(
        &server,
        AuthSource::Env {
            var: "WEBFANG_TEST_ENV_BEARER_KEY".to_string(),
        },
    );
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter con auth env");

    adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect("el endpoint responde 200");

    let seen = recorded_authorization(&server).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].as_deref(),
        Some("Bearer sk-expected"),
        "auth env debe seguir firmando la petición"
    );
}

// ---------------------------------------------------------------------------
// 4. Un 401 se distingue según haya travelled credencial o no
// ---------------------------------------------------------------------------

/// AC4 (fila A): 401 de provider **sin** credencial. El mensaje tiene que
/// decir que la petición era anónima — si dijera "credencial inválida"
/// mandaría al operador a rotar una key que nunca existió.
#[tokio::test]
async fn unauthorized_without_credential_is_not_reported_as_an_invalid_credential() {
    let server = wiremock::MockServer::start().await;
    let _mock = mount_embeddings(&server, 401, r#"{"error":"unauthorized"}"#).await;
    let cfg = config_for(&server, AuthSource::None);
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter con auth none");

    let err = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("401 debe fallar");
    let msg = err.to_string();

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "sigue siendo Inference (mismo tipo que hoy), cambió el mensaje: {msg}"
    );
    assert!(
        msg.contains("401"),
        "el mensaje debe nombrar el estado observado: {msg}"
    );
    assert!(
        !msg.contains("inválida") && !msg.contains("invalida"),
        "un 401 anónimo NO es una credencial inválida, el mensaje no puede decirlo: {msg}"
    );
    assert!(
        msg.contains("none") || msg.contains("anónim") || msg.contains("anonim"),
        "el mensaje debe declarar que la petición iba sin credencial: {msg}"
    );
}

/// AC4 (fila B): 401 de provider **con** credencial. Distinto del anterior,
/// y explícitamente un rechazo de la credencial que sí se mandó.
#[tokio::test]
async fn unauthorized_with_credential_is_reported_as_a_rejected_credential() {
    let server = wiremock::MockServer::start().await;
    let _guard = EnvGuard::with(&[("WEBFANG_TEST_401_KEY", "sk-rejected")]);
    let _mock = mount_embeddings(&server, 401, r#"{"error":"unauthorized"}"#).await;
    let cfg = config_for(
        &server,
        AuthSource::Env {
            var: "WEBFANG_TEST_401_KEY".to_string(),
        },
    );
    let adapter = RemoteEmbeddingAdapter::new(cfg).expect("adapter con auth env");

    let err = adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("401 debe fallar");
    let msg = err.to_string();

    assert!(msg.contains("401"), "{msg}");
    assert!(
        msg.contains("credencial") || msg.contains("env"),
        "el mensaje debe señalar que había credencial y su fuente: {msg}"
    );

    // Las dos filas de AC4 tienen que ser distinguibles entre sí: es el
    // punto del criterio, no dos mensajes que se parecen.
    let anonymous_server = wiremock::MockServer::start().await;
    let anon_mock = mount_embeddings(&anonymous_server, 401, r#"{"error":"unauthorized"}"#).await;
    let anon_adapter = RemoteEmbeddingAdapter::new(config_for(&anonymous_server, AuthSource::None))
        .expect("adapter con auth none");
    let anon_err = anon_adapter
        .embed_batch(&["ping".to_string()])
        .await
        .expect_err("401 anónimo debe fallar");
    assert_ne!(
        anon_err.to_string(),
        msg,
        "el 401 anónimo y el 401 con credencial no pueden renderizar el mismo texto"
    );
    drop(anon_mock);
}

// ---------------------------------------------------------------------------
// 5. Una credencial configurada pero no resoluble sigue siendo error duro
// ---------------------------------------------------------------------------

/// Invariante 2: sin fallback. `Env { var }` sin la variable seteada NO
/// degrada a "sin auth" — falla duro, y con el mismo mensaje de siempre.
#[test]
fn unresolvable_env_credential_still_fails_hard_without_degrading_to_none() {
    let _guard = EnvGuard::clean(&["WEBFANG_TEST_UNSET_CREDENTIAL"]);
    let cfg = config_without_server(AuthSource::Env {
        var: "WEBFANG_TEST_UNSET_CREDENTIAL".to_string(),
    });

    let err = match RemoteEmbeddingAdapter::new(cfg) {
        Err(e) => e,
        Ok(_) => panic!("una credencial no resoluble debe seguir fallando"),
    };
    assert!(
        matches!(err, ProviderInitError::AuthFailed { .. }),
        "debe seguir siendo AuthFailed, no una degradación silenciosa: {err}"
    );
    match &err {
        ProviderInitError::AuthFailed { source, .. } => assert!(
            matches!(source, AuthError::EnvNotSet(_)),
            "y con la variante de error de siempre: {source}"
        ),
        other => panic!("variante inesperada: {other}"),
    }
    assert!(
        err.to_string().contains("WEBFANG_TEST_UNSET_CREDENTIAL"),
        "el mensaje nombra la variable: {err}"
    );
}

// ---------------------------------------------------------------------------
// 7. HUECO 1 — el camino de completion, alcanzado por config de usuario real
// ---------------------------------------------------------------------------

/// Un fichero de providers real (JSON, tal cual lo escribiría el usuario)
/// con capability `completion` y `auth: {source: none}`. Nada de esto se
/// construye a mano en el test: todo pasa por el deserializador.
const COMPLETION_ANON_CONFIG: &str = r#"{
  "providers": [
    {
      "id": "anon-chat",
      "display_name": "Chat anónimo",
      "kind": "open_ai_compatible",
      "base_url": "https://api.example.com/v1",
      "auth": { "source": "none" },
      "capabilities": ["completion"],
      "model": "gpt-test"
    }
  ]
}"#;

fn llm_opts(provider_id: &str) -> CrawlOptions {
    CrawlOptions {
        extract_with_llm: true,
        llm_provider: Some(provider_id.to_string()),
        ..Default::default()
    }
}

/// Ruta 1 — la vía que un usuario realmente toca: `--extract-with-llm`
/// contra un provider anónimo declarado en el fichero de configuración.
/// Falla en startup (`ConfigError`, exit 78), y el mensaje declara el
/// endpoint anónimo en vez de insinuar una credencial rota.
#[test]
fn completion_provider_with_auth_none_fails_at_startup_with_the_anonymous_message() {
    let providers: ProvidersConfig =
        serde_json::from_str(COMPLETION_ANON_CONFIG).expect("config de completion anónimo parsea");

    let err = match build_llm_provider(&llm_opts("anon-chat"), &providers) {
        Err(e) => e,
        Ok(_) => panic!("un provider de completion anónimo debe fallar en startup"),
    };
    let msg = match &err {
        CliExit::ConfigError(msg) => msg,
        other => panic!("debe ser ConfigError (exit 78), no: {other:?}"),
    };
    assert!(
        msg.contains("anon-chat"),
        "el mensaje nombra el provider que falló: {msg}"
    );
    assert!(
        msg.contains("anónimo"),
        "el mensaje declara que el endpoint es anónimo: {msg}"
    );
    assert!(
        !msg.contains("no se pudo resolver credencial"),
        "ese texto pertenece a AuthFailed, la OTRA variante: {msg}"
    );
}

/// La variante tipada, sobre el `ProviderConfig` **que sale del fichero de
/// configuración** (no una construcción artificial del enum): fija el
/// contrato de `provider.rs` y no sólo el texto que la CLI proyecta.
#[test]
fn completion_provider_with_auth_none_yields_the_anonymous_variant_not_auth_failed() {
    let providers: ProvidersConfig =
        serde_json::from_str(COMPLETION_ANON_CONFIG).expect("config de completion anónimo parsea");
    let config = providers.providers[0].clone();
    assert!(
        matches!(config.auth, AuthSource::None),
        "la config del fichero debe traer auth none"
    );
    assert!(config.has_capability(Capability::Completion));

    let err = match OpenAiCompatibleProvider::new(config) {
        Err(e) => e,
        Ok(_) => panic!("auth none en completion debe fallar"),
    };
    match &err {
        ProviderInitError::AnonymousCompletionUnsupported { provider_id } => {
            assert_eq!(provider_id, "anon-chat");
        },
        other => panic!("esperaba AnonymousCompletionUnsupported, obtuve: {other}"),
    }
    // Invariante 4: `None` NO es un fallo de credencial, ni aquí ni ahora.
    assert!(
        !matches!(err, ProviderInitError::AuthFailed { .. }),
        "None jamás debe mapearse a AuthFailed: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("auth: none"),
        "el mensaje nombra la causa: {msg}"
    );
    // El texto completo es de cara al usuario, y AGENTS.md exige español.
    // Asertar sólo `auth: none` dejaría pasar un mensaje que perdiera la
    // mitad que explica POR QUÉ el camino anónimo no está soportado — que
    // es la parte accionable para quien configure el provider.
    assert!(
        msg.contains("todavía no soportado por el camino de chat/completions"),
        "el mensaje debe explicar que el chat/completions anónimo no está soportado todavía: {msg}"
    );
}

/// Ruta 2 — **la segunda ruta de error del mismo escenario**, medida: si el
/// provider anónimo declara `kind: local_onnx`, la resolución del slot de
/// completion **pasa** (tiene capability `completion`) y el fallo ocurre
/// después, por kind. Es un mensaje distinto y con otra causa, así que
/// necesita su propia fila: sin ella, un cambio que moviera el orden de
/// estas dos comprobaciones pasaría inadvertido.
#[test]
fn completion_onnx_kind_with_auth_none_fails_with_the_kind_message_not_the_auth_one() {
    let raw = r#"{
      "providers": [
        {
          "id": "anon-pool",
          "display_name": "Pool anónimo",
          "kind": "local_onnx",
          "base_url": "http://127.0.0.1:11434/v1",
          "auth": { "source": "none" },
          "capabilities": ["completion"],
          "model": "local"
        }
      ]
    }"#;
    let providers: ProvidersConfig = serde_json::from_str(raw).expect("parsea");

    let err = match build_llm_provider(&llm_opts("anon-pool"), &providers) {
        Err(e) => e,
        Ok(_) => panic!("local_onnx no es una fuente de completions"),
    };
    let msg = match &err {
        CliExit::ConfigError(msg) => msg,
        other => panic!("debe ser ConfigError (exit 78), no: {other:?}"),
    };
    assert!(
        msg.contains("local_onnx"),
        "esta ruta falla por kind: {msg}"
    );
    assert!(
        !msg.contains("anónimo"),
        "el mensaje no debe atribuir el fallo a la auth cuando la causa es el kind: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 6. Loopback y offline: no se rompen
// ---------------------------------------------------------------------------

/// Loopback (`127.0.0.1`) sigue permitido con el opt-in de config — la fila
/// de SSRF no se toca por añadir una fuente de auth que no lleva credencial.
#[test]
fn loopback_with_opt_in_still_builds_under_auth_none() {
    let mut cfg = config_without_server(AuthSource::None);
    cfg.base_url = url::Url::parse("http://127.0.0.1:9/v1").expect("url");
    cfg.allow_loopback = true;

    let adapter = match RemoteEmbeddingAdapter::new(cfg) {
        Ok(a) => a,
        Err(e) => panic!("loopback con opt-in debe seguir construyendo: {e}"),
    };
    assert_eq!(adapter.id(), "anon-endpoint");
}

/// Y sin el opt-in sigue bloqueado: `auth: None` no es una vía para relajar
/// el SSRF guard.
#[test]
fn loopback_without_opt_in_is_still_blocked_under_auth_none() {
    let mut cfg = config_without_server(AuthSource::None);
    cfg.base_url = url::Url::parse("http://127.0.0.1:9/v1").expect("url");
    cfg.allow_loopback = false;

    let err = match RemoteEmbeddingAdapter::new(cfg) {
        Err(e) => e,
        Ok(_) => panic!("loopback sin opt-in debe seguir bloqueado"),
    };
    assert!(
        matches!(err, ProviderInitError::InvalidBaseUrl { .. }),
        "auth none no debe cambiar la decisión del SSRF guard: {err}"
    );
}
