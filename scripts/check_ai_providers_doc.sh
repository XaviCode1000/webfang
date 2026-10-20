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
# Uso: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]
# Salida: 0 limpio · 1 drift (fail-closed). `--advisory` imprime los mismos
# hallazgos pero sale con 0.
# ============================================================================
set -uo pipefail

DOC_REL="docs/architecture/ai-providers-design.md"
CORE_SRC_REL="crates/webfang_core/src"

usage() {
  cat >&2 <<'EOF'
Uso: scripts/check_ai_providers_doc.sh [--root DIR] [--advisory]

  --root DIR   raíz del repo (default: la raíz derivada de BASH_SOURCE)
  --advisory   imprime los mismos hallazgos pero sale con 0

Salida: 0 limpio · 1 drift (fail-closed)
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

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT

FINDINGS=0
report() { # file line message
  printf '::error file=%s,line=%s::%s\n' "$1" "$2" "$3" >&2
  FINDINGS=$((FINDINGS + 1))
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

# ─── resolución de rutas (S5) ────────────────────────────────────────────────
resolve_path() { # path → imprime la ruta relativa al repo; 1 si no resuelve
  local p="${1#./}" base count matches
  if [[ -f "$ROOT/$p" ]]; then
    printf '%s\n' "$p"
    return 0
  fi
  if [[ -f "$ROOT/$CORE_SRC_REL/$p" ]]; then
    printf '%s\n' "$CORE_SRC_REL/$p"
    return 0
  fi
  base="${p##*/}"
  matches="$(find "$ROOT/crates" -type f -name "$base" 2>/dev/null | sort)"
  count="$(printf '%s' "$matches" | grep -c . || true)"
  if [[ "$count" == "1" ]]; then
    printf '%s\n' "${matches#"$ROOT"/}"
    return 0
  fi
  return 1
}

# ─── caché de archivos normalizados ─────────────────────────────────────────
NORM_KEYS=()
NORM_VALS=()
norm_file() { # ruta relativa → imprime el path del temporal con las líneas normalizadas
  local f="$1" i out
  for i in "${!NORM_KEYS[@]}"; do
    if [[ "${NORM_KEYS[$i]}" == "$f" ]]; then
      printf '%s\n' "${NORM_VALS[$i]}"
      return 0
    fi
  done
  out="$TMPD/$(printf '%s' "$f" | tr '/.' '__')"
  norm_stream <"$ROOT/$f" >"$out"
  NORM_KEYS+=("$f")
  NORM_VALS+=("$out")
  printf '%s\n' "$out"
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
  report "$DOC_REL" 1 "no existe el doc normativo $DOC_REL bajo la raíz $ROOT"
  exit 1
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
  report "$DOC_REL" 1 "C1: no existe el bloque del manifiesto (<!-- claims:begin --> … <!-- claims:end -->)"
  exit 1
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
  report "$DOC_REL" $((begin_idx + 1)) "C1: el bloque del manifiesto no contiene un fence con info-string 'claims'"
  exit 1
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
  if ! res="$(resolve_path "$file")"; then
    report "$DOC_REL" "$i" "C1: no se pudo resolver la ruta '$file' (no existe, o el basename es ambiguo bajo crates/)"
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
  report "$DOC_REL" $((claims_start + 1)) "C1: el manifiesto está vacío — el doc no declara ninguna afirmación verificable"
  exit 1
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
      nf="$(norm_file "$res")"
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
        rfile="$(resolve_path "$mfile")" || {
          report "$DOC_REL" "$at" "C1: no se pudo resolver la ruta '$mfile' de la entrada 'absent'"
          continue
        }
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
  local p="$1" ln="$2" lnend="$3" dln="$4" res
  if ! res="$(resolve_path "$p")"; then
    report "$DOC_REL" "$dln" "C6: no se pudo resolver la ruta '$p' de una cita del doc (no existe o el basename es ambiguo)"
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
