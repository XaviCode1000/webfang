use std::env;
use std::path::Path;
use thiserror::Error;
use tracing::info;

/// Errores relacionados con la configuración de Brave
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Sistema operativo no soportado: {0}")]
    UnsupportedOS(String),

    #[error("Brave no encontrado en: {0}")]
    BraveNotFound(String),
}

/// Obtiene la ruta de instalación de Brave según el OS
///
/// # Plataformas soportadas
///
/// - **Linux**: `/usr/bin/brave`
/// - **macOS**: `/Applications/Brave Browser.app/Contents/MacOS/Brave Browser`
/// - **Windows**: `C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe`
fn get_brave_path() -> Result<String, ConfigError> {
    let path = match env::consts::OS {
        "linux" => "/usr/bin/brave".to_string(),
        "macos" => "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".to_string(),
        "windows" => {
            "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe".to_string()
        }
        os => return Err(ConfigError::UnsupportedOS(os.to_string())),
    };
    Ok(path)
}

/// Valida que Brave esté instalado en la ruta esperada
fn validate_brave_installation(brave_path: &str) -> Result<(), ConfigError> {
    if Path::new(brave_path).exists() {
        Ok(())
    } else {
        Err(ConfigError::BraveNotFound(brave_path.to_string()))
    }
}

/// Configura las variables de entorno necesarias para usar Brave con spider
///
/// # Errores
///
/// Retorna un `ConfigError` si:
/// - El sistema operativo no es soportado
/// - Brave no está instalado en la ruta esperada
///
/// # Ejemplo
///
/// ```no_run
/// use brave_rag_scraper_v2::config;
///
/// config::setup_brave_env()?;
/// println!("Brave configurado correctamente");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn setup_brave_env() -> Result<(), ConfigError> {
    let brave_path = get_brave_path()?;
    validate_brave_installation(&brave_path)?;

    // Configurar variables de entorno para que spider use Brave
    unsafe {
        env::set_var("CHROME_PATH", &brave_path);
        env::set_var("BRAVE_ENABLED", "true");
    }

    info!("✅ Entorno de Brave configurado en: {}", brave_path);
    Ok(())
}

/// Inicializa el sistema de logging con tracing y tracing-subscriber
///
/// Configura un formato de logs legible con timestamps y niveles de severidad.
/// La verbosidad se controla con la variable de entorno RUST_LOG.
///
/// # Ejemplo
///
/// ```no_run
/// use brave_rag_scraper_v2::config;
///
/// config::init_logging();
/// ```
pub fn init_logging() {
    use tracing_subscriber::filter::EnvFilter;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("brave_rag_scraper_v2=info,spider=warn"));

    tracing_subscriber::registry()
        .with(fmt::layer().pretty().with_target(true))
        .with(env_filter)
        .init();
}
