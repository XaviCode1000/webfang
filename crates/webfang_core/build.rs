fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The `built` build-dependency is GONE (issue #1797): it pulled `git2` and
    // `libgit2-sys` into the build graph to feed `version_string()`, a function
    // with zero call sites. Its only consumer is deleted, so the metadata it
    // produced was read by nobody. The `cfg(loom)` emission below is this
    // build script's OTHER mission and is untouched by that removal.

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
