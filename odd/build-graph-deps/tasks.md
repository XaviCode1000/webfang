# Grafo de build — Tasks

- [ ] T1 · S1 — `crates/webfang_core/src/infrastructure/autotuning.rs`: `ram_budget_from_bytes` (público, bytes→KiB explícito) + `ram_budget_from_kib` (privado, la regla del 70 %); borrar `ram_budget_from` del espacio público; `detect_ram_budget` lee con `sysinfo::System::new()` + `refresh_memory()` + `total_memory()` bajo `#[cfg(not(miri))]`, gate `#[cfg(feature = "persistence")]` intacto; docs de módulo, fallback y `warn!` actualizados. Prueba: test nombrado `ram_budget_from_bytes_converts_to_kib` + `test_ram_budget_from_32gib_machine` (entrada en bytes) + `test_ram_budget_from_none_falls_back_to_2gib` sobre la función nueva; `cargo nextest run -p webfang_core --lib autotuning` verde.
- [ ] T2 · S1 — `crates/webfang_core/Cargo.toml` (feature `persistence` y dependencia opcional `sys-info`) y `Cargo.toml` del workspace (`sys-info = "0.9"`): borrar `sys-info`; regenerar `Cargo.lock`. Prueba: `grep -c '^name = "sys-info"' Cargo.lock` → 0 (lo afirma T7).
- [ ] T3 · S2 — `crates/webfang_core/src/infrastructure/http/waf_engine.rs` y `crates/webfang_core/src/domain/scraper_port/dom_pruner.rs`: `once_cell::sync::Lazy` → `std::sync::LazyLock`; `crates/webfang_core/tests/tokio_console_smoke_test.rs`: `once_cell::sync::OnceCell` → `std::sync::OnceLock`; borrar `once_cell` de `crates/webfang_core/Cargo.toml` y de `[workspace.dependencies]`. Prueba: cero `use once_cell` en `crates/` (src y tests) y `once_cell` fuera de los manifiestos (T7); tests de `waf_engine`, `dom_pruner` y `tokio_console_smoke_test` verdes.
- [ ] T4 · S3 — `crates/webfang_core/build.rs`: borrar `built::write_built_file()`, conservar el cfg de `loom` y el `rerun-if-changed`; `crates/webfang_core/src/lib.rs`: borrar el módulo `built_info` y `version_string()`; `crates/webfang_core/Cargo.toml`: borrar `[build-dependencies] built`; regenerar `Cargo.lock` y `fuzz/Cargo.lock`. Prueba: `built`, `git2` y `libgit2-sys` fuera de los dos lock (T7); `core_test.rs::version_contains_version_string` verde; el `--version` de la CLI sigue imprimiendo la versión del paquete.
- [ ] T5 · S3 — `deny.toml`: purgar los ignores `RUSTSEC-2026-0183` y `RUSTSEC-2026-0184` sólo si `git2` ya no está en `Cargo.lock` ni en `fuzz/Cargo.lock` (su razón escrita queda sin objeto). Prueba: `bash scripts/check_advisory_policy.sh` en verde; si `cargo-deny`/`cargo-audit` no están instalados, el paso se reporta como pendiente, no se maquilla en verde.
- [ ] T6 · S4 — `crates/webfang_benchmark/Cargo.toml`: quitar `features = ["chromium"]` de la dependencia `webfang_core` y declarar `[features] chromium = ["webfang_core/chromium"]`; `crates/webfang_benchmark/src/runner.rs`: `STRATEGIES` incluye `JsStrategy::Full` sólo con la feature; `crates/webfang_benchmark/tests/e2e_smoke_test.rs`: las aserciones del conjunto de tiers siguen a la feature. Prueba: `cargo nextest run -p webfang_benchmark` verde sin la feature y con `--features chromium`; `cargo tree --offline --workspace -e normal --prefix none | grep -c '^chromiumoxide'` → 0 y el mismo con `-p webfang_benchmark --features chromium` → ≥ 1.
- [ ] T7 · S1–S4 — `scripts/test_build_graph.sh` (nuevo): harness de negación del grafo, con control positivo por check. Checks: `sys-info` fuera de `Cargo.lock`; `built`/`git2`/`libgit2-sys` fuera de `Cargo.lock` y de `fuzz/Cargo.lock`; `once_cell` fuera de todo manifiesto pero presente en `Cargo.lock`; cero `use once_cell` en `crates/**/*.rs`; el benchmark declara `chromium = ["webfang_core/chromium"]` y no pide `features = ["chromium"]`; `chromiumoxide` → 0 en el árbol default y ≥ 1 con la feature del benchmark; `deny.toml` sin `RUSTSEC-2026-0183`/`0184`. Salida `PASS=<n> FAIL=<n>`. Prueba: RED contra el árbol pre-fix por la razón del defecto; GREEN después de T1–T6.
- [ ] T8 · S1–S4 — cablear T7 en `.github/workflows/ci.yml` (job `repo-guards`). Coordinación: el mapa de colisiones del plan lista `ci.yml` para L2, L3, L4 y L13 y no para L9 ⇒ este paso necesita la serialización que fije el owner (o aplica la regla del plan: cada lote commitea su step en su rama y se re-aplica sobre el `main` nuevo).
- [ ] T9 · S1–S4 — verificación: `cargo nextest run -p webfang_core --lib autotuning`; `cargo nextest run -p webfang_benchmark` con y sin `--features chromium`; `bash scripts/test_build_graph.sh`; `bash scripts/check_advisory_policy.sh`; `cargo check` + `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` + `cargo fmt --all -- --check` + `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`.

Spec: `odd/tasks/build-graph-deps.md`. Cierra #1799, #1797, #1796.

## Estado (2026-10-22)

- Lote **L9** del plan de backlog (`odd/tasks/backlog-implementation-plan.md`), rama
  `fix/build-graph-deps`, worktree
  `/home/xavi/Projects/Rust/webfang-worktrees/fix-build-graph-deps`, base `3a785d8e`.
- Este archivo es **sólo especificación**: no se escribió código de producción, no se
  corrió `cargo`, no se commiteó nada. Sin PR (decisión del owner).
- **Premisas falsas que la spec corrige** (verificadas en el código, no en la issue):
  `sys-info` 0.9.1 devuelve **KiB en todas las plataformas** (macOS incluida), así que no
  hay sobreestimación del presupuesto de RAM que corregir; el "6" de líneas
  `chromiumoxide*` es una medición caducada (hoy son 4 crates en el lock); y el `build.rs`
  propio con `GIT_COMMIT_HASH_SHORT`/`BUILT_TIME_UTC` no se escribe (decisión D18 del
  owner): la función que lo alimentaría tiene 0 call sites.
- **Test nombrado del lote**: `autotuning.rs::ram_budget_from_bytes_converts_to_kib`
  (más `scripts/test_build_graph.sh` para lo que es grafo y no comportamiento).
