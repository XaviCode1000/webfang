# Campaña de remediación fail-open — Ola 3, flip del gate intra-crate (S4 + S5)

**El último slice de la Ola 3** (#2130). El extractor ya existe (#2165, `tools/intra-crate-lint/`);
este slice convierte el gate en su consumidor y **borra** el escáner awk que hoy decide.

Umbrella: #2127 · Ola 3: #2130 · Predecesores: #2158 (gate inter-crate), #2159 (higiene del
harness), #2164 (la constante SSRF al dominio), #2165 (el extractor con `syn`).

## Specs

- **S1** — "the marker is a crude grep of `#[cfg(test)]|mod tests`: a **DOC-COMMENT** that
  mentions the attribute truncates the scan." (B36, audit)
  El gate ya no escanea con grep: consume el extractor, y la región de test se evalúa
  estructuralmente sobre el predicado `cfg`, con el predicado de `cfg` como regla y no la forma
  del texto.
- **S2** — "a **STRING LITERAL** with `/*` opens a phantom block comment and blinds the rest of the
  file: a false **negative**, contrary to what the script's own header claims." (B37, audit)
  Un string con `/*` no puede cegar nada: el extractor parsea, no busca.
- **S3** — "the regex requires `crate::<layer>::<segment>`, so it does not see
  `use crate::infra::{A,B}`, `use crate::infra;`, `::*`, `as infra`, `crate::{infra::Foo}` nor the
  spaces around `::`." (B38, audit)
  Las seis formas de path se emiten; el filtro de capas sigue siendo **política del shell**.
- **S4** — "the choke point requires the literal text `ValidatingResolver::.new(`, with a LITERAL
  DOT between `::` and `new` that real Rust never emits: the rule cannot fire." (B39, audit)
  El choke se decide sobre las referencias que el extractor emite, no sobre el texto de una línea.
- **S5** — "the checker never verifies a pattern matched ≥1 site; a stale entry only emits
  `::warning::` and exits 0." (B44, Ola 5, PR #2174)
  La regla que la Ola 5 implanta en el gate con awk **se arrastra** al gate nuevo: una entry del
  allowlist que matchea cero sitios es un fallo en modo strict. No se implementa dos veces: se
  implementa una, aquí.
- **S6** — "the gate must not have a fallback: a gate that degrades to the weak scanner when the
  strong one is unavailable is the same fail-open being closed." (decisión de la Ola 3)
  Si el extractor no está, no compila, o falla al parsear un fichero, el gate **falla cerrado** con
  el fichero nombrado. El extractor awk se **borra**: no queda como camino alternativo.

## El contrato del extractor (ya implementado en #2165)

```
intra-crate-lint --root <dir>
  stdout: <relative-file>\t<line>\t<kind>\t<path>   kind ∈ {use, inline, choke}
  exit 0  = árbol escaneado por completo
  exit 1  = uso incorrecto
  exit 2  = un fichero no se pudo leer/parsear (cada fallo nombrado en stderr)
```

El tool es un **extractor**, no el dueño de la política: el conjunto de capas, el allowlist y los
criterios de veredicto siguen en el shell. El shell consume el TSV y decide.

## Tasks

| ID | Specs | Ruta | PR | Commit |
|---|---|---|---|---|
| T1 | S1–S4, S6 | `scripts/check_intra_crate_direction.sh` (reescrito: el awk desaparece, el gate invoca al extractor y aplica la misma política) | PR-S4 | `0ac8ebf5` + `ab6b418a` (junto con T2) |
| T2 | S5 | el min-match del allowlist, arrastrado al gate nuevo | PR-S4 | `0ac8ebf5` |
| T3 | S1–S6 | `scripts/test_intra_crate_direction.sh` (harness): tool ausente ⇒ fail-closed · parse error ⇒ fail-closed · sin fallback al awk · las seis formas de path · la región de test estructural · el choke · la entry caduca | PR-S4 | `6b1523bc` |
| T4 | S1–S6 | Verificación: la salida del gate sobre el árbol real es **byte-idéntica** a la del gate awk (comparación en vivo, ver L7) y el canon de la clase queda 11/0 | — | ver L7 |

T1 y T2 viajan en el MISMO commit: la regla de B44 vive en el fichero que T1
reescribe, y separarla dejaría `scripts/test_intra_crate_allowlist.sh` en rojo
en el commit intermedio.

## Log

