//! `OpenAiCompatibleProvider` — composición de `ProviderConfig` + cliente HTTP
//! compartido + credencial resuelta (doc `ai-providers-design.md`).
//!
//! Contrato fijado en revisión:
//! - `AuthSource::resolve()` se resuelve UNA VEZ en el constructor: falla en
//!   startup, no en la primera llamada.
//! - `AuthError` se convierte en [`ProviderInitError`] con `provider_id` en el
//!   mensaje — nunca se propaga crudo hasta el Container.
//! - El cliente HTTP se comparte a nivel proceso vía
//!   [`build_default_http_client`]; el provider lo acepta con `with_http`.

use crate::domain::auth_source::{AuthError, AuthSource};
use crate::domain::llm_port::LlmPort;
use crate::infrastructure::llm::client::OpenAiLlmClient;
use url::Url;

/// Error de inicialización de un provider.
///
/// Convierte `AuthError` preservando contexto (`provider_id`) para que el
/// Container/reportes no pierdan el origen. Los mensajes son user-facing
/// (español, por convención del repo).
#[derive(Debug, thiserror::Error)]
pub enum ProviderInitError {
    /// La credencial no pudo resolverse al construir el provider (falla en
    /// startup, no en la primera llamada).
    #[error("no se pudo resolver credencial para provider '{provider_id}': {source}")]
    AuthFailed {
        /// Identificador del provider que falló (contexto del Container).
        provider_id: String,
        /// Error original de resolución (fuente preservada).
        #[source]
        source: AuthError,
    },
    /// URL base inválida. Reservada para la carga de configuración (parse de
    /// la URL desde string en el wizard/container); el constructor con `Url`
    /// ya validada no la produce.
    #[error("URL base inválida para provider '{provider_id}': {source}")]
    InvalidBaseUrl {
        /// Identificador del provider cuya URL falló.
        provider_id: String,
        /// Error original del parse/construcción.
        #[source]
        source: crate::error::ScraperError,
    },
    /// El constructor del cliente HTTP compartido falló (config de red/TLS).
    #[error("no se pudo crear el cliente HTTP compartido: {0}")]
    HttpClient(String),
}

/// Configuración declarativa de un provider (serde: el wizard la escribe en
/// el config file).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    /// Identificador único del provider (p. ej. "openai", "ollama-local").
    pub id: String,
    /// Nombre visible.
    pub display_name: String,
    /// URL base OpenAI-compatible (p. ej. `https://api.openai.com/v1`).
    pub base_url: Url,
    /// Fuente de credencial declarada. Se resuelve UNA VEZ en `new`.
    pub auth: AuthSource,
    /// Modelo por defecto para chat/completions.
    pub default_model: String,
}

/// Build the process-wide shared HTTP client for providers.
///
/// Un solo `wreq::Client` (Chrome145, timeouts, SSRF guard) compartido por
/// todos los providers: pooling de conexiones y una única superficie de
/// fingerprint TLS. Devuelve el client pelado — cada provider lo compone con
/// su base_url/api_key.
///
/// # Errors
///
/// [`ProviderInitError::HttpClient`] si el builder falla.
pub fn build_default_http_client() -> Result<wreq::Client, ProviderInitError> {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Profile::Chrome145)
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10));
    crate::domain::ssrf_guard::ssrf_guard()
        .secure_client(builder)
        .build()
        .map_err(|e| ProviderInitError::HttpClient(e.to_string()))
}

/// Provider OpenAI-compatible listo para inyectar en el Container como
/// `Arc<dyn LlmPort>`.
///
/// Sin `Debug` deliberado: contiene la credencial resuelta (vía el cliente).
/// Los diagnósticos usan `config()` + `id()`, nunca el secreto.
pub struct OpenAiCompatibleProvider {
    client: OpenAiLlmClient,
    config: ProviderConfig,
}

impl OpenAiCompatibleProvider {
    /// Construye el provider resolviendo la credencial UNA VEZ (startup).
    ///
    /// Usa el cliente HTTP compartido del proceso
    /// ([`build_default_http_client`]). Para reutilizar un cliente ya
    /// existente (tests, wiremock), [`with_http`](Self::with_http).
    ///
    /// # Errors
    ///
    /// [`ProviderInitError`] si la credencial no resuelve, la URL es inválida
    /// o no se puede construir el cliente compartido.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
        let http = build_default_http_client()?;
        Self::with_http(config, http)
    }

    /// Variante con cliente HTTP inyectado (composición root/tests).
    ///
    /// # Errors
    ///
    /// [`ProviderInitError::AuthFailed`] / `InvalidBaseUrl` — la credencial
    /// se resuelve aquí, NO en la primera llamada.
    pub fn with_http(
        config: ProviderConfig,
        http: wreq::Client,
    ) -> Result<Self, ProviderInitError> {
        let secret = config
            .auth
            .resolve()
            .map_err(|source| ProviderInitError::AuthFailed {
                provider_id: config.id.clone(),
                source,
            })?;
        let client = OpenAiLlmClient::with_http(http, config.base_url.clone(), secret);
        Ok(Self { client, config })
    }

    /// Identificador del provider.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.config.id
    }

    /// Configuración declarativa (para diagnósticos/snapshots).
    #[must_use]
    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }
}

