//! Generación de la identity `age` local (primer run en Linux, §8a).
//!
//! La identity vive en `~/.config/webfang/identity.key` con permisos 0600
//! (modelo `~/.ssh/id_ed25519`). Escritura ATÓMICA con 0600 desde la
//! creación: nunca write-then-chmod (deja ventana legible por otros).

use std::path::{Path, PathBuf};

use crate::domain::auth_source::default_identity_path;

/// Error de setup del wizard (mensajes user-facing en español).
#[derive(Debug, thiserror::Error)]
pub enum WizardError {
    /// El archivo de identity ya existe: el wizard nunca sobrescribe en
    /// silencio — rotate es el camino explícito ([`crate::infrastructure::wizard::RotationPlan`]).
    #[error("la identity ya existe en {}: usa rotate, no se sobrescribe", .0.display())]
    IdentityAlreadyExists(PathBuf),
    /// Fallo de I/O con la ruta afectada (p. ej. `~/.config/webfang/` no
    /// escribible: el wizard falla con el error original, sin fallback
    /// silencioso).
    #[error("no se pudo escribir la identity en {}: {source}", .path.display())]
    Io {
        /// Ruta que se intentaba escribir.
        path: PathBuf,
        /// Error original del sistema.
        #[source]
        source: std::io::Error,
    },
}

/// Ruta por defecto donde el wizard genera la identity (misma que
/// `resolve_identity` lee — una sola definición, no dos que diverjan).
#[must_use]
pub fn wizard_default_identity_path() -> PathBuf {
    default_identity_path()
}

/// Genera una identity `age` nueva y la escribe en `path` con 0600 atómico.
///
/// # Errors
///
/// - [`WizardError::IdentityAlreadyExists`] si el archivo ya existe (nunca
///   sobrescribe; rotate es el camino).
/// - [`WizardError::Io`] si los dirs padre no se pueden crear o el archivo no
///   se puede escribir (p. ej. sin permiso: falla con el original).
pub fn generate_identity_file(path: &Path) -> Result<age::x25519::Identity, WizardError> {
    if path.exists() {
        return Err(WizardError::IdentityAlreadyExists(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| WizardError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    let identity = age::x25519::Identity::generate();
    use age::secrecy::ExposeSecret as _;
    let secret = identity.to_string();
    write_0600_atomic(path, secret.expose_secret().as_bytes()).map_err(|source| {
        WizardError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(identity)
}

/// Escritura con 0600 desde la creación (nunca write-then-chmod).
#[cfg(unix)]
fn write_0600_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    // `create_new` + mode 0600 ya garantiza el modo; verificación defensiva
    // contra umask/filesystems con semántica rara: si el modo final no es
    // 0600 exacto, borrar y fallar antes de dejar un secreto mal protegido.
    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode != 0o600 {
        std::fs::remove_file(path)?;
        return Err(std::io::Error::other(format!(
            "el filesystem no respeta modo 0600 (obtenido {mode:o}): mueve el config a un FS POSIX"
        )));
    }
    Ok(())
}

/// Sin POSIX no hay modo 0600 que verificar: fail closed con mensaje claro
/// (mover el config a un FS POSIX o usar el override de env consciente).
#[cfg(not(unix))]
fn write_0600_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::remove_file(path).ok();
    Err(std::io::Error::other(format!(
        "sin permisos POSIX no se puede proteger {}: usa un FS POSIX o el override {}",
        path.display(),
        "WEBFANG_AGE_IDENTITY"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret as _;

    fn tmp_identity() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let path = tmp_path(&tmp);
        (tmp, path)
    }

    fn tmp_path(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("webfang").join("identity.key")
    }

    #[test]
    fn generates_parseable_identity_with_0600() {
        let (_tmp, path) = tmp_identity();
        // Dirs padre no existen: generate los crea.
        let identity = match generate_identity_file(&path) {
            Ok(i) => i,
            Err(e) => panic!("genera: {e}"),
        };
        let on_disk = std::fs::read_to_string(&path).expect("legible");
        assert_eq!(on_disk.trim(), identity.to_string().expose_secret().trim());
        // Re-parsea: lo escrito es una identity válida.
        let _: age::x25519::Identity = on_disk.trim().parse().expect("parsea");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "identity con modo exacto 0600");
        }
    }

    #[test]
    fn second_generate_fails_without_overwriting() {
        let (_tmp, path) = tmp_identity();
        generate_identity_file(&path).expect("primera genera");
        let before = std::fs::read(&path).expect("contenido previo");
        let err = match generate_identity_file(&path) {
            Err(e) => e,
            Ok(_) => panic!("segunda debe fallar"),
        };
        assert!(
            matches!(err, WizardError::IdentityAlreadyExists(_)),
            "debe ser AlreadyExists, got: {err}"
        );
        let after = std::fs::read(&path).expect("contenido posterior");
        assert_eq!(before, after, "el archivo existente queda intacto");
    }

    #[test]
    fn write_is_not_world_readable_from_creation() {
        let (_tmp, path) = tmp_identity();
        generate_identity_file(&path).expect("genera");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o077;
            assert_eq!(mode, 0o000, "sin bits para grupo/otros: {mode:o}");
        }
    }

    #[test]
    fn unreadable_parent_dir_fails_with_original_io_error() {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let blocked = tmp.path().join("blocked");
        std::fs::create_dir_all(&blocked).expect("mkdir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500))
                .expect("chmod 0500");
            let path = blocked.join("sub").join("identity.key");
            match generate_identity_file(&path) {
                Err(WizardError::Io { .. }) => {},
                Err(other) => panic!("debe ser Io con el error original, got: {other}"),
                // Entorno privilegiado (root/CI): los bits 0500 no bloquean y
                // el test no aplica. Se documenta, no se fuerza el fallo.
                Ok(_) => eprintln!("skip: entorno privilegiado ignora 0500"),
            }
        }
    }

    #[test]
    fn wizard_default_path_matches_resolver() {
        assert_eq!(wizard_default_identity_path(), default_identity_path());
    }
}
