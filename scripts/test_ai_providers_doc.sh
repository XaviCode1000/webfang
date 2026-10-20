#!/usr/bin/env bash
# ============================================================================
# HARNESS — semántica del guard de drift doc↔código (ai-providers-design.md)
# ============================================================================
# Un guard que dejó de reconocer drift es indistinguible de no tener guard
# (mismo argumento que justifica el harness de los pines de versión). Cada
# check tiene al menos un caso que prueba su fallo CERRADO, sobre fixtures en
# mktemp -d: sin red, sin cargo, sin tocar el repo.
#
# El archivo tiene DOS secciones:
#
#   1. SEMÁNTICA — un caso por check del guard, sobre fixtures en mktemp -d.
#   2. WIRING   — la EDGE: que el guard esté invocado desde ci.yml y desde el
#      fast gate, y que su fase advisory esté declarada.
#
# La sección de wiring vive AQUÍ y no en un `test_ai_providers_doc_wiring.sh`
# aparte, a propósito: el meta-guard (scripts/test_guard_coverage.sh) sólo
# conoce `scripts/test_ai_providers_doc.sh`, así que un archivo de wiring
# separado quedaría sin cobertura propia — exactamente la forma #1707 que
# este repo combate.
#
# Uso: scripts/test_ai_providers_doc.sh [ci-yml] [fast-gate]
#   Los paths opcionales apuntan las aserciones de wiring a revisiones
#   mutadas; sin argumentos se usan las copias del repo.
# Salida: 0 si todos los casos pasan · 1 si alguno falla
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check_ai_providers_doc.sh"
CI_YML="${1:-$REPO_ROOT/.github/workflows/ci.yml}"
FAST_GATE="${2:-$REPO_ROOT/scripts/ci_fast_gate.sh}"
GUARD_REL="scripts/check_ai_providers_doc.sh"
HARNESS_REL="scripts/test_ai_providers_doc.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# ─── fixture base ────────────────────────────────────────────────────────────
# Un doc mínimo con las secciones numeradas que C7 necesita, dos módulos de
# código y un manifiesto embebido que cubre todas las citas de la prosa.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/docs/architecture" "$dir/crates/webfang_core/src/domain"

  cat > "$dir/crates/webfang_core/src/domain/sample.rs" <<'RS'
//! Módulo de ejemplo.
//!
pub struct Thing {
    pub id: String,
}

impl Thing {
    pub fn resolve(&self) -> Result<u32, String> {
        Ok(1)
    }
}
RS

  cat > "$dir/crates/webfang_core/src/domain/kind.rs" <<'RS'
pub enum Kind {
    Alpha,
}
RS

  cat > "$dir/docs/architecture/ai-providers-design.md" <<'MD'
# Ejemplo

## Conceptos

### 1. `Thing` — un tipo

```rust
pub struct Thing {
    pub id: String,
}
```

- **(a)** Un detalle del contrato.

Implementado en `crates/webfang_core/src/domain/sample.rs:3`.

### 2. `resolve` — resolución

```rust
impl Thing {
    pub fn resolve(&self) -> Result<u32, String> {
        Ok(1)
    }
}
```

Verificado en `crates/webfang_core/src/domain/sample.rs:8` y `kind.rs:1`.
El campo vive en `sample.rs:4`. Ver §1 y §1a.

<!-- claims:begin -->
```claims
shape  | crates/webfang_core/src/domain/sample.rs | 3 | pub struct Thing { | id
impl   | crates/webfang_core/src/domain/sample.rs | 8 | pub fn resolve(&self) -> Result<u32, String> {
claim  | crates/webfang_core/src/domain/sample.rs | 4 | pub id: String,
shape  | crates/webfang_core/src/domain/kind.rs | 1 | pub enum Kind { | Alpha
absent | * | - | Ghost
```
<!-- claims:end -->
MD

  printf '%s\n' "$dir"
}

