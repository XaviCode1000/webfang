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

S3. Los bytes **abandonados** que se preservan son los de la cola truncada,
    no el archivo entero: hoy `preserve_abandoned_bytes` recibe el buffer
    completo y lo que se preserva es la parte que se va a perder.

S4. El caso "el archivo no tiene NINGÚN `\n`" (una sola línea sin terminar)
    sigue.truncando todo el archivo ⇒ ese caso se resuelve con la misma
    lectura por ventana, y si el archivo entero cabe en la ventana se
    preserva completo; si NO cabe, el archivo tiene al menos un `\n` antes
    ⇒ hay que crecer la ventana.

S5. El comportamiento observable **no cambia**: los mismos bytes se
    truncan, el mismo backup se escribe, los mismos warnings salen. Sólo
    cambia de dónde se leen los bytes.

## Tasks

- [ ] T1 · S1, S2, S3, S4, S5 — `crates/webfang_core/src/infrastructure/export/jsonl_writer.rs`:
      la lectura por ventana + el crecimiento con tope explícito + la
      preservación de sólo la cola. **Prueba**: un test con un archivo
      grande whose tail is long (la línea truncada mide más que la ventana
      inicial ⇒ obliga a crecer) y otro con la cola corta.
- [ ] T2 · S1 — la **medición**: el pico de memoria del proceso antes y
      después, sobre el mismo archivo (el verificador independiente de #2230
      midió +74,7 MiB vs +71,9 MiB sobre 69.244.058 bytes, contra un control
      de `fs::read` de +69,2 MiB) ⇒ el número va en el Log, medido, no
      estimado.
- [ ] T3 · S1–S5 — verificación: los tests de exportación verdes, `cargo
      check`, `cargo clippy --all-targets --all-features -- -D warnings`,
      `cargo fmt --all -- --check`, `cargo doc` con `RUSTDOCFLAGS=-D warnings`.

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
