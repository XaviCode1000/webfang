# Falsos verdes de test — Tasks

Spec: `odd/tasks/false-green-tests.md`. Rama `fix/false-green-tests`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-false-green-tests`.
Cierra #2066, #1852, #1828, #1834, #1816. **No** cierra #1833 (espera O4, decisión del owner).

- [ ] T1 · S1 — `crates/webfang_core/src/cli/crash_points.rs` (`kill_self`): detectar
  `getpid() == 1` **antes** de enviar el `SIGKILL` (en un init de PID-namespace el kernel lo
  descarta aunque `kill()` devuelva 0) y morir por el **camino diagnóstico único** — el mismo que
  usa la rama del `kill` rehusado — con un `tracing::error!` que nombre el descarte y conserve el
  `errno` en la rama del `errno`. Sin flag, sin env var, sin segundo camino de muerte.
- [ ] T2 · S1 — `crates/webfang_core/tests/crash_matrix_test.rs`: test
  `crash_point_never_hangs_when_sigkill_is_discarded`. Lanza el binario real como init de un
  PID-namespace (`unshare --user --map-root-user --fork --pid`) con `WEBFANG_CRASH_AT` armado;
  espera con ventana acotada; afirma que el hijo **no se cuelga**, que **no** sale con éxito y que
  su stderr trae el diagnóstico. RED contra el código pre-fix = el hijo se cuelga (el síntoma real).
  En Linux, si el namespace no se puede crear, el test **falla** con el comando de remediación
  (no se salta en silencio). En plataformas sin namespaces de PID el test no existe (`cfg`).
- [ ] T3 · S2 — `crates/webfang_mcp/Cargo.toml`: `[features] default = ["mcp"]` (la feature `mcp`
  es un marker vacío: no cuesta build ni dependencias) + `crates/webfang_test_utils/Cargo.toml`:
  `webfang_core` con `default-features = false` (D8/O1). Se verifica con el conteo real:
  `cargo nextest list -p webfang_mcp` lista los 22 archivos gated sin flags.
- [ ] T4 · S3 — `crates/webfang_core/Cargo.toml`: declarar con `required-features` los cuatro
  targets gated (`sqlite_integration`→`persistence`, `stress_test`→`console`,
  `tokio_console_smoke_test`→`console`, `adaptive_selector_integration`→`adaptive-selectors`).
- [ ] T5 · S3 — `scripts/check_empty_test_targets.sh` (nuevo: guard estático, sin cargo, sin red:
  todo target gated por una feature fuera del `default` tiene que estar declarado con
  `required-features`) + `scripts/test_empty_test_targets.sh` (nuevo: harness de negación con
  fixtures `mktemp`, cuatro casos) + un step en el job `repo-guards` de `.github/workflows/ci.yml`.
- [ ] T6 · S4 — `crates/webfang_core/tests/crash_matrix_test.rs`: fila `during_cancel_drain`
  (SIGINT entregado cuando wiremock vio la k-ésima petición, con el patrón de
  `tests/behavioral/cli/signal_lifecycle_test.rs`) y **borrado** del comentario "reserved for
  PR5b" + `docs/persistence-resume.md`: la tabla de la matriz lista las once filas (las diez
  actuales + `during_cancel_drain`, incluyendo `mid_extraction` y `post_extraction_pre_pipeline`,
  que hoy no lista).
- [ ] T7 · S5 — `crates/webfang_core/tests/sqlite_integration.rs`:
  `test_sqlite_rows_pass_integrity_check_after_reopen` (escribir filas → reabrir el pool →
  `PRAGMA integrity_check` ⇒ `ok` → las filas sobreviven). La fila WAL-kill **no** se agrega: exige
  un punto de arming nuevo en producción (D16).
- [ ] T8 · S5 — decisiones registradas (en la issue, con evidencia, no con promesas) para las otras
  cuatro filas de la AC 1 de #1828 (writer death, network partition, 0xFF JSONL, `PermitGuard`) y
  para el `cfg(loom)` de la AC 3. Sin código.
- [ ] T9 · S6 — `crates/webfang_core/src/infrastructure/platform/windows_console.rs`: test *in-module*
  `#[cfg(windows)]` `a_real_console_close_event_reaches_the_termination_source` (instala el
  handler, lanza un hijo que llama `GenerateConsoleCtrlEvent(CTRL_CLOSE_EVENT, 0)`, espera con
  timeout y afirma que el evento llega; omite limpiamente con el motivo impreso si
  `GetConsoleWindow()` es null) + el generador `#[ignore]` que el padre invoca con
  `--ignored --exact` + `docs/test-inventory.md` (fila del `#[ignore]` nuevo en el catálogo
  congelado) + el comentario de la lane `test-crossplatform` en `.github/workflows/ci.yml`.
- [ ] T10 · S8 — `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs`: fixture 429 con
  `Retry-After` no nulo + `issue_1816_waf_gauntlet_429_retry_after_is_observed_and_exits_69`
  (pacing observado como cota inferior + exit 69) +
  `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs`:
  `an_append_issued_during_an_inflight_flush_is_durable_after_the_next_flush` (la AC 3 de #1816
  corregida al contrato que el diseño tiene).
- [ ] T11 · S7 — sin código: #1833 se comenta con la evidencia (la decisión registrada del
  2026-09-30 en `odd/tasks/windows-console-lifecycle.md` sigue vigente; la lane `windows-latest` ya
  compila y ejecuta el brazo Windows) y queda a la espera de O4.
- [ ] T12 · S1–S8 — verificación: `cargo check`; `cargo clippy --all-targets --all-features --
  -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`; `cargo fmt --all --
  --check`; `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps`;
  `cargo nextest run` de los targets tocados; el harness nuevo en verde; `bash -n` de los scripts
  tocados; y el RED demostrado de T2 y de T5.

## Notas de alcance (del Log de la spec)

- **S7 (#1833) no se implementa**: su propuesta (`scripts/check_windows_platform.sh` + gate local)
  revierte la decisión registrada del 2026-09-30 ("the CI `test-crossplatform` lane only";
  `rustup target add x86_64-pc-windows-msvc` was declined). O4 sigue pendiente con el owner.
- **Waivers de #1816** (la AC los admite: *"resolved or explicitly waived"*): AC 2 (SIGINT E2E
  sobre el rate limiter: la mitad unitaria ya existe en `crawl_task.rs`; la variante E2E depende de
  wall-clock), AC 4 (cobertura de `application/pipeline/`: otra raíz) y AC 5 (los asserts débiles
  que quedan, el size-bound de los stores y los tres nombres N-1: familia de calidad de
  aserciones de #1589). Los dos de los cuatro asserts débiles que la issue enumera **no existen**
  (uno estaba en el crate `webfang_ai`, borrado por ADR-0004 paso 6; el otro apuntaba a una línea
  que ya derivó a un `assert!(...is_err())` real).
- **Colisiones de path agregadas al mapa del plan**: `crates/webfang_core/Cargo.toml` (L8 → L9 →
  L10), `crates/webfang_mcp/Cargo.toml` (L7 → L8), `.github/workflows/ci.yml` (un step por lote).
- **Fuera de alcance, a propósito**: `crates/webfang_core/tests/tokio_console_smoke_test.rs` sigue
  vacío en todos los builds (`tokio_unstable` es un cfg, no una feature; el workspace nunca lo
  habilita). Hacerlo correr exige tocar la feature de `tokio` o borrar el archivo: decisión del
  owner.
