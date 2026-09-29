# Feature: crossplatform-portability (issue #1631)

**Issue:** #1631 — `fix(platform): cross-platform integration suite portability — first Windows/macOS CI run`
**Labels:** `type:bug`, `status:approved` (ya aprobado, se puede abrir PR sin esperar al maintainer)
**Branch:** `fix/crossplatform-portability`
**Worktree:** `~/Projects/Rust/webfang-worktrees/fix-crossplatform-portability`

## Por qué esta issue (y por qué NO es urgente)

`main` muestra rojo en `Tests (macos-latest)` y `Tests (windows-latest)`, y esas lanes son
**advisory por diseño**.

> ⚠️ **Corrección 2026-09-28.** Este documento originalmente justificaba la issue como "cuello de
> botella con dependientes pagos (main + todo PR nuevo)". **Era falso**, y fue corregido tras
> verificación cruzada contra las fuentes:
>
> - El job `gate` (ci.yml:748) tiene `needs: [change-scope, toolchain, repo-guards, fmt, clippy,
>   test-core, test-full, test-release-provenance, security, doc-quality]`. **`test-crossplatform`
>   NO está ahí**, y el comentario del propio job (ci.yml:472-483) dice literal *"Advisory only —
>   never in `gate` needs … must not block merges"*.
> - Los required contexts de branch protection son exactamente tres: `Validate PR metadata`,
>   `cargo-mutants (PR diff)`, `CI Gate`. Ni `Coverage` ni las lanes cross-platform están.
> - Consecuencia: **#1631 es trabajo válido, pero no bloquea merges ni a `main`.** No debe
>   motorizar ninguna decisión de planificación.
>
> El error de fondo fue tratar un job rojo como evidencia de un gate sin abrir la definición del
> gate. Un job en rojo no es un bloqueo hasta que se lea su `needs:` y la lista de required
> contexts.

## Alcance: 6 clases, no 1

El issue NO es un fix único. Son 6 clases de fallo independientes, con reproducibilidad
distinta. Se corta en **slices encadenados**, uno por PR.

