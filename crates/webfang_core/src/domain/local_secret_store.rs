//! LocalSecretStore — helper de setup, NO un runtime resolver.
//!
//! Especificación: `docs/src/ai-providers-design.md` §7. Durante la ejecución
//! normal, `AuthSource::resolve()` obtiene las credenciales. Este módulo solo
//! se usa en el wizard de configuración inicial para informar qué fuentes de
//! credenciales están disponibles en el sistema del usuario.

use std::path::PathBuf;

use serde::Serialize;

use super::auth_source::AuthSource;

/// Diagnóstico informativo de fuentes de credenciales disponibles.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedStore {
    /// Almacén de secretos del sistema disponible (Linux: kernel keyutils;
    /// macOS: keychain; Windows: Credential Manager).
    pub keyring_available: bool,
    /// Ruta de un almacén de credenciales cifrado detectado, si existe.
    pub encrypted_file_path: Option<PathBuf>,
    /// Variables de entorno candidatas detectadas (prefijo `WEBFANG_`).
    pub env_vars_detected: Vec<String>,
    /// Fuente recomendada para guardar la próxima credencial.
    ///
    /// Regla institucional en Linux (evidencia: docs oficiales de
    /// `linux-keyutils-keyring-store`): el keyring del kernel es
    /// "completely in-memory and will not persist across reboots", la
    /// persistent keyring expira a los pocos días
    /// (`/proc/sys/kernel/keys/persistent_keyring_expiry`) y "a reboot
    /// clears all keyrings". Por eso `recommendation` en Linux es
    /// `EncryptedFile` (age, persistente), con `Keyring` como override
    /// explícito solo si el usuario ya usa el keyring a sabiendas.
    ///
    /// macOS/Windows: sus backends (keychain / Credential Manager) sí son
    /// persistentes; `Keyring` es la recomendación cuando está disponible.
    pub recommendation: AuthSource,
}

/// Prefijo de variables de entorno que `detect` reporta como candidatas.
const ENV_VAR_PREFIX: &str = "WEBFANG_";

impl DetectedStore {
    /// Sondea el sistema y devuelve un diagnóstico de setup.
    ///
    /// No resuelve credenciales y su resultado no debe usarse como runtime
    /// resolver (criterio de aceptación del doc §7).
    pub fn detect() -> Self {
        let keyring_available = detect_keyring();
        let encrypted_file_path = detect_default_encrypted_file();
        let env_vars_detected: Vec<String> = std::env::vars()
            .filter(|(name, _)| name.starts_with(ENV_VAR_PREFIX))
            .map(|(name, _)| name)
            .collect();
        let recommendation = recommendation_for(keyring_available, &encrypted_file_path);
        Self {
            keyring_available,
            encrypted_file_path,
            env_vars_detected,
            recommendation,
        }
    }
}

/// Recomendación por plataforma. Extraída de `detect` para poder testearla
/// inyectando los hechos observados, sin depender del entorno real.
fn recommendation_for(keyring_available: bool, detected: &Option<PathBuf>) -> AuthSource {
    #[cfg(target_os = "linux")]
    let recommendation = {
        // El keyring del kernel no persiste: JAMÁS se recomienda en Linux.
        let _ = keyring_available;
        AuthSource::EncryptedFile {
            path: detected.clone().unwrap_or_else(default_encrypted_file_path),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let recommendation = if keyring_available {
        // La constante canónica de servicio llega con el wire-up del
        // provider; por ahora el wizard propone este par explícito.
        AuthSource::Keyring {
            service: "webfang".to_string(),
            account: "default".to_string(),
        }
    } else {
        AuthSource::EncryptedFile {
            path: detected.clone().unwrap_or_else(default_encrypted_file_path),
        }
    };
    recommendation
}

/// Ruta por defecto del almacén cifrado (`~/.config/webfang/credentials.age`),
/// respetando `XDG_CONFIG_HOME`.
fn default_encrypted_file_path() -> PathBuf {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    dir.join("webfang").join("credentials.age")
}

/// Comprueba el backend de keyring compilado para la plataforma actual.
fn detect_keyring() -> bool {
    let Ok(entry) = keyring::Entry::new("webfang-setup-probe", "detect") else {
        return false;
    };
    // Una entrada inexistente con backend operativo devuelve NoEntry, no error
    // de plataforma: eso significa "keyring disponible".
    !matches!(
        entry.get_password(),
        Err(keyring::Error::PlatformFailure(_) | keyring::Error::Ambiguous(_))
    )
}

fn detect_default_encrypted_file() -> Option<PathBuf> {
    let candidate = default_encrypted_file_path();
    candidate.exists().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_a_diagnostic_without_panicking() {
        let store = DetectedStore::detect();
        // Solo verificamos estructura observable: el valor de keyring_available
        // depende del entorno (CI headless puede no tener backend).
        let _ = store.keyring_available;
        let _ = store.encrypted_file_path;
        // Nunca debe contener el valor de las variables, solo sus nombres.
        for name in &store.env_vars_detected {
            assert!(name.starts_with(ENV_VAR_PREFIX));
            assert!(!name.contains('='), "solo nombres, nunca valores");
        }
    }

    #[test]
    fn detected_store_never_leaks_env_values() {
        let var = "WEBFANG_TEST_DETECT_LEAK_CHECK";
        let guard = webfang_test_utils::EnvGuard::with(&[(var, "sk-secret-value-must-not-appear")]);
        let json = serde_json::to_string(&DetectedStore::detect()).unwrap();
        assert!(!json.contains("sk-secret-value-must-not-appear"));
        drop(guard);
    }

    /// Regla institucional fijada con la evidencia de persistencia (docs de
    /// linux-keyutils-keyring-store): en Linux la recomendación es SIEMPRE
    /// `EncryptedFile` — el keyring del kernel no sobrevive reboot.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_recommendation_is_always_encrypted_file() {
        let rec = recommendation_for(true, &None);
        assert!(
            matches!(rec, AuthSource::EncryptedFile { .. }),
            "linux debe recomendar EncryptedFile aunque el keyring esté disponible"
        );
    }

    /// En plataformas con almacén nativo persistente, keyring disponible
    /// implica recomendación `Keyring`.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn persistent_platforms_recommend_keyring_when_available() {
        assert!(matches!(
            recommendation_for(true, &None),
            AuthSource::Keyring { .. }
        ));
    }
}
