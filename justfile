# justfile — webfang
# Complementa a bacon (inner loop). Esto es para tareas manuales (outer loop).

# -- Verificación --

default: check

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings -W clippy::pedantic -A clippy::uninlined_format_args -A clippy::case_sensitive_file_extension_comparisons -A clippy::doc_markdown -A clippy::needless_raw_string_hashes -A clippy::missing_errors_doc -A clippy::must_use_candidate -A clippy::redundant_closure -A clippy::match_same_arms -A clippy::items_after_statements -A clippy::missing_panics_doc -A clippy::module_name_repetitions -A clippy::too_many_lines -A clippy::too_many_arguments -A clippy::struct_excessive_bools -A clippy::similar_names -A clippy::cast_precision_loss -A clippy::cast_possible_truncation -A clippy::cast_sign_loss -A clippy::default_trait_access -A clippy::wildcard_imports -A clippy::enum_glob_use -A clippy::explicit_iter_loop -A clippy::map_unwrap_or -A clippy::if_not_else -A clippy::manual_let_else -A clippy::needless_continue -A clippy::needless_pass_by_value -A clippy::return_self_not_must_use -A clippy::single_match_else -A clippy::trivially_copy_pass_by_ref -A clippy::struct_field_names -A clippy::assigning_clones -A clippy::float_cmp -A clippy::cast_lossless -A clippy::unused_async -A clippy::format_push_string -A clippy::used_underscore_binding -A clippy::unnested_or_patterns -A clippy::single_char_pattern -A clippy::redundant_else -A clippy::needless_late_init -A clippy::explicit_iter_loop -A clippy::unused_self -A clippy::redundant_closure_for_method_calls -A clippy::used_underscore_items -A clippy::cast_possible_wrap -A clippy::non_std_lazy_statics -A clippy::stable_sort_primitive -A clippy::unnecessary_debug_formatting -A clippy::unnecessary_semicolon -A clippy::explicit_into_iter_loop -A clippy::manual_assert -A duplicate_macro_attributes -A clippy::unreadable_literal

check-fast:
    cargo check

# -- Tests --

# Refresca el knowledge graph de GitNexus (el agente lo necesita para saber qué cambió)
analyze:
    gitnexus analyze || echo "GitNexus ya estaba actualizado"

# Tests durante desarrollo → SOLO lo afectado (8-25 segundos típico)
test-dev:
    @echo "🚀 Ejecutando tests solo de cambios (GitNexus impact analysis)..."
    cargo nextest run \
        --no-fail-fast \
        --profile dev

# Tests completos para agentes (gate final)
test:
    @echo "🧪 Ejecutando suite completa de tests..."
    cargo nextest run \
        --no-fail-fast \
        --profile agent

# Tests con filtro preciso (GitNexus lo usa cuando sabe exactamente qué módulos cambiaron)
test-filter filter:
    @echo "🎯 Ejecutando tests filtrados: {{filter}}"
    cargo nextest run \
        --no-fail-fast \
        --profile dev \
        -E '{{filter}}'

test-ai:
    cargo nextest run --profile agent --features ai

# -- Auditoría --

audit:
    cargo audit
    cargo deny check
    cargo machete

# -- Coverage --

cov:
    cargo llvm-cov --html --output-dir coverage-llvm

# -- Format --

fmt:
    cargo fmt

# -- Build --

build-release:
    cargo build --release

# -- CI --

# -- Maintenance --

fix-typos:
    typos -w

# -- Setup --

setup:
    @echo "Verificando herramientas..."
    @which cargo-nextest || (echo "Falta: cargo binstall cargo-nextest"; exit 1)
    @which just || (echo "Falta: cargo binstall just"; exit 1)
    @which cargo-machete || (echo "Falta: cargo binstall cargo-machete"; exit 1)
    @which cargo-audit || (echo "Falta: cargo binstall cargo-audit"; exit 1)
    @which cargo-deny || (echo "Falta: cargo binstall cargo-deny"; exit 1)
    @which typos || (echo "Falta: cargo binstall typos-cli"; exit 1)
    @which sccache || (echo "Falta: sccache"; exit 1)
    @which mold || (echo "Falta: mold"; exit 1)
    @echo "Setup completo — todas las herramientas verificadas"

# =============================================
# WATCH MODE (el que más vas a usar con el agente)
# =============================================

