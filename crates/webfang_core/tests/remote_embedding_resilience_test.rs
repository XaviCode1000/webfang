//! Resiliencia de `RemoteEmbeddingAdapter` contra un endpoint remoto hostil.
//!
//! ## Por qué estas filas son de integración y no unitarias
//!
//! `remote_embedding.rs` ya tiene un módulo de tests unitarios que cubre la
//! capa pura (parseo del cuerpo, política de backoff) y varias filas de wire.
//! Lo que **no** cubría — y lo que el ADR-0004 exige — es la mitad de la
//! cadena de guarda del lado del servidor: la clasificación de reintentos
//! cuando el endpoint responde 429 con `Retry-After`, 5xx sostenido, cuerpo
//! degenerado, orden de batch y nombre ilegible. Estas filas entran por el
//! puerto público (`EmbeddingPort` sobre `webfang_core`), que es lo que el
//! CLI y el MCP usan de verdad, y por eso una regresión en la firma o en el
//! cable las rompe aunque el módulo interno siga verde.
//!
//! ## Qué fijan (ADR-0004 §Condición de revisión, suite wiremock del PR-8)
//!
//! 1. 429 con `Retry-After` en segundos → recupera tras respetarlo.
//! 2. 429 con `Retry-After` en fecha HTTP → se honra por el cable: una fecha
//!    ya pasada se recorta a cero (y el reintento sale en ~1s, el suelo
//!    exponencial) en vez de dormirse los ~32 años que pide la cabecera; una
//!    cabecera inservible degrada a backoff exponencial sin convertir el
//!    reintento en un error. La conversión pura —las dos formas de RFC 9110
//!    §10.2.3, con `now` inyectado— está cubierta en el módulo de tests
//!    in-crate de `remote_embedding.rs`, que es donde una función pura y
//!    privada se prueba; aquí lo que se afirma es el comportamiento
//!    observable a través del puerto público.
//! 3. 5xx sostenido → agota exactamente `EMBEDDING_MAX_ATTEMPTS` intentos y
//!    reporta el **último** estado observado, nunca uno fijo.
//! 4. Respuesta malformada (JSON inválido, `data` vacío, descuadre de
//!    conteo) → `SemanticError::Inference` con el mensaje en español.
//! 5. Orden de batch → `input` conserva el orden de entrada y los vectores
//!    vuelven posicionalmente 1:1; `index` se parsea pero **no** reordena.
//! 6. DNS → nombre de TLD reservado: error de transporte, nunca un panic.

use std::time::Duration;

use webfang_core::domain::auth_source::AuthSource;
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
use webfang_core::domain::EmbeddingPort;
use webfang_core::error::SemanticError;
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Ruta del endpoint de embeddings, compartida por los mocks y por el
/// `path()` matcher: una cadena repetida en dos sitios es una cadena que
/// puede desincronizarse sin que nada falle.
const EMBEDDINGS_PATH: &str = "/embeddings";

/// TLD reservado por RFC 2606: `nonexistent.invalid` **no** resuelve nunca,
/// así que la fila de DNS no depende de la red real ni de un wildcard del
/// entorno. (A diferencia de un nombre inventado en `.com`, que sí podría
/// ser capturado por un resolver con wildcard.)
const UNRESOLVABLE_HOST: &str = "http://nonexistent.invalid/v1";

/// `ProviderConfig` de embeddings apuntando al wiremock, anónimo.
///
/// `AuthSource::None` y no un `EnvGuard`: esta suite no afirma nada sobre
/// credenciales (eso es territorio de `auth_source_none_test.rs`), así que
/// evita mutar el entorno y se queda con un Arrange de una línea.
fn anonymous_config(base_url: String) -> ProviderConfig {
    ProviderConfig {
        id: "resilience-endpoint".to_string(),
        display_name: "Resilience Endpoint".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: url::Url::parse(&base_url).expect("base_url parses"),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        embedding_dim: None,
        // Loopback es el wiremock; el opt-in de config es lo que lo permite.
        allow_loopback: true,
    }
}

