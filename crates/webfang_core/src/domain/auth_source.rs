//! AuthSource — fuente de una credencial de proveedor AI.
//!
//! Especificación: `docs/src/ai-providers-design.md` §1–§3.
//!
//! Decisiones de diseño (cerradas, no re-discutir):
//! - Sin variante `Inline` en v1 (secrets en config = riesgo sin mitigación real).
//! - Sin fallback implícito entre fuentes: la fuente configurada es la única
//!   que se intenta; si falla, falla explícitamente con su error.
//! - `Env` es legacy (CI/desarrollo), nunca el default.

use std::path::PathBuf;

use crate::domain::credentials::ApiKey;

/// Errores de resolución de credencial (`docs/src/ai-providers-design.md` §2).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// El backend de keyring no está disponible en este sistema.
    #[error("keyring no disponible en este sistema")]
    KeyringUnavailable,
    /// No existe credencial para el par servicio/cuenta solicitado.
    #[error("no se encontró credencial para {service}/{account}")]
    CredentialNotFound {
        /// Servicio solicitado.
        service: String,
        /// Cuenta solicitada.
        account: String,
    },
    /// El backend de keyring existe pero falló al operar.
    #[error("error del keyring: {0}")]
    KeyringFailure(String),
    /// El archivo de credenciales no existe en la ruta configurada.
    #[error("archivo de credenciales no encontrado: {0}")]
    FileNotFound(PathBuf),
    /// Error de I/O al leer el archivo de credenciales.
    #[error("no se pudo leer el archivo de credenciales {0}: {1}")]
    FileRead(PathBuf, String),
    /// No se pudo descifrar el archivo (identity ausente/inválida o formato).
    #[error("no se pudo descifrar el archivo de credenciales {0}: {1}")]
    Decrypt(PathBuf, String),
    /// El archivo se descifró pero no contiene una credencial utilizable.
    #[error("formato de credencial inválido en {0}")]
    FileFormat(PathBuf),
    /// La variable de entorno configurada no existe.
    #[error("variable de entorno {0} no configurada")]
    EnvNotSet(String),
    /// La credencial obtenida no es utilizable (vacía, no-Unicode, etc.).
    #[error("credencial inválida: {0}")]
    Invalid(String),
}

/// De dónde se obtiene la API key de un proveedor.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AuthSource {
    /// Credencial en el almacén de secretos del sistema
    /// (Linux: kernel keyutils; macOS: keychain; Windows: Credential Manager).
    Keyring {
        /// Servicio (nombre de aplicación) bajo el que se guardó la credencial.
        service: String,
        /// Cuenta (usuario/campo) dentro del servicio.
        account: String,
    },
    /// Credencial en archivo cifrado con `age` en disco.
    EncryptedFile {
        /// Ruta del archivo cifrado con la credencial.
        path: PathBuf,
    },
    /// Credencial en variable de entorno (legacy, solo CI o desarrollo).
    Env {
        /// Nombre de la variable de entorno que contiene la API key.
        var: String,
    },
}

impl AuthSource {
    /// Resuelve la credencial desde su fuente. Sin fallback: la fuente
    /// configurada es la única que se intenta (doc §3).
    pub fn resolve(&self) -> Result<ApiKey, AuthError> {
        match self {
            Self::Keyring { service, account } => resolve_keyring(service, account),
            Self::EncryptedFile { path } => resolve_encrypted_file(path),
            Self::Env { var } => resolve_env(var),
        }
    }
}

fn resolve_keyring(service: &str, account: &str) -> Result<ApiKey, AuthError> {
    let entry = keyring::Entry::new(service, account).map_err(|_| AuthError::KeyringUnavailable)?;
    match entry.get_password() {
        Ok(secret) if secret.is_empty() => Err(AuthError::Invalid(format!(
            "credencial vacía en keyring {service}/{account}"
        ))),
        Ok(secret) => Ok(ApiKey::new(secret)),
        Err(keyring::Error::NoEntry) => Err(AuthError::CredentialNotFound {
            service: service.to_string(),
            account: account.to_string(),
        }),
        Err(err) => Err(AuthError::KeyringFailure(err.to_string())),
    }
}

