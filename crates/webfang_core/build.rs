fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    built::write_built_file().expect("Failed to acquire build-time information");

    // ADR-0014 loom model tests: emit `cfg(loom)` for THIS crate's compilation
    // ONLY when the `loom-model` feature is enabled. Build-script cfgs are
    // crate-scoped, so the rest of the dependency graph (tokio, wiremock,
    // concurrent-queue — all of which have `cfg(loom)` code paths that require
    // `loom` to be linked) compiles untouched. This is why the flag is NOT
    // passed via RUSTFLAGS, which would apply globally and break those crates.
    if std::env::var("CARGO_FEATURE_LOOM_MODEL").is_ok() {
        println!("cargo:rustc-cfg=loom");
    }
}
