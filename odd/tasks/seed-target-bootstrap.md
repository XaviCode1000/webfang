# seed-target-bootstrap — Sembrar el `CARGO_TARGET_DIR` de un worktree por CoW

## Objective

Que un worktree nuevo reciba un `CARGO_TARGET_DIR` aislado ya poblado, mediante clon
CoW de un seed compatible, sin pagar el cold build completo (2 m 23 s medidos) y sin
introducir contaminación entre árboles.

## Problem

`#1267` obliga a que cada worktree tenga su propio target dir, y eso implica recompilar
las ~490 unidades del grafo —incluidos los 669 objetos C++ de BoringSSL— en cada
árbol nuevo. El aislamiento es no negociable; el costo no debería serlo.

Hay además un fallo de **corrección** ya medido y congelado en `scripts/test_seed_contamination.sh`:
sembrar un target cuyo artefacto es más nuevo que las fuentes del destino hace que
`cargo build` termine en 0 y enlace código de **otro** árbol. La causa es que Cargo decide
frescura por mtime, y el clon CoW preserva mtimes.

## Why ahora

- El guard de aislamiento de `#1678` ya está mergeado y es la invariante sobre la que
  este trabajo se apoya: `worktree ⇒ target aislado`, comparado por identidad.
- La auditoría de los 476 Giens(default) shared target эмпирическиilujo respondió lo que
  este diseño necesitaba saber: **7 recetas, 2 seed-relevantes** (`rustc 1.88` sin flags, y
  `rustc 1.88` + los dos `link-arg`). El fan-out de `seeds/<key>/` es acotado y un
  worktree busca una sola.
- El target histórico de `main` **no** sirve como seed: 46 worktrees muertos referenciados
  en sus fingerprints, y uno vivo compilando dentro.

## Scope

In scope:

- `SeedCompatibilityKey` semántico (no los hashes internos de Cargo) + `manifest.toml`.
- Publicación explícita de un seed: build de referencia, poda de units path-dependent,
  escritura del manifest.
- Bootstrap consumidor: decide `seeded` / `cold`, y **loguea la decisión con su razón**.
- Guard: `CARGO_TARGET_DIR` bajo la raíz de seeds → rechazo, fail-closed, con la misma
  forma que el guard de `#1678`.
- El test de integración invoca el bootstrap **de producción** (follow-up de `#1709`).

Out of scope:

- **GC de los 476 G.** El 84% son binarios de test y el filesystem tiene 657 G libres. Es
  un problema separado con su propia política.
- Automatizar la publicación. Publicar es deliberado y explícito (ver decisión D3).
- La caché cross-workspace nativa de Cargo. Es dirección futura, no esto.

## Decisiones

- **D1 — El seed vive FUERA del target de main**: `~/.cache/cargo-target/seeds/<key>/`.
  El target de main es laUME de basura que accumulates; un seed dentro sería daño colateral
  de cualquier limpieza futura.
- **D2 — La key es semántica, no los hashes de Cargo.** La auditoría encontró **461 hashes
  de `profile` distintos** en uso real porque ese hash varía por rol de unidad (build
  script = host, lib = target). Usarlos como contrato externo habría producido 461 "recetas"
  y ningún seed.
- **D3 — La publicación es explícita** (`seed_publish.sh`), no automática. Si cada worktree
  publicara al no encontrar seed, volveríamos el fan-out y abriríamos una carrera de
  escritura entre worktrees concurrentes. El bootstrap solo consume.
- **D4 — La key NO incluye** commit, branch, path del worktree ni identidad del workspace.
  Esos valores son lo que un seed debe poder compartir. Sí incluye `cargo_lock_sha256`
  (conservador por acuerdo) y el conjunto de features, que lo aporta el invocante.
- **D5 — `reflink=always`, nunca `=auto`.** Medido: `auto` en un FS sin CoW copia entero y
  sale con exit 0 (64 MB por 64 MB en tmpfs). `always` falla ruidosamente → cold build.
- **D6 — El seed nunca se usa como `CARGO_TARGET_DIR` de un build.** Dos defensas:物理
  readonly **y** el guard explícito, porque el agente corre como el mismo usuario y puede
  revertir permisos.

## Invariantes

1. Ningún agente comparte `target/` con otro agente.
2. Un seed es inmutable después de publicado.
3. `CARGO_TARGET_DIR` bajo `seeds/` → rechazo antes de Cargo, fail-closed.
4. `reflink=always` falla → cold build. El proyecto no depende funcionalmente del seed.
5. Cada operación registra `seeded` o `cold` **con su razón**.
6. El test de integración ejercita el bootstrap de producción, no una copia de su poda.

## Tasks

- [ ] T1 — `seed_compat_key.sh`: computar la key semántica + `manifest.toml`.
- [ ] T2 — `seed_publish.sh`: build de referencia + poda + manifest + publicar a `seeds/<key>`.
- [ ] T3 — `seed_target.sh`: consumidor. Decide seeded/cold, loguea la razón, fallback.
- [ ] T4 — Guard en `ci_fast_gate.sh`: `CARGO_TARGET_DIR` bajo seeds → exit 2.
- [ ] T5 — `test_seed_contamination.sh` pasa a invocar el bootstrap de producción.
- [ ] T6 — AGENTS.md § worktree bootstrap: documentar el flujo y las invariantes.

## Verificación

- T1: key estable entre invocaciones; cambia con cada campo; dos worktrees del mismo
  commit y distinto path **producen la misma key**.
- T2: el seed publicado no contiene ninguna unit path-dependent; el manifest coincide
  con lo que el bootstrap.calc.
- T3: `seeded` vs `cold` con su razón; `reflink=always` fallando → cold, exit 0.
- T4: los casos del guard de `#1678` siguen verdes; el nuevo caso symlink→seed → 2.
- T5: el test sigue GREEN y sigue siendo RED con la poda de producción mutada.
- T6: `scripts/ci_fast_gate.sh` GREEN.
