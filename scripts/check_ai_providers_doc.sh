#!/usr/bin/env bash
# ============================================================================
# AI-PROVIDERS DOC DRIFT GUARD — docs/architecture/ai-providers-design.md
# ============================================================================
# Qué protege: `docs/architecture/ai-providers-design.md` es una spec NORMATIVA
# (13 archivos de código la citan) que vive FUERA del libro mdBook
# (`docs/book.toml` → `src = "src"`), así que ningún gate la leía. Consecuencia
# medida: una firma que NO compilaba —`AuthSource::resolve()` documentada como
# `Result<ApiKey, AuthError>` cuando la real es `Result<Option<ApiKey>,
# AuthError>`— sobrevivió meses (#2136). El doc se corrigió a mano (#2027/#2137);
# lo que faltaba era el mecanismo que note el próximo.
#
# Este guard es el LECTOR: el doc se audita contra el árbol de código, no
# contra su propia prosa. El manifiesto vive DENTRO del doc
# (`<!-- claims:begin -->` … `<!-- claims:end -->`), porque el doc es la fuente
# de verdad de sus propias afirmaciones y el guard sólo lo lee. Es el mismo
# modelo que el guard de los pines de versión: documento con literales + fuente
# de verdad + check fail-closed.
#
# Checks (TODOS fail-closed):
#   C1 manifest   bloque presente, campos parseables y no vacíos, archivos
#                 existentes, líneas dentro del archivo
#   C2 claim      la línea citada (trimmed) contiene la subcadena esperada,
#                 coincidencia literal (no regex)
#   C3 shape      los miembros del tipo declarado == los miembros esperados, en
#                 AMBOS sentidos (variantes para enum, campos para struct)
#   C4 impl       el fence del doc que contiene la declaración está contenido en
#                 el archivo citado (containment, no igualdad de conjunto)
#   C5 absent     el símbolo NO aparece en el archivo (o en todo `crates/`)
#   C6 cobertura  toda cita `archivo:línea` de la PROSA del doc está registrada
#                 en el manifiesto — una cita sin claim verificable es drift
#   C7 §          todo `§N` / `§Na` del doc Y del código que lo cita resuelve a
#                 una sección numerada (`### N.`) o a un ítem letra (`- **(a)**`)
#
# Normalización de rutas (S5), en orden: (a) contra la raíz del repo, (b) contra
# `crates/webfang_core/src/`, (c) por basename único bajo `crates/`. Si no
# resuelve o es ambigua → drift. Nunca se adivina.
#
# AISLAMIENTO DE RUTAS: toda ruta que el guard LEE queda confinada a la raíz
# del repo. Sin ese aislamiento, una cita como `../../etc/passwd` resolvería a
# un archivo real FUERA del árbol y el guard imprimiría su contenido en el log
# de CI (diagnóstico C2/C3). La ruta se normaliza contra su base y la que sale
# se reporta como drift SIN leerse.
#
# Uso: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]
# Salida: 0 limpio · 1 drift (fail-closed). `--advisory` NUNCA falla por drift
# ni por estructura ausente: imprime el hallazgo y sale con 0. Un error de
# invocación (flag desconocido, `--root` que no existe) sigue fallando: no es
# un hallazgo, es una llamada mal formada.
# ============================================================================
set -uo pipefail

DOC_REL="docs/architecture/ai-providers-design.md"
CORE_SRC_REL="crates/webfang_core/src"

usage() {
  cat >&2 <<'EOF'
Uso: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]

  --root DIR   raíz del repo (default: la raíz derivada de BASH_SOURCE)
  --advisory   imprime los mismos hallazgos y sale SIEMPRE con 0

Salida: 0 limpio · 1 drift (fail-closed). Con --advisory, el drift y la
estructura ausente (doc, bloque del manifiesto, fence 'claims', manifiesto
vacío) salen con 0 y el hallazgo se imprime igual. Un error de invocación
sigue saliendo con 1.
EOF
}

ROOT=""
ADVISORY=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root)
      if [[ $# -lt 2 ]]; then
        printf '::error::falta el valor de --root\n' >&2
        exit 1
      fi
      ROOT="$2"
      shift 2
      ;;
    --root=*)
      ROOT="${1#--root=}"
      shift
      ;;
    --advisory)
      ADVISORY=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf '::error file=%s,line=1::argumento desconocido: %s\n' "$DOC_REL" "$1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$ROOT" ]]; then
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
if [[ ! -d "$ROOT" ]]; then
  printf '::error file=%s,line=1::la raíz %s no existe\n' "$DOC_REL" "$ROOT" >&2
  exit 1