/// Variable de entorno con la identidad `age` (clave privada X25519). SOLO
/// override explícito para CI/entornos headless donde no hay disco de usuario
/// confiable. En máquina normal la identidad vive en
/// `~/.config/webfang/identity.key` con permisos 0600 (modelo `~/.ssh/id_ed25519`).
///
/// Regla de seguridad (decisión cerrada): la identidad NO se diseña para vivir
/// en env de forma permanente — `/proc/<pid>/environ` es legible por cualquier
/// proceso del mismo usuario y aparece en crash dumps. El env es el camino
/// corto de CI, nunca el default que el wizard recomienda.
const AGE_IDENTITY_ENV: &str = "WEBFANG_AGE_IDENTITY";

/// Ruta por defecto de la identidad `age` local (`~/.config/webfang/identity.key`),
/// respetando `XDG_CONFIG_HOME`.
fn default_identity_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from(".config"));
    dir.join("webfang").join("identity.key")
}

/// Resuelve la identidad `age`: env si está configurado (override de CI),
/// si no el archivo local por defecto. Verifica permisos 0600 en el archivo.
///
/// Operativa (contrato del wizard):
/// - **Generación**: la crea el wizard en el primer run (`age` genera la
///   identity, escribe con 0600). Si `~/.config/webfang/` no es escribible,
///   el wizard falla con el `io::Error` original — no hay fallback silencioso.
/// - **Pérdida**: si `identity.key` se pierde, TODAS las credenciales cifradas
///   con ella quedan inaccesibles. La única salida es re-configurar el
///   provider (nueva identity + re-cifrar). Esto es by-design (age no tiene
///   recovery) y el wizard debe decirlo al generar, no descubrirse en
///   producción.
/// - **Fail closed en permisos**: filesystems sin permisos POSIX (FAT32,
///   `/mnt/c` de WSL, NFS sin ACL) reportan modos laxos siempre y el chequeo
///   `0600` los rechaza — deliberado. La salida es mover el config a un FS
///   POSIX o usar el override de env de forma consciente, nunca relajar el
///   chequeo global.
fn resolve_identity() -> Result<age::x25519::Identity, AuthError> {
    use std::io::Read as _;

    if let Ok(env_identity) = std::env::var(AGE_IDENTITY_ENV) {
        return env_identity.trim().parse().map_err(|e| {
            AuthError::Invalid(format!("identity de {AGE_IDENTITY_ENV} inválida: {e}"))
        });
    }
    let path = default_identity_path();
    if !path.exists() {
        return Err(AuthError::FileNotFound(path));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)
            .map_err(|e| AuthError::FileRead(path.clone(), e.to_string()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(AuthError::Invalid(format!(
                "permisos inseguros en {}: {:o} (requerido 0600, visible para grupo/otros)",
                path.display(),
                mode & 0o777
            )));
        }
    }
    let mut key = String::new();
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut key))
        .map_err(|e| AuthError::FileRead(path.clone(), e.to_string()))?;
    key.trim()
        .parse()
        .map_err(|e| AuthError::Invalid(format!("identity inválida en {}: {e}", path.display())))
}

fn resolve_encrypted_file(path: &std::path::Path) -> Result<ApiKey, AuthError> {
    use std::io::Read as _;

    if !path.exists() {
        return Err(AuthError::FileNotFound(path.to_path_buf()));
    }
    let identity = resolve_identity()?;
    let ciphertext =
        std::fs::read(path).map_err(|e| AuthError::FileRead(path.to_path_buf(), e.to_string()))?;
    let decryptor = age::Decryptor::new(ciphertext.as_slice())
        .map_err(|e| AuthError::Decrypt(path.to_path_buf(), e.to_string()))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as _))
        .map_err(|e| AuthError::Decrypt(path.to_path_buf(), e.to_string()))?;
    let mut secret = String::new();
    reader
        .read_to_string(&mut secret)
        .map_err(|e| AuthError::Decrypt(path.to_path_buf(), e.to_string()))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(AuthError::FileFormat(path.to_path_buf()));
    }
    Ok(ApiKey::new(secret))
}

