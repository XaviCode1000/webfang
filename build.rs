// build.rs - Configuración de build para zvec-sys
//
// Requiere en el sistema:
// - CMake 3.13+
// - C++17 compiler (GCC 9+ o Clang 10+)
// - liblz4-dev (Linux) o lz4 (macOS)
//
// Para instalar dependencias en Ubuntu/Debian:
//   sudo apt-get install -y build-essential cmake git pkg-config liblz4-dev
//
// Para macOS:
//   brew install cmake git lz4

fn main() {
    // zvec-sys se compila solo cuando la feature "zvec" está habilitada
    // El crate maneja su propia configuración de build internamente

    // Indicamos a Cargo que re-run si el feature cambia
    println!("cargo:rerun-if-changed=build.rs");
}