impl LlmPort for OpenAiCompatibleProvider {
    fn send_completion<'a>(
        &'a self,
        request: crate::domain::llm_port::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<crate::domain::llm_port::LlmResponse>,
                > + Send
                + 'a,
        >,
    > {
        self.client.send_completion(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm_port::{ChatMessage, LlmRequest};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with(auth: AuthSource) -> ProviderConfig {
        ProviderConfig {
            id: "test-provider".to_string(),
            display_name: "Test".to_string(),
            base_url: Url::parse("https://api.example.com/v1").expect("url"),
            auth,
            default_model: "gpt-test".to_string(),
        }
    }

    /// Contrato clave: resolve() en CONSTRUCTOR, no en la primera llamada.
    /// Una credencial ausente debe fallar `with_http`, no `send_completion`.
    #[test]
    fn missing_credential_fails_at_construction_not_first_call() {
        let cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_WIREUP_MISSING_VAR".to_string(),
        });
        let err = match OpenAiCompatibleProvider::new(cfg) {
            Err(e) => e,
            Ok(_) => panic!("debe fallar en construcción"),
        };
        // El mensaje lleva provider_id (contrato de conversión AuthError →
        // ProviderInitError) y preserva la fuente.
        assert!(err.to_string().contains("test-provider"), "{err}");
        assert!(matches!(
            err,
            ProviderInitError::AuthFailed {
                source: AuthError::EnvNotSet(_),
                ..
            }
        ));
    }

    /// Wire-up completo contra wiremock: credencial resuelta en el
    /// constructor, request firmado con la key resuelta, vía LlmPort.
    #[tokio::test]
    async fn provider_resolves_credential_and_signs_request() {
        let server = MockServer::start().await;
        // La key que el provider debe firmar (resuelta de Env en el ctor).
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer sk-wireup-test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "pong"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })))
            .mount(&server)
            .await;

        let env = webfang_test_utils::EnvGuard::with(&[(
            "WEBFANG_TEST_WIREUP_OK_VAR",
            "sk-wireup-test-key",
        )]);
        let cfg = ProviderConfig {
            base_url: Url::parse(&server.uri()).expect("url"),
            ..config_with(AuthSource::Env {
                var: "WEBFANG_TEST_WIREUP_OK_VAR".to_string(),
            })
        };
        let provider = match OpenAiCompatibleProvider::new(cfg) {
            Ok(p) => p,
            Err(e) => panic!("wire-up debe resolver: {e}"),
        };
        drop(env); // el ctor ya resolvió: borrar el env no afecta

        let response = match provider
            .send_completion(LlmRequest {
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: "ping".to_string(),
                }],
                model: "gpt-test".to_string(),
                max_tokens: 16,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => panic!("completions OK: {e}"),
        };
        assert_eq!(response.content, "pong");
        assert_eq!(provider.id(), "test-provider");
    }

    /// El cliente compartido se construye una vez y sirve N providers; el
    /// contexto de error (provider_id) no se mezcla entre configs.
    #[test]
    fn shared_http_client_reports_each_provider_context() {
        let client = match build_default_http_client() {
            Ok(c) => c,
            Err(e) => panic!("cliente compartido debe construirse: {e}"),
        };
        let cfg1 = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_SHARED_1".to_string(),
        });
        let mut cfg2 = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_SHARED_2".to_string(),
        });
        cfg2.id = "second-provider".to_string();
        let err1 = match OpenAiCompatibleProvider::with_http(cfg1, client.clone()) {
            Err(e) => e,
            Ok(_) => panic!("sin credencial debe fallar"),
        };
        let err2 = match OpenAiCompatibleProvider::with_http(cfg2, client.clone()) {
            Err(e) => e,
            Ok(_) => panic!("sin credencial debe fallar"),
        };
        assert!(err1.to_string().contains("test-provider"));
        assert!(err2.to_string().contains("second-provider"));
    }
}