# Fixture de COLISIÓN de la caché de archivos normalizados: dos rutas
# distintas que la traducción `tr '/.' '__'` de la clave antigua llevaba al
# MISMO nombre (`domain/x.a.rs` y `domain/x/a.rs`).
#
# El contenido de `x.a.rs` NO contiene la línea "42" del fence de `beta`; el
# de `x/a.rs` sí. Con la clave colisionante, el containment de C4 de la
# tercera entrada (que vuelve a citar `x.a.rs`) se satisface con el contenido
# del OTRO archivo: un falso verde.
new_fixture_collision() {
  local dir="$WORK/$1"
  mkdir -p "$dir/docs/architecture" "$dir/crates/webfang_core/src/domain/x"

  cat > "$dir/crates/webfang_core/src/domain/x.a.rs" <<'RS'
pub fn alpha(&self) -> u32 {
    7
}

pub fn beta(&self) -> u32 {
    9
}
RS

  cat > "$dir/crates/webfang_core/src/domain/x/a.rs" <<'RS'
pub fn alpha(&self) -> u32 {
    7
}

pub fn beta(&self) -> u32 {
    42
}
RS

  cat > "$dir/docs/architecture/ai-providers-design.md" <<'MD'
# Ejemplo

## Conceptos

### 1. `alpha` — resolución

```rust
pub fn alpha(&self) -> u32 {
    7
}
```

Implementado en `crates/webfang_core/src/domain/x.a.rs:1` y `crates/webfang_core/src/domain/x/a.rs:1`.

### 2. `beta` — resolución

```rust
pub fn beta(&self) -> u32 {
    42
}
```

Verificado en `crates/webfang_core/src/domain/x.a.rs:5`.

<!-- claims:begin -->
```claims
impl   | crates/webfang_core/src/domain/x.a.rs | 1 | pub fn alpha(&self) -> u32 {
impl   | crates/webfang_core/src/domain/x/a.rs | 1 | pub fn alpha(&self) -> u32 {
impl   | crates/webfang_core/src/domain/x.a.rs | 5 | pub fn beta(&self) -> u32 {
```
<!-- claims:end -->
MD

  printf '%s\n' "$dir"
}

# run_guard <dir> [args...]: exit code en $?, stdout+stderr en $WORK/out
run_guard() {
  local dir="$1"
  shift
  bash "$GUARD" --root "$dir" "$@" >"$WORK/out" 2>&1
}

DOC_OF() { printf '%s/docs/architecture/ai-providers-design.md\n' "$1"; }

# expect_exit <expected> <label> <dir> [args...]
expect_exit() {
  local expected="$1" label="$2" dir="$3"
  shift 3
  run_guard "$dir" "$@"
  local got=$?
  if [[ $got -eq $expected ]]; then
    pass "$label"
  else
    fail "$label (exit $got, esperado $expected) — $(head -c 400 "$WORK/out")"
  fi
}

# ─── 1. árbol limpio pasa ───────────────────────────────────────────────────
# NEGATION: pre-fix no existía ningún check; el doc normativo era prosa que
# nadie leía.
D="$(new_fixture clean)"
expect_exit 0 "árbol limpio pasa" "$D"

# ─── 2. claim con subcadena incorrecta falla ────────────────────────────────
# NEGATION: pre-fix una subcadena que no aparece en el código no era drift.
D="$(new_fixture claim-substring)"
sed -i 's/| pub id: String,/| pub id: StringX,/' "$(DOC_OF "$D")"
expect_exit 1 "claim con subcadena incorrecta falla" "$D"
if grep -q 'sample.rs' "$WORK/out"; then
  pass "el hallazgo de claim nombra el archivo"
else
  fail "el hallazgo de claim nombra el archivo"
fi

# ─── 3. claim con línea desfasada falla ─────────────────────────────────────
# NEGATION: pre-fix una cita desplazada un par de líneas no era drift.
D="$(new_fixture claim-line)"
sed -i 's#| crates/webfang_core/src/domain/sample.rs | 4 |#| crates/webfang_core/src/domain/sample.rs | 5 |#' "$(DOC_OF "$D")"
expect_exit 1 "claim con línea desfasada falla" "$D"

# ─── 4. shape con miembro faltante falla ────────────────────────────────────
# NEGATION: pre-fix el doc podía declarar un campo que el código no tiene.
D="$(new_fixture shape-missing)"
sed -i 's#| pub enum Kind { | Alpha#| pub enum Kind { | Alpha,Gamma#' "$(DOC_OF "$D")"
expect_exit 1 "shape con miembro faltante falla" "$D"

# ─── 5. shape con miembro de más falla ──────────────────────────────────────
# La dirección inversa: el código tiene un miembro que el doc no declara.
D="$(new_fixture shape-extra)"
cat > "$D/crates/webfang_core/src/domain/kind.rs" <<'RS'
pub enum Kind {
    Alpha,
    Beta,
}
RS
expect_exit 1 "shape con miembro de más falla" "$D"

# ─── 6. impl con firma que no existe en el código falla ─────────────────────
D="$(new_fixture impl-signature)"
sed -i 's/Result<u32, String>/Result<u32, Error>/' "$D/crates/webfang_core/src/domain/sample.rs"
expect_exit 1 "impl con firma ausente del código falla" "$D"

