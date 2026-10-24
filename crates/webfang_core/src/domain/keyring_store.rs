//! Keyring store — el ÚNICO lugar que decide qué almacén de credenciales
//! backs el keyring en cada plataforma, y con qué clave se direcciona la
//! credencial.
//!
//! # Por qué existe este módulo
//!
//! El crate `keyring` 3 (la fachada que usaba el proyecto) deja el grafo:
//! todas sus versiones declaran los back ends de secret-service como
//! dependencias **opcionales**, y `Cargo.lock` registra también las
//! opcionales — o sea que la cadena sin mantenimiento `zbus 4` sólo se va
//! del lock cuando el PAQUETE `keyring` se va del grafo, no cambiando
//! features. En su lugar entran `keyring-core` (el modelo
//! independiente de la plataforma: traits + errores) y el store concreto de
//! cada plataforma, que es **el mismo back end de hoy**: keyutils en Linux,
//! el keychain en macOS, Credential Manager en Windows.
//!
//! # El requisito: la clave NO cambia (D20)
//!
//! Un refactor de dependencias de seguridad no puede dejar huérfanas las
//! credenciales del usuario. El criterio de aceptación es uno solo y
//! verificable: **la clave con la que se direcciona una credencial
//! `(service, account)` tiene que ser byte-idéntica a la de `keyring` 3**.
//!
//! - Linux: `keyring` 3 armaba la *description* de la clave keyutils como
//!   `keyring-rs:{user}@{service}` (`keyring-3.6.3/src/keyutils.rs`,
//!   `new_with_target`, sin target explícito). El store nuevo la arma como
//!   `{prefix}{user}{divider}{service}{suffix}` y su default es
//!   `prefix = "keyring:"`, `divider = "@"`, `suffix = ""` ⇒ la clave
//!   CAMBIARÍA (`keyring:{user}@{service}`) y las credenciales ya guardadas
//!   quedarían infindables. Por eso el store se CONFIGURA aquí con el
//!   prefijo legacy (`keyring-rs:`).
//! - macOS: el atributo `service` es el service y el atributo `account` es
//!   el account — idéntico por construcción a `keyring` 3.
//! - Windows: el `target_name` es `{account}.{service}` — idéntico por
//!   construcción a `keyring` 3.
//!
//! Mecanismo elegido: **(a) configurar el store**. Sin migración, sin
//! estado nuevo, sin lectura doble, y sin tocar el contrato de
//! "sin fallback" de la resolución de credenciales.
//!
//! # Por qué no migración
//!
//! Una migración escribe una credencial nueva y borra la vieja: deja
//! estado nuevo, deja restos si falla a mitad de camino, y no arregla a
//! quien ya la tenía guardada si el borrado falla. Una clave que no cambia
//! no tiene ninguno de esos problemas.

use keyring_core::{Entry, Error};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use keyring_core::api::CredentialStoreApi;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Linux: la clave tiene que ser byte-idéntica a la de `keyring` 3.
// ---------------------------------------------------------------------------

/// Prefijo de la clave que direcciona la credencial en Linux.
///
/// `keyring` 3 (keyutils) usaba `keyring-rs:{user}@{service}`. El store
/// nuevo usa por defecto `keyring:{user}@{service}`: distinto prefijo, y
/// por lo tanto una credencial DISTINTA para el mismo par
/// (service, account).
#[cfg(target_os = "linux")]
const LEGACY_PREFIX: &str = "keyring-rs:";

/// Divisor entre el usuario y el servicio (el de `keyring` 3).
#[cfg(target_os = "linux")]
const LEGACY_DIVIDER: &str = "@";

/// Sufijo de la clave (vacío en `keyring` 3).
#[cfg(target_os = "linux")]
const LEGACY_SUFFIX: &str = "";

/// Nombre del store en los logs (inglés: los campos de tracing van en
/// inglés aunque el mensaje de cara al usuario vaya en español).
#[cfg(target_os = "linux")]
const STORE_NAME: &str = "linux-keyutils";
#[cfg(target_os = "macos")]
const STORE_NAME: &str = "apple-keychain";
#[cfg(target_os = "windows")]
const STORE_NAME: &str = "windows-credential-manager";

/// Configuración de clave del store de Linux.
///
/// Es la MISMA que usa [`store`] en producción: el test del contrato
/// (D20) lee esta función, no una constante repetida, para que la
/// configuración y la clave que se produce no puedan divergir.
#[cfg(target_os = "linux")]
fn store_config() -> std::collections::HashMap<&'static str, &'static str> {
    std::collections::HashMap::from([
        ("prefix", LEGACY_PREFIX),
        ("divider", LEGACY_DIVIDER),
        ("suffix", LEGACY_SUFFIX),
    ])
}

/// El store de credenciales de esta plataforma.
///
/// Linux: keyutils del kernel, configurado para producir la clave legacy
/// byte-idéntica (D20).
#[cfg(target_os = "linux")]
fn store() -> Result<Arc<linux_keyutils_keyring_store::Store>, Error> {
    linux_keyutils_keyring_store::Store::new_with_configuration(&store_config())
}

/// El store de credenciales de esta plataforma.
///
/// macOS: el keychain "legacy" (User/login), que es el que usaba
/// `keyring` 3 con `apple-native`. La clave NO cambia: el atributo
/// `service` es el service y el `account` es el account.
#[cfg(target_os = "macos")]
fn store() -> Result<Arc<apple_native_keyring_store::keychain::Store>, Error> {
    apple_native_keyring_store::keychain::Store::new()
}

