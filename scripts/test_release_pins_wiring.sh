#!/usr/bin/env bash
# test_release_pins_wiring.sh
#
# Harness de semántica para el WIRING de los pines de versión de la
# documentación. Complemento de scripts/test_bump_docs_version_pins.sh (que
# fija la semántica del ESCRITOR) y de scripts/test_apply_release_pins.sh (que
# fija la del ORQUESTADOR): este archivo fija que esas piezas estén
# CONECTADAS a algo.
#
# POR QUÉ EXISTE ESTE ARCHIVO
# El guard de pines (#2017) lleva una release entera inerte: su semántica era
# correcta, pero ningún workflow lo invocaba, así que un árbol con nueve
# recetas de instalación viejas siguió verde. El defecto no estaba en el
# guard: estaba en la ausencia de una arista. Y nada fijó esa arista después:
# un paso borrado es YAML válido, y una corrida verde sin guard es
# indistinguible de una corrida verde con guard.
#
# Lo que demuestra, offline y NO VACUAMENTE (un control que no puede ver su
# entrada falla en vez de pasar):
#
#   - .github/workflows/release-plz.yml invoca scripts/apply_release_pins.sh en
#     el job `release-plz-pr`, DESPUÉS del paso `Run release-plz` (el PR tiene
#     que existir antes de buscarlo) y con el APP token
#     (steps.app-token.outputs.token): un push con GITHUB_TOKEN no entrega los
#     eventos pull_request, así que el head SHA del Release PR se queda sin
#     checks y branch protection bloquea el merge;
#   - .github/workflows/ci.yml corre los tres harnesses en `repo-guards`, un
#     job sin `paths:` y sin `if:`, para que una regresión reds el CI Gate
#     requerido en CADA PR;
#   - scripts/ci_fast_gate.sh los corre en la lane `lane_ci`, con un trigger
#     que los matchea (y que matchea los workflows que ellos leen), porque
#     lane_docs nunca corre para un cambio `scripts/**` — el único tipo de
#     cambio que puede romperlos.
#
# Uso: bash scripts/test_release_pins_wiring.sh [release-plz-yml] [ci-yml] [fast-gate]
# Las rutas opcionales existen para apuntar cada comprobación a una revisión
# mutada en los controles negativos; por defecto son las del repo.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RELEASE_PLZ_YML="${1:-$ROOT/.github/workflows/release-plz.yml}"
CI_YML="${2:-$ROOT/.github/workflows/ci.yml}"
FAST_GATE="${3:-$ROOT/scripts/ci_fast_gate.sh}"

WRITER_HARNESS="scripts/test_bump_docs_version_pins.sh"
ORCHESTRATOR_HARNESS="scripts/test_apply_release_pins.sh"
WIRING_HARNESS="scripts/test_release_pins_wiring.sh"
HARNESSES=("$WRITER_HARNESS" "$ORCHESTRATOR_HARNESS" "$WIRING_HARNESS")

ORCHESTRATOR_SCRIPT="scripts/apply_release_pins.sh"
STEP_NAME="Own the docs version pins"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

declare -i cases_run=0 cases_failed=0 case_fail=0

# job_block <workflow> <job> — el bloque YAML de un job: su clave de 2
# espacios hasta (sin incluir) la siguiente clave de 2 espacios. Imprime nada
# cuando el job no existe, y quien llama trata eso como un harness ciego, no
# como un aprobado.
job_block() {
  awk -v job="$2" '
    $0 == "  " job ":" { in_job = 1; print; next }
    in_job && /^  [a-z0-9_-]+:/ { exit }
    in_job { print }
  ' "$1"
}

# job_header <block-file> — el bloque del job hasta su clave `steps:`. Las
# claves de nivel job viven a 4 espacios ahí, así que un `paths:` o un `if:`
# dentro del cuerpo de un paso no puede confundirse con uno de nivel job.
job_header() {
  awk '/^    steps:/ { exit } { print }'
}

# lane_block <fast-gate> <function> — el cuerpo de una función bash: su línea
# `name() {` hasta la primera `}` en la columna 0.
lane_block() {
  awk -v fn="$2" '
    $0 == fn "() {" { in_fn = 1; print; next }
    in_fn && /^}/ { print; exit }
    in_fn { print }
  ' "$1"
}

