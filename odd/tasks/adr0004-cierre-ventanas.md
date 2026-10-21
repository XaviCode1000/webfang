# ADR-0004 — cierre de las dos ventanas abiertas (shims + umbral 0.3)

Feature: cerrar las ventanas que ADR-0004 dejó abiertas y que quedaron sin ancla alcanzable
tras la publicación de v4.0.0. Cuatro work units (#2166, #2119, #2168, #2167), una por una,
cada una con su propio worktree, su rama y su PR.

## Specs

- **S1** — "vamos a crear las issues nuevas y luego  atacar todas las issues una por una, delega el trabajo a workers y que siempre utilicen las potentes tools de *codedb* y *codegraph* debes ordenarles que las usen."
- **S2** — Las tres cosas sin issue, registradas sólo en el log del ADR, que el usuario decidió convierte en issues:
  - "El ancla 3.2.0 es inalcanzable. support.json ya declara 4.0 STABLE / 3.0 MAINTENANCE / 2.13 EOL: ninguna release futura puede llegar a 3.2.0. Afecta a #2119 (ancla de los shims) y está anotado como 'PR de docs aparte' en el fuera de alcance de #2105."
  - "La semántica de --offline sigue desalineada: el guard de llm_wire.rs:141-147 rechaza todo provider open_ai_compatible, incluido loopback, mientras el aviso y el ADR prometen 'loopback permitido, remoto público → exit 78'. El log dice literalmente 'corregir el guard o el texto es decisión del owner' — y es la misma clase de mentira que el flag de mi slice."
  - "El umbral 0.3 huérfano (ADR §58): la mitigación (aviso por corrida + threshold_used) debía terminar en el flip, salvo decisión del owner sin golden set; hoy el aviso sigue emitido."
- **S3** — Decisiones del owner tomadas en esta sesión:
  - `--offline` → "Rechazar todo y corregir el texto": "El guard no cambia (exit 78 para cualquier provider, con o sin loopback). Se corrigen el aviso, el mensaje de error, doctor, la guía y el ADR §46. Es lo que ya afirma AGENTS.md tras el paso 6."
  - umbral 0.3 → "Aviso permanente documentado": "Se mantiene el aviso por corrida + threshold_used en el summary, y el ADR §58 lo registra como decisión permanente, con su riesgo (0.3 no está calibrado para el provider remoto actual)."
  - PRs ya abiertos → "Que los mergee el otro agente": "No los toco: la otra sesión tiene el trabajo en curso sobre #2106 y #2105. Yo ataco sólo #2166, #2119, #2168 y #2167, que no tienen PR."
- **S4** — Issues del ADR-0004 que entran en el alcance de esta feature: #2105 (unidad C, PR #2154), #2106 (unidad D, PR #2161), #2119 (shims, sin PR), #2123 (unidad E, PR #2145).
- **S5** — "Cerradas ya: A (#2099/#2100, ProviderKind::LocalOnnx eliminado) y B (#2104/#2108, los mensajes de SemanticError ya no nombran ONNX), más #2122 (E, PR #2124)."
- **S6** — "delega el trabajo a workers y que siempre utilicen las potentes tools de *codedb* y *codegraph*": cada worker recibe el mandato explícito de explorar con `codedb` y `codegraph` (CLI por `bash`, con la ruta ABSOLUTA del worktree) antes de editar, y verificar con `cargo check` antes de devolver.

## Tasks

| # | Tarea | S# | Route | Commit |
|---|-------|----|-------|--------|
| T0 | Crear las tres issues nuevas: #2166 (`--offline`), #2167 (umbral 0.3), #2168 (ancla 3.2.0) | S1,S2 | inline | n/a (issues) |
| T1 | #2166 — alinear aviso, mensaje de error, `doctor`, guía de migración y ADR §46 con el guard real (exit 78 para todo provider con `--offline`) | S2,S3,S6 | worker `gentle-ai-worker` + verificación inline | ✅ |
| T2 | #2119 — retirar los shims `--ai-model`, `--max-tokens`, `WEBFANG_MAX_TOKENS`, `AI_MODEL_ID`/`WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE`; marker de ruptura, merge con `--merge` | S4,S6 | worker `gentle-ai-worker` + verificación inline | ⏳ |
| T3 | #2168 — registrar en ADR-0004 que el ancla 3.2.0 quedó inalcanzable, que la promesa de la ventana se cumplió (2.13.0 minor + 3.0.0 major) y que la ventana cierra anticipadamente | S2,S3 | worker `gentle-ai-worker` (docs) | ⏳ |
| T4 | #2167 — registrar en ADR-0004 §58 la decisión "aviso permanente documentado" para el umbral 0.3, con su riesgo, y dejar el aviso y `threshold_used` coherentes | S2,S3 | worker `gentle-ai-worker` | ⏳ |

Fuera de alcance (decisión S3): #2105, #2106 y #2123 — sus PR (#2154, #2161, #2145) los mergea
la otra sesión. Este feature no hace `update-branch`, ni push, ni merge sobre esos tres PR.

## Log

- **L1** (2026-10-21, request verbatim): "que issues abiertas relacionadas con el adr siguien abiertas y sin ser tratadas aun?" →
  "vamos a crear las issues nuevas y luego  atacar todas las issues una por una, delega el trabajo a workers y que siempre utilicen las potentes tools de *codedb* y *codegraph* debes ordenarles que las usen.

  Issues abiertas del ADR-0004

  | # | Unidad | Qué falta | Estado |
  | #2105 | C (MED) | LocalEmbeddingPort y Tier2EmbeddingPort::Local no se construyen en producción ... | abierta, status:approved |
  | #2106 | D (LOW) | show y doctor imprimen granite-97m, un modelo local retirado ... | abierta, status:approved |
  | #2119 | shims | Retirar los shims de migración ONNX: --ai-model, --max-tokens, WEBFANG_MAX_TOKENS, AI_MODEL_ID/WEBFANG_AI_MODEL_ID, WEBFANG_AI_ENGINE. Exige marker de ruptura + merge con --merge. | abierta, status:approved |
  | #2123 | E (LOW) | --enable-ai / WEBFANG_MCP_AI — la mía, PR #2145 esperando merge. | en vuelo |

  Cerradas ya: A (#2099/#2100, ProviderKind::LocalOnnx eliminado) y B (#2104/#2108, los mensajes de SemanticError ya no nombran ONNX), más #2122 (E, PR #2124).

  Tres cosas sin issue, registradas sólo en el log del ADR

  1. El ancla 3.2.0 es inalcanzable. support.json ya declara 4.0 STABLE / 3.0 MAINTENANCE / 2.13 EOL: ninguna release futura puede llegar a 3.2.0. Afecta a #2119 (ancla de los shims) y está anotado como 'PR de docs aparte' en el fuera de alcance de #2105.
  2. La semántica de --offline sigue desalineada: el guard de llm_wire.rs:141-147 rechaza todo provider open_ai_compatible, incluido loopback, mientras el aviso y el ADR prometen 'loopback permitido, remoto público → exit 78'. El log dice literalmente 'corregir el guard o el texto es decisión del owner' — y es la misma clase de mentira que el flag de mi slice.
  3. El umbral 0.3 huérfano (ADR §58): la mitigación (aviso por corrida + threshold_used) debía terminar en el flip, salvo decisión del owner sin golden set; hoy el aviso sigue emitido."

- **L2** (2026-10-21, decisiones del owner): `--offline` → rechazar todo y corregir el texto (el guard no cambia). Umbral 0.3 → aviso permanente documentado, registrado en ADR-0004 §58. Los tres PR ya abiertos (#2154, #2161, #2145) los mergea la otra sesión; este feature no los toca.

- **L3** (2026-10-21, evidencia medida al mapear las ventanas): el fallo `Tests (windows-latest)` del PR #2145 es `webfang_core::scraper_service_test::test_batch_without_pacing_runs_unthrottled`, flaky ya trackeado en **#1962** — no es de #2123. `main` tiene su propio CI rojo (`Miri (core)`, `Security Audit`) en el run 39504890719.

- **L4** (2026-10-21, decisión del owner sobre el aviso `onnx_deprecation`): "El aviso se va con los shims (AC literal)". #2119 borra `crates/webfang_core/tests/onnx_deprecation_notice_test.rs` entero, así que la frase de `--offline` que T1 acaba de corregir en el aviso queda muerta en el slice siguiente — es el precio de la decisión y queda escrito acá para que no parezca un descuido. Dos consecuencias que T2 tiene que respetar: (1) los **dos tests del aviso de umbral 0.3** (`orphan_threshold_warning_fires_once_with_the_default_threshold` y `orphan_threshold_warning_does_not_fire_with_an_explicit_threshold`) **se reubican antes de borrar el archivo**, porque la decisión de T4 los deja permanentes; (2) la verdad de `--offline` que T1 instala en el aviso no se pierde: vive además en el mensaje del guard, en `doctor`, en el `--help`, en `installation.md` y en la corrección fechada del ADR §46.

- **L5** (2026-10-21, T1 amplía su propia superficie): el `--help` de `--offline` decía "Run AI model in offline mode" y su doc-comment hablaba del resolver de `hf_hub` — la misma mentira que el aviso, en el texto más leído del CLI. Se corrigió en T1 (y no en una unidad aparte) por dos razones: pertenece al mismo AC de #2166 ("ningún texto user-facing afirma…"), y abrir una issue nueva para eso la dejaría **bloqueada**: el label `status:approved` es protegido y los agentes no se auto-aprueban (#1981/#1993), así que ningún PR podría enlazarla hasta que el maintainer la etiquete. Superficie añadida: `crates/webfang_core/src/domain/options_spec/ai.rs`, `crates/webfang_core/src/application/crawl_options.rs`, `docs/src/cli-reference.md` (GENERADO — se regenera con `scripts/gen-cli-reference.sh`) y los dos snapshots del `--help` (`crates/webfang_core/tests/behavioral/snapshots/behavioral__help.snap`, `crates/webfang_core/tests/snapshots/cli_binary_test__test_help_contains_scraper.snap`).
