//! LocalSecretStore — helper de setup, NO un runtime resolver.
//!
//! Especificación: `docs/src/ai-providers-design.md` §7. Durante la ejecución
//! normal, `AuthSource::resolve()` obtiene las credenciales. Este módulo solo
//! se usa en el wizard de configuración inicial para informar qué fuentes de
//! credenciales están disponibles en el sistema del usuario.

use std::path::PathBuf;

use serde::Serialize;

/// Diagnóstico informativo de fuentes de credenciales disponibles.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DetectedStore {
    /// Almacén de secretos del sistema disponible (Linux: kernel keyutils;
    /// macOS: keychain; Windows: Credential Manager).
    pub keyring_available: bool,
    /// Ruta de un almacén de credenciales cifrado detectado, si existe.
    pub encrypted_file_path: Option<PathBuf>,
    /// Variables de entorno candidatas detectadas (prefijo `WEBFANG_`).
    pub env_vars_detected: Vec<String>,
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
        let env_vars_detected = std::env::vars()
            .filter(|(name, _)| name.starts_with(ENV_VAR_PREFIX))
            .map(|(name, _)| name)
            .collect();
        Self {
            keyring_available,
            encrypted_file_path,
            env_vars_detected,
        }
    }
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
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
    let candidate = dir.join("webfang").join("credentials.age");
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
}
