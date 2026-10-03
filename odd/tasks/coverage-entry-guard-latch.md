# Fix: latch del entry guard SSRF compartido entre tests en `--lib`

Issue: #1788 (flake de `Coverage` — el guard de entrada no está armado)

## Contexto

`Coverage` corre `cargo llvm-cov --all-features --workspace --lcov` **sin `--nextest`**, o sea
libtest: todos los tests de `--lib` en **un proceso, N hilos**. `webfang_test_utils::EnvGuard`
escribe la env con `unsafe { env::set_var }` bajo `ENV_LOCK` (no reentrante) y lo **desarma
durante toda su vida**.

`ENV_LOCK` serializa solo a quienes lo toman. Cuatro tests de
`crates/webfang_core/src/application/crawler/sitemap_discovery.rs` exigen que el guard esté
**armado** y **no toman el lock**: si un hilo hermano tiene abierta una ventana
`entry_guard_off()`, observan el guard caído, el literal RFC1918 pasa, y el downloader
diala `192.168.1.5:59999` → `Http { Connect }` en vez de `InvalidUrl`.

## Tareas

1. **`EnvGuard::entry_guard_on()`** en `crates/webfang_test_utils/src/lib.rs`.
   Toma `ENV_LOCK` por toda su vida y falla con diagnóstico explícito si
   `DISABLE_ENTRY_GUARD_ENV == "1"`. Serializa contra todo `entry_guard_off()` /
   `ssrf_hatches_off()` y convierte un leak real en un error que nombra la causa.
   Documentar por qué el caso "armado" necesita el lock igual que el caso "desarmado".

2. **Los 4 tests que exigen armado** lo toman:
   `sitemap_discovery_rejects_loopback_seed_pre_socket`,
   `..._rejects_forbidden_literal_sitemap_url`,
   `..._rejects_robots_directive_pointing_at_literal`,
   `..._rejects_index_child_pointing_at_literal`.
   Corregir el comentario de L1216, que hoy razona "no `EnvGuard` ⇒ production posture".

3. **Test de aceptación de #1788**: uno que falle si el latch no se restaura tras una salida
   por panic (`catch_unwind` + aserción sobre el valor restaurado).

## Verificación

- `cargo check --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`
- `cargo fmt --all -- --check`
- `cargo llvm-cov --all-features --workspace --lcov -- --skip test_mcp` (el comando exacto
  del job que falla) — debe reproducir la postura de libtest, no la de nextest.
- `cargo nextest run -p webfang_core --lib` (no debe regressar)

## Criterio de aceptación de la issue

- [x] Causa identificada: leak de estado global entre tests de `--lib` bajo libtest paralelo.
- [x] Test que falle si el latch no se restaura tras panic.
- [ ] `Coverage` verde (se valida en CI, no localmente — 3 corridas consecutivas).
- [x] El mensaje del SSRF **sigue verificándose**: `InvalidUrl` + "SSRF detectado" + el literal.

### Por qué ese checkbox no se cierra con este PR

Revisando CI despues de publicar: el job `Coverage` esta rojo, pero **no por este PR**.
Falla `mcp_server::auth::tests::a_rejection_log_carries_neither_the_expected_nor_the_presented_token`
(`crates/webfang_mcp/src/mcp_server/auth.rs:308`), en un crate que este cambio no toca, y
**exactamente el mismo test falla en `main`** (run `37079594733`, job `Coverage`, commit
anterior a este PR). Es un flake preexistente de otra clase, tambien por env compartida en
el mismo proceso libtest.

Los tests objetivo de este PR si pasaron en CI. Como `Coverage` no es check requerido, esto
no bloquea el merge, pero si impide que la acceptance de la issue se cumpla por la via de
"3 corridas consecutivas": con el flake de auth vivo, ese contador no avanza.

Por eso el follow-up que este PR deja afuera — agregar `--nextest` al job — no es un
nice-to-have: es el cierre real de #1788, porque mata la clase de los dos flakes a la vez.

## Reproducción: antes / después

Mismo binario `--lib`, mismo filtro, mismas 16 hilos, 15 corridas cada uno.

| | fallos |
| :--- | :--- |
| `HEAD` (sin fix) | **15/15** |
| Con el fix | **0/15** |

Panic reproducido byte por byte como el de la issue, en su línea exacta
(`sitemap_discovery.rs:1291`):

```
expected CrawlError::InvalidUrl, got: Http { status: 0, url: "max retries exceeded:
error sending request for uri (http://192.168.1.5:59999/sitemap.xml): client error (Connect)" }
```

El segundo test sin fix falla por la misma causa con otro síntoma:
`rejects_loopback_seed_pre_socket` recibe `Parse("invalid sitemap structure")`
— el seed loopback atravesó el guard y llegó al parser.

## Verificación ejecutada

| Gate | Resultado |
| :--- | :--- |
| `cargo check --all-targets --all-features` | verde, 13.84 s |
| `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | verde, 0 warnings |
| `cargo fmt --all -- --check` | verde |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` | verde |
| `cargo test -p webfang_test_utils --all-features` | 27/27 |
| `cargo test -p webfang_core --lib --all-features` × 3, 16 hilos | 2735/2735 en las tres |

## Nota de proceso

El primer intento de writer murió colgado, y no por el bug que corregía: su propio
test `entry_guard_on_refuses_a_disarmed_entry_guard` tomaba `ENV_LOCK` con
`entry_guard_off()` y después invocaba `entry_guard_on()` **en el mismo hilo**, que
reintenta el lock no reentrante. Deadlock: el binario de test se quedó colgado 13
minutos sin producir salida. Un deadlock de este tipo **no se manifiesta como test
fallido**, sino como proceso que deja de responder — indetectable desde el log de
CI salvo por el timeout del job.

La lección es la misma que motivó el fix, aplicada a un nivel más arriba: si el
problema es que un mutex no reentrante se puede reentrar, entonces *cualquier* test
que lo tome dos veces en el mismo hilo tiene el mismo riesgo, incluidos los que
escriben el arnés. El test se reescribió para fijar el rechazo sobre
`assert_entry_guard_armed` — la unidad que decide — en vez de sobre el constructor
que además toma el lock.

## Commits

- `749b9fa1` — `fix(test): give SSRF entry-guard observers the lock mutators already take (#1788)`
  en `fix/coverage-entry-guard-latch`. PR #1791.
- (este commit) — corrige el mojibake del doc, registra la identidad del commit de trabajo y
  anota el flake de auth que impide cerrar la acceptance de la issue por la via de CI.
