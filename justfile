# justfile — rust_scraper
# Complementa a bacon (inner loop). Esto es para tareas manuales (outer loop).

# -- Verificación --

default: check

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings -W clippy::pedantic -A clippy::uninlined_format_args -A clippy::case_sensitive_file_extension_comparisons -A clippy::doc_markdown -A clippy::needless_raw_string_hashes -A clippy::missing_errors_doc -A clippy::must_use_candidate -A clippy::redundant_closure -A clippy::match_same_arms -A clippy::items_after_statements -A clippy::missing_panics_doc -A clippy::module_name_repetitions -A clippy::too_many_lines -A clippy::too_many_arguments -A clippy::struct_excessive_bools -A clippy::similar_names -A clippy::cast_precision_loss -A clippy::cast_possible_truncation -A clippy::cast_sign_loss -A clippy::default_trait_access -A clippy::wildcard_imports -A clippy::enum_glob_use -A clippy::explicit_iter_loop -A clippy::map_unwrap_or -A clippy::if_not_else -A clippy::manual_let_else -A clippy::needless_continue -A clippy::needless_pass_by_value -A clippy::return_self_not_must_use -A clippy::single_match_else -A clippy::trivially_copy_pass_by_ref -A clippy::struct_field_names -A clippy::assigning_clones -A clippy::float_cmp -A clippy::cast_lossless -A clippy::unused_async -A clippy::format_push_string -A clippy::used_underscore_binding -A clippy::unnested_or_patterns -A clippy::single_char_pattern -A clippy::redundant_else -A clippy::needless_late_init -A clippy::explicit_iter_loop -A clippy::unused_self -A clippy::redundant_closure_for_method_calls -A clippy::used_underscore_items -A clippy::cast_possible_wrap -A clippy::non_std_lazy_statics -A clippy::stable_sort_primitive -A clippy::unnecessary_debug_formatting -A clippy::unnecessary_semicolon -A clippy::explicit_into_iter_loop -A clippy::manual_assert -A duplicate_macro_attributes -A clippy::unreadable_literal

check-fast:
    cargo check

# -- Tests --

test:
    cargo nextest run --test-threads 2

test-ai:
    cargo nextest run --test-threads 2 --features ai

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

test-ci:
    cargo nextest run --profile ci

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