| Clase | Síntoma | OS | Dónde vive (verificado) | Reproduce |
| --- | --- | --- | --- | --- |
| INT-1 | texto errno del SO en snapshots | ambos | **harness** (las 2 copias de `redact_nondeterministic`) | **sí, siempre** |
| INT-2 | separador `\` de Windows en markdown | Windows | **el test** (`sitemap_test.rs:113` arma el heading con `relative.display()`) | intermitente |
| INT-3 | backslash sin escapar en fixtures JSON | Windows | **harness** (`common/mod.rs:160`, `format!` crudo) | intermitente |
| INT-4 | config TOML de budget no llega a enforcement | ambos | **test + 1 línea de producto** | **sí, siempre** |
| INT-5 | hang de `batch_empty_file_exits_64` | Windows | sin tocar | no en el último run |
| INT-6 | `trybuild` excede el slow-timeout en target frío | Windows | no (infra de test) | **sí** |

### Corrección de la clasificación original (2026-09-28)

El issue clasificó INT-2 e INT-4 como "bugs de producto sospechados". **Verificado: no lo son**
(lectura directa del código + `codedb_callers` + fuente de `dirs-5.0.1`/`dirs-sys-0.4.1`):

- **INT-2** — el exporter ya escribe el árbol anidado **correctamente** en ambos OSes
  (`file_saver.rs:181` une el `to_full_path()` escrito con `/` a través de `Path::join`, que
  acepta `/` embebido en Windows). El que renderiza el heading es el propio test, con
  `relative.display()`, que es nativo de plataforma. Es test, no producto.
- **INT-4** — `dirs::config_dir()` resuelve `$XDG_CONFIG_HOME` en Linux,
  `$HOME/Library/Application Support` en macOS, y en Windows llama
  `SHGetKnownFolderPath(FOLDERID_RoamingAppData)` (API de Shell que **no lee ninguna env var** —
  verificado en `dirs-sys-0.4.1/src/lib.rs:151`). Los tests sólo setean `XDG_CONFIG_HOME`, así que
  macOS lo ignora en silencio, Windows **no es redirecteable por env**, y
  `ConfigDefaults::load` (`config.rs:59`) cae en silencio a `default()`. El test es cross-platform
  sólo en apariencia.

**Consecuencia:** T1 y T4 colapsan en un solo PR (todo test-side salvo el override de abajo), y
el override de producto pasa a ser **necesario** en vez de opcional: sin él, Windows no tiene
ninguna palanca de test.

### Decisiones tomadas por el maintainer

1. **Alcance del PR 1:** INT-1 + INT-2 + INT-3 + INT-4 juntas. Son un solo problema — "la suite
   asume Linux" — y desbloquean las dos lanes y `main` de una.
2. **INT-4:** override `WEBFANG_CONFIG` en `resolve_config_path()`. Aprobado explícitamente tras
   demostrar que el approach test-only por env var es **imposible en Windows**. Cierra además un
   hueco real de usuario: hoy un usuario de Windows no puede decirle a webfang dónde está su
   config.

### Bug de producto colateral (en el mismo PR)

`sanitize_env` (`cli_harness.rs:146`) hermeticiza `XDG_CACHE_HOME` pero **no** la config. Todo
test que no setee una env var de config explícita lee el `~/.config/webfang/config.toml` real del
desarrollador. Se cierra apuntando `WEBFANG_CONFIG` a un path hermético inexistente por
invocación.

## Regla de cierre del issue

Solo el **último** slice lleva `Closes #1631`. Todos los anteriores llevan
`Closes part of #1631` (pasa validación, no auto-cierra el umbrella — precedente #1010).

## Tareas

- [x] **T1 — Root-cause de INT-4 + reclasificación de INT-2.** Hecho 2026-09-28. Ver arriba.
- [x] **T2 — PR 1: INT-1 + INT-2 + INT-3 + INT-4 + el hole de hermeticidad.** Cerrada 2026-09-28
      en la PR #1651 (head `f0b70556`). La verificación que cierra la tarea es la **lane**, no la
      suite local: ver "Verificación de lane" abajo.
      Superficies: `cli_harness.rs`, `common/mod.rs`, `webfang_test_utils/src/lib.rs`,
      `sitemap_test.rs`, `budget_override_test.rs`, `webfang_cli/src/main.rs`.
- [ ] **T3 — Drift entre las dos copias de `redact_nondeterministic`.** La copia de
      `webfang_test_utils` no tiene la regla `<TRACE_ID>` que la de `cli_harness` sí tiene.
      30 call sites dependen de esto. **Fuera del PR 1 a propósito** — tocar sólo la regla nueva
      en ambas deja el drift preexistente igual de visible, que es lo correcto. Issue aparte.
- [x] **T4 — `WEBFANG_CONFIG` en los otros dos call sites.** **Obsoleta — cerrada sin trabajo
      adicional.** El commit `4646fa8d` borró la copia privada de `resolve_config_path` en
      `webfang_cli/src/main.rs` y movió el override al resolver canónico de
      `webfang_core::cli::config`. Los tres call sites del CLI llaman ahora a la función única, así
      que heredan el override sin tocar nada. De paso se cerró **#1648** por el camino corto: el
      defeecto de las dos copias no se documentó, se eliminó.
- [ ] **T5 — INT-5 + INT-6.** Hang de stdin en Windows y slow-timeout de `trybuild`. INT-6 tiene
      sus propios acceptance criteria en el issue (incluido "un fix por timeout no puede
      convertir una regresión real de compilación en pass o skip").
- [ ] **T6 — Cierre.** Re-verificar ambas lanes, reconciliar el inventario en un comentario de
      #1631, y `Closes #1631` en la PR final.

## Fuera de alcance (deliberadamente)

- **#1633** — `continue-on-error` en el lane advisory. Es la causa del ruido, pero es un
  cambio de `.github/` y tiene semántica de gate propia. No se mezcla con fixes de tests.
- **#1638** — el flake de `tracing` bajo llvm-cov. Issue separada, 1 test.
- **INT-6** requiere criterio propio en el issue; no se resuelve "de paso" con un timeout.

## Baseline verificado

8/8 de los tests afectados **pasan en Linux** (worktree `fix-crossplatform-portability` @ `fb78e2dc`):

```
toml_rate_limit_burst_zero_hard_errors          PASS  0.678s
toml_crawl_and_cli_download_survive_same_merge  PASS  0.772s
unreachable_host_stderr_mentions_failure         PASS  0.814s
mock_vault_has_obsidian_json                     PASS  0.004s
mock_vault_metadata_json_content                 PASS  0.004s
toml_concurrency_reaches_scrape_enforcement     PASS  0.844s
sitemap_url_scrapes_listed_urls                  PASS  0.397s
dry_run_refused_seed_exits_69_with_spanish_error PASS 0.378s
Summary: 8 tests run: 8 passed, 3822 skipped
```

Confirma que son fallos **de portabilidad**, no rotura general: la suite está verde donde el
problema no existe.

## Evidencia de commits

| # | Commit | Qué aportó |
| --- | --- | --- |
| 1 | `6c7ca07d` `feat(cli): honor WEBFANG_CONFIG…` | Override de producto (T2a) |
| 2 | `08d3e9d8` `test(core): make the integration suite…` | INT-1/2/3/4 en el harness (T2b) |
| 3 | `4646fa8d` `feat(cli): … and delete the duplicate config resolver` | Cierra #1648; **efecto colateral no declarado**: `dirs` quedó sin uso en `webfang_cli`, y el CLI pasó a honrar `XDG_CONFIG_HOME` absoluto en macOS/Windows |
| 4 | `0003501d` `fix(test): collapse the I/O error layer with the network failure` | **El fix de INT-1** — ver abajo |
| 5 | `f0b70556` `chore(cli): drop the now-unused dirs dep, and document WEBFANG_CONFIG` | Resuelve el rojo de `cargo-machete` y documenta la env var |

> ⚠️ **Los commits 1 y 2 solos NO cerraron INT-1.** Su "3813/3813 verde" era la suite **local en
> Linux**, donde el bug no se manifiesta. El commit 4 existe porque la lane de Windows siguió roja
> con 1 `FAILED` después de que 1 y 2 ya fueran verdes. La lección queda escrita: en una PR de
> portabilidad, el verde local **no** es evidencia.

## Verificación de lane (la autoritativa)

Run `36462875420`, head `f0b70556`:

| Lane | Resultado |
| --- | --- |
| `CI Gate` | pass |
| `Tests (macos-latest)` | pass |
| `Code quality (machete + duplication ratchet)` | pass |
| `Tests (windows-latest)` | **0 failed** — 866 run, 865 passed (1 leaky), 1 timed out, 16 skipped |

Los tres tests de las clases tocadas, en verde **sobre Windows real**:

```
unreachable_host_stderr_mentions_failure            PASS   5.847s   <- el blocker de INT-1
test_single_page_custom_timeout_is_used_by_scrape_client  PASS  13.725s
dry_run_refused_seed_exits_69_with_spanish_error    PASS   3.585s
```

**Lo único que queda rojo en Windows es `batch_empty_file_exits_64` (`TMT`, timeout ×3) — eso es
INT-5, explícitamente fuera de scope y trackeado en T5.** nextest sale distinto de cero también por
`TMT`, por eso el job sigue en rojo por una razón que esta PR declaró que no cubría.

### Lane final, head `e23dae4c` (run `36477820517`)

El run anterior era sobre `f0b70556`. El head de entrega es `e23dae4c`, y su lane está **completamente
verde, Windows y macOS incluidos**:

| Lane | Resultado |
| --- | --- |
| `CI Gate` | pass |
| `Tests (windows-latest)` | pass |
| `Tests (macos-latest)` | pass |
| `Tests (all features)` | pass |
| `Tests (unit + integration + behavioral)` | pass |
| `Clippy (strict)` | pass |
| `Check formatting` | pass |
| `Documentation quality` | pass |

El `TMT` de INT-5 **no** disparó en este run: el job de Windows salió verde sin el fix. No es
una señal de que INT-5 esté resuelto — sigue siendo flake, y T5 lo trackea. Lo que la lane final
demuestra es lo que esta PR afirma: INT-2, INT-3 e INT-4 están verdes **sobre Windows y macOS
reales**, que es la razón de ser del PR.

> La descripción de la PR citaba solo la evidencia local de Linux bajo "Verification". Eso leía
> como "no se probó en Windows" y era una inferencia razonable pero falsa: la matriz siempre corrió.
> Corregido en la descripción con el run y el head explícitos.

## Por qué el commit 4 fue necesario (mecanismo, para el que lea esto después)

El token `<NET_ERR>` del commit 2 unificaba el texto OS de la *hoja*, pero no la *capa de la cadena*
que webfang agrega arriba. Son dos variantes de producto distintas:

- `DownloadError::Io` → `#[error("I/O error: {0}")]` — `domain/downloader_port.rs:78`
- `DownloadError::Timeout` → `request timed out after {0}s` — `domain/downloader_port.rs:99`

Un connect **rechazado** cae en la variante `Io`; en Windows, el mismo connect a un puerto loopback
cerrado **pierde la carrera contra el timeout** y cae en `Timeout`, que no lleva esa capa. Resultado:
`error de red: I/O error: <NET_ERR>` en Linux contra `error de red: <NET_ERR>` en Windows. El fix es
que la regla consuma la capa, no que la matchee.

**Detalle que explica por qué el otro snapshot no se rompió:** `unreachable_host` corre con
`--timeout-secs 2` y `dry_run_refused_seed` con el presupuesto default. Ambos golpean un puerto
loopback cerrado, pero sólo el presupuesto corto pierde la carrera en Windows. Por eso
`dry_run_refused_seed` seguía verde con `I/O error:` presente mientras `unreachable_host` fallaba.

**Sobre el pin que faltaba:** los dos pins del commit 2 pasaban con la regla rota, porque cada uno
afirmaba sólo su propia forma — el de Unix además afirmaba que la capa `I/O error: ` *sobrevive*,
es decir, codificaba el bug. El defecto sólo era visible al comparar los dos. El commit 4 agrega un
tercer pin que asserta la **comparación** (`unix == windows`), no una forma individual. Dos pins
individualmente correctos no pueden ver una divergencia cross-platform por construcción.

## Nota de hygiene

- El título de la PR es `fix(cli,test): …` y **no** `fix(test): …`: la PR cambió comportamiento de
  producto (`WEBFANG_CONFIG` + el honor de `XDG_CONFIG_HOME` en el CLI). Como el merge por defecto
  es `--squash`, **release-plz lee el título de la PR, no el commit `feat(cli):` de adentro** —
  arrancando con `fix` el bump sigue siendo patch. Mergear con `--merge` en su lugar haría
  aflorear ese `feat:` como minor.
- `docs/src/cli-reference.md` recibió una sección "Configuration file" para `WEBFANG_CONFIG`
  (commit 5). Sin eso, la env var era indemostrable y la promesa de "cerrar un hueco de usuario"
  era falsa.

**Nota de conteo (para el que lea esto después):** la suite corre **3833** tests
(3813 passed + 20 skipped). `cargo nextest list` devuelve 3813 porque no lista los ignorados; no
es una discrepancia con el "3825 skipped" del run focalizado (8 + 3825 = 3833).

### El poder de un test se restauró en el MISMO commit

El worker reportó que `test_single_page_custom_timeout_is_used_by_scrape_client` perdía su
evidencia de que `--timeout-secs 1` se aplicaba — el `1s` del snapshot redactado. Parcialmente
cierto, pero con un matiz que el worker no distinguió. Lo verifiqué:

- **Verificado por falsación.** Cambié `--timeout-secs 1` → `5` (el mock delayea 2s, así que 5s
  no da timeout) y el test falla. O sea que la aserción restaurada **muerda**.
- **Precisión sobre el alcance real.** `timeout_secs` es `u64` (default 30). exit 69 + el
  `expect(4)` prueban que un timeout < 2s disparó y se reintentó 3 veces, pero **no distinguen
  `--timeout-secs 0` de `1`** — ambos dan exit 69. Sólo el mensaje separa 0 de 1, y ≥2 ya lo
  coge el exit 0. Por eso la aserción sobre el string sin redactar aporta algo real y no es
  decorativa.
- **Es cross-platform por construcción:** `request timed out after {0}s` es el
  `DownloadError::Timeout` **nuestro** (`domain/downloader_port.rs:99`), no un string de std ni
  del SO. Verificado.

El worker propuso un "follow-up slice en `cli_binary_test.rs`": se hizo en este PR, porque dejar
un test que ya no prueba lo que su nombre afirma es una regresión que yo introduje, y diferirla
la deja pudrir.