/// Un vector de una dimensión, con el valor que lo identifica: la ordenación
/// posicional se afirma sobre valores distintos, no sobre longitudes iguales.
fn one_d(scalar: f32) -> String {
    format!(r#"{{"data":[{{"embedding":[{scalar}]}}]}}"#)
}

/// Counts the requests the server actually saw.
async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

/// El adapter anónimo bajo prueba, ya construido contra el servidor.
fn adapter_for(server: &MockServer) -> RemoteEmbeddingAdapter {
    RemoteEmbeddingAdapter::new(anonymous_config(server.uri())).expect("anonymous adapter builds")
}

/// Monta un `status` constante en el endpoint de embeddings.
async fn mount_status(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS_PATH))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// Monta un 429 de **un solo intento** seguido de un 200 de cortesía.
///
/// El 429 se monta primero porque wiremock evalúa los mocks en orden de
/// montaje: la primera petición recibe el 429 y el reintento cae en el 200
/// montado después. `retry_after` es el valor crudo de la cabecera —
/// `None` la omite, que es el caso de un servidor que no la manda.
async fn mount_429_then_ok(server: &MockServer, retry_after: Option<&str>) {
    let mut throttled = ResponseTemplate::new(429);
    if let Some(value) = retry_after {
        throttled = throttled.insert_header("retry-after", value);
    }
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS_PATH))
        .respond_with(throttled.set_body_string("límite alcanzado"))
        .up_to_n_times(1)
        .mount(server)
        .await;
    mount_status(server, 200, &one_d(0.1)).await;
}

// ---------------------------------------------------------------------------
// 1. 429 con Retry-After en segundos
// ---------------------------------------------------------------------------

/// Un 429 con `Retry-After` en delay-seconds se respeta **por encima** del
/// suelo exponencial, y el reintento recupera cuando el endpoint pasa a 200.
///
/// El valor es `2`, no `1`, por una razón que la fila tiene que demostrar:
/// el backoff exponencial del primer intento es exactamente 1000 ms, así que
/// un `Retry-After: 1` sería indistinguible de ignorarlo — la fila pasaría
/// igual con el arreglo dentro que fuera. Con `2` las dos hipótesis divergen
/// (2000 ms honrado contra 1000 ms si se descarta) y la aserción del suelo
/// inferior decide entre ellas. Por eso el test mide tiempo: es la única
/// señal por la que el honor del wire es observable desde fuera, ya que la
/// conversión pura vive en un módulo in-crate.
#[tokio::test]
async fn status_429_with_retry_after_seconds_recovers_on_retry() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("2")).await;
    let started = std::time::Instant::now();

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("429 + Retry-After: 2 recovers on the next attempt");

    assert_eq!(vecs.len(), 1);
    assert_eq!(
        request_count(&server).await,
        2,
        "exactly one 429 plus one recovery attempt"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "the server asked for 2s, which must beat the 1000ms exponential \
         floor — a faster retry means Retry-After was discarded; took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 2. 429 con Retry-After en fecha HTTP
// ---------------------------------------------------------------------------

/// La fila de wire de la fecha HTTP: un 429 que manda una fecha **ya
/// pasada** debe recortar a cero y recuperar, en vez de intentar dormir los
/// ~32 años de diferencia (o, en el código viejo, caerse al backoff sin
/// decir nada). El aserto es de dos lados: por debajo, un segundo es el suelo
/// exponencial y prueba que **hubo** un reintento real y no un bucle
/// instantáneo; por arriba, treinta segundos son el techo —un orden de
/// magnitud por encima del ~1s real, con hueco para un runner de CI cargado—
/// y prueban que no se respetó la fecha al pie de la letra: si se respetara,
/// el test no terminaría nunca.
///
/// Lo que la fila **no** puede ver es si el recorte fue a cero o una caída a
/// backoff exponencial: por el cable ambos producerán ~1s. Esa distinción la
/// fija el test puro del módulo in-crate, con `now` inyectado; aquí se afirma
/// el comportamiento observable.
#[tokio::test]
async fn status_429_with_past_http_date_clamps_and_recovers() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("Sun, 06 Nov 1994 08:49:37 GMT")).await;
    let started = std::time::Instant::now();

    adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("a past HTTP-date clamps to zero and the retry recovers");

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "a past HTTP-date must clamp to the exponential floor (~1s), not to \
         the ~32-year delta the header literally asks for; the 30s bound is an \
         order of magnitude above the real ~1s and leaves headroom on a loaded \
         CI runner — the discriminating power is the magnitude, not the exact \
         ceiling; took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(900),
        "the retry must go through the backoff floor, not a hot loop; took {elapsed:?}"
    );
    assert_eq!(request_count(&server).await, 2);
}

