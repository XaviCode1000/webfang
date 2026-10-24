# Los crates sin mantenimiento salen del grafo — Tasks

- [ ] T1 · S2, S3, S4 — `Cargo.toml` (workspace) y `crates/webfang_core/Cargo.toml`: `serde_yaml`
  sale, `serde_norway` entra. `crates/webfang_core/src/infrastructure/output/frontmatter.rs`
  (`generate_with_metadata` + los pines de PI-7/#1601) se adaptan. En
  `crates/webfang_core/src/error.rs` se borra `ScraperError::Yaml` y su brazo en `ErrorClass`
  (commit con marcador `BREAKING CHANGE:`). Prueba: los tests existentes de `frontmatter.rs` y el
  snapshot `obsidian_byline_artifact_test__empty_byline_fragment_repaired_in_excerpt.snap` **sin
  cambios** (el frontmatter tiene que salir byte a byte igual).
- [ ] T2 · S2, S3, S4 — `deny.toml`: se borra la entrada `RUSTSEC-2024-0449` (con su `# Expires:`
  y su bloque de comentario). Prueba: `bash scripts/check_advisory_policy.sh` en verde y
  `bash scripts/test_advisory_policy.sh` en verde.
- [ ] T3 · S3 — `Cargo.lock` y `fuzz/Cargo.lock` regenerados (el del fuzz está stale y es lo que
  mantiene viva la supresión de `RUSTSEC-2024-0449`). Prueba: `cargo tree -p webfang_core
  --target all` sin `unsafe-libyaml` y el audit del lock del fuzz en verde dentro de
  `scripts/check_advisory_policy.sh`.
- [ ] T4 · S5, S6, S8 — `crates/webfang_core/Cargo.toml`: `keyring` sale de las tres tablas
  target-specific; entran `keyring-core` y los tres stores por plataforma. Módulo nuevo
  `crates/webfang_core/src/domain/keyring_store.rs` (el único lugar que decide store y clave,
  registrado en `domain/mod.rs`). Se adaptan `domain/auth_source.rs::resolve_keyring` y
  `domain/local_secret_store.rs::detect_keyring` / `probe_reports_operational_backend`. Prueba: el
  test in-module nuevo `linux_keyutils_description_matches_the_legacy_keyring_3_layout` (la clave
  no cambia: `keyring-rs:{account}@{service}`), los tests existentes
  `probe_ok_or_noentry_means_backend_available` y `probe_any_other_error_means_backend_unavailable`,
  y `no_implicit_fallback_from_keyring_to_env`.
- [ ] T5 · S7 — `deny.toml`: se borran las tres entradas `RUSTSEC-2026-0370/0371/0372` y su bloque
  de comentario. Prueba: `bash scripts/check_advisory_policy.sh` en verde sobre el lock
  regenerado (`zbus`, `zbus_names`, `zvariant` y `zvariant_derive` fuera del lock).
- [ ] T6 · S1 — `deny.toml`: se purga la entrada `RUSTSEC-2024-0436` y se corrige la nota del
  "RE-ADD PROTOCOL". `scripts/check_advisory_policy.sh`: check nuevo "ningún ignore puede quedar
  muerto" (todo id suprimido tiene que matchear en alguno de los dos lockfiles según el propio
  `cargo audit`). `scripts/test_advisory_policy.sh`: caso nuevo de negación (un id que el audit no
  matchea tiene que fallar el guard; matcheado, pasa) — el caso se ve **RED** contra el guard de
  hoy.
- [ ] T7 · S1–S8 — verificación del lote: `bash scripts/test_advisory_policy.sh` en verde con el
  caso nuevo, `bash scripts/check_advisory_policy.sh` en verde, los tests de `webfang_core` que
  tocan frontmatter y keyring en verde, `cargo check --workspace --all-targets --all-features`,
  `cargo clippy --all-targets --all-features -- -D warnings` y `cargo fmt --all -- --check` en
  verde.
- [ ] T8 · S1–S8 — `scripts/quality-baselines.json`: medir el delta de duplicación después del
  cambio y corregir el baseline **sólo hacia abajo** si la medición lo exige (con la cifra medida
  en la nota); si no cambia, el archivo no se toca. Colisión con L12 y el PR #2230: este lote sólo
  hace la corrección descendente.

Spec: `odd/tasks/unmaintained-crates.md`. Cierra #1984 y #2031.