- **L1** (2026-10-21, petición del mantenedor): "tengo un agente mergeando las pr's abiertas continuamos con las siguientes olas?"
- **L2** (decisión de la Ola 3, registrada en el issue #2130): el gate inter-crate lee el grafo real
  (`cargo metadata`) y el intra-crate se escanea con un parser de verdad (`syn`). El shell conserva
  la política; el tool extrae.
- **L3** (coordinación con la Ola 5): **B44 se implementa una sola vez**. La Ola 5 lo implanta en el
  gate con extractor awk (#2174) y este slice lo arrastra al gate nuevo. Implementarlo en los dos
  sitios sobre `scripts/check_intra_crate_direction.sh` creaba un conflicto de merge seguro.
- **L4** (orden de aterrizaje): este slice arranca **después** de que #2165 (el extractor) y #2174
  (B44) estén en `main`, y su rama se crea desde ese `main` — no desde las ramas de esos PRs.
  Motivo: un PR cuyo diff contenga el extractor o la regla de B44 duplicaría esas PRs y abriría un
  conflicto con cada una.
- **L5** (medición de referencia, 2026-10-21): el extractor sobre el árbol real da **1630 sitios**
  (1016 `use`, 599 `inline`, 15 `choke`) y **0 errores de parseo**. Con la misma política, el gate
  awk da 0 infracciones y el extractor da **1**: `domain/ssrf_guard.rs:135` (domain rank 3
  importando application rank 2 — outward). Esa infracción ya no está: #2164 movió la constante al
  dominio. El allowlist count coincide (44 = 44), corroboración de que el conjunto AST reproduce
  los sitios compartidos con el awk.
- **L6** (aceptación de T4): el gate nuevo debe producir **la misma salida** que el gate awk sobre
  el árbol real. La diferencia medida (un sitio) es exactamente la infracción que #2164 eliminó, así
  que tras ese merge la equivalencia es total y la comparación byte a byte es la prueba.
- **L7** (2026-10-21, este slice — aceptación de T4 medida): las DOS implementaciones sobre el
  MISMO árbol, en el mismo momento, producen la misma salida: `cmp` sin diferencias, 175 bytes, 2
  filas (`allowlisted 44 (max 5, …, entries: 2)` + `OK: intra-crate Clean Architecture layering is
  inward-only (ADR-0010, strict mode)`), 0 infracciones en las dos. La cifra de «12 filas» que este
  documento arrastraba desde su redacción **no se reproduce en esta base**: el criterio de
  aceptación es la identidad byte a byte entre las dos implementaciones, medida en vivo, no un
  número memorizado. La comparación se repitió después de cada cambio del gate.
- **L8** (2026-10-21, este slice — T2): la regla de B44 **se arrastra** al gate nuevo tal cual (mismos
  contadores, mismo mensaje, mismo orden de salida). No se reimplementa ni se cambia: el extractor
  reporta los mismos sitios, así que la regla decide sobre los mismos sitios. El harness de B44
  (`scripts/test_intra_crate_allowlist.sh`, de #2174) sigue verde sin tocarlo.
- **L9** (2026-10-21, este slice — T3): el harness NO fija ningún valor medido contra el árbol real:
  todos sus fixtures viven en un árbol mktemp y sólo afirman DECISIONES. Un contador de sitios del
  árbol real es un temporizador (el golden set del extractor, `real_tree_sites.txt`, depende del
  árbol y lo refresca quien mergea; este slice no lo toca). La prueba de equivalencia de T4 es la
  comparación en vivo de las dos implementaciones, no un número guardado en el harness.
- **L10** (2026-10-21, este slice — resolución del binario): `$INTRA_CRATE_LINT_BIN` →
  `tools/intra-crate-lint/target/release/intra-crate-lint` →
  `tools/intra-crate-lint/target/debug/intra-crate-lint` → `intra-crate-lint` en el PATH. Los dos
  candidatos del repositorio se resuelven contra la raíz del repo (el directorio del propio
  script), así que el gate se comporta igual desde cualquier directorio de trabajo — condición que
  necesita el repro del audit, que lo invoca desde el fixture. Un `$INTRA_CRATE_LINT_BIN` explícito
  se usa EN EXCLUSIVIDAD: si no es un fichero ejecutable el gate falla cerrado nombrándolo, porque
  una petición que no se puede cumplir no puede convertirse en otro binario en silencio. Si no hay
  ningún candidato, el gate falla cerrado nombrando la herramienta: no hay escáner de reserva.
- **L11** (2026-10-21, este slice — `ff548aef`): con `CARGO_TARGET_DIR` fijado (direnv fija un
  target dir por worktree), un `cargo build --manifest-path tools/intra-crate-lint/Cargo.toml` a
  secas deja el binario en el target dir compartido, donde el gate no lo busca. El mensaje de
  fallo cerrado lo dice y ofrece las dos salidas (`env -u CARGO_TARGET_DIR` o
  `$INTRA_CRATE_LINT_BIN`).