# step_block <job-block> <needle> — el paso (bloque `      - name:`) que
# contiene la cadena buscada. Imprime nada cuando ningún paso la contiene.
step_block() {
  awk -v needle="$2" '
    /^      - name: / {
      if (block != "" && found) { print block; block = ""; exit }
      block = $0 "\n"; found = 0; next
    }
    {
      if (block != "") {
        if (index($0, needle)) found = 1
        block = block $0 "\n"
      }
    }
    END {
      if (block != "" && found) print block
    }
  ' <<< "$1"
}

# check_release_plz_wiring <workflow> — nonzero cuando el paso que bumpea los
# pines no está donde tiene que estar, una línea por motivo.
check_release_plz_wiring() {
  local wf="$1" bad=0 block step run_line release_line
  block="$(job_block "$wf" release-plz-pr)"
  if [[ -z "$block" ]]; then
    echo "el job 'release-plz-pr' no existe: el paso que bumpea los pines se llevaría con él, sin avisar"
    return 1
  fi

  run_line="$(grep -m1 -nE "^[[:space:]]*run:[[:space:]]*bash[[:space:]]+${ORCHESTRATOR_SCRIPT//./\\.}" <<< "$block" \
    | cut -d: -f1)"
  if [[ -z "$run_line" ]]; then
    echo "ningún paso del job 'release-plz-pr' ejecuta $ORCHESTRATOR_SCRIPT: el Release PR queda rojo por construcción, sin dueño para el bump"
    bad=1
  fi

  release_line="$(grep -m1 -nE "^[[:space:]]*- name: Run release-plz\$" <<< "$block" | cut -d: -f1)"
  if [[ -z "$release_line" ]]; then
    echo "el job 'release-plz-pr' no tiene el paso 'Run release-plz': el orden no se puede comprobar"
    bad=1
  elif [[ -n "$run_line" && "$run_line" -le "$release_line" ]]; then
    echo "el paso que bumpea los pines corre ANTES de 'Run release-plz': el Release PR todavía no existe cuando se lo busca"
    bad=1
  fi

  step="$(step_block "$block" "$ORCHESTRATOR_SCRIPT")"
  if [[ -z "$step" ]]; then
    echo "no se puede aislar el paso que ejecuta $ORCHESTRATOR_SCRIPT: el token no se puede comprobar"
    bad=1
  else
    if grep -qF -- 'secrets.GITHUB_TOKEN' <<< "$step"; then
      echo "el paso '$STEP_NAME' usa secrets.GITHUB_TOKEN: un push con GITHUB_TOKEN no entrega los eventos pull_request, así que el head SHA del Release PR se queda sin checks y branch protection bloquea el merge"
      bad=1
    fi
    if ! grep -qF -- 'steps.app-token.outputs.token' <<< "$step"; then
      echo "el paso '$STEP_NAME' no usa el App token (steps.app-token.outputs.token): el push no dispararía el CI del Release PR"
      bad=1
    fi
  fi

  [[ $bad -eq 0 ]]
}

# check_ci_wiring <workflow> — nonzero cuando los harnesses están en el árbol
# pero no los corre nadie, una línea por motivo.
check_ci_wiring() {
  local wf="$1" bad=0 block header script
  block="$(job_block "$wf" repo-guards)"
  if [[ -z "$block" ]]; then
    echo "el job 'repo-guards' no existe: los harnesses quedarían en el árbol sin ejecución"
    return 1
  fi
  printf '%s\n' "$block" | job_header > "$tmp/ci-repo-guards-header.yml"
  if grep -qE '^    paths(-ignore)?:' "$tmp/ci-repo-guards-header.yml"; then
    echo "el job 'repo-guards' está filtrado por paths: un PR fuera del filtro no correría los harnesses"
    bad=1
  fi
  if grep -qE '^    if:' "$tmp/ci-repo-guards-header.yml"; then
    echo "el job 'repo-guards' es condicional: los harnesses se saltarían siempre que la condición fuera falsa"
    bad=1
  fi
  for script in "${HARNESSES[@]}"; do
    if ! grep -qE "run:[[:space:]]*bash[[:space:]]+${script//./\\.}" <<< "$block"; then
      echo "ningún paso de 'repo-guards' corre $script: el harness está en el árbol y no lo ejecuta nadie"
      bad=1
    fi
  done
  [[ $bad -eq 0 ]]
}