# ─── 6b. impl con línea del fence que no existe en el código falla ──────────
# La declaración sigue matcheando; lo que no aparece es el cuerpo del fence.
D="$(new_fixture impl-body)"
sed -i 's/        Ok(1)/        Ok(2)/' "$(DOC_OF "$D")"
expect_exit 1 "impl con línea de fence ausente del código falla" "$D"

# ─── 7. absent con símbolo presente falla ───────────────────────────────────
D="$(new_fixture absent-present)"
cat > "$D/crates/webfang_core/src/domain/kind.rs" <<'RS'
pub enum Kind {
    Alpha,
}

pub struct Ghost;
RS
expect_exit 1 "absent con símbolo presente falla" "$D"

# ─── 8. cita de prosa sin registrar falla ───────────────────────────────────
# NEGATION: pre-fix se podía agregar una afirmación normativa nueva sin
# registrar ningún chequeo — la puerta que este guard cierra.
D="$(new_fixture uncovered-citation)"
# El backtick es literal: es la notación de cita abreviada que el guard parsea.
# shellcheck disable=SC2016
printf 'Otra cita sin registrar: `kind.rs:2`.\n' >> "$(DOC_OF "$D")"
expect_exit 1 "cita de prosa sin registrar falla" "$D"

# ─── 9. referencia § rota falla ─────────────────────────────────────────────
D="$(new_fixture broken-section)"
printf '\nVer §9.\n' >> "$(DOC_OF "$D")"
expect_exit 1 "referencia § rota en el doc falla" "$D"

# ─── 10. manifiesto ausente falla cerrado ───────────────────────────────────
# NEGATION: pre-fix el doc sin manifiesto era indistinguible de un doc sin
# claims: el guard degradaba a "skip" en vez de fallar.
D="$(new_fixture no-manifest)"
sed -i '/<!-- claims:begin -->/,/<!-- claims:end -->/d' "$(DOC_OF "$D")"
expect_exit 1 "manifiesto ausente falla cerrado" "$D"

# ─── 11. --advisory sobre árbol con drift: exit 0 + hallazgos impresos ──────
D="$(new_fixture advisory)"
sed -i 's/| pub id: String,/| pub id: StringX,/' "$(DOC_OF "$D")"
expect_exit 0 "--advisory sale con 0 sobre un árbol con drift" "$D" --advisory
if grep -q '::error' "$WORK/out"; then
  pass "--advisory imprime los mismos hallazgos"
else
  fail "--advisory imprime los mismos hallazgos"
fi

# ─── 12. referencia § rota en el código que cita el doc falla ───────────────
D="$(new_fixture broken-section-code)"
cat > "$D/crates/webfang_core/src/domain/citing.rs" <<'RS'
//! Especificación: `docs/architecture/ai-providers-design.md` §9.
RS
expect_exit 1 "referencia § rota en el código que cita el doc falla" "$D"

# ─── 13. basename ambiguo falla (nunca adivinar) ────────────────────────────
D="$(new_fixture ambiguous-basename)"
mkdir -p "$D/crates/webfang_core/src/otro"
printf 'pub enum Dup {\n    A,\n}\n' > "$D/crates/webfang_core/src/domain/dup.rs"
printf 'pub enum Dup {\n    B,\n}\n' > "$D/crates/webfang_core/src/otro/dup.rs"
sed -i 's#^absent | \* | - | Ghost#absent | * | - | Ghost\nclaim  | dup.rs | 1 | pub enum Dup {#' "$(DOC_OF "$D")"
expect_exit 1 "basename ambiguo falla en vez de adivinar" "$D"

# ─── 14. línea del manifiesto fuera del archivo falla ───────────────────────
D="$(new_fixture line-out-of-range)"
sed -i 's#| crates/webfang_core/src/domain/kind.rs | 1 |#| crates/webfang_core/src/domain/kind.rs | 900 |#' "$(DOC_OF "$D")"
expect_exit 1 "línea del manifiesto fuera del archivo falla" "$D"

# ─── 15. el desborde de línea lo reporta C1, no otro check ──────────────────
# NEGATION: pre-fix el control de RANGO de línea (C1b) no estaba pineado: al
# neuterarlo el harness quedaba 100% verde, porque otro check (C3) reportaba
# el mismo drift. C1b podía desaparecer del guard y nadie lo notaría.
D="$(new_fixture line-out-of-range-owner)"
sed -i 's#| crates/webfang_core/src/domain/kind.rs | 1 |#| crates/webfang_core/src/domain/kind.rs | 900 |#' "$(DOC_OF "$D")"
expect_exit 1 "línea fuera del archivo: el guard falla" "$D"
# El exit code NO alcanza para aislar C1b: con el rango sin comprobar, la
# entrada sobrevive al parseo del manifiesto y C3 reporta la línea vacía. Lo
# que aísla el check es QUIÉN lo reporta: sólo C1 dice "fuera del archivo".
if grep -q 'C1: la línea citada está fuera del archivo' "$WORK/out"; then
  pass "el desborde de línea lo reporta C1 (rango), no C2/C3/C4"
