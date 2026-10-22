# Los crates sin mantenimiento salen del grafo: `serde_yaml` y `keyring` 3

Objetivo: el grafo de build carga dos cadenas de crates sin mantenimiento que hoy viven bajo
suppressions con fecha — `serde_yaml` (obsoleto) → `unsafe-libyaml` (RUSTSEC-2024-0449) y `keyring` 3
→ `secret-service` → `zbus` 4 (RUSTSEC-2026-0370/0371/0372). Este lote las saca del grafo **sin
cambiar el producto**: el frontmatter se sigue emitiendo igual y el almacén de credenciales sigue
siendo el MISMO (keyutils en Linux, keychain en macOS, Credential Manager en Windows). Rama:
`fix/unmaintained-crates`. Worktree:
`/home/xavi/Projects/Rust/webfang-worktrees/fix-unmaintained-crates`. Cierra: #1984 y #2031.

---

## Specs

**S1.** El ignore muerto de `RUSTSEC-2024-0436` se purga y se verifica que no vuelve (de #1984):

> El ignore muerto de `RUSTSEC-2024-0436` se purga y se verifica que no vuelve

> **Hallazgo adjunto, mismo job:** `deny.toml:124` vuelve a tener el ignore de `RUSTSEC-2024-0436`
> (paste) que #1422 dice haber purgado, y ahora es muerto --- `cargo deny` lo reporta como
> `warning[advisory-not-detected]: no crate matched advisory criteria`. O sea, el trabajo de #1422
> se revirtió parcialmente. Un ignore muerto es peligroso justamente porque silencia: si el crate
> vuelve al grafo, el ignore lo seguiría cubriendo sin que nadie lo note.

Estado verificado (ver Log): el hallazgo de la issue es **cierto** en `main@3a785d8e` — `paste` no
está en `Cargo.lock` ni en `fuzz/Cargo.lock` (lo único con ese nombre es `pastey`, otro crate, no
cubierto por ese advisory). El crate que lo arrastraba (`macro_rules_attribute 0.2.2`, vía
`tokenizers`, del crate `webfang_ai`) salió del grafo con el paso 6 del ADR-0004 (`e9b7217a`).
Purgar el ignore **no** pone el audit en rojo: no hay crate al que ese id pueda matchear.

Código que hay que cambiar:

- `deny.toml` — la entrada `{ id = "RUSTSEC-2024-0436", ... }` y su bloque de comentario (la nota
  del "RE-ADD PROTOCOL" que lo justifica como re-added queda desactualizada y se corrige).
- `scripts/check_advisory_policy.sh` — check nuevo: **ningún ignore puede quedar muerto**. Por cada
  id de la lista de `deny.toml`, tiene que existir un crate de ese advisory matcheado por el audit
  en alguno de los dos lockfiles (`Cargo.lock` y `fuzz/Cargo.lock`). La fuente de verdad de "qué
  matchea" es el propio `cargo audit`: se enumera lo que el advisory DB reporta contra cada lock
  (los ids salen de la URL `rustsec.org/advisories/RUSTSEC-…` que el auditor imprime) y se exige
  que todo id suprimido esté en ese conjunto. Sin el check, el ignore se vuelve a "silenciar" sin
  que nadie lo note — que es exactamente el defecto que la issue denuncia.
- `scripts/test_advisory_policy.sh` — caso nuevo (negación): un `deny.toml` de fixture con un id
  que el audit **no** matchea tiene que **fallar** el guard, y el mismo id matcheado tiene que
  pasar. Es el caso que ve RED contra el guard de hoy (que no mira la lista de ids contra el lock).

**S2.** El reemplazo de `serde_yaml` cubre los mismos casos de parseo que el camino actual (de
#1984):

> Si se migra de `serde_yaml`: el reemplazo cubre los mismos casos de parseo que el camino actual
> (config de providers y fixtures de test)

Corrección de premisa (ver Log): la parte "config de providers" no aplica — `serde_yaml` no
participa de la config de providers (esa es TOML). El camino real de parseo es **el frontmatter**:
`serde_yaml::to_string` en producción y `serde_yaml::from_str` **sólo en tests**, sobre el mismo
YAML que el propio repo emite.

Código que hay que cambiar:

- `Cargo.toml` (workspace, `[workspace.dependencies]`) — `serde_yaml = "0.9"` sale; entra el
  reemplazo.
- `crates/webfang_core/Cargo.toml` (`[dependencies]`) — `serde_yaml = { workspace = true }` sale.
- `crates/webfang_core/src/infrastructure/output/frontmatter.rs` — `generate_with_metadata` (el
  único call site de producción) y los pines de PI-7 (#1601) que parsean el frontmatter emitido
  para comprobar que un escalar hostil sigue siendo un escalar.
- `crates/webfang_core/src/error.rs` — la variante `ScraperError::Yaml` existe **sólo** porque
  `serde_yaml` estaba en el grafo: no hay ningún call site que la construya en todo el workspace
  (el emisor del frontmatter usa `unwrap_or_else`, no propaga el error). Se **borra** la variante
  y su brazo en la clasificación de `ErrorClass` (preferir borrar antes que dejar un enum público
  con una variante que nadie puede construir). Es un cambio rompiente de API pública ⇒ el commit
  lleva el marcador `BREAKING CHANGE:`.

El reemplazo debe cumplir tres cosas, y las tres se prueban sin red:

1. **Misma salida byte a byte.** El frontmatter que se escribe en el disco del usuario no cambia.
   Lo fijan los tests existentes de `frontmatter.rs` (p.ej. `test_generate_normalizes_excerpt_whitespace`
   exige `excerpt: '...'` con comilla simple) y el snapshot
   `crates/webfang_core/tests/snapshots/obsidian_byline_artifact_test__empty_byline_fragment_repaired_in_excerpt.snap`.
2. **Mismo poder de parseo.** Los pines de PI-7 (#1601) siguen parseando el frontmatter emitido y
   siguen exigiendo: un `\n` en el title no abre una key nueva, una comilla doble no escapa el
   escalar, un backslash no se come el carácter siguiente, `---` sigue siendo un escalar y no un
   marker de documento, y el title vacío sigue siendo un escalar vacío. Esos pines son la
   evidencia; si el reemplazo no puede parsear, no cumple.
3. **Cero advisories.** El crate que entra no está en la base de advisories de RustSec (ver Log),
   y el crate transitivo que trae tampoco: por eso la entrada `RUSTSEC-2024-0449` se puede borrar
   de `deny.toml` en vez de re-despistarse.

**S3.** `cargo deny check` termina en verde, sin el error de `unsafe-libyaml`, y el build sigue
verde (de #1984):

> `cargo deny check` termina en verde, sin el error de `unsafe-libyaml`

> `cargo check --workspace --all-targets --all-features` sigue verde

Estado verificado (ver Log): este criterio **ya está cumplido hoy** — `deny.toml` ya declara
`RUSTSEC-2024-0449` con motivo y `# Expires: 2027-03-20` (lo puso `217c3959`, PR #1986), así que el
job no está rojo por `unsafe-libyaml`; el rojo que la issue reporta era de antes de esa PR. Lo que
falta es la **salida**: que el crate salga del grafo y que la supresión con fecha pueda borrarse.

Código que hay que cambiar:

- `Cargo.lock` — regenerado: `serde_yaml` y `unsafe-libyaml` salen del grafo.
- `fuzz/Cargo.lock` — regenerado: hoy está **stale** (pinta `webfang_core 2.0.0` cuando el crate
  va por 5.0.0) y todavía arrastra `serde_yaml`/`unsafe-libyaml`. Mientras ese lock exista con el
  crate adentro, la supresión de `RUSTSEC-2024-0449` **sigue viva** y borrarla del `deny.toml`
  pone el audit del lock del fuzz en rojo. Por eso el refresco del lock del fuzz es parte de este
  lote, no un extra: es lo que permite borrar la supresión y cerrar la salida de verdad.
- `deny.toml` — se borra la entrada `RUSTSEC-2024-0449` (con su `# Expires:` y su bloque de
  comentario).

**S4.** La entrada de `deny.toml` lleva razón y `# Expires:` según `scripts/check_advisory_policy.sh`
(de #1984):

> Si se declara en `deny.toml`: la entrada lleva razón y `# Expires:` según
> `scripts/check_advisory_policy.sh`

Estado verificado (ver Log): la rama "se declara en `deny.toml`" del criterio **ya está satisfecha**
— la entrada existe con motivo, `Tracking:` y `# Expires: 2027-03-20`, y el guard la acepta. Este
lote no la relaja ni la renueva: la borra, porque el crate que justificaba la entrada sale del
grafo (S2, S3). El criterio queda cumplido por la vía de la salida, no por la de la declaración.

**S5.** `zbus`/`zvariant` desaparecen del lock y `cargo audit` queda verde (de #2031):

> Rama de prueba con cada salida: `cargo tree -p webfang_core --target all` confirma que
> zbus/zvariant desaparecen del lock, y `cargo audit` verde.

Estado verificado (ver Log): la premisa de la issue ("el backend Linux de `keyring` arrastra
`secret-service` → `zbus 4`") es **parcialmente falsa** — en el grafo *compilado* el backend Linux
es keyutils (`linux-native`); la arista a zbus existe **sólo en el lock**, porque `Cargo.lock`
incluye las dependencias opcionales de un paquete aunque ninguna feature las active. Eso es
justamente lo que la issue anticipa en su "trampa", y es lo que hace que un cambio de features no
alcanze: la única forma de sacar `zbus` del lock es que el paquete `keyring` (cualquier versión:
3.x y 4.x declaran `secret-service`/`zbus-secret-service-keyring-store` como opcionales) salga del
grafo.

Salida elegida: `keyring-core` 1 + los stores por plataforma, **el mismo store** de hoy:

| Plataforma | Hoy (`keyring` 3) | Después | Clave de la credencial |
|---|---|---|---|
| Linux | `linux-native` (keyutils) | `linux-keyutils-keyring-store` | `keyring-rs:{account}@{service}` (hoy) — la nueva store usa `keyring:{service}@{account}` por defecto, así que hay que pasar el modificador `description` para **no dejar huérfanas las credenciales ya guardadas** |
| macOS | `apple-native` (keychain) | `apple-native-keyring-store` con la feature `keychain` | atributo `service` = service, `account` = account (idéntico) |
| Windows | `windows-native` (Credential Manager) | `windows-native-keyring-store` | `target_name` = `{account}.{service}` (idéntico) |

Código que hay que cambiar:

- `crates/webfang_core/Cargo.toml` — las tres tablas `[target.'cfg(target_os = …)'.dependencies]`
  de `keyring` salen; entran `keyring-core` (dependencia normal) y los tres stores (target-specific).
- Módulo nuevo `crates/webfang_core/src/domain/keyring_store.rs` — **el único lugar** que decide
  qué store se usa en cada plataforma y con qué clave se direcciona la credencial. Sin flags, sin
  estados nuevos: una función que devuelve la `Entry` ya construida. Se registra en
  `crates/webfang_core/src/domain/mod.rs`.
- `crates/webfang_core/src/domain/auth_source.rs` — `resolve_keyring` deja de usar
  `keyring::Entry::new` y usa el módulo anterior. El contrato no cambia: sin fallback, `Ok(None)`
  sólo para `AuthSource::None`, y los errores siguen siendo `AuthError::KeyringUnavailable`,
  `AuthError::CredentialNotFound` y `AuthError::KeyringFailure`.
- `crates/webfang_core/src/domain/local_secret_store.rs` — `detect_keyring` y
  `probe_reports_operational_backend` (y sus tests in-module) se adaptan al tipo de error del
  crate nuevo. `probe_reports_operational_backend` sigue siendo una función **pura** con la misma
  semántica: sólo `Ok` y `NoEntry` cuentan como backend operativo.

**S6.** MSRV y `libdbus-1-dev` verificados antes de elegir (de #2031):

> MSRV y `libdbus-1-dev` verificados antes de elegir.

Verificado (ver Log): la salida elegida **no usa `libdbus`** — el store `dbus-secret-service`
queda descartado, así que no hay `libdbus-1-dev` que instalar en runners ni en máquinas de
usuarios. El MSRV de los crates elegidos es ≤ 1.88 (el MSRV del workspace): `keyring-core` 1.0.0,
`linux-keyutils-keyring-store` 1.0.0, `apple-native-keyring-store` 1.0.0 y
`windows-native-keyring-store` 1.0.0 declaran `rust-version` 1.85 o menos. La salida #2 de la issue
(`keyring` 4 con Secret Service) queda descartada por dos motivos, ambos registrados en el Log:
mueve las credenciales del usuario a otro almacén (y el contrato de no-fallback deja al usuario
sin salida) y arrastra el zbus 6, cuyo MSRV (1.89, según D10 del plan madre) es superior al del
workspace (1.88).

**S7.** Las tres entradas de `deny.toml` de #2030 se retiran (de #2031):

> Elegida la salida, se retiran las tres entradas de `deny.toml` de #2030.

Código que hay que cambiar: `deny.toml` — se borran las tres entradas `RUSTSEC-2026-0370`,
`RUSTSEC-2026-0371` y `RUSTSEC-2026-0372` (con sus `# Expires: 2027-01-17` y el bloque de
comentario que las agrupa). Con `keyring` fuera del grafo, `zbus`, `zbus_names`, `zvariant` y
`zvariant_derive` desaparecen del lock ⇒ las tres supresiones quedan muertas ⇒ se van con ellas.
El check de S1 es el que lo hace verificable y no reintroducible.

**S8.** El lock es lo que lee `cargo audit`, no el árbol compilado (de #2031):

> Trampa a verificar en cualquiera de las dos: el feature `crypto-rust` de keyring solo aporta
> aristas *weak* (`secret-service?/crypto-rust`), así que el árbol puede seguir en `Cargo.lock`
> aunque no se compile. Como `cargo audit` lee el **lock**, un cambio de feature puede no ponernos
> en verde. Hay que probarlo en una rama aparte, sin bloquear #2004.

De ahí sale la restricción que gobierna S5: cambiar features no alcanza, hay que sacar el paquete
`keyring` del grafo. Y de ahí sale la evidencia que hay que dejar escrita: el criterio de
aceptación es sobre el **lock**, así que la prueba es `cargo tree -p webfang_core --target all` +
`cargo audit` verde sobre el lock regenerado, no la ausencia de zbus en el árbol compilado.

---

## Tasks

- [ ] T1 · S2, S3, S4 — `Cargo.toml` (workspace) + `crates/webfang_core/Cargo.toml`: `serde_yaml`
  sale, el reemplazo entra. `crates/webfang_core/src/infrastructure/output/frontmatter.rs`:
  `generate_with_metadata` y los pines de PI-7 (#1601) pasan a la API del reemplazo.
  `crates/webfang_core/src/error.rs`: se borra `ScraperError::Yaml` y su brazo en `ErrorClass`
  (commit con marcador `BREAKING CHANGE:`). Se prueba con los tests existentes de
  `frontmatter.rs` (incluidos `test_generate_normalizes_excerpt_whitespace` y
  `test_title_with_newline_cannot_inject_yaml_keys`) y con el snapshot
  `obsidian_byline_artifact_test__empty_byline_fragment_repaired_in_excerpt.snap` **sin cambios**.
- [ ] T2 · S2, S3, S4 — `deny.toml`: se borra la entrada `RUSTSEC-2024-0449` y su bloque de
  comentario. Se prueba con `bash scripts/check_advisory_policy.sh` en verde y con el harness
  `scripts/test_advisory_policy.sh` (el caso de S1).
- [ ] T3 · S3 — `Cargo.lock` y `fuzz/Cargo.lock` regenerados (el del fuzz hoy está stale y es lo
  que mantiene viva la supresión de `RUSTSEC-2024-0449`). Se prueba con
  `cargo tree -p webfang_core --target all` (no aparece `unsafe-libyaml`) y con el audit del lock
  del fuzz dentro de `scripts/check_advisory_policy.sh`.
- [ ] T4 · S5, S6, S8 — `crates/webfang_core/Cargo.toml`: `keyring` sale de las tres tablas
  target-specific; entran `keyring-core` y los tres stores. Módulo nuevo
  `crates/webfang_core/src/domain/keyring_store.rs` (el único lugar que decide store y clave),
  registrado en `crates/webfang_core/src/domain/mod.rs`.
  `crates/webfang_core/src/domain/auth_source.rs::resolve_keyring` y
  `crates/webfang_core/src/domain/local_secret_store.rs::detect_keyring` /
  `probe_reports_operational_backend` se adaptan al crate nuevo. Se prueba con el test in-module
  nuevo `linux_keyutils_description_matches_the_legacy_keyring_3_layout` (la clave de la
  credencial no cambia: `keyring-rs:{account}@{service}`), con los tests existentes
  `probe_ok_or_noentry_means_backend_available` y
  `probe_any_other_error_means_backend_unavailable` (adaptados al tipo de error nuevo, con la
  misma semántica) y con `no_implicit_fallback_from_keyring_to_env`.
- [ ] T5 · S7 — `deny.toml`: se borran las tres entradas `RUSTSEC-2026-0370/0371/0372` y su bloque
  de comentario. Se prueba con `bash scripts/check_advisory_policy.sh` en verde y con el audit
  sobre el lock regenerado.
- [ ] T6 · S1 — `deny.toml`: se purga la entrada `RUSTSEC-2024-0436` (con su comentario, que ya
  no describe la realidad) y se corrige la nota del "RE-ADD PROTOCOL".
  `scripts/check_advisory_policy.sh`: check nuevo "ningún ignore muerto" (todo id de la lista
  tiene que matchear en alguno de los dos lockfiles según el propio `cargo audit`).
  `scripts/test_advisory_policy.sh`: caso nuevo de negación (un id que el audit no matchea tiene
  que fallar el guard; matcheado, pasa). El caso se ve **RED** contra el guard de hoy.
- [ ] T7 · S1–S8 — verificación del lote: `bash scripts/test_advisory_policy.sh` en verde con el
  caso nuevo incluido; `bash scripts/check_advisory_policy.sh` en verde; los tests de
  `webfang_core` que tocan frontmatter y keyring en verde; `cargo check --workspace --all-targets
  --all-features` en verde; `cargo clippy --all-targets --all-features -- -D warnings` en verde;
  `cargo fmt --all -- --check` en verde.
- [ ] T8 · S1–S8 — `scripts/quality-baselines.json`: medir el delta de duplicación con el detector
  del repo **después** del cambio y corregir el baseline **sólo hacia abajo** si la medición lo
  exige (la nota del archivo tiene que llevar la cifra medida). Si la medición no cambia, el
  archivo no se toca. Ojo con la colisión: `scripts/quality-baselines.json` lo tocan también L12 y
  el PR #2230 (orden en el plan: #2230 → L12), así que este lote sólo hace la corrección
  descendente y no toca nada más del archivo.

---

## Log

L1 — Petición original del owner (verbatim):

> tengo muchas y  varias issues abiertas y necesito un plan para ir implementando utliza la
> skill adecuada para triage de issues

L2 — Correcciones del owner (verbatim):

> debes tener en cuenta que nuesto sistema ha cambiado sobre todo en lo relacionado con
> modelos de *AI LOCAL* Y LAS ISSUES pueden decir cosas o indormacion antigua o falsa,

> utiliza siempre tu y ordenar a  todos los workers las potentes tools *codedb* y *codegraph*
> en lugar de las otras tools

> ya tengo un agente trabajando en lote k1 en
> /home/xavi/Projects/Rust/webfang-worktrees/fix-ssrf-this-network-justification

> no abras pr's conserva el trabajo en worktree comiteado. ya mas tarde abrimos las pr's.

> te dejo la responsabilidad de decision a ti como experto, y si tienes que investigar en
> internet para buscar la mejor opcion o mejor alternativa tienes mi permiso total.

> pero tus decisiones nunca debe ser la mas facil, debe ser la mas adecuada a nuestro
> producto.

L3 — Decisión del owner que habilita este lote (verbatim, O2 del plan madre, resuelta 2026-10-22):

> Los tres: sysinfo, keyring, serde_yaml

L4 — Lectura del plan madre (`odd/tasks/backlog-implementation-plan.md`, leído entero hasta `## Log`).
La fila **L10** dice, textual: "borrar `serde_yaml` (sólo serializa en prod) y salir de `keyring` 3
preservando el **mismo store** (linux-keyutils); las 3 supresiones zbus expiran 2027-01-17", con
test nombrado `scripts/test_advisory_policy.sh` (caso nuevo) y Δ `−30/+45`. El criterio del plan se
respeta: una fila por lote, un fix por raíz. La fila del plan, en cambio, subestima el Δ: este lote
mueve el grafo de dos dependencias, cuatro archivos de código, dos lockfiles, `deny.toml` y dos
scripts.

L5 — **La decisión D12 del plan madre tiene una premisa falsa, y este lote la corrige.** D12 dice:
"#1984 → borrar `serde_yaml`, **no** \"purgar ignores muertos\" | `cargo audit` lee el lock y **sí**
detecta `paste 1.0.15`: purgar ese ignore deixa el Security Audit en rojo." Verificado en
`main@3a785d8e`: **`paste` no está en el lock** (ni en `Cargo.lock` ni en `fuzz/Cargo.lock`; lo
único con ese nombre es `pastey`, crate distinto y no cubierto por RUSTSEC-2024-0436). El crate que
lo arrastraba (`macro_rules_attribute 0.2.2`, vía `tokenizers`) salió con el paso 6 del ADR-0004:
`git log -S 'name = "paste"' -- Cargo.lock` →
`e9b7217a feat!(ai): retirar la feature ai y el crate webfang_ai (ADR-0004 paso 6) (#2072)`. O sea:
el ignore de `paste` está muerto (que es lo que dice la issue #1984) y purgarlo **no** pone el audit
en rojo. La advertencia de D12 sí aplica —pero a otro id: `RUSTSEC-2024-0449` sigue matcheando
mientras `fuzz/Cargo.lock` (stale) arrastre `unsafe-libyaml`. Por eso este lote refresca el lock
del fuzz antes de borrar esa entrada.

L6 — Premisas de las issues verificadas contra el código (CodeDB 0.2.5849 + CodeGraph sobre
`main@3a785d8e`, worktree idéntico):

- **#1984, "el gate está rojo en `main`"**: FALSA hoy. `deny.toml` ya declara `RUSTSEC-2024-0449`
  con motivo, `Tracking:` y `# Expires: 2027-03-20`; lo puso `217c3959` (PR #1986). La rama "se
  declara en `deny.toml`" de la issue ya está satisfecha. Lo que falta es la salida.
- **#1984, "config de providers"**: FALSA. `serde_yaml` no toca la config de providers (es TOML).
  Sus usos reales: `crates/webfang_core/src/infrastructure/output/frontmatter.rs` (1 call site de
  producción: `serde_yaml::to_string` en `generate_with_metadata`; 3 de test:
  `serde_yaml::from_str::<serde_yaml::Value>` — una dentro del helper `title_of` y dos en los tests
  de contrato) y `crates/webfang_core/src/error.rs` (`ScraperError::Yaml`).
- **#1984, "el ignore muerto de `RUSTSEC-2024-0436`"**: CIERTA (ver L5).
- **#2031, "el backend Linux de `keyring` arrastra `secret-service` → `zbus 4`"**: PARCIALMENTE
  FALSA. En el grafo compilado el backend Linux es keyutils: `crates/webfang_core/Cargo.toml`
  declara `keyring = { version = "3", default-features = false, features = ["linux-native",
  "crypto-rust"] }` y `deny.toml` lo documenta ("nothing activates `dep:secret-service` or
  `dep:zbus`"). La arista a zbus existe **sólo en el lock** (`Cargo.lock` lista las deps opcionales
  de `keyring` 3.6.3: `dbus-secret-service`, `linux-keyutils`, `secret-service`), y por eso
  `cargo audit` la ve. La issue lo intuye en su "trampa"; el lote lo confirma y lo cumple (S8).
- **#2031, "`keyring` 4 … exige Rust 1.88"**: el MSRV real de `keyring` 4.2.0 es `rust-version =
  1.88.0` (igual al del workspace), o sea que la preocupación de MSRV no era el bloqueo; el
  bloqueo real es otro: `keyring` 4 sigue declarando `zbus-secret-service-keyring-store` como
  dependencia opcional ⇒ `zbus` sigue entrando al lock ⇒ no hay salida por ese lado.

L7 — Superficie de código tocada (CodeDB `callers` + CodeGraph `explore`):

- `resolve_keyring` (`crates/webfang_core/src/domain/auth_source.rs`): **1** call site
  (`AuthSource::resolve`, el match de `AuthSource::Keyring`). El test
  `no_implicit_fallback_from_keyring_to_env` y el de parseo de `auth = { source = "keyring" }`
  viven en el mismo archivo.
- `detect_keyring` (`crates/webfang_core/src/domain/local_secret_store.rs`): **1** call site
  (`DetectedStore::detect`).
- `probe_reports_operational_backend`: **8** call sites (1 en producción dentro de `detect_keyring`
  + 7 en los dos tests in-module `probe_ok_or_noentry_means_backend_available` y
  `probe_any_other_error_means_backend_unavailable`).
- `keyring::` aparece en **sólo dos archivos** de todo el workspace
  (`domain/auth_source.rs` y `domain/local_secret_store.rs`); `serde_yaml::` en **uno**
  (`infrastructure/output/frontmatter.rs`) más la variante de error en `error.rs`.
- Consumidores del frontmatter emitido (CodeGraph `explore generate_with_metadata`):
  `crates/webfang_core/src/infrastructure/output/file_saver.rs` (el save de la CLI),
  `crates/webfang_mcp/src/mcp_server/handlers/content.rs` (`generate_rich_metadata`) y
  `crates/webfang_mcp/src/mcp_server/server.rs`. Los tres escriben el mismo frontmatter ⇒ el
  cambio de crate tiene que ser **byte-idéntico** en los tres.

L8 — Hechos de crate verificados contra el registro de crates y contra la base de advisories de
RustSec (`rustsec/advisory-db`, vía la API de GitHub):

- `keyring` 4.2.0: `rust-version = 1.88.0`; deps opcionales `linux-keyutils-keyring-store`,
  `apple-native-keyring-store`, `windows-native-keyring-store`, `dbus-secret-service-keyring-store`,
  `zbus-secret-service-keyring-store`, `android-native-keyring-store`, `db-keystore` ⇒ **cualquier**
  versión de `keyring` arrastra zbus al lock. `keyring-core` 1.0.0 no tiene ninguna.
- `linux-keyutils-keyring-store` 1.0.0: deps `keyring-core ^1` + `linux-keyutils ^0.2` (el mismo
  crate y la misma versión 0.2.5 que ya está en el lock ⇒ el keyring del kernel y sus keyrings
  sesión/persistentes son los mismos).
- La clave de la credencial **cambia** si se usa el store nuevo tal cual: `keyring` 3.6.3
  (`src/keyutils.rs`) arma la description como `keyring-rs:{user}@{service}`; el store nuevo usa
  `prefix + service + divider + user + suffix` con `prefix = "keyring:"` y `divider = "@"` ⇒
  `keyring:{service}@{user}`. Distinto prefijo y distinto orden: las credenciales ya guardadas
  quedarían **infindables**, y con el contrato de no-fallback eso no deja salida. Por eso S5 exige
  el modificador `description` (que el store acepta: `Store::build` lee el modificador
  `description`) con el valor legacy `keyring-rs:{account}@{service}`.
- macOS y Windows: el mapeo **no** cambia. `keyring` 3.6.3 en macOS usa
  `set_generic_password(service, account)` y el store nuevo (feature `keychain`) busca por
  atributo `service` = service y `account` = account; en Windows `keyring` 3.6.3 usa el
  `target_name` `username.service` y el store nuevo genera `{user}.{service}` con prefijo y sufijo
  vacíos y divisor `.` ⇒ idéntico.
- `serde_yaml_ng` 0.10.0 (la otra fork alternativa) **no sirve**: depende de `unsafe-libyaml`
  0.2.11, o sea que la entrada `RUSTSEC-2024-0449` seguiría matcheando. `serde_norway` 0.9.42
  depende de `unsafe-libyaml-norway ^0.2.13` (la fork mantenida del C) y ni `serde_norway` ni
  `unsafe-libyaml-norway` están en la base de advisories de RustSec (comprobado crate por crate
  contra `rustsec/advisory-db`). `serde_yaml` sí tiene `RUSTSEC-2018-0005` anotado, y
  `unsafe-libyaml` tiene `RUSTSEC-2023-0075` y `RUSTSEC-2024-0449`.
- Alternativa descartada y por qué: **borrar el crate de YAML y escribir el frontmatter a mano**
  (el emitter propio, ~50 líneas). Se descartó porque (a) crea una segunda representación de la
  verdad sobre el formato de salida — el plan lo prohíbe explícitamente —, (b) obliga a convertir
  los pines de PI-7 (#1601), que hoy son parse-based, en pines textuales, con menos evidencia, y
  (c) mete un emitter de YAML a mano en el camino de un escalar hostil (`title`, `author`).
  La vía elegida mantiene **un** emitter (el de la fork, mismo linaje de código) y por eso el
  frontmatter sigue saliendo byte a byte igual.
- API del crate nuevo (docs.rs, `keyring-core` 1.0.0): `set_default_store` / `get_default_store` /
  `unset_default_store`, `Entry::new`, `Entry::new_with_modifiers(service, user, &HashMap<&str,
  &str>)`, `CredentialStoreApi::build(&self, service, user, modifiers) -> Result<Entry>` y
  `Error` (`#[non_exhaustive]`) con las variantes `NoEntry`, `BadEncoding`, `BadStoreFormat`,
  `TooLong(String, u32)`, `Invalid`, `Ambiguous`, `NoDefaultStore`, `NotSupportedByStore`,
  `PlatformFailure` y `NoStorageAccess` (estas dos últimas llevan un `PlatformError`
  platform-dependiente ⇒ los tests del probe tienen que usar las variantes construibles sin
  salirse de la semántica: "cualquier error que no sea `Ok` ni `NoEntry` ⇒ backend no disponible").
  El store se puede usar sin tocar el global: `Store::build(...)` sobre el `Arc` del store, que es
  lo que el módulo nuevo va a hacer (no se introduce un estado global nuevo).

L9 — Hallazgos colaterales que este lote NO arregla (quedan anotados para el owner, no se mezclan
en el diff):

1. `fuzz/Cargo.lock` está stale: pinta `webfang_core 2.0.0` cuando el crate va por 5.0.0, y su
   subgrafo tiene 391 paquetes contra los 668 del lock del workspace. Este lote lo refresca porque
   hace falta para la salida de `RUSTSEC-2024-0449` (S3), pero el desfase en sí es un defecto
   aparte.
2. `deny.toml` documenta que `unused-ignored-advisory` queda en `"warn"` **a propósito**, porque
   `cargo deny` reporta "not-detected" para cuatro ids que `cargo audit` sí matchea
   (RUSTSEC-2026-0183/0184/0221/0253). Por eso el check de S1 se apoya en `cargo audit` y no en
   `cargo deny`: promover ese knob a `"deny"` rompería el control positivo. Un detector general de
   "ignores muertos" basado en `cargo deny` no es viable; el de S1 (basado en `cargo audit`) sí.
3. `deny.toml` conserva la nota de que las tres supresiones de zbus expiraban el 2027-01-17: este
   lote las anticipa.

L10 — Riesgo asumido y su mitigación: la fork de YAML elegida (`serde_norway`) no tiene release
desde 2024-12. La mitigación es que **ningún** crate de esa cadena tiene advisory en RustSec (L8) y
que el frontmatter sigue siendo byte-idéntico (lo fijan tests y snapshot existentes, T1). Si en
la implementación la salida no fuese byte-idéntica, el lote **no** debe aceptar el cambio de
salida: habría que volver a la decisión de emitter.

L11 — **S1: el ignore muerto, purgado, y un guard que no deja que vuelva.** El
hecho: `RUSTSEC-2024-0436` (paste) no matchea nada — el crate que lo arrastraba
(`macro_rules_attribute 0.2.2`, via `tokenizers`) salio del grafo con el paso 6
del ADR-0004, y `paste` ya no esta en ningun lock. Un ignore muerto es
peligroso justamente porque SILENCIA: si el crate vuelve al grafo, el ignore lo
seguiria cubriendo sin que nadie lo note. Se purga de `deny.toml`, y la nota
del "RE-ADD PROTOCOL" que lo justificaba como re-added queda corregida (dice
que vuelve a estar muerto, y por que).

El guard nuevo vive en `scripts/check_no_dead_ignores.sh` y lo llama el guard
principal como check 6a. **Desviacion de la spec, y por que**: la spec dice
que el check va dentro de `scripts/check_advisory_policy.sh`, y ese archivo ya
esta en 701 lineas — por encima del limite de ~500 por archivo del repo, y
anadirle mas lo empeoraria. Ademas, el check recibe la lista de ids YA
DERIVADA (la variable `IDS` del guard principal), asi que no hay un SEGUNDO
parser de `deny.toml` que mantener sincronizado: la fuente de verdad de los
ids sigue siendo una sola.

La regla: cada id de la lista de ignores tiene que matchear un advisory que
el auditor REALmente reporte en alguno de los dos lockfiles. La fuente de
verdad de "que matchea" es el propio auditor: el informe JSON de
`cargo audit` (que corre SIN la lista de ignores, porque si no suprimiria
exactamente lo que hay que ver) y los ids salen de los objetos `advisory.id`.
El codigo de salida NO es la respuesta (una vulnerabilidad real tambien sale
con codigo distinto de cero, y eso es asunto del check 5): lo que decide es
si el INFORME se puede parsear. Un informe que no se parsea significa que el
auditor no corrio, y eso es un FALLO, nunca un conjunto vacio de matcheos —
un check que no ve no puede decir "todo bien".

El flag `--skip-dead-ignores` existe por el mismo motivo que
`--skip-duplicates`: los otros casos del harness usan un `cargo` falso que no
produce un informe de auditoria, asi que lo saltan de forma explicita. El
default NO lo salta: el camino de CI lo corre entero.

**RED y GREEN**: el caso nuevo del harness ("un ignore que no matchea nada
tiene que fallar el guard, nombrandolo") falla contra el guard SIN el check
(12 pasan, 1 falla ⇒ el test discrimina) y pasa con el (13/13). El caso
positivo accompanying ("un ignore que SI matchea pasa") esta ahi para que el
check no pueda quedarse verde por no mirar nada.

L12 — **El harness con un `cargo` falso no puede detectar que mi suposición
era incorrecta: el oraculo es la corrida real.** El primer guard REAL (con
el auditor de verdad) reporto los CATORCE ignores como muertos —imposible:
casi todos matchean algo del grafo. La causa no era el grafo: mi jq pedia
`warnings.list`, y la forma REAL del informe de `cargo audit --json` es
`warnings.unmaintained[]` y `warnings.unsound[]` (mas `vulnerabilities.list`).
Un jq que busca donde no hay nada no ve ningun matcheo y dice "todo el
grafo esta limpio" — la direccion fail-closed, pero con el motivo
equivocado.

Y el harness NO lo agarro, porque el `cargo` falso lo escribi yo, con MI
suposicion: la fixture coincidia con la suposicion por construccion, o sea
una prueba tautologica. Las dos correcciones: el jq lee la forma real (que
saque del informe que imprime el auditor de verdad) y la fixture del harness
usa esa misma forma real. La distincion que queda escrita: un doble con
forma SUPPUESTA prueba la logica del check; solo la corrida real prueba la
suposicion. El guard real, con la forma real, da exit 0: 14 ignores
justificados, ninguno muerto (el de paste, purgado en L11, ya no esta).