# check_fast_gate_wiring <fast-gate> — nonzero cuando el espejo local perdió
# una de sus aristas, una línea por motivo.
check_fast_gate_wiring() {
  local fg="$1" bad=0 lane script target pattern matched
  lane="$(lane_block "$fg" lane_ci)"
  if [[ -z "$lane" ]]; then
    echo "la función 'lane_ci' no existe en el fast gate: el espejo local no se puede comprobar"
    return 1
  fi
  # Un path nombrado en un comentario NO es un wiring: lalane tiene que
  # EJECUTAR el harness. La forma de casa aquí es
  # `run_step "<nombre>" bash <script>`.
  for script in "${HARNESSES[@]}"; do
    if ! grep -vE '^[[:space:]]*#' <<< "$lane" \
      | grep -qE "run_step .*${script//./\\.}|bash ${script//./\\.}"; then
      echo "lane_ci no corre $script: un cambio \`scripts/**\` — el único que puede romperlo — no lo ejercitaría localmente"
      bad=1
    fi
  done
  # El trigger se evalúa de verdad: se extraen los patrones \`grep -Eq\` de la
  # lane y se prueba cada path contra ellos. Comparar el path contra el texto
  # del trigger no probaría nada — el patrón puede haber cambiado de forma.
  while IFS= read -r target; do
    [[ -n $target ]] || continue
    matched=false
    while IFS= read -r pattern; do
      [[ -n $pattern ]] || continue
      if printf '%s\n' "$target" | grep -Eq "$pattern" 2>/dev/null; then
        matched=true
        break
      fi
    done < <(grep -oE "grep -Eq '[^']*'" <<< "$lane" | sed -e "s/^grep -Eq '//" -e "s/'\$//")
    if [[ "$matched" != "true" ]]; then
      echo "ningún trigger de lane_ci matchea $target: un PR que lo toque saltaría los harnesses"
      bad=1
    fi
  done < <(printf '%s\n' "${HARNESSES[@]}" ".github/workflows/release-plz.yml" ".github/workflows/ci.yml")
  [[ $bad -eq 0 ]]
}

# finish_case <label> — contabiliza y reporta un caso terminado.
finish_case() {
  local label="$1"
  cases_run+=1
  if [[ $case_fail -eq 0 ]]; then
    echo "ok: caso $cases_run — $label"
  else
    cases_failed+=1
    echo "FAIL: caso $cases_run — $label"
  fi
  case_fail=0
}

# --- casos -------------------------------------------------------------------

# Caso 1: el workflow commiteado conecta el bump de pines al Release PR.
if check_release_plz_wiring "$RELEASE_PLZ_YML" > "$tmp/out.txt" 2>&1; then
  echo "  ok: el job release-plz-pr ejecuta el orquestador, después de release-plz y con el App token"
else
  echo "  FAIL: el wiring commiteado del release-plz.yml no alcanza al Release PR"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "el release-plz.yml commiteado invoca el orquestador en release-plz-pr"

# Caso 2: control negativo — se borra el paso del orquestador. El workflow
# sigue siendo YAML válido y sólo este harness lo nota.
sed "/^      - name: ${STEP_NAME}\$/,+4d" "$RELEASE_PLZ_YML" > "$tmp/rel-no-step.yml"
if cmp -s "$RELEASE_PLZ_YML" "$tmp/rel-no-step.yml"; then
  echo "  FAIL: la inyección del fixture no aplicó — el ancla del sed falló y este caso no probó nada"
  case_fail=1
elif check_release_plz_wiring "$tmp/rel-no-step.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: un paso borrado pasó — el bump de pines puede desaparecer sin que nadie lo note"
  case_fail=1
elif grep -qF -e "$ORCHESTRATOR_SCRIPT" "$tmp/out.txt"; then
  echo "  ok: el paso borrado fue rechazado y nombrado"
else
  echo "  FAIL: fue rechazado, pero no se nombró el script"
  case_fail=1
fi
finish_case "control negativo: el paso del orquestador se borra de release-plz.yml"

# Caso 3: control negativo — el token del paso pasa a secrets.GITHUB_TOKEN.
awk '
  /^      - name: Own the docs version pins$/ { in_step = 1 }
  in_step && /steps\.app-token\.outputs\.token/ { sub(/steps\.app-token\.outputs\.token/, "secrets.GITHUB_TOKEN") }
  { print }
  in_step && /^        run: / { in_step = 0 }
' "$RELEASE_PLZ_YML" > "$tmp/rel-bad-token.yml"
if cmp -s "$RELEASE_PLZ_YML" "$tmp/rel-bad-token.yml"; then
  echo "  FAIL: la inyección del fixture no aplicó — el ancla del awk falló y este caso no probó nada"
  case_fail=1