fi
ROOT="$(cd "$ROOT" && pwd)"

# Canonicalización de la raíz: el aislamiento de rutas compara por forma
# canónica. `realpath -m` resuelve los symlinks del prefijo existente y no
# falla con rutas inexistentes. Sin `realpath` (macOS sin coreutils) el
# aislamiento queda en su nivel LEXICAL — que ya bloquea `..` — y se avisa:
# un symlink DENTRO del repo que apunte afuera no se detectaría.
REALPATH_OK=0
ROOT_CANON="$ROOT"
if command -v realpath >/dev/null 2>&1; then
  REALPATH_OK=1
  ROOT_CANON="$(realpath -m -- "$ROOT" 2>/dev/null || printf '%s' "$ROOT")"
else
  # Los backticks son literales: es el nombre de la herramienta, no una
  # sustitución de comandos.
  # shellcheck disable=SC2016
  printf 'aviso: `realpath` no está disponible — el aislamiento de rutas sólo es léxico (un symlink que apunte fuera de la raíz no se detecta)\n' >&2
fi

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

FINDINGS=0
report() { # file line message
  printf '::error file=%s,line=%s::%s\n' "$1" "$2" "$3" >&2
  FINDINGS=$((FINDINGS + 1))
}

# Salida estructural: el doc ausente, el bloque del manifiesto ausente, el
# fence 'claims' ausente y el manifiesto vacío son HALLAZGOS, no una avería
# del script. En modo `--advisory` no pueden abortar al llamador: el hallazgo
# se imprime y se sale con 0, como en la rama final. Sin este contrato,
# `--advisory` sólo cubría el drift detectado al final del recorrido y una
# estructura ausente todavía salía con 1.
abort_finding() { # línea motivo
  report "$DOC_REL" "$1" "$2"
  printf '::error::guard de drift doc↔código: %d hallazgo(s) en %s\n' "$FINDINGS" "$DOC_REL" >&2
  if [[ $ADVISORY -eq 1 ]]; then
    exit 0
  fi
  exit 1
}

# ─── normalización ───────────────────────────────────────────────────────────
# Colapsa runs de whitespace a un espacio y quita la coma antes de `)` / `]`.
# Se aplica igual a los dos lados de cada comparación.
norm_stream() {
  sed -e 's/[[:space:]][[:space:]]*/ /g' -e 's/^ //' -e 's/ $//' \
      -e 's/,)/)/g' -e 's/,]/]/g'
}

norm() { printf '%s' "$1" | norm_stream; }

# Un comentario de una línea (`//`, `///`) deja la línea vacía: se ignora.
strip_comment() { printf '%s' "${1%%//*}"; }

# Los guiones largos (–, —, −) se reducen a `-` para que el recognizer de
# rangos sea ASCII puro y no dependa del locale.
EN_DASH="$(printf '\xe2\x80\x93')"
EM_DASH="$(printf '\xe2\x80\x94')"
MINUS_SIGN="$(printf '\xe2\x88\x92')"
DASH_SED="s/${EN_DASH}/-/g; s/${EM_DASH}/-/g; s/${MINUS_SIGN}/-/g"

