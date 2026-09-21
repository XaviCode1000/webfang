//! Plan de rotación de identity (pérdida de `identity.key`, §8-rotate).
//!
//! Puro, sin I/O: dado el path de la identity perdida y la lista de archivos
//! `.age` conocidos, produce el plan. `execute_rotation` NO se implementa aquí
//! — requiere las API keys nuevas del usuario (interactivo) para re-cifrar; el
//! wizard interactivo lo hará con este plan como entrada.

use std::path::PathBuf;

/// Plan de rotación: qué se invalida, qué hay que re-cifrar, qué borrar.
///
/// La pérdida de la identity invalida TODOS los ciphertexts cifrados con ella
/// a la vez (tantos providers como usen esa identity) — por eso el plan lista
/// todos los `.age` afectados juntos, nunca de a uno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationPlan {
    /// Ruta de la identity perdida (donde irá la nueva).
    pub identity_path: PathBuf,
    /// Todos los archivos `.age` cifrados con la identity perdida.
    pub affected_files: Vec<PathBuf>,
    /// Nota en español: los `.age` viejos quedan huérfanos y deben borrarse
    /// tras re-cifrar (el anti-patrón "re-configurar" los deja tirados
    /// pareciendo en uso).
    pub orphan_note: String,
}

/// Construye el plan de rotación (puro: sin I/O, sin side-effects).
#[must_use]
pub fn plan_rotation(
    identity_path: &std::path::Path,
    credential_files: &[PathBuf],
) -> RotationPlan {
    let affected_files = credential_files.to_vec();
    let orphan_note = if affected_files.is_empty() {
        "no hay archivos .age conocidos: genera la nueva identity y configura los providers desde cero".to_string()
    } else {
        format!(
            "{} archivo(s) .age quedan huérfanos (indescifrables con la nueva identity): re-cifra cada provider con su API key y borra los viejos",
            affected_files.len()
        )
    };
    RotationPlan {
        identity_path: identity_path.to_path_buf(),
        affected_files,
        orphan_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_lists_all_affected_files_at_once() {
        let files = vec![
            PathBuf::from("/c/a.age"),
            PathBuf::from("/c/b.age"),
            PathBuf::from("/c/c.age"),
        ];
        let plan = plan_rotation(std::path::Path::new("/c/identity.key"), &files);
        assert_eq!(plan.affected_files, files);
        assert_eq!(plan.identity_path, PathBuf::from("/c/identity.key"));
        assert!(
            plan.orphan_note.contains("3 archivo(s)"),
            "la nota cuenta los afectados: {}",
            plan.orphan_note
        );
        assert!(
            plan.orphan_note.contains("borra"),
            "la nota ordena borrar los huérfanos: {}",
            plan.orphan_note
        );
    }

    #[test]
    fn empty_rotation_names_fresh_start() {
        let plan = plan_rotation(std::path::Path::new("/c/identity.key"), &[]);
        assert!(plan.affected_files.is_empty());
        assert!(
            plan.orphan_note.contains("desde cero"),
            "sin archivos, el plan lo dice: {}",
            plan.orphan_note
        );
    }
}
