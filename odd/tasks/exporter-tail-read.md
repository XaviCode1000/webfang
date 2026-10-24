# El export lee sólo la cola que necesita (issue #2237)

`recover_torn_tail` lee el archivo de export ENTERO con `fs::read`, así que el
pico de memoria del proceso sigue creciendo con el tamaño del trabajo
acumulado. El índice de resume ya es streaming (#1812); esta es la otra mitad.

## Specs

S1. La recuperación de la cola truncada lee **sólo la cola**: el archivo se
    abre y se lee una **ventana** del final, no entero. El pico de memoria
    deja de crecer con el tamaño del archivo.

S2. La ventana tiene que cubrir el peor caso: una línea truncada puede ser
    tan larga como el registro más grande escrito ⇒ si la ventana no alcanza
    al último `\n`, la ventana **crece** (hacia atrás) hasta alcanzarlo, con
    un tope explícito y un mensaje de error honesto si se supera. Un tope
    ciego que trunque en silencio sería un verde falso.

    Tope: 8 MiB (`MAX_TAIL_WINDOW_BYTES`). Una cola truncada más larga que
    el tope hace **FALLAR la apertura del export** con un error que nombra la
    causa ("no newline boundary in the last N bytes of a M-byte file, and the
    8388608-byte window cap was reached") y el archivo NO se toca (ni `.bak`
    ni truncado): cortar a ciegas sería pérdida de datos y un verde falso.

S3. Los bytes **abandonados** que se preservan son los de la cola truncada,
    no el archivo entero: hoy `preserve_abandoned_bytes` recibe el buffer
    completo y lo que se preserva es la parte que se va a perder.

S4. El caso "el archivo no tiene NINGÚN `\n`" (una sola línea sin terminar)
    sigue.truncando todo el archivo ⇒ ese caso se resuelve con la misma
    lectura por ventana, y si el archivo entero cabe en la ventana se
    preserva completo; si NO cabe, el archivo tiene al menos un `\n` antes
    ⇒ hay que crecer la ventana.

S5. El comportamiento observable **no cambia**: los mismos bytes se
    truncan, los warnings salen y el `.bak` se escribe en el mismo sitio.
    Sólo cambian DOS cosas, y las dos a la vez: de dónde se leen los bytes
    (una ventana al final, no el archivo entero) y QUÉ lleva el `.bak` — la
    cola **abandonada** (la que la truncación destruye), no el archivo
    entero. El prefijo que sobrevive sigue en el archivo vivo ⇒ prefijo +
    `.bak` reconstruyen el archivo anterior a la truncación.

## Tasks

- [x] T1 · S1, S2, S3, S4, S5 — `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs`:
      la lectura por ventana + el crecimiento con tope explícito + la
      preservación de sólo la cola. **Prueba**: un test con un archivo
      grande whose tail is long (la línea truncada mide más que la ventana
      inicial ⇒ obliga a crecer) y otro con la cola corta.
- [x] T2 · S1 — la **medición**: el pico de memoria del proceso antes y
      después, sobre el mismo archivo (el verificador independiente de #2230
      midió +74,7 MiB vs +71,9 MiB sobre 69.244.058 bytes, contra un control
      de `fs::read` de +69,2 MiB) ⇒ el número va en el Log, medido, no
      estimado.
- [x] T3 · S1–S5 — verificación: los tests de exportación verdes, `cargo
      check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all -- --check`, `cargo doc` con `RUSTDOCFLAGS=-D warnings`.
- [x] T4 · S5 + alcance — **cierre de la unidad**: la partición del fichero
      grande (`tail_window.rs`), el refresco del golden set del
      intra-crate-lint y la corrección de S5 (la decisión y su evidencia
      están en L6).

## Log

L1 — Petición del owner (verbatim, del issue #2237): "`recover_torn_tail` lee
el archivo de export entero, así que el pico de memoria sigue creciendo con su
tamaño". Y: "Que `recover_torn_tail` lea **sólo la cola** que necesita (los
últimos bytes del archivo, o línea a línea desde el final) en vez del archivo
entero. Es la misma técnica que ya se aplicó al índice."

L2 — Riesgo declarado por la issue, verbatim: "**Riesgo: ítem 1 (memoria) —
leer de arodear el final del archivo.** La ventana de lectura tiene que cubrir
el peor caso: una línea truncada puede ser tan larga como el registro más
grande escrito." ⇒ de ahí salen S2 (el crecimiento con tope explícito) y S3
(preservar sólo la cola).

L3 — Mapa medido (por el padre): `recover_torn_tail` está en
`crates/webfang_core/src/infrastructure/export/jsonl_writer.rs` y su único
llamador está en el mismo fichero. Hoy hace `let data = fs::read(path)?` y
busca el último `\n` con `rposition`; las dos ramas llaman a
`preserve_abandoned_bytes(path, &data, ...)` pasando el buffer COMPLETO,
aunque lo que se preserva es la cola que se va a perder.

L4 — Decisión del padre: el cambio es de implementación interna, no de
comportamiento observable ⇒ S5 lo fija: los mismos bytes se truncan y el
mismo backup se escribe. La PR es `Closes #2237`, label `type:bug` (el
prefijo del título tiene que derivar esa label: `fix(...)`).

L5 — Implementación (T1, T2, T3). `recover_torn_tail` ya no hace
`fs::read(path)`: lee una VENTANA del final del archivo y, si la ventana no
alcanza al último `\n`, la CRECE hacia atrás hasta alcanzarlo.

    - Tamaño inicial de la ventana: 64 KiB (`TAIL_WINDOW_BYTES`).
    - Crecimiento: se DOBLA hacia atrás (64 KiB → 128 KiB → 256 KiB → …)
      hasta encontrar un `\n` o hasta cubrir el archivo entero.
    - Tope explícito: 8 MiB (`MAX_TAIL_WINDOW_BYTES`). Alcanzado el tope sin
      `\n`, la recuperación FALLA con un error que nombra la causa ("no
      newline boundary in the last N bytes of a M-byte file, and the
      8388608-byte window cap was reached") y el archivo NO se toca (ni
      backup ni truncado): un corte a ciegas sería un verde falso.
    - La rama "el archivo no tiene NINGÚN `\n`" se resuelve con la misma
      lectura por ventana: si el archivo entero cabe en la ventana se
      preserva completo y se borra; si NO cabe, la ventana tiene que crecer
      (y si supera el tope, el error honesto de arriba).
    - `preserve_abandoned_bytes` recibe ahora los bytes ABANDONADOS (la cola
      que se va a perder), NO el archivo entero (S3). El prefijo que
      sobrevive sigue en el archivo vivo, así que prefijo + `.bak`
      reconstruyen el archivo pre-truncación.

    CONSECUENCIA OBSERVABLE (S3 contra S5 — para el padre): el `.bak` de una
    cola truncada pasa de contener el archivo entero a contener SÓLO la cola
    abandonada. No cambian: los bytes que se truncan, el caminho del `.bak`,
    los `stage` (`jsonl_torn_tail_truncated`, `jsonl_unterminated_file_wiped`)
    ni los warnings (`truncated_bytes`, `total_bytes`, que ahora es el tamaño
    del archivo). El test unitario `torn_tail_recovery_backs_up_the_pre_
    truncation_bytes` se renombró a `torn_tail_recovery_backs_up_the_
    abandoned_bytes` y ahora afirma la cola abandonada.

    T2 — MEDICIÓN (medida, no estimada). Método: un test ignorado por
    defecto (`tail_read_memory_probe`, en el módulo de tests de
    `jsonl_writer.rs`) re-ejecuta el binario de tests como PROCESO HIJO, una
    vez por estrategia, y mide el pico de RSS del hijo con
    `/proc/self/status` (`VmRSS` antes de la operación, `VmHWM` después):
    el crecimiento del pico = `VmHWM` después − `VmRSS` antes. Artefacto: un
    `export.jsonl` de 69.244.058 bytes (el tamaño de referencia de #2237)
    con cola truncada; cada estrategia corre sobre una COPIA del mismo
    archivo.

        estrategia   pico (KiB)   − idle (KiB)
        idle            1.476          0      (el piso del harness)
        io_only         1.860        +384      (abrir y leer 1 byte)
        read_only       2.752      +1.276      (la ventana sola: 64 KiB)
        control        68.848     +67.372      (fs::read del archivo ENTERO = la implementación VIEJA)
        window          3.972       +2.496      (la recuperación nueva, completa)

    ⇒ el pico de la lectura de la cola pasó de +67.372 KiB (65,8 MiB ≈ el
    archivo entero) a +2.496 KiB (2,4 MiB) para la recuperación completa, de
    los cuales +1.276 KiB son la lectura por ventana (la ventana de 64 KiB) y
    el resto el piso del proceso. El número nuevo NO crece con el tamaño del
    export: queda acotado por el tope de 8 MiB.
    Repetibilidad (tres corridas del mismo código): control − idle = 67.372 /
    67.436 / 67.636 KiB; window − idle = 2.496 / 2.192 / 2.760 KiB. El número
    de control coincide con el tamaño del archivo (69.244.058 B = 66,0 MiB):
    la medición es la del `fs::read` de la implementación vieja, no una
    estimación.

    T3 — VERIFICACIÓN (todas verdes, en el worktree
    `fix-exporter-tail-read`, `CARGO_TARGET_DIR=$HOME/.cache/cargo-target/
    fix-exporter-tail-read`):

        cargo nextest run -p webfang_core --lib --test export_data_integrity_test
          --test batch_export_exit_code_test --test jsonl_concurrency_test
          --test adversarial_stress_matrix --test export_non_utf8_policy_test
          --test export_torn_tail_recovery_test
          ⇒ Summary: 2937 tests run: 2937 passed, 5 skipped
        cargo check --workspace --all-features          ⇒ Finished, sin avisos
        cargo clippy --all-targets --all-features -- -D warnings
          -W clippy::cognitive_complexity -W clippy::too_many_lines
                                                        ⇒ Finished, sin avisos
        cargo fmt --all -- --check                      ⇒ limpio
        env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features
          --no-deps                                    ⇒ Finished, sin avisos

    Cobertura nueva: `crates/webfang_core/tests/export_torn_tail_recovery_test.rs`
    (API pública) con los cuatro casos — cola corta, cola MÁS LARGA que la
    ventana inicial (obliga a crecerla), archivo sin ningún `\n`, y cola más
    larga que el tope (error honesto, sin destruir nada) — más tres tests
    unitarios nuevos en `jsonl_writer.rs` (cola larga, sin `\n`, tope) y el
    test del backup adaptado a S3.

    NOTA DE ALCANCE: `jsonl_writer.rs` ya excedía los ~500 líneas antes de este
    cambio (630) y quedó en 1.017 con la implementación, los tests y la sonda
    de memoria. Partirlo exigiría un archivo nuevo en `src/` (fuera de las
    superficies permitidas de esta tarea): queda a criterio del padre.

L6 — Decisión del padre (cierre de la unidad) y evidencia.

    - SOBRE EL `.bak` (corrección de S5). El padre verificó que NADIE lee los
      `.bak` que produce `preserve_abandoned_bytes`: el helper escribe un
      sibling `.bak` forense, su doc dice "abandoned state bytes", y el
      prefijo que sobrevive sigue en el fichero vivo ⇒ prefijo + `.bak`
      reconstruyen el fichero pre-truncación. ⇒ S3 (preservar los bytes
      ABANDONADOS, no el fichero entero) es la correcta y S5 ("el mismo
      backup se escribe") estaba redactada con sloppy ⇒ S5 reescrita para
      decir la verdad: el `.bak` contiene la cola abandonada, el prefijo
      sobrevive en el fichero vivo, y la combinación reconstructa el fichero
      anterior a la truncación. L5 no se reescribe: la decisión vive aquí.

    - SOBRE EL TOPE DE 8 MiB. Una cola truncada más larga que el tope hace
      FALLAR la apertura del export con un error que nombra la causa, en vez
      de truncar a ciegas ⇒ es el comportamiento pedido por S2 (un corte a
      ciegas sería pérdida de datos y un verde falso). El fichero NO se toca:
      ni `.bak` ni truncado. Confirmado en S2.

    - SOBRE LA PARTICIÓN DEL FICHERO (la NOTA DE ALCANCE de L5 queda
      respondida por esta entrada: el padre autorizó el fichero nuevo, así
      que `jsonl_writer.rs` se partió). El fichero nuevo
      `crates/webfang_core/src/infrastructure/export/tail_window.rs` lleva la
      VENTANA de cola (`TAIL_WINDOW_BYTES`, `MAX_TAIL_WINDOW_BYTES`,
      `read_tail_window`, `read_range`), la RECUPERACIÓN que la consume
      (`recover_torn_tail`: el truncado, el `.bak` y los warnings) y su
      EVIDENCIA (los tests unitarios de la cola y la sonda de memoria de T2).
      `jsonl_writer.rs` conserva la sesión de escritura (el canal, el bucle
      del escritor, el índice de hash, el GC de `.tmp`) y sus tests. La
      partición NO cambia el comportamiento: mismos bytes truncados, mismo
      `.bak` con la cola abandonada, mismos warnings, mismo error honesto al
      superar el tope.

    - GOLDEN SET. `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt`
      es un snapshot CON números de línea del árbol real ⇒ este slice cambia
      `crates/webfang_core/src`, así que el fixture va refrescado en el MISMO
      commit que describe el árbol (no es un PR aparte).

    - T2 TRAS LA PARTICIÓN (la sonda sigue siendo la misma): la sonda de
      memoria se movió de fichero con el resto de la evidencia de la cola y
      se re-ejecutó para comprobar que sigue midiendo lo mismo:
      `cargo test -p webfang_core --lib -- --ignored --nocapture
      tail_read_memory_probe` ⇒ `control_minus_idle_kib=67640` (L5: 67.372 /
      67.436 / 67.636) y `window_minus_idle_kib=2184` (L5: 2.496 / 2.192 /
      2.760): los dos números caen dentro de la banda de repetibilidad que
      L5 ya registra ⇒ la partición no movió la medición.