else
  fail "el desborde de línea lo reporta C1 (rango), no C2/C3/C4 — $(head -c 400 "$WORK/out")"
fi

# ─── 16. la línea citada del impl no contiene la declaración (C4a) ─────────
# NEGATION: pre-fix la validación "la línea citada contiene la declaración"
# no estaba pineada: al neuterarla el harness quedaba verde, porque el otro
# check de C4 (el fence contenido en el archivo) reportaba el mismo caso.
D="$(new_fixture impl-line-misses-decl)"
# La entrada `impl` apunta a una línea que NO contiene la declaración …
sed -i 's#| crates/webfang_core/src/domain/sample.rs | 8 |#| crates/webfang_core/src/domain/sample.rs | 3 |#' "$(DOC_OF "$D")"
# … y la prosa deja de citar esa línea: si siguiera citándola, C6 taparía el
# aislamiento (el hallazgo vendría de la cobertura, no de C4a).
# El backtick es literal: es la notación de cita que el guard parsea.
# shellcheck disable=SC2016
sed -i 's#`crates/webfang_core/src/domain/sample.rs:8`#`crates/webfang_core/src/domain/sample.rs:3`#' "$(DOC_OF "$D")"
expect_exit 1 "impl: la línea citada no contiene la declaración → falla" "$D"
if grep -q 'C4: la línea no contiene la declaración' "$WORK/out"; then
  pass "el desajuste de la línea del impl lo reporta C4, no C2/C3/C6"
else
  fail "el desajuste de la línea del impl lo reporta C4, no C2/C3/C6 — $(head -c 400 "$WORK/out")"
fi

# ─── 17. una cita que sale de la raíz NO se lee (F1) ────────────────────────
# NEGATION: pre-fix `resolve_path` probaba `[[ -f "$ROOT/$p" ]]` con la ruta
# tal cual, sin normalizar ni rechazar los `..`: una cita `../fuera.txt`
# resolvía a un archivo real un nivel por encima de la raíz y el guard
# imprimía SU CONTENIDO en el log de CI (diagnóstico C2). El caso falla si el
# secreto vuelve a la salida: el exit code solo no alcanza, porque el
# contenido filtrado y el drift ausente comparten la misma forma.
D="$(new_fixture escape-root)"
# El archivo está un nivel POR ENCIMA de la raíz del fixture.
printf 'SECRETO_FUERA_DE_LA_RAIZ\n' > "$WORK/fuera.txt"
sed -i 's#^absent | \* | - | Ghost$#absent | * | - | Ghost\nclaim  | ../fuera.txt | 1 | NO_ESTA_EN_EL_ARCHIVO#' "$(DOC_OF "$D")"
expect_exit 1 "una cita que sale de la raíz del repo produce drift" "$D"
if grep -q 'SECRETO_FUERA_DE_LA_RAIZ' "$WORK/out"; then
  fail "el contenido de un archivo fuera de la raíz NO aparece en la salida"
else
  pass "el contenido de un archivo fuera de la raíz NO aparece en la salida"
fi
if grep -q 'sale de la raíz del repo' "$WORK/out"; then
  pass "la cita que escapa de la raíz se reporta como escape"
else
  fail "la cita que escapa de la raíz se reporta como escape — $(head -c 400 "$WORK/out")"
fi

# ─── 18. la caché de archivos normalizados no colisiona (F2) ───────────────
# NEGATION: pre-fix la clave de la caché era la traducción de la ruta con
# `tr '/.' '__'`, que NO es inyectiva: `domain/x.a.rs` y `domain/x/a.rs` dan
# el mismo nombre de archivo. La segunda escritura pisaba la primera y el
# containment de C4 se satisfacía con el contenido del OTRO archivo — un
# falso verde, la peor dirección posible. El caso falla si el containment se
# satisface con el contenido ajeno.
D="$(new_fixture_collision cache-collision)"
expect_exit 1 "la colisión de la caché no produce un falso verde" "$D"
if grep -q 'C4: la línea del fence' "$WORK/out"; then
  pass "el containment de C4 se lee contra el contenido del archivo citado"
else
  fail "el containment de C4 se lee contra el contenido del archivo citado — $(head -c 400 "$WORK/out")"