# Modo desarrollo automático: vigila cambios y ejecuta solo tests afectados
watch-dev:
    @echo "👀 Modo watch activado - GitNexus + tests inteligentes"
    @echo "   (Detén con Ctrl+C)"
    cargo watch --clear \
        --watch . \
        --ignore "target/" \
        --ignore "logs/" \
        --shell "just test-dev-with-impact"

# Versión inteligente que usa GitNexus para filtrar exactamente qué cambió
test-dev-with-impact:
    @echo "🚀 Ejecutando tests de desarrollo (con GitNexus awareness)..."
    @echo "🎯 Ejecutando tests optimizados (excluyendo AI integration)..."
    cargo nextest run \
        --profile dev \
        --no-fail-fast \
        -E "not test(ai_integration)"

# =============================================
# FUZZING (seguridad — detecta vulns y panics)
# =============================================

# Smoke test: ejecuta cada target por 5 segundos
fuzz-smoke:
    @echo "🔍 Smoke testing all fuzz targets (5s each)..."
    @for target in fuzz_html_cleaner fuzz_convert_to_markdown fuzz_readability_parse fuzz_extract_text fuzz_parse_sitemap fuzz_extract_links fuzz_url_validation fuzz_url_normalization fuzz_waf_detection fuzz_compression_detect fuzz_wikilinks fuzz_syntax_highlight fuzz_slug_from_url fuzz_extract_assets; do \
        echo "  → $$target"; \
        cargo +nightly fuzz run $$target -- -max_total_time=5 2>&1 | tail -1; \
    done
    @echo "✅ All targets smoke-tested"

# Fuzz un target específico por N segundos (default: 60)
fuzz-target target seconds="60":
    @echo "🎯 Fuzzing {{target}} for {{seconds}}s..."
    cargo +nightly fuzz run {{target}} -- -max_total_time={{seconds}}

# Fuzz todos los targets por 10 minutos (para CI nocturno)
fuzz-full:
    @echo "🔬 Running full fuzz suite (10 min per target)..."
    @for target in fuzz_html_cleaner fuzz_convert_to_markdown fuzz_readability_parse fuzz_extract_text fuzz_parse_sitemap fuzz_extract_links fuzz_url_validation fuzz_url_normalization fuzz_waf_detection fuzz_compression_detect fuzz_wikilinks fuzz_syntax_highlight fuzz_slug_from_url fuzz_extract_assets; do \
        echo "=== $$target ==="; \
        cargo +nightly fuzz run $$target -- -max_total_time=600 || echo "FAILED: $$target"; \
    done
    @echo "✅ Full fuzz suite complete"

# Coverage report: qué tan bien cubre el fuzzing el código
fuzz-coverage target:
    @echo "📊 Generating coverage for {{target}}..."
    cargo +nightly fuzz coverage {{target}}
    @echo "✅ Coverage report in fuzz/coverage/{{target}}/"

# Limpiar artifacts de fuzzing (corpus, crashes, coverage)
fuzz-clean:
    @echo "🧹 Cleaning fuzz artifacts..."
    rm -rf fuzz/artifacts/*/crash-* fuzz/artifacts/*/oom-* fuzz/artifacts/*/leak-*
    @echo "✅ Fuzz artifacts cleaned"

# =============================================
# CI / GATE FINAL (antes de commit / PR)
# =============================================

# Validación completa: fmt + clippy + tests completos
test-ci:
    @echo "🔥 Iniciando GATE FINAL (CI)..."
    @echo "1/4 → Formateando código..."
    cargo fmt --all -- --check
    @echo "2/4 → Ejecutando Clippy (strict)..."
    cargo clippy --all-targets --all-features -- -D warnings
    @echo "3/4 → Refrescando GitNexus..."
    gitnexus analyze || echo "GitNexus ya estaba actualizado"
    @echo "4/4 → Ejecutando suite completa de tests..."
    cargo nextest run \
        --profile ci \
        --no-fail-fast
    @echo "✅ CI PASADO - Listo para commit/push/PR"

# Versión rápida para cuando solo quieres tests + clippy (sin fmt)
test-ci-quick:
    @echo "🔥 CI rápido (clippy + tests)..."
    cargo clippy --all-targets --all-features -- -D warnings
    gitnexus analyze || echo "GitNexus ya estaba actualizado"
    cargo nextest run --profile ci --no-fail-fast
    @echo "✅ CI rápido pasado"