/// El store de credenciales de esta plataforma.
///
/// Windows: Credential Manager, que es el que usaba `keyring` 3 con
/// `windows-native`. La clave NO cambia: el `target_name` es
/// `{account}.{service}`.
#[cfg(target_os = "windows")]
fn store() -> Result<Arc<windows_native_keyring_store::Store>, Error> {
    windows_native_keyring_store::Store::new()
}

/// La entrada que direcciona la credencial `(service, account)` en el
/// almacén de credenciales de esta plataforma.
///
/// Es la función que el resto del código usa: devuelve la `Entry` YA
/// construida, con la clave que esta plataforma direcciona. No hay estado
/// global, ni flags, ni registro de store por defecto: el store se elige
/// aquí y en ningún otro sitio.
///
/// # Errores
///
/// Devuelve el error del crate (`keyring_core::Error`) si el store no
/// está disponible o no acepta el par (service, account).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(crate) fn entry(service: &str, account: &str) -> Result<Entry, Error> {
    let store = store()?;
    let entry = store.build(service, account, None)?;
    tracing::debug!(
        store = STORE_NAME,
        service = %service,
        "keyring entry built"
    );
    Ok(entry)
}

/// La entrada que direcciona la credencial `(service, account)`.
///
/// En una plataforma sin store nativo (Android, iOS, BSD…) no hay almacén
/// de credenciales que direccionar: se reporta como no soportado, que el
/// resolver traduce a "keyring no disponible en este sistema".
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn entry(_service: &str, _account: &str) -> Result<Entry, Error> {
    Err(Error::NotSupportedByStore(
        "no credential store for this platform".to_string(),
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// D20 — la clave de una credencial NO cambia.
    ///
    /// `keyring` 3 (keyutils) direccionaba la credencial con
    /// `keyring-rs:{user}@{service}`. El store nuevo, por defecto, usa
    /// `keyring:{user}@{service}`: mismo orden, prefijo DISTINTO. Sin la
    /// configuración de este módulo, la credencial que el usuario ya tiene
    /// guardada quedaría huérfana — y con el contrato de "sin fallback" no
    /// habría salida.
    ///
    /// Sólo aplica a Linux: en macOS la clave es el par de atributos
    /// `service`/`account` y en Windows el `target_name = {account}.{service}`,
    /// idénticos por construcción.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_credential_key_is_byte_identical_to_the_legacy_key() {
        const SERVICE: &str = "webfang-legacy-key-service";
        const ACCOUNT: &str = "webfang-legacy-key-account";
        let legacy_key = format!("keyring-rs:{ACCOUNT}@{SERVICE}");

        // RED: el store POR DEFECTO no direcciona la clave legacy.
        let default_store =
            linux_keyutils_keyring_store::Store::new().expect("store por defecto debe construirse");
        assert_ne!(
            default_store.delimiters[0], LEGACY_PREFIX,
            "el store por defecto usa el prefijo \"keyring:\" y por lo tanto \
             direcciona OTRA credencial: sin la configuración de este módulo \
             la clave del usuario quedaría huérfana"
        );

        // GREEN: el store de producción está configurado con la clave legacy.
        let store = super::store().expect("store de producción debe construirse");
        assert_eq!(
            store.delimiters,
            [
                LEGACY_PREFIX.to_string(),
                LEGACY_DIVIDER.to_string(),
                LEGACY_SUFFIX.to_string()
            ],
            "el store de producción tiene que usar la configuración legacy"
        );
        assert!(
            !store.service_no_divider,
            "`keyring` 3 no rechazaba un service con el divisor dentro: \
             rechazarlo ahora dejaría fuera credenciales existentes"
        );

        // Y la clave que el store REAL produce es byte a byte la legacy.
        // Esto exige el keyring del kernel; si el entorno no lo tiene, la
        // comprobación de configuración de arriba es la que sostiene el
        // contrato (y se avisa, para que el skip no pase por verde).
        match store.build(SERVICE, ACCOUNT, None) {
            Ok(entry) => {
                let description = entry
                    .as_any()
                    .downcast_ref::<linux_keyutils_keyring_store::Cred>()
                    .expect("la entrada del store de keyutils")
                    .description
                    .clone();
                assert_eq!(
                    description, legacy_key,
                    "la clave de la credencial tiene que ser byte-idéntica a la legacy"
                );
            },
            Err(keyring_core::Error::NoStorageAccess(_)) => {
                eprintln!(
                    "AVISO: sin keyring del kernel en este entorno; la comprobación \
                     de la clave real se omite y el contrato lo sostiene la \
                     configuración del store"
                );
            },
            Err(err) => panic!("error inesperado al construir la entrada: {err}"),
        }
    }

    /// La configuración que usa el store es la legacy, y no una constante
    /// repetida: el test anterior y la producción leen la misma función.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_store_configuration_is_the_legacy_one() {
        let config = super::store_config();
        assert_eq!(config.get("prefix"), Some(&LEGACY_PREFIX));
        assert_eq!(config.get("divider"), Some(&LEGACY_DIVIDER));
        assert_eq!(config.get("suffix"), Some(&LEGACY_SUFFIX));
    }
}