fi

# ─── 19. --advisory cubre el doc normativo ausente (F3) ────────────────────
# NEGATION: pre-fix `--advisory` sólo cubría la rama FINAL del recorrido: el
# doc ausente salía con 1 ANTES de llegar a ella. Un llamador que confiaba
# en `--advisory` para no fallar abortaba, y la tolerancia del llamador
# (`|| true`) sostenía el contrato en vez del script.
D="$(new_fixture advisory-no-doc)"
rm -f "$(DOC_OF "$D")"
expect_exit 1 "doc ausente: exit 1 en modo normal" "$D"
expect_exit 0 "doc ausente: exit 0 en modo --advisory" "$D" --advisory
if grep -q '::error' "$WORK/out"; then
  pass "doc ausente: --advisory imprime el hallazgo"
else
  fail "doc ausente: --advisory imprime el hallazgo — $(head -c 400 "$WORK/out")"
fi

# ─── 20. --advisory cubre el bloque del manifiesto ausente (F3) ────────────
D="$(new_fixture advisory-no-manifest-block)"
sed -i '/<!-- claims:begin -->/,/<!-- claims:end -->/d' "$(DOC_OF "$D")"
expect_exit 1 "bloque del manifiesto ausente: exit 1 en modo normal" "$D"
expect_exit 0 "bloque del manifiesto ausente: exit 0 en modo --advisory" "$D" --advisory
if grep -q 'C1: no existe el bloque del manifiesto' "$WORK/out"; then
  pass "bloque del manifiesto ausente: --advisory imprime el hallazgo"
else
  fail "bloque del manifiesto ausente: --advisory imprime el hallazgo — $(head -c 400 "$WORK/out")"
fi

# ─── 21. --advisory cubre el fence 'claims' ausente (F3) ───────────────────
D="$(new_fixture advisory-no-claims-fence)"
# El fence del manifiesto deja de ser 'claims': el bloque sigue ahí, pero el
# fence que el guard busca no existe. Los backticks son literales.
# shellcheck disable=SC2016
sed -i 's#^```claims$#```text#' "$(DOC_OF "$D")"
expect_exit 1 "fence 'claims' ausente: exit 1 en modo normal" "$D"
expect_exit 0 "fence 'claims' ausente: exit 0 en modo --advisory" "$D" --advisory
if grep -q "C1: el bloque del manifiesto no contiene un fence con info-string 'claims'" "$WORK/out"; then
  pass "fence 'claims' ausente: --advisory imprime el hallazgo"
else
  fail "fence 'claims' ausente: --advisory imprime el hallazgo — $(head -c 400 "$WORK/out")"
fi

# ─── 22. --advisory cubre el manifiesto vacío (F3) ─────────────────────────
D="$(new_fixture advisory-empty-manifest)"
sed -i -E '/^(shape|impl|claim|absent) +\|/d' "$(DOC_OF "$D")"
expect_exit 1 "manifiesto vacío: exit 1 en modo normal" "$D"
expect_exit 0 "manifiesto vacío: exit 0 en modo --advisory" "$D" --advisory
if grep -q 'C1: el manifiesto está vacío' "$WORK/out"; then
  pass "manifiesto vacío: --advisory imprime el hallazgo"
else
  fail "manifiesto vacío: --advisory imprime el hallazgo — $(head -c 400 "$WORK/out")"
fi

# ═══ WIRING ════════════════════════════════════════════════════════════════
# Las aserciones de arriba fijan lo que el guard DECIDE; esta sección fija sus
# CONEXIONES. Un guard presente, verde y conectado a nada es la forma #1707:
# las aserciones existen, ningún paso las ejecuta, y el defecto se puede
# revertir en silencio — que es exactamente lo que pasó con el doc normativo
# de providers: viva, citada por 13 archivos de código, y leída por nadie.
#
# LA FASE se declara EXACTAMENTE UNA VEZ (S7): `continue-on-error: true`
# (advisory) o el marcador `# guard-required` (required). La promoción a fase
# 2 es REEMPLAZAR el flag por el marcador — una edición. Quitar el flag sin
# dejar el marcador deja el paso SIN FASE, y las dos fases a la vez son una
# PROMOCIÓN FALSA (el paso seguiría siendo advisory mientras el harness lo
# certificaría como bloqueante). Las dos se rechazan.
#
# NEGATION: pre-fix no existía ninguna conexión — el doc normativo vivía fuera
# del libro mdBook, así que ningún gate lo leía y una firma que no compilaba
# sobrevivió meses sin que nadie se enterara.