# ─── aislamiento y resolución de rutas (S5) ─────────────────────────────────
# El aislamiento y la resolución son la misma puerta: una ruta que el guard
# LEE tiene que quedar confinada a la raíz del repo. Sin ese aislamiento, una
# cita como `../../etc/passwd` resolvería a un archivo real FUERA del árbol y
# el guard imprimiría su contenido en el log de CI (diagnóstico C2/C3).
#
# `confina <rel> <base>` normaliza `rel` (relativa a la raíz) contra `base`
# (relativa a la raíz) e imprime el resultado; devuelve 1 si la ruta sale de
# `base`. El nivel de anidamiento de la base es el PISO: un `..` no puede
# bajar de ahí. El aislamiento es LEXICAL — no depende de que la ruta exista,
# y una ruta que escapa no se mira siquiera.
confina() { # rel base → ruta normalizada relativa a la raíz; 1 si escapa
  local rel="$1" base="$2" seg
  local -a stack=() rel_segs=() base_segs=()
  IFS='/' read -r -a base_segs <<<"$base"
  for seg in ${base_segs[@]+"${base_segs[@]}"}; do
    case "$seg" in
      ''|'.') continue ;;
      '..')
        [[ ${#stack[@]} -gt 0 ]] || return 1
        stack=("${stack[@]:0:${#stack[@]}-1}")
        ;;
      *) stack+=("$seg") ;;
    esac
  done
  IFS='/' read -r -a rel_segs <<<"$rel"
  for seg in ${rel_segs[@]+"${rel_segs[@]}"}; do
    case "$seg" in
      ''|'.') continue ;;
      '..')
        [[ ${#stack[@]} -gt 0 ]] || return 1
        stack=("${stack[@]:0:${#stack[@]}-1}")
        ;;
      *) stack+=("$seg") ;;
    esac
  done
  [[ ${#stack[@]} -gt 0 ]] || return 1
  (IFS=/; printf '%s\n' "${stack[*]}")
}

# canon_under_root <rel>: 1 si la forma canónica de la ruta sale de la raíz
# canónica — el caso del symlink DENTRO del repo que apunta afuera, que el
# aislamiento lexical no puede ver.
canon_under_root() { # rel → 1 si escapa
  [[ $REALPATH_OK -eq 1 ]] || return 0
  local canon
  canon="$(realpath -m -- "$ROOT/$1" 2>/dev/null)" || return 1
  case "$canon" in
    "$ROOT_CANON"/*) return 0 ;;
    *) return 1 ;;
  esac
}

# motivo de una ruta no resuelta: 1 = no existe o es ambigua, 2 = escapa de la
# raíz. El motivo va en el hallazgo porque son defectos distintos: el segundo
# es el que puede filtrar contenido de fuera del árbol.
motivo_ruta() { # rc → texto
  if [[ "$1" -eq 2 ]]; then
    printf 'sale de la raíz del repo — el guard no lee ningún archivo fuera de ella'
  else
    printf 'no existe, o el basename es ambiguo bajo crates/'
  fi
}

# ─── resolución de rutas (S5) ────────────────────────────────────────────────
resolve_path() { # path → imprime la ruta relativa al repo; 1 si no resuelve; 2 si escapa de la raíz
  local p="${1#./}" base count matches rel
  # (a) contra la raíz del repo
  if ! rel="$(confina "$p" "")"; then
    return 2
  fi
  if [[ -f "$ROOT/$rel" ]]; then
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  # (b) contra crates/webfang_core/src/
  if ! rel="$(confina "$p" "$CORE_SRC_REL")"; then
    return 2
  fi
  if [[ -f "$ROOT/$rel" ]]; then
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  # (c) por basename único bajo crates/
  base="${p##*/}"
  matches="$(find "$ROOT/crates" -type f -name "$base" 2>/dev/null | sort)"
  count="$(printf '%s' "$matches" | grep -c . || true)"
  if [[ "$count" == "1" ]]; then
    if ! rel="$(confina "${matches#"$ROOT"/}" "")"; then
      return 2
    fi
    canon_under_root "$rel" || return 2
    printf '%s\n' "$rel"
    return 0
  fi
  return 1
}

# ─── caché de archivos normalizados ─────────────────────────────────────────
# Una entrada POR RUTA, y la caché FUNCIONA. `norm_file` deja el path en
# `NF_PATH` en vez de imprimirlo: un `$(...)` corre en un subshell, así que
# las mutaciones de los arrays se perdían y la caché era CÓDIGO MUERTO — cada
# llamada reescribía el archivo y el lookup nunca encontraba nada.
#
# La clave del archivo de caché NO puede ser la traducción de la ruta con
# `tr '/.' '__'`: esa traducción no es inyectiva (`domain/x.a.rs` y
# `domain/x/a.rs` dan el MISMO nombre), así que la segunda escritura pisaba la
# primera y el containment de C4 se satisfacía con el contenido de OTRO
# archivo — un falso verde, la peor dirección posible. El nombre de la entrada
# lo lleva un contador (único por construcción) y el contenido de la caché es
# SIEMPRE el del archivo citado.
NORM_KEYS=()
NORM_VALS=()
NORM_N=0
NF_PATH=""
norm_file() { # ruta relativa → NF_PATH: el temporal con las líneas normalizadas
  local f="$1" i out
  for i in "${!NORM_KEYS[@]}"; do
    if [[ "${NORM_KEYS[$i]}" == "$f" ]]; then
      NF_PATH="${NORM_VALS[$i]}"
      return 0
    fi
  done
  NORM_N=$((NORM_N + 1))
  out="$TMPD/norm.$NORM_N"
  norm_stream <"$ROOT/$f" >"$out"
  NORM_KEYS+=("$f")
  NORM_VALS+=("$out")
  NF_PATH="$out"
}

# ─── extracción de miembros de un tipo (C3) ─────────────────────────────────
# El programa awk lleva `$0` y `$1`: deben llegar literales al intérprete, así
# que la comilla simple es obligatoria.
# shellcheck disable=SC2016
AWK_MEMBERS='
NR < start { next }
NR == start {
  raw = $0
  n = gsub(/\{/, "{", raw); m = gsub(/\}/, "}", raw)
  depth = n - m
  next
}
{
  raw = $0
  if (depth == 1) {
    t = raw
    sub(/\/\/.*$/, "", t)
    gsub(/^[ \t]+/, "", t)
    if (kind == "enum") {
      if (t ~ /^[A-Za-z_][A-Za-z0-9_]*([ \t]*[,({=]|$)/) {
        name = t
        sub(/[ \t]*[,({=].*$/, "", name)
        sub(/[ \t]*$/, "", name)
        if (name != "") print name
      }
    } else {
      if (t ~ /^((pub(\([^)]*\))?[ \t]+)?)?[A-Za-z_][A-Za-z0-9_]*[ \t]*:/) {
        name = t
        sub(/^((pub(\([^)]*\))?[ \t]+)?)?/, "", name)
        sub(/[ \t]*:.*$/, "", name)
        sub(/[ \t]*$/, "", name)
        if (name != "") print name
      }
    }
  }
  n = gsub(/\{/, "{", raw); m = gsub(/\}/, "}", raw)
  depth += n - m
  if (depth <= 0) exit
}
'

extract_members() { # archivo línea tipo → imprime un miembro por línea
  awk -v start="$2" -v kind="$3" "$AWK_MEMBERS" "$1"
}

# ─── doc ─────────────────────────────────────────────────────────────────────
DOC="$ROOT/$DOC_REL"
if [[ ! -f "$DOC" ]]; then
  abort_finding 1 "no existe el doc normativo $DOC_REL bajo la raíz $ROOT"
fi
mapfile -t DOC_LINES <"$DOC"

# ─── fences del doc ─────────────────────────────────────────────────────────
FENCE_OPEN=()
FENCE_CLOSE=()
FENCE_LANG=()
FENCE_OPEN_RE='^```(.*)$'
FENCE_CLOSE_RE='^```[[:space:]]*$'
in_fence=0
fi=0
for i in "${!DOC_LINES[@]}"; do
  l="${DOC_LINES[$i]}"
  if [[ $in_fence -eq 0 ]]; then
    if [[ "$l" =~ $FENCE_OPEN_RE ]]; then
      in_fence=1
      FENCE_OPEN+=( "$i" )
      FENCE_LANG+=( "${BASH_REMATCH[1]}" )
      FENCE_CLOSE+=( -1 )
    fi
  else
    if [[ "$l" =~ $FENCE_CLOSE_RE ]]; then
      in_fence=0
      FENCE_CLOSE[fi]=$i
      fi=$((fi + 1))
    fi
  fi
done

# ─── bloque del manifiesto (C1) ─────────────────────────────────────────────
begin_idx=-1
end_idx=-1
for i in "${!DOC_LINES[@]}"; do
  t="${DOC_LINES[$i]//[[:space:]]/}"
  if [[ "$t" == "<!--claims:begin-->" && $begin_idx -lt 0 ]]; then
    begin_idx=$i
  elif [[ "$t" == "<!--claims:end-->" && $end_idx -lt 0 ]]; then
    end_idx=$i
  fi
done

if [[ $begin_idx -lt 0 || $end_idx -lt 0 || $end_idx -le $begin_idx ]]; then
  abort_finding 1 "C1: no existe el bloque del manifiesto (<!-- claims:begin --> … <!-- claims:end -->)"
fi

CLAIMS_FENCE_RE='^```claims[[:space:]]*$'
claims_start=-1
for ((i = begin_idx + 1; i < end_idx; i++)); do
  if [[ "${DOC_LINES[$i]}" =~ $CLAIMS_FENCE_RE ]]; then
    claims_start=$i
    break
  fi
done

if [[ $claims_start -lt 0 ]]; then
  abort_finding $((begin_idx + 1)) "C1: el bloque del manifiesto no contiene un fence con info-string 'claims'"
fi

# ─── parsing del manifiesto ─────────────────────────────────────────────────
M_KIND=()
M_FILE=()
M_LINE=()
M_F4=()
M_F5=()
M_RES=()
M_AT=()
COVERED=()

is_covered() { # clave "ruta:línea"
  local key="$1" c
  for c in "${COVERED[@]+"${COVERED[@]}"}"; do
    [[ "$c" == "$key" ]] && return 0
  done
  return 1
}

i=$((claims_start + 1))
while [[ $i -lt $end_idx ]]; do
  raw="${DOC_LINES[$i]}"
  if [[ "$raw" =~ $FENCE_CLOSE_RE ]]; then
    break
  fi
  t="${raw#"${raw%%[![:space:]]*}"}"
  t="${t%"${t##*[![:space:]]}"}"
  i=$((i + 1))
  if [[ -z "$t" || "$t" == \#* ]]; then
    continue
  fi

  IFS='|' read -r -a parts <<<"$t"
  kind="${parts[0]//[[:space:]]/}"
  nfields="${#parts[@]}"

  expected=0
  case "$kind" in
    claim | impl | absent) expected=4 ;;
    shape) expected=5 ;;
    *)
      report "$DOC_REL" "$i" "C1: tipo de claim desconocido '$kind' (claim|shape|impl|absent)"
      continue
      ;;
  esac
  if [[ $nfields -ne $expected ]]; then
    report "$DOC_REL" "$i" "C1: la entrada '$kind' tiene $nfields campos y se esperaban $expected"
    continue
  fi

  file=""
  lineno=""
  f4=""
  f5=""
  empty=0
  for ((f = 1; f < nfields; f++)); do
    v="${parts[$f]}"
    v="${v#"${v%%[![:space:]]*}"}"
    v="${v%"${v##*[![:space:]]}"}"
    if [[ -z "$v" ]]; then
      empty=1
    fi
    case $f in
      1) file="$v" ;;
      2) lineno="$v" ;;
      3) f4="$v" ;;
      4) f5="$v" ;;
    esac
  done
  if [[ $empty -eq 1 ]]; then
    report "$DOC_REL" "$i" "C1: la entrada '$kind' tiene un campo vacío"
    continue
  fi

  if [[ "$kind" == "absent" ]]; then
    if [[ "$lineno" != "-" ]]; then
      report "$DOC_REL" "$i" "C1: la entrada 'absent' debe llevar '-' en el campo de línea (lleva '$lineno')"
      continue
    fi
    M_KIND+=("$kind"); M_FILE+=("$file"); M_LINE+=("$lineno")
    M_F4+=("$f4"); M_F5+=("$f5"); M_RES+=(""); M_AT+=("$i")
    continue
  fi

  if [[ ! "$lineno" =~ ^[0-9]+$ ]] || [[ "$lineno" -lt 1 ]]; then
    report "$DOC_REL" "$i" "C1: la línea citada '$lineno' no es un número de línea válido"
    continue
  fi

  res=""
  rc=0
  res="$(resolve_path "$file")" || rc=$?
  if [[ $rc -ne 0 ]]; then
    report "$DOC_REL" "$i" "C1: no se pudo resolver la ruta '$file' ($(motivo_ruta "$rc"))"
    continue
  fi

  total="$(awk 'END{print NR}' "$ROOT/$res")"
  if [[ "$lineno" -gt "$total" ]]; then
    report "$res" "$lineno" "C1: la línea citada está fuera del archivo ($res tiene $total líneas)"
    continue
  fi

  M_KIND+=("$kind"); M_FILE+=("$file"); M_LINE+=("$lineno")
  M_F4+=("$f4"); M_F5+=("$f5"); M_RES+=("$res"); M_AT+=("$i")
  COVERED+=("$res:$lineno")
done

if [[ ${#M_KIND[@]} -eq 0 ]]; then
  abort_finding $((claims_start + 1)) "C1: el manifiesto está vacío — el doc no declara ninguna afirmación verificable"
fi

# ─── C2 / C3 / C4 / C5 ───────────────────────────────────────────────────────
checked_claim=0
checked_shape=0
checked_impl=0
checked_absent=0

find_fence_for_decl() { # declaración normalizada → índice del fence
  local decl="$1" f c ln
  for f in "${!FENCE_OPEN[@]}"; do
    [[ "${FENCE_LANG[$f]}" == "claims" ]] && continue
    for ((c = FENCE_OPEN[f] + 1; c < FENCE_CLOSE[f]; c++)); do
      ln="$(norm "${DOC_LINES[$c]}")"
      if [[ "$ln" == "$decl" ]]; then
        printf '%s\n' "$f"
        return 0
      fi
    done
  done
  return 1
}

for ((e = 0; e < ${#M_KIND[@]}; e++)); do
  kind="${M_KIND[$e]}"
  res="${M_RES[$e]}"
  mfile="${M_FILE[$e]}"
  lineno="${M_LINE[$e]}"
  f4="${M_F4[$e]}"
  f5="${M_F5[$e]}"
  at="${M_AT[$e]}"

  case "$kind" in
    claim)
      checked_claim=$((checked_claim + 1))
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      trimmed="${line#"${line%%[![:space:]]*}"}"
      trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
      if [[ "$trimmed" != *"$f4"* ]]; then
        report "$res" "$lineno" "C2: la línea no contiene '$f4' (contenido: '$trimmed')"
      fi
      ;;
    shape)
      checked_shape=$((checked_shape + 1))
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      trimmed="${line#"${line%%[![:space:]]*}"}"
      trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
      decl_n="$(norm "$f4")"
      if [[ "$trimmed" != *"$decl_n"* ]]; then
        report "$res" "$lineno" "C3: la línea no declara '$f4' (contenido: '$trimmed')"
        continue
      fi
      if [[ ! "$f4" =~ (enum|struct)[[:space:]]+([A-Za-z_][A-Za-z0-9_]*) ]]; then
        report "$res" "$lineno" "C3: no se pudo deducir el tipo declarado en '$f4'"
        continue
      fi
      tkind="${BASH_REMATCH[1]}"
      tname="${BASH_REMATCH[2]}"
      actual="$(extract_members "$ROOT/$res" "$lineno" "$tkind" | sort -u)"
      expected="$(printf '%s' "$f5" | tr ',' '\n' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' | grep -v '^$' | sort -u)"
      missing="$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"))"
      extra="$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"))"
      if [[ -n "$missing" ]]; then
        report "$res" "$lineno" "C3: '$tname' no declara el(s) miembro(s) $(printf '%s' "$missing" | tr '\n' ' ')— el manifiesto los espera"
      fi
      if [[ -n "$extra" ]]; then
        report "$res" "$lineno" "C3: '$tname' declara el(s) miembro(s) $(printf '%s' "$extra" | tr '\n' ' ')— el manifiesto no los declara"
      fi
      ;;
    impl)
      checked_impl=$((checked_impl + 1))
      decl_n="$(norm "$f4")"
      line="$(sed -n "${lineno}p" "$ROOT/$res")"
      code_n="$(norm "$line")"
      if [[ "$code_n" != *"$decl_n"* ]]; then
        report "$res" "$lineno" "C4: la línea no contiene la declaración '$f4' (contenido: '$code_n')"
        continue
      fi
      fence=""
      if ! fence="$(find_fence_for_decl "$decl_n")"; then
        report "$res" "$lineno" "C4: ningún fence del doc contiene la declaración '$f4'"
        continue
      fi
      norm_file "$res"
      nf="$NF_PATH"
      for ((c = FENCE_OPEN[fence] + 1; c < FENCE_CLOSE[fence]; c++)); do
        dline="$(strip_comment "${DOC_LINES[$c]}")"
        dn="$(norm "$dline")"
        [[ -z "$dn" ]] && continue
        if ! grep -qF -- "$dn" "$nf"; then
          report "$res" "$lineno" "C4: la línea del fence '$dn' no está en $res"
        fi
      done
      ;;
    absent)
      checked_absent=$((checked_absent + 1))
      sym="$f4"
      if [[ "$mfile" == "*" ]]; then
        if [[ ! -d "$ROOT/crates" ]]; then
          report "$DOC_REL" "$at" "C5: no existe crates/ — no se puede verificar la ausencia de '$sym'"
          continue
        fi
        # El doc afirma "cero ocurrencias en `crates/`": se busca en TODO el
        # árbol, no sólo en los `.rs` — un símbolo mentioned en un fixture o en
        # un doc interno también es una ocurrencia.
        hit="$(grep -rnF -- "$sym" "$ROOT/crates" 2>/dev/null | head -n 1)"
      else
        rc=0
        rfile="$(resolve_path "$mfile")" || rc=$?
        if [[ $rc -ne 0 ]]; then
          report "$DOC_REL" "$at" "C1: no se pudo resolver la ruta '$mfile' de la entrada 'absent' ($(motivo_ruta "$rc"))"
          continue
        fi
        hit="$(grep -nF -- "$sym" "$ROOT/$rfile" 2>/dev/null | head -n 1)"
      fi
      if [[ -n "$hit" ]]; then
        where="${hit%%:*}"
        where="${where#"$ROOT"/}"
        report "$where" 1 "C5: el símbolo '$sym' aparece en el código, pero el manifiesto declara su ausencia"
      fi
      ;;
  esac
done

# ─── C6: cobertura de las citas de la prosa ─────────────────────────────────
# Una cita `archivo:línea` (o `:línea` abreviada, con el archivo como contexto)
# tiene que estar registrada en el manifiesto. Las abreviadas sólo se aceptan
# dentro de backticks (`:102`) para no confundirse con otra notación.
# Los recognizers van en comillas simples: los backticks son literales.
# shellcheck disable=SC2016
CIT_RE='`:[0-9]+(-[0-9]+)?`|[A-Za-z0-9_][A-Za-z0-9_./-]*\.(rs|md|json|toml|sh|yaml|yml)(:[0-9]+(-[0-9]+)?)?'
# shellcheck disable=SC2016
SHORT_RE='^`:([0-9]+)(-[0-9]+)?`$'
PATHLINE_RE='^(.+):([0-9]+)(-[0-9]+)?$'

check_citation() { # ruta línea fin-rango línea-del-doc
  local p="$1" ln="$2" lnend="$3" dln="$4" res rc
  rc=0
  res="$(resolve_path "$p")" || rc=$?
  if [[ $rc -ne 0 ]]; then
    report "$DOC_REL" "$dln" "C6: no se pudo resolver la ruta '$p' de una cita del doc ($(motivo_ruta "$rc"))"
    return
  fi
  if ! is_covered "$res:$ln"; then
    report "$DOC_REL" "$dln" "C6: la cita '$p:$ln' no está registrada en el manifiesto"
  fi
  if [[ -n "$lnend" && "$lnend" != "$ln" ]]; then
    if ! is_covered "$res:$lnend"; then
      report "$DOC_REL" "$dln" "C6: el extremo '$p:$lnend' del rango no está registrado en el manifiesto"
    fi
  fi
}

citations=0
ctx=""
for ((i = 0; i < ${#DOC_LINES[@]}; i++)); do
  [[ $i -ge $begin_idx && $i -le $end_idx ]] && continue
  line="${DOC_LINES[$i]}"
  [[ "$line" == *:* ]] || continue
  while IFS= read -r tok; do
    [[ -z "$tok" ]] && continue
    if [[ "$tok" =~ $SHORT_RE ]]; then
      s_start="${BASH_REMATCH[1]}"
      s_end="${BASH_REMATCH[2]}"
      if [[ -z "$ctx" ]]; then
        report "$DOC_REL" $((i + 1)) "C6: la cita abreviada ':$s_start' no tiene un archivo de contexto"
        continue
      fi
      citations=$((citations + 1))
      check_citation "$ctx" "$s_start" "$s_end" $((i + 1))
    else
      if [[ "$tok" =~ $PATHLINE_RE ]]; then
        p="${BASH_REMATCH[1]}"
        ln="${BASH_REMATCH[2]}"
        lnend="${BASH_REMATCH[3]#-}"
        ctx="$p"
        citations=$((citations + 1))
        check_citation "$p" "$ln" "$lnend" $((i + 1))
      else
        ctx="$tok"
      fi
    fi
  done < <(printf '%s\n' "$line" | grep -oE "$CIT_RE" || true)
done

# ─── C7: referencias § ──────────────────────────────────────────────────────
declare -A SEC_OK=()
declare -A ITEM_OK=()
cur_sec=""
for i in "${!DOC_LINES[@]}"; do
  l="${DOC_LINES[$i]}"
  if [[ "$l" == \#\#* ]]; then
    cur_sec=""
    if [[ "$l" =~ ^###[[:space:]]+([0-9]+)\. ]]; then
      cur_sec="${BASH_REMATCH[1]}"
      SEC_OK["$cur_sec"]=1
    fi
    continue
  fi
  # El ítem se declara como `- **(a) Texto**`: la letra va tras el paréntesis,
  # y el `**` que cierra el bold puede estar mucho más lejos.
  if [[ -n "$cur_sec" && "$l" =~ ^-[[:space:]]+\*\*\(([a-z])\) ]]; then
    ITEM_OK["${cur_sec}${BASH_REMATCH[1]}"]=1
  fi
done

SEC_RE='§[0-9]+[a-z]?(-§?[0-9]+[a-z]?)?'
SEC_BODY_RE='^([0-9]+[a-z]?)(-§?([0-9]+[a-z]?))?$'

check_sec_one() { # número-letra archivo línea token
  local nl="$1" f="$2" ln="$3" tok="$4" n l
  n="${nl%%[a-z]*}"
  l="${nl#"$n"}"
  if [[ -z "${SEC_OK[$n]:-}" ]]; then
    report "$f" "$ln" "C7: la referencia '$tok' no resuelve a ninguna sección numerada del doc (§$n)"
    return
  fi
  if [[ -n "$l" && -z "${ITEM_OK["$n$l"]:-}" ]]; then
    report "$f" "$ln" "C7: la referencia '$tok' no resuelve a ningún ítem letra de la sección §$n"
  fi
}

check_sec_token() { # token archivo línea
  local tok="$1" f="$2" ln="$3" body first second
  body="${tok#§}"
  first="$body"
  second=""
  if [[ "$body" =~ $SEC_BODY_RE ]]; then
    first="${BASH_REMATCH[1]}"
    second="${BASH_REMATCH[3]}"
  fi
  check_sec_one "$first" "$f" "$ln" "$tok"
  if [[ -n "$second" ]]; then
    check_sec_one "$second" "$f" "$ln" "$tok"
  fi
}

secs=0
for ((i = 0; i < ${#DOC_LINES[@]}; i++)); do
  [[ $i -ge $begin_idx && $i -le $end_idx ]] && continue
  line="${DOC_LINES[$i]}"
  [[ "$line" == *§* ]] || continue
  while IFS= read -r tok; do
    [[ -z "$tok" ]] && continue
    secs=$((secs + 1))
    check_sec_token "$tok" "$DOC_REL" $((i + 1))
  done < <(printf '%s\n' "$line" | sed -e "$DASH_SED" | grep -oE "$SEC_RE" || true)
done

# El código que cita el doc también tiene que resolver sus §.
while IFS= read -r cf; do
  [[ -n "$cf" ]] || continue
  rel="${cf#"$ROOT"/}"
  while IFS= read -r hit; do
    [[ -n "$hit" ]] || continue
    cline="${hit%%:*}"
    ctext="${hit#*:}"
    while IFS= read -r tok; do
      [[ -z "$tok" ]] && continue
      secs=$((secs + 1))
      check_sec_token "$tok" "$rel" "$cline"
    done < <(printf '%s\n' "$ctext" | sed -e "$DASH_SED" | grep -oE "$SEC_RE" || true)
  done < <(grep -nE "$SEC_RE" "$cf" | sed -e "$DASH_SED" || true)
done < <(grep -rlF --include='*.rs' "ai-providers-design.md" "$ROOT/crates" 2>/dev/null | sort)

# ─── resultado ───────────────────────────────────────────────────────────────
if [[ $FINDINGS -gt 0 ]]; then
  printf '::error::guard de drift doc↔código: %d hallazgo(s) en %s\n' "$FINDINGS" "$DOC_REL" >&2
  if [[ $ADVISORY -eq 1 ]]; then
    exit 0
  fi
  exit 1
fi

printf 'OK: %s — %d claim, %d shape, %d impl, %d absent; %d cita(s) de prosa cubiertas; %d referencia(s) § resueltas.\n' \
  "$DOC_REL" "$checked_claim" "$checked_shape" "$checked_impl" "$checked_absent" "$citations" "$secs"
exit 0