elif check_release_plz_wiring "$tmp/rel-bad-token.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: un paso con secrets.GITHUB_TOKEN pasó — el push no dispararía el CI del Release PR"
  case_fail=1
elif grep -q -e 'GITHUB_TOKEN' "$tmp/out.txt"; then
  echo "  ok: el token incorrecto fue rechazado y nombrado"
else
  echo "  FAIL: fue rechazado, pero no se nombró el token"
  case_fail=1
fi
finish_case "control negativo: el paso del orquestador usa secrets.GITHUB_TOKEN"

# Caso 4: el ci.yml commiteado corre los tres harnesses, sin filtro ni condición.
if check_ci_wiring "$CI_YML" > "$tmp/out.txt" 2>&1; then
  echo "  ok: repo-guards corre los tres harnesses, sin paths: ni if:"
else
  echo "  FAIL: el wiring commiteado de ci.yml no corre los harnesses"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "el ci.yml commiteado corre los tres harnesses en repo-guards"

# Caso 5: control negativo — se borra un harness de ci.yml.
sed '/^      - name: Release pins orchestrator semantics harness$/,+1d' "$CI_YML" > "$tmp/ci-no-harness.yml"
if cmp -s "$CI_YML" "$tmp/ci-no-harness.yml"; then
  echo "  FAIL: la inyección del fixture no aplicó — el ancla del sed falló y este caso no probó nada"
  case_fail=1
elif check_ci_wiring "$tmp/ci-no-harness.yml" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: un harness borrado de ci.yml pasó — el harness puede quedar inerte sin avisar"
  case_fail=1
elif grep -qF -e "$ORCHESTRATOR_HARNESS" "$tmp/out.txt"; then
  echo "  ok: el harness borrado fue rechazado y nombrado"
else
  echo "  FAIL: fue rechazado, pero no se nombró el harness"
  case_fail=1
fi
finish_case "control negativo: un harness se borra de repo-guards"

# Caso 6: el fast gate commiteado corre los harnesses en lane_ci.
if check_fast_gate_wiring "$FAST_GATE" > "$tmp/out.txt" 2>&1; then
  echo "  ok: lane_ci corre los tres harnesses y su trigger los matchea"
else
  echo "  FAIL: el wiring commiteado del fast gate está incompleto"
  sed 's/^/        /' "$tmp/out.txt"
  case_fail=1
fi
finish_case "el fast gate commiteado corre los harnesses en lane_ci"

# Caso 7: control negativo — el harness se queda en lane_ci sólo como un
# comentario que nombra su path. Un comentario no es una arista, y una
# comprobación que sólo compara paths no puede notar la diferencia: este caso
# existe porque un verificador independiente probó que la primera versión de
# esa comprobación podía satisfacerse con un comentario.
awk '
  /^lane_ci\(\) \{/ { in_fn = 1 }
  in_fn && /run_step "release pins wiring \(offline\)"/ {
    print "    # see scripts/test_release_pins_wiring.sh for the wiring harness"
    next
  }
  { print }
  in_fn && /^\}/ { in_fn = 0 }
' "$FAST_GATE" > "$tmp/fg-comment-only.sh"
if cmp -s "$FAST_GATE" "$tmp/fg-comment-only.sh"; then
  echo "  FAIL: la inyección del fixture no aplicó — el ancla del awk falló y este caso no probó nada"
  case_fail=1
elif check_fast_gate_wiring "$tmp/fg-comment-only.sh" > "$tmp/out.txt" 2>&1; then
  echo "  FAIL: una lane que sólo NOMBRA el harness en un comentario pasó — ese es exactamente el hueco que este caso cierra"
  case_fail=1
elif grep -qF -e "$WIRING_HARNESS" "$tmp/out.txt"; then
  echo "  ok: la mención en comentario fue rechazada y nombrada"
else
  echo "  FAIL: fue rechazada, pero no se nombró el harness"
  case_fail=1
fi
finish_case "control negativo: el harness sólo mencionado en un comentario en lane_ci"

if [[ $cases_failed -eq 0 ]]; then
  # Los dos lados son la cuenta realmente corrida: un total fijo es un número
  # más que puede quedar desfasado de los casos.
  echo "OK: $cases_run/$cases_run casos del wiring de pines de versión pasaron"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run casos del wiring de pines de versión fallaron"
exit 1