# job_block <workflow> <job>: el bloque YAML de un job (clave de 2 espacios
# hasta la próxima clave de 2 espacios). No imprime nada cuando el job no
# existe, y los llamadores tratan eso como un harness ciego, no como un pase.
job_block() {
  awk -v job="$2" '
    $0 == "  " job ":" { in_job = 1; print; next }
    in_job && /^  [a-z0-9_-]+:/ { exit }
    in_job { print }
  ' "$1"
}

# step_block <file>: el paso YAML que ejecuta el guard, si lo hay. Un paso
# empieza en `      - ` y termina en el paso siguiente.
step_block() {
  awk '
    /^      - / {
      if (hit) { printf "%s", buf; found = 1; exit }
      buf = $0 "\n"; hit = 0; next
    }
    { buf = buf $0 "\n" }
    /run:[[:space:]].*scripts\/check_ai_providers_doc\.sh/ { hit = 1 }
    END { if (!found && hit) printf "%s", buf }
  ' "$1"
}

# lane_block <fast-gate> <fn>: el cuerpo de una función bash: desde su línea
# `fn() {` hasta la primera `}` en la columna 0.
lane_block() {
  awk -v fn="$2" '
    $0 == fn "() {" { in_fn = 1; print; next }
    in_fn && /^}/ { print; exit }
    in_fn { print }
  ' "$1"
}

# strip_guard_step <workflow>: el workflow SIN el paso del guard.
# El ancla es la línea `run:` del paso, nunca su `name:` — la promoción a
# required renombra el paso, y un control negativo anclado en el nombre dejaría
# de mutar (y empezaría a fallar) justo en la edición que tiene que sobrevivir.
strip_guard_step() {
  awk '
    /^      - / {
      if (!pending) { printf "%s", pre; pre = "" }
      else if (!hit) { printf "%s", buf }
      pending = 1; buf = $0 "\n"; hit = 0; next
    }
    pending { buf = buf $0 "\n" }
    !pending { pre = pre $0 "\n" }
    /run:[[:space:]].*scripts\/check_ai_providers_doc\.sh/ { hit = 1 }
    END {
      if (pending && !hit) printf "%s", buf
      if (!pending) printf "%s", pre
    }
  ' "$1"
}

# check_wiring <ci-yml> <fast-gate>: sale distinto de 0 cuando el guard no está
# conectado a sus dos bordes (CI y fast gate local), una línea por motivo.
check_wiring() {
  local wf="$1" fg="$2" bad=0 lane step
  local job="$WORK/wire-repo-guards.yml"
  local guard_esc="${GUARD_REL//./\\.}"
  local harness_esc="${HARNESS_REL//./\\.}"

  job_block "$wf" repo-guards > "$job"
  if [[ ! -s "$job" ]]; then
    echo "job 'repo-guards' not found: the guard would sit in a renamed job, unnoticed"
    return 1
  fi

  # (a) el guard corre en ese job
  step="$(step_block "$job")"
  if [[ -z "$step" ]]; then
    echo "no step in 'repo-guards' runs $GUARD_REL: the guard is in the tree and connected to nothing"
    bad=1
  else
    # (d) la fase está declarada EXACTAMENTE UNA VEZ (S7). La fase se lee de
    # las LÍNEAS EFECTIVAS del paso, no de su prosa: un comentario que
    # menciona `continue-on-error: true` al explicar la promoción no declara
    # la fase — y sin esta distinción, la prosa del propio ci.yml la
    # declararía dos veces.
    adv=0
    req=0
    if grep -qE '^[[:space:]]*continue-on-error:[[:space:]]*true([[:space:]]|$)' <<< "$step"; then
      adv=1
    fi
    if grep -qE '^[[:space:]]*#[[:space:]]*guard-required([[:space:]]|$)' <<< "$step"; then
      req=1
    fi
    if [[ $adv -eq 1 && $req -eq 1 ]]; then
      # Falso: el paso conserva `continue-on-error: true`, así que sigue siendo
      # advisory — pero el harness lo certificaría como bloqueante.
      echo "the guard step declares both phases ('continue-on-error: true' AND '# guard-required'): a false promotion — the step would still be advisory"
      bad=1
    elif [[ $adv -eq 0 && $req -eq 0 ]]; then
      # Un paso sin fase no dice si su fallo bloquea o no.
      echo "the guard step declares no phase: it needs 'continue-on-error: true' (advisory) or the '# guard-required' marker (required) — exactly one"
      bad=1
    fi
  fi

  # (b) el harness corre en el mismo job
  if ! grep -qE "run:[[:space:]].*${harness_esc}" "$job"; then
    echo "no step in 'repo-guards' runs $HARNESS_REL: the harness that proves the guard can fail is connected to nothing"
    bad=1
  fi

  # (c) el espejo local referencia el guard en las tres lanes que ven una de
  # las dos superficies del drift. Un path sólo nombrado en un comentario no
  # es una conexión.
  for lane in lane_docs lane_ci lane_fmt_and_guards; do
    lane_block "$fg" "$lane" > "$WORK/wire-$lane.sh"
    if [[ ! -s "$WORK/wire-$lane.sh" ]]; then
      echo "function '$lane' not found in the fast gate: the local mirror cannot be checked"
      bad=1
      continue
    fi
    if ! grep -vE '^[[:space:]]*#' "$WORK/wire-$lane.sh" \
      | grep -qE "run_advisory_guard .*${guard_esc}|bash ${guard_esc}"; then
      echo "lane '$lane' does not run $GUARD_REL: a doc or code change would pass locally with the drift unnamed"
      bad=1
    fi
  done

  [[ $bad -eq 0 ]]
}