/// Una cabecera de `Retry-After` inservible degrada a backoff exponencial y
/// **se dice** — el `warn!` estructurado lleva `provider_id`, `raw` y
/// `reason`. Lo observable desde fuera es que la fila no se cuelga ni falla:
/// la degradación es un reintento, nunca un error.
#[tokio::test]
async fn unparsable_retry_after_degrades_without_failing_the_request() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some("pronto")).await;

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("an unusable Retry-After degrades to backoff, it is not an error");

    assert_eq!(vecs.len(), 1);
    assert_eq!(request_count(&server).await, 2);
}

/// La fila que **sí** distingue el arreglo de la regresión.
///
/// Las otras dos filas de esta sección usan valores que no separan las dos
/// hipótesis: un `retry-after: "1"` coincide con el suelo exponencial, y una
/// fecha ya pasada produce ~1s tanto si se recorta a cero como si se
/// descarta. Lo que separa "se honra" de "se ignora" es una fecha **futura**:
/// honrada duerme lo que pide, ignorada cae al backoff exponencial.
///
/// El header se construye a `now + 4s` con `chrono`, y la aserción pide al
/// menos 3s. Ese margen absorbe el truncamiento a segundos del formato
/// IMF-fixdate y los milisegundos entre construir el header y leerlo, sin
/// dar margen al bug (que produciría ~1s).
#[tokio::test]
async fn status_429_with_future_http_date_waits_what_the_server_asked() {
    // RFC 9110 §10.2.3: IMF-fixdate es la forma que los servidores envían.
    let future = chrono::Utc::now() + Duration::from_secs(4);
    let header = future.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let server = MockServer::start().await;
    mount_429_then_ok(&server, Some(&header)).await;
    let started = std::time::Instant::now();

    let vecs = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect("a future HTTP-date is honored and the retry recovers");

    assert_eq!(vecs.len(), 1);
    assert_eq!(request_count(&server).await, 2);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "the server asked for ~{header}, which must beat the 1000ms \
         exponential floor — a ~1s retry means the HTTP-date was discarded \
         (the pre-fix behavior); took {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 3. 5xx sostenido
// ---------------------------------------------------------------------------

/// Un 5xx se clasifica como reintentable y agota el presupuesto completo de
/// intentos (4: el inicial más 3 reintentos). El error nombra el estado
/// **último observado**: es la regla de la cadena de guarda ("retries
/// exhausted report the LAST observed status, never a hardcoded one"), y por
/// eso el mock devuelve 503 — un código distinto del 500 de otras filas —:
/// si el adapter quemara un estado fijo, esta fila lo delataría.
#[tokio::test]
async fn persistent_5xx_spends_full_budget_and_names_the_last_status() {
    let server = MockServer::start().await;
    mount_status(&server, 503, "no disponible").await;

    let err = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a persistent 503 must fail after the attempt budget");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "exhaustion is Inference, got: {err}"
    );
    assert!(
        err.to_string().contains("503"),
        "the error must name the last observed status, got: {err}"
    );
    assert_eq!(
        request_count(&server).await,
        4,
        "the budget is the initial attempt plus three retries"
    );
}

// ---------------------------------------------------------------------------
// 4. Respuesta malformada
// ---------------------------------------------------------------------------

