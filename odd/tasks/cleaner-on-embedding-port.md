# Feature: #1923 — SemanticCleanerImpl sobre `EmbeddingPort` + `--max-tokens` → `--max-chars`

**Issue:** #1923 — `refactor(ai): rewrite SemanticCleanerImpl over EmbeddingPort + rename --max-tokens to --max-chars`
**Labels:** `type:refactor` (`status:approved` pendiente del owner)
**Branch:** `feat/cleaner-on-embedding-port` (base `main` @ `e57b68f5`)
**Worktree:** `~/Projects/Rust/webfang-worktrees/feat-cleaner-on-embedding-port`
**ADR:** `docs/adr/0004-onnx-local-removal.md`, Tramo D paso 2

> **Nota de proceso (honesta).** Este doc se creó DESPUÉS de la implementación, no antes
> como en #1921. El owner pidió arrancar en paralelo al merge de #1922, y prioricé eso
> sobre el tracking. Queda registrado porque el desvío es real: sin este doc, el contexto
> del brief se perdía al primer relanzamiento del writer.

## Por qué Tasks 1 y 2 son un solo cambio

El writer lo demonstró, y es la razón por la que "mismo PR" no fue sólo convenience:
`clean()` deriva el guard `ChunkTooLarge` de `ModelInput::seq_len()` (`:478`, `:483`),
único uso del tokenizer en todo el archivo. Sacar el tokenizer destruye la base del guard,
así que Task 1 sola entregaría un cleaner **sin guard de budget** más un knob muerto
(`ModelConfig::max_tokens`, con un solo caller). No hay estado intermedio que compile y sea
honesto.

## Decisiones y por qué

| decisión | motivo |
| --- | --- |
| El cleaner depende de `Arc<dyn EmbeddingPort>` | `EmbeddingAdapter::embed` ya hace `tokenize → infer` detrás del port (`embedding_adapter.rs:151-153`); `infer` ≡ `embed` es relocalización, no capacidad nueva |
| `clean()` NO migra a `embed_batch` | El default del trait (`embedding_port.rs:63-76`) es un loop secuencial de `embed` y `EmbeddingAdapter` no lo overridea: batch **serializaría** la inferencia ONNX. Es regresión de performance, no mejora |
| `is_ready()` == `true` | `build_onnx_embedding_port` ya devuelve `Result`: un puerto que no se pudo construir es error de startup, no estado de readiness. Shift semántico documentado |
| `shared_inference()` eliminado | El cleaner deja de exponer API con forma de engine. El 3-tuplo de `build_onnx_embedding_port` cubre a CLI/MCP, que lo necesitan para vault ports y `GraniteDomInspector` |
| Guard en caracteres | ADR-0004 condición 4. El respaldo real del límite es el degradado por 400/413 con troceado, nunca la estimación |
| `MAX_CHARS` sin tope 32768 | Ese número era el Max Sequence Length de Granite codificado como política. Contra un endpoint remoto el límite real es el contexto del modelo |
| Shim que **advierte y mapea**, no que erroriza | El ADR tiene dos fases: release N anuncia, release N+1 quita. Errorizar en N salta la fase 1. Mi brief original decía "rechaza" — error mío, corregido |
| `DEFAULT_CHARS_PER_TOKEN` y `DEFAULT_MAX_CHARS` en core | Dos literales `3.0` / `98_304` en dos crates son la clase de drift que se vuelve un bug silencioso de budget. `webfang_ai` los lee; core no puede ver a `webfang_ai` |

## El seam del warning (no obvio)

El warning de deprecación **no se puede emitir** desde `build_ai_config`: argv se resuelve en
el paso 6b de `main.rs` y `init_logging_dual` instala el subscriber en 6b2, así que un
`tracing::warn!` ahí se descarta (seam documentado en `main.rs:158-162`, #796). Por eso la
procedencia viaja en `AiConfig::deprecated_max_tokens` y el aviso se emite una vez en
`main.rs`, después del subscriber.

## Precedencia del shim

`crates/webfang_core/src/cli/args/mod.rs:630`, tabla documentada en `:600-629`:

| `--max-chars` elegido | `--max-tokens` dado | resultado | aviso |
| --- | --- | --- | --- |
| sí | ignorado | `max_chars` literal | ninguno |
| no | sí | `tokens × DEFAULT_CHARS_PER_TOKEN` | una vez |
| no | no | default de `MAX_CHARS` | ninguno |

"Elegido" = `ArgMatches::value_source` es `CommandLine` o `EnvVariable`.

## Fuera de alcance (reportado, no editado)

- `crates/webfang_ai/src/infrastructure_ai/mod.rs:46` — prosa con `--max-tokens` (archivo de #1922)
- `crates/webfang_core/src/domain/error/error_class.rs:29` — doc comment que menciona tokens
- `docs/src/cli-reference.md:575-578` — referencia CLI generada, hay que regenerarla
- `webfang_benchmark` construye `CrawlOptions` desde `Args`: hereda la conversión y la
  procedencia pero no tiene warn site, así que convierte en silencio
- Ungatear `semantic_cleaner_impl` — vive en `mod.rs`, que es de #1922. Es el slice C

## Evidence

Verificación delegateada a `gentle-ai-verify`; target propio
(`~/.cache/cargo-target/feat-cleaner-on-embedding-port`).

| # | check | resultado |
| --- | --- | --- |
| 1 | `cargo check --workspace` | PASS |
| 2 | `cargo check -p webfang_ai --features ai` | PASS |
| 3 | `cargo nextest run -p webfang_ai --features ai` | PASS (233/233, 9 skipped) |
| 4 | `cargo nextest run --workspace --all-features` | PASS (4704/4704, 35 skipped) |
| 5 | clippy CI-exacto con ratchets #516 | PASS |
| 6 | `cargo fmt --all -- --check` | PASS (verificación, no fixer) |
| 7 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` | PASS |

RED del test nuevo (antes del rewrite):
`cargo nextest run -p webfang_ai --features ai --test embedding_port_cleaner_test` → exit 101,
`error[E0277]: the trait bound 'dyn EmbeddingPort: InferenceEngine' is not satisfied`.

Incidente del writer, recuperado y verificado por el padre: un
`cargo insta test --accept --unreferenced=delete` acotado a un target borró 104 snapshots
ajenos. Restaurados byte-idénticos. `git diff --name-status -- '*.snap'` confirma que sólo
quedan los 2 modificados (bloques de help) y los 3 `max_tokens_bound_test__*` realmente
obsoletos. Segunda vuelta: sin `--unreferenced=delete`, borrado por path explícito.

## Risk accepted

- El shim mapea con un factor 3.0 aproximado (chars por token varían por idioma y tokenizer).
  El aviso declara la aritmética para que el operador vea el budget que realmente obtiene.
- `MAX_CHARS` default = 98304 preserva el techo efectivo local (32768 × 3.0). Es un número
  derivado, no una constante documentada de ningún modelo; la derivación está en el spec.

## Commits

_(pendiente)_