# expect_wiring <expected-ok?> <label> <ci-yml> <fast-gate> [motivo-esperado]
# El motivo esperado es opcional, pero los controles negativos lo usan: sin él,
# un caso rojo podría estar rechazando la revisión mutada por un motivo
# distinto del que dice comprobar (y probaríamos otra cosa).
expect_wiring() {
  local want="$1" label="$2" wf="$3" fg="$4" reason="${5:-}"
  if check_wiring "$wf" "$fg" > "$WORK/wiring.out" 2>&1; then
    if [[ "$want" == "ok" ]]; then
      pass "$label"
    else
      fail "$label — la aserción pasó sobre una revisión mutada: no está viendo su input"
    fi
  else
    if [[ "$want" == "ok" ]]; then
      fail "$label — $(head -c 400 "$WORK/wiring.out")"
    elif [[ -n "$reason" ]] && ! grep -qF "$reason" "$WORK/wiring.out"; then
      fail "$label — el wiring se rechazó por otro motivo, no por el esperado ('$reason'): $(head -c 400 "$WORK/wiring.out")"
    else
      pass "$label"
    fi
  fi
}

# phase_variant <src> <dst> <advisory|required|both|neither>: copia de <src> con
# la declaración de fase del paso del guard mutada. La mutación se ancla en el
# `run:` del paso (nunca en su `name:`): la promoción renombra el paso, y un
# ancla en el nombre dejaría de mutar — y empezaría a fallar — justo en la
# edición que tiene que sobrevivir.
#
# La mutación es RELATIVA A LA FASE, no al texto: deja la fase pedida y quita
# la otra. Así la variante se puede derivar de cualquier base —incluida una
# revisión ya mutada— y el caso sigue significando lo mismo.
#
#   advisory   la fase comiteada (fase 1): sólo `continue-on-error: true`
#   required   la promoción (fase 2): sólo el marcador `# guard-required`
#   both       las dos fases a la vez (promoción falsa)
#   neither    ninguna fase (paso sin fase declarada)
phase_variant() {
  local src="$1" dst="$2" mode="$3"
  awk -v mode="$mode" '
    {
      lines[NR] = $0
      if ($0 ~ /^      - /) { coe = 0; req = 0 }
      if ($0 ~ /^[[:space:]]*continue-on-error:[[:space:]]*true([[:space:]]|$)/) coe = NR
      if ($0 ~ /^[[:space:]]*#[[:space:]]*guard-required([[:space:]]|$)/) req = NR
      if (run == 0 && $0 ~ /run:[[:space:]].*scripts\/check_ai_providers_doc\.sh/) { run = NR; gcoe = coe; greq = req }
    }
    END {
      if (run == 0) exit 3
      keep_adv = (mode == "advisory" || mode == "both")
      keep_req = (mode == "required" || mode == "both")
      anchor = (gcoe > 0 ? gcoe : (greq > 0 ? greq : run))
      n = 0
      for (i = 1; i <= NR; i++) {
        if (i == anchor) {
          if (keep_adv) out[++n] = "        continue-on-error: true"
          if (keep_req) out[++n] = "        # guard-required"
          if (i != gcoe && i != greq) out[++n] = lines[i]
          continue
        }
        if (i == gcoe || i == greq) continue
        out[++n] = lines[i]
      }
      for (i = 1; i <= n; i++) print out[i]
    }
  ' "$src" > "$dst"
}

# ─── W1. el wiring comiteado está completo ─────────────────────────────────
expect_wiring ok "wiring: ci.yml ejecuta el guard y el harness, el fast gate lo refleja" \
  "$CI_YML" "$FAST_GATE"

# ─── W2. control negativo: ci.yml sin el paso del guard ─────────────────────
# NEGATION: pre-fix un paso borrado del workflow es YAML válido y un run verde
# sin guard es indistinguible de un run verde con guard.
strip_guard_step "$CI_YML" > "$WORK/wire-ci-no-step.yml"
if cmp -s "$CI_YML" "$WORK/wire-ci-no-step.yml"; then
  fail "control negativo de ci.yml: la mutación no aplicó — el caso no probó nada"
else
  expect_wiring red "control negativo de ci.yml: el paso del guard borrado se rechaza" \
    "$WORK/wire-ci-no-step.yml" "$FAST_GATE"
fi

# ─── W3. control negativo: fast gate sin la referencia al guard ─────────────
# NEGATION: pre-fix el fast gate podía perder el espejo sin que nadie lo
# notara — el guard seguía verde en CI y nadie lo veía en local.
sed "\|${GUARD_REL}|d" "$FAST_GATE" > "$WORK/wire-fg-no-ref.sh"
if cmp -s "$FAST_GATE" "$WORK/wire-fg-no-ref.sh"; then
  fail "control negativo del fast gate: la mutación no aplicó — el caso no probó nada"
else
  expect_wiring red "control negativo del fast gate: la referencia perdida se rechaza" \
    "$CI_YML" "$WORK/wire-fg-no-ref.sh"
fi

# ─── W4. la fase advisory sola es una fase válida ──────────────────────────
# La primera mitad de "exactamente una vez": el estado comiteado (advisory)
# declara su fase con `continue-on-error: true` y nada más.
if phase_variant "$CI_YML" "$WORK/wire-ci-advisory.yml" advisory; then
  expect_wiring ok "fase advisory sola ('continue-on-error: true'): válida" \
    "$WORK/wire-ci-advisory.yml" "$FAST_GATE"
else
  fail "fase advisory sola: no se pudo generar la revisión — el caso no probó nada"
fi

# ─── W5. la promoción a required: el flag REEMPLAZADO por el marcador ──────
# NEGATION: pre-fix la receta de promoción era ambigua: la spec decía "quitar
# continue-on-error: true" y el harness exigía "advisory O required" — dos
# recetas incompatibles, y la primera dejaba el harness en rojo. La receta
# corregida es REEMPLAZAR, y ésta es la fase 2 tal como queda.
if phase_variant "$CI_YML" "$WORK/wire-ci-required.yml" required; then
  expect_wiring ok "fase required sola ('# guard-required'): la promoción es válida" \
    "$WORK/wire-ci-required.yml" "$FAST_GATE"
else
  fail "promoción a required: no se pudo generar la revisión — el caso no probó nada"
fi

# ─── W6. control negativo: las dos fases a la vez (promoción falsa) ─────────
# NEGATION: pre-fix la detección de fase era un OR: `continue-on-error: true`
# JUNTO con `# guard-required` pasaba el harness, y el paso seguía siendo
# advisory — el harness certificaba un guard que no bloquea.
if phase_variant "$CI_YML" "$WORK/wire-ci-both.yml" both; then
  if cmp -s "$CI_YML" "$WORK/wire-ci-both.yml"; then
    fail "control negativo de la promoción falsa: la mutación no aplicó — el caso no probó nada"
  else
    expect_wiring red "control negativo: las dos fases a la vez se rechazan (promoción falsa)" \
      "$WORK/wire-ci-both.yml" "$FAST_GATE" "both phases"
  fi
else
  fail "control negativo de la promoción falsa: no se pudo generar la revisión — el caso no probó nada"
fi

# ─── W7. control negativo: ninguna fase (paso sin fase declarada) ───────────
# NEGATION: pre-fix quitar `continue-on-error: true` sin dejar el marcador
# dejaba el paso sin fase — la receta que la spec antigua daba por válida.
# Ahora el rechazo es explícito y su motivo es verificable.
if phase_variant "$CI_YML" "$WORK/wire-ci-neither.yml" neither; then
  if cmp -s "$CI_YML" "$WORK/wire-ci-neither.yml"; then
    fail "control negativo del paso sin fase: la mutación no aplicó — el caso no probó nada"
  else
    expect_wiring red "control negativo: el paso sin fase declarada se rechaza" \
      "$WORK/wire-ci-neither.yml" "$FAST_GATE" "no phase"
  fi
else
  fail "control negativo del paso sin fase: no se pudo generar la revisión — el caso no probó nada"
fi

printf '\nharness: %d ok, %d fail\n' "$PASSED" "$FAILED"
[[ $FAILED -eq 0 ]]