/// Un cuerpo degenerado es un [`SemanticError::Inference`] con el mensaje en
/// español que ve el usuario final — no un `unwrap`, no un `panic`, y no un
/// `200` que se reporta como éxito con cero vectores.
///
/// Se afirma el texto exacto de cada caso porque es la superficie que el
/// usuario lee; un matcher genérico ("contains Inference") no distinguiría
/// un cuerpo vacío de un descuadre de conteo.
#[tokio::test]
async fn malformed_body_names_its_own_failure_in_spanish() {
    // 1. JSON inválido: ni siquiera hay documento que parsear.
    assert_malformed("esto no es json", "cuerpo inválido").await;

    // 2. `data` vacío: un 200 que no trae nada que servir.
    assert_malformed(r#"{"data":[]}"#, "`data` vacío").await;

    // 3. Descuadre de conteo: 2 vectores para 1 texto. El mensaje nombra el
    //    conteo servido *y* el esperado, que es lo que permite al operador
    //    ver si el vendor truncó.
    assert_malformed(
        r#"{"data":[{"embedding":[0.1]},{"embedding":[0.2]}]}"#,
        "2 vectores para 1 textos",
    )
    .await;
}

/// Monta un 200 con `body`, invoca el puerto y comprueba que el fallo es
/// `Inference` y que el mensaje contiene `expected_message`.
///
/// Es un helper y no tres tests porque el Arrange (montar el mismo mock y
/// repetir la misma invocación) es idéntico en los tres casos: lo que varía
/// —el cuerpo degenerado— es el dato, no el escenario.
async fn assert_malformed(body: &str, expected_message: &str) {
    let server = MockServer::start().await;
    mount_status(&server, 200, body).await;

    let err = adapter_for(&server)
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a degenerate body must not be served as a success");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "malformed body must be Inference, got: {err}"
    );
    assert!(
        err.to_string().contains(expected_message),
        "message must name the failure cause ({expected_message}), got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 5. Orden de batch
// ---------------------------------------------------------------------------

/// El orden es **posicional**: `input` viaja en el orden en que se le pasó y
/// los vectores vuelven en el mismo orden de la respuesta.
///
/// El cuerpo que devuelve el mock trae los índices `index` **invertidos**
/// (`2, 1, 0`) a propósito. `EmbeddingDatum::index` se parsea para tolerar
/// el wire de cada vendor pero está deliberadamente sin uso: si algún día
/// alguien lo usara para reordenar, esta fila —que devuelve 0.3, 0.2, 0.1 en
/// ese orden de cable— lo delataría, porque el resultado esperado es
/// posicional y no indexado.
#[tokio::test]
async fn batch_preserves_input_order_and_returns_vectors_positionally() {
    let server = MockServer::start().await;
    mount_status(
        &server,
        200,
        r#"{"data":[
            {"index":2,"embedding":[0.3]},
            {"index":1,"embedding":[0.2]},
            {"index":0,"embedding":[0.1]}
        ]}"#,
    )
    .await;

    let inputs = ["uno".to_string(), "dos".to_string(), "tres".to_string()];
    let vecs = adapter_for(&server)
        .embed_batch(&inputs)
        .await
        .expect("a well-formed batch is served");

    // El request sale en el orden de entrada.
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        1,
        "one request per batch, never one per text"
    );
    let sent: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the request body is JSON");
    assert_eq!(
        sent["input"],
        serde_json::json!(["uno", "dos", "tres"]),
        "input must travel in the caller's order"
    );

    // Y los vectores vuelven posicionalmente: en el orden del cable, no en el
    // de `index` (que aquí está deliberadamente invertido).
    assert_eq!(
        vecs,
        vec![vec![0.3], vec![0.2], vec![0.1]],
        "vectors are positional 1:1; `index` is parsed but never re-sorts"
    );
}

// ---------------------------------------------------------------------------
// 6. DNS
// ---------------------------------------------------------------------------

/// Un nombre de TLD reservado no resuelve: la petición falla en transporte y
/// se reporta con el mensaje en español que nombra el endpoint.
///
/// `entry_gate` deja pasar el host porque sólo comprueba esquema e IP
/// literal; la resolución es trabajo del resolver validado del cliente
/// guardado, así que el fallo aparece en la capa de transporte — el mismo
/// brazo que un connection refused.
///
/// Nota de entorno: `.invalid` está reservado por RFC 2606 y no puede
/// resolver, así que la fila no depende de la red real ni de un wildcard del
/// entorno. Lo único dependiente del entorno es la *rapidez* del NXDOMAIN: un
/// resolver que demorase la respuesta llevaría el intento al
/// `connect_timeout` de 10s — y un timeout sí es reintentable, así que la
/// fila tardaría más sin cambiar de veredicto.
#[tokio::test]
async fn unresolvable_host_reports_a_transport_error() {
    let adapter = RemoteEmbeddingAdapter::new(anonymous_config(UNRESOLVABLE_HOST.to_string()))
        .expect("a reserved hostname passes the entry gate: no literal IP");

    let err = adapter
        .embed_batch(&["uno".to_string()])
        .await
        .expect_err("a reserved TLD never resolves");

    assert!(
        matches!(err, SemanticError::Inference(_)),
        "a DNS failure must be Inference, got: {err}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains("error de transporte"),
        "a resolution failure belongs to the transport arm, got: {rendered}"
    );
    assert!(
        rendered.contains("resilience-endpoint"),
        "the error must name the provider, got: {rendered}"
    );
}
