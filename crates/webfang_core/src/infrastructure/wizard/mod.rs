//! Wizard de setup inicial de providers (doc `ai-providers-design.md` §8).
//!
//! Setup interactivo, NO runtime resolver: durante la ejecución normal
//! `AuthSource::resolve()` obtiene las credenciales; este módulo solo se usa
//! en la configuración inicial para guiar al usuario.
//!
//! Contrato §8 (decisiones cerradas antes de escribir código):
//! - Primer run en Linux genera la identity automáticamente (modelo SSH),
//!   pero el wizard lo dice en voz alta ([`wizard_message_for_new_identity`]).
//! - Dos flujos por SO sin abstracción unificadora (Linux `EncryptedFile`
//!   vs macOS/Windows `Keyring`).
//! - Pérdida de identity → rotate explícito ([`RotationPlan`]), nunca
//!   "re-configurar" silencioso que deje `.age` huérfanos.

pub mod identity;
pub mod rotate;

pub use identity::{generate_identity_file, WizardError};
pub use rotate::RotationPlan;

use std::path::Path;

/// Mensaje OBLIGATORIO del §8a: el wizard lo muestra antes de generar la
/// identity en Linux. Sin este aviso `EncryptedFile` parece "seguro por
/// magia" y el usuario nunca hace backup.
#[must_use]
pub fn wizard_message_for_new_identity(path: &Path) -> String {
    format!(
        "voy a crear una identity age en {}; si la perdés, vas a tener que rotar tus providers (los archivos .age viejos quedarán indescifrables)",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_identity_message_names_path_and_rotation() {
        let msg = wizard_message_for_new_identity(std::path::Path::new(
            "/home/u/.config/webfang/identity.key",
        ));
        assert!(
            msg.contains("/home/u/.config/webfang/identity.key"),
            "el mensaje debe nombrar la ruta concreta: {msg}"
        );
        assert!(
            msg.contains("rotar"),
            "el mensaje debe nombrar la consecuencia (rotar), no solo el archivo: {msg}"
        );
    }
}