fn resolve_env(var: &str) -> Result<ApiKey, AuthError> {
    match std::env::var(var) {
        Ok(value) if !value.trim().is_empty() => Ok(ApiKey::new(value.trim())),
        Ok(_) => Err(AuthError::Invalid(format!(
            "variable {var} configurada pero vacía"
        ))),
        Err(std::env::VarError::NotPresent) => Err(AuthError::EnvNotSet(var.to_string())),
        Err(std::env::VarError::NotUnicode(_)) => Err(AuthError::Invalid(format!(
            "variable {var} no es Unicode válido"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webfang_test_utils::{env_remove, env_set};

    #[test]
    fn env_resolves_configured_var() {
        let var = "WEBFANG_TEST_AUTHSOURCE_ENV_OK";
        env_set(var, "sk-test-123");
        let source = AuthSource::Env {
            var: var.to_string(),
        };
        let key = source.resolve().expect("debe resolver");
        assert_eq!(key.expose_secret(), "sk-test-123");
        env_remove(var);
    }

    #[test]
    fn env_not_set_fails_with_envnotset() {
        let source = AuthSource::Env {
            var: "WEBFANG_TEST_AUTHSOURCE_MISSING_VAR_XYZ".to_string(),
        };
        let err = source.resolve().expect_err("debe fallar");
        assert!(matches!(err, AuthError::EnvNotSet(_)));
        assert!(err
            .to_string()
            .contains("WEBFANG_TEST_AUTHSOURCE_MISSING_VAR_XYZ"));
    }

    #[test]
    fn env_empty_value_is_invalid() {
        let var = "WEBFANG_TEST_AUTHSOURCE_ENV_EMPTY";
        env_set(var, "   ");
        let source = AuthSource::Env {
            var: var.to_string(),
        };
        let err = source.resolve().expect_err("valor vacío debe fallar");
        assert!(matches!(err, AuthError::Invalid(_)));
        env_remove(var);
    }

    #[test]
    fn encrypted_file_missing_fails_with_filenotfound() {
        let source = AuthSource::EncryptedFile {
            path: "/nonexistent/webfang-test-key.age".into(),
        };
        let err = source.resolve().expect_err("debe fallar");
        assert!(matches!(err, AuthError::FileNotFound(_)));
    }

    #[test]
    fn no_implicit_fallback_from_keyring_to_env() {
        // Keyring con credencial inexistente debe fallar con CredentialNotFound
        // (o KeyringUnavailable si el backend no existe) y NUNCA intentar Env,
        // aunque la trampa esté configurada en el entorno.
        let var = "WEBFANG_TEST_AUTHSOURCE_FALLBACK_TRAP";
        env_set(var, "sk-should-not-be-used");
        let source = AuthSource::Keyring {
            service: "webfang-test-nonexistent-service".to_string(),
            account: "no-such-account".to_string(),
        };
        let err = source.resolve().expect_err("debe fallar sin fallback");
        assert!(matches!(
            err,
            AuthError::CredentialNotFound { .. }
                | AuthError::KeyringUnavailable
                | AuthError::KeyringFailure(_)
        ));
        env_remove(var);
    }

    #[test]
    fn serde_tag_parsing_snake_case() {
        let env: AuthSource = serde_json::from_str(r#"{"source": "env", "var": "FOO"}"#).unwrap();
        assert!(matches!(env, AuthSource::Env { .. }));
        let kr: AuthSource =
            serde_json::from_str(r#"{"source": "keyring", "service": "s", "account": "a"}"#)
                .unwrap();
        assert!(matches!(kr, AuthSource::Keyring { .. }));
        let ef: AuthSource =
            serde_json::from_str(r#"{"source": "encrypted_file", "path": "/tmp/k.age"}"#).unwrap();
        assert!(matches!(ef, AuthSource::EncryptedFile { .. }));
        // No existe variante inline en v1 (criterio de aceptación del doc).
        let inline: Result<AuthSource, _> =
            serde_json::from_str(r#"{"source": "inline", "value": "sk-x"}"#);
        assert!(inline.is_err());
    }

    #[test]
    fn encrypted_file_roundtrip_with_age() {
        use age::secrecy::ExposeSecret as _;
        // Genera identity + recipient, cifra una credencial con age y verifica
        // que resolve() la recupera intacta a través de WEBFANG_AGE_IDENTITY.
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public();
        let plaintext = "sk-age-roundtrip-secret";
        use std::io::Write as _;
        let ciphertext = {
            let mut out = age::Encryptor::with_recipients(std::iter::once(
                &recipient as &(dyn age::Recipient),
            ))
            .expect("encryptor")
            .wrap_output(Vec::new())
            .expect("wrap_output");
            out.write_all(plaintext.as_bytes()).expect("write");
            out.finish().expect("finish")
        };

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let path = tmp.path().join("cred.age");
        std::fs::write(&path, &ciphertext).expect("write ciphertext");

        // La identidad vive en el archivo local (modelo ~/.ssh/id_ed25519),
        // NO en env. Un SOLO guard: dos EnvGuard simultáneos = deadlock
        // (ENV_LOCK, política #1349).
        let identity_str = identity.to_string().expose_secret().to_string();
        let mut env = webfang_test_utils::EnvGuard::with(&[(
            "XDG_CONFIG_HOME",
            tmp.path().to_str().expect("utf8 tmp"),
        )]);
        let id_path = default_identity_path();
        std::fs::create_dir_all(id_path.parent().expect("parent")).expect("mkdir identity dir");
        std::fs::write(&id_path, identity_str.trim()).expect("write identity");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&id_path, std::fs::Permissions::from_mode(0o600))
                .expect("chmod 0600");
        }
        env.remove(AGE_IDENTITY_ENV);
        let source = AuthSource::EncryptedFile { path };
        let key = source.resolve().expect("roundtrip debe resolver");
        assert_eq!(key.expose_secret(), plaintext);
    }

    /// Regla de seguridad: la identidad JAMÁS se toma de env por defecto ni
    /// se recomienda ahí; `EncryptedFile` en Linux es real solo porque la
    /// identidad vive en disco con 0600 (modelo ~/.ssh/id_ed25519).
    #[cfg(unix)]
    #[test]
    fn identity_file_with_loose_permissions_is_rejected() {
        use age::secrecy::ExposeSecret as _;
        use std::os::unix::fs::PermissionsExt as _;

        let identity = age::x25519::Identity::generate();
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let mut env = webfang_test_utils::EnvGuard::with(&[(
            "XDG_CONFIG_HOME",
            tmp.path().to_str().expect("utf8 tmp"),
        )]);
        // Sin override de env: si existiera WEBFANG_AGE_IDENTITY, taparía el
        // check de permisos del archivo.
        env.remove(AGE_IDENTITY_ENV);
        let id_path = default_identity_path();
        std::fs::create_dir_all(id_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&id_path, identity.to_string().expose_secret().trim()).expect("write");
        // 0644: legible por grupo/otros — debe rechazarse antes de parsear.
        std::fs::set_permissions(&id_path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod 0644");

        let err = match resolve_identity() {
            Err(e) => e,
            Ok(_) => panic!("0644 debe rechazarse"),
        };
        assert!(
            err.to_string().contains("permisos inseguros"),
            "el error debe nombrar los permisos, no fallar con otra cosa: {err}"
        );
    }

    /// El override de CI: si WEBFANG_AGE_IDENTITY está configurado, gana sobre
    /// el archivo (y no requiere que exista el archivo local).
    #[test]
    fn env_identity_overrides_file() {
        use age::secrecy::ExposeSecret as _;

        let identity = age::x25519::Identity::generate();
        // XDG apunta a un dir SIN identity.key: si el env no ganara, sería
        // FileNotFound. Con el env, debe resolver. Un solo guard (#1349).
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let mut env = webfang_test_utils::EnvGuard::with(&[(
            "XDG_CONFIG_HOME",
            tmp.path().to_str().expect("utf8 tmp"),
        )]);
        env.set(
            AGE_IDENTITY_ENV,
            identity.to_string().expose_secret().trim(),
        );
        let resolved = match resolve_identity() {
            Ok(id) => id,
            Err(e) => panic!("env override debe resolver, falló: {e}"),
        };
        assert_eq!(
            resolved.to_string().expose_secret().trim(),
            identity.to_string().expose_secret().trim()
        );
    }
}
