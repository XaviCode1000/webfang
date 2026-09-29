#!/usr/bin/env bash
# seed_recipe.sh — the SINGLE source of truth for a seed's build contract.
#
# Sourced by seed_compat_key.sh, seed_publish.sh and seed_target.sh. It exists
# because the previous arrangement had one defect with several faces:
#
#     KEY  was computed from the CALLER'S AMBIENT ENVIRONMENT
#     BUILD was executed under a DIFFERENT environment
#
# and the two were free to disagree. Measured divergences, all of which shipped:
#
#     --target aarch64-…   key said aarch64, build ran `cargo build` (no target)
#     --profile release    key said release,    build ran the dev profile
#     --features ai        key said ai,         build had no --features at all
#     -- --release …       reached cargo,       key did not change
#     RUSTUP_TOOLCHAIN     key hashed nightly,  build unset it and used another
#     CARGO_INCREMENTAL=1  key said on,         build forced 0
#
# The golden rule this module exists to make true:
#
#     If two builds can produce different artifacts, they must not produce the
#     same SeedCompatibilityKey. And whoever holds a key must build exactly the
#     contract that key describes.
#
# The way to get that is structural, not procedural: the key is a pure function
# of a RECIPE, and the build runs the same RECIPE. There is no code path that
# reads a value for one of them and not the other, because there is only one
# place a value can come from.
#
# Every contract-relevant input is named here, resolved ONCE, and then used
# verbatim by both sides. Nothing downstream re-reads the environment.

# Bump when the set of contract fields changes. A key_schema change re-keys every
# seed, which is correct: a seed published under an older contract is not a seed
# under this one.
SEED_RECIPE_SCHEMA=1

# The module's own directory. It cannot borrow a SCRIPT_DIR from whichever
# script sourced it — that is undefined here, and a helper that silently fails
# because of it produced an empty digest for every tree.
SEED_RECIPE_SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Environment variables that are deliberately NOT part of the build, removed on
# both sides. This is a POLICY, and it is hashed into the key as one, so that
# changing the policy re-keys every seed instead of silently producing artifacts
# under a key that claims the old policy.
#
# RUSTC_WRAPPER: sccache is a CACHE, not a compiler. Measured that it does not
# dedupe across target dirs anyway (that is why this whole mechanism exists), so
# removing it costs nothing here and removes a variable that could otherwise
# change how rustc is invoked. SCCACHE_DIR/SCCACHE_BASEDIRS go with it.
SEED_RECIPE_STRIPPED_ENV="RUSTC_WRAPPER SCCACHE_DIR SCCACHE_BASEDIRS"
SEED_RECIPE_WRAPPER_POLICY="prescribed-empty"

# --- resolution -------------------------------------------------------------

# Resolve the toolchain ONCE and PIN it, so the key and the build cannot be
# looking at different ones.
#
# This is the direct fix for the RUSTUP_TOOLCHAIN divergence: the old publisher
# unset RUSTUP_TOOLCHAIN for the build, which meant the build used whatever
# rust-toolchain.toml said while the key had hashed the ambient override. Here
# the active toolchain is captured, and the build is given that exact one
# explicitly rather than being left to re-resolve it.
seed_recipe_resolve_toolchain() {
  local repo="$1"
  SEED_RECIPE_TOOLCHAIN=""
  if command -v rustup >/dev/null 2>&1; then
    SEED_RECIPE_TOOLCHAIN="$(
      cd "$repo" 2>/dev/null && env -u RUSTUP_TOOLCHAIN rustup show active-toolchain 2>/dev/null | cut -d' ' -f1
    )"
  fi
}

# The digest of the FULL configuration surface cargo reads, not just the repo's
# own file. Cargo resolves config from `$CARGO_HOME/config.toml` and from every
# `.cargo/config.*` walking up from the working directory to the root, and any of
# them can set `build.rustflags`, `build.incremental`, `target`, `rustc-wrapper`
# and more.
#
# Hashing only the repo's file — which is what this did before — means a change
# to $CARGO_HOME changes the build and leaves the key untouched, and a seed gets
# reused against a contract it was never built for.
#
# This is deliberately a CONSERVATIVE SUPERSET, not a precedence parser: a change
# anywhere in the chain moves the key, even if cargo would have shadowed it. That
# asymmetry is the point. A false positive costs one cold build; a false negative
# costs correctness. Modelling cargo's full precedence chain to be cleverer than
# that would buy nothing and risk being wrong in the expensive direction.
seed_recipe_config_digest() {
  local repo="$1" home="${CARGO_HOME:-$HOME/.cargo}" out="" f dir
  # CONTENT, not paths.
  #
  # The first version of this recorded "F <absolute path> <hash>" for every
  # config file it found, and that made the key depend on WHERE the checkout is:
  # the same commit built from two directories hashed differently, purely because
  # the ancestor walk visited different absolute paths. That breaks the one
  # property the seed exists for — crossing worktrees — and it is exactly the
  # kind of defect that a test run from a single checkout cannot see.
  #
  # Only the bytes matter: two config files with identical content have identical
  # effect on the build regardless of where they live, so they must not produce
  # different keys. Absent files contribute nothing, which is right for the same
  # reason — no file, no effect.
  for f in "$repo/.cargo/config.toml" "$repo/.cargo/config" \
           "$home/config.toml" "$home/config"; do
    if [ -f "$f" ]; then
      out+="F $(sha256sum "$f" | cut -d' ' -f1)"$'\n'
    fi
  done
  local dir="$repo"
  while [ "$dir" != "/" ] && [ -n "$dir" ]; do
    for f in "$dir/.cargo/config.toml" "$dir/.cargo/config"; do
      if [ -f "$f" ]; then
        out+="F $(sha256sum "$f" | cut -d' ' -f1)"$'\n'
      fi
    done
    dir="$(dirname "$dir")"
  done
  printf 'sha256:%s' "$(printf '%s' "$out" | sha256sum | cut -d' ' -f1)"
}

# build-dir refusal. Shared so the producer and the consumer cannot drift apart —
# which they had: the consumer had this check and the producer did not, so a seed
# built under a build-dir configuration could enter the store under a key that
# looked valid to a consumer without one.
#
# Refusing rather than keying is a policy decision, not a limitation. Cargo keeps
# build-script output in build-dir, separately from target-dir, and that is
# exactly where the bulk of what a seed saves lives; its layout is an internal
# detail. Hashing it would manufacture a compatibility claim out of a gap in
# what can be reasoned about. It has been stable since Rust 1.91, so on an older
# toolchain the flag is inert — which is exactly why refusing it is a decision
# rather than a workaround.
seed_recipe_has_build_dir() {
  local repo="$1" home="${CARGO_HOME:-$HOME/.cargo}" f dir
  if [ -n "${CARGO_BUILD_BUILD_DIR:-}" ]; then return 0; fi
  for f in "$repo/.cargo/config.toml" "$repo/.cargo/config" "$home/config.toml" "$home/config"; do
    if [ -f "$f" ] && grep -q 'build-dir' "$f" 2>/dev/null; then return 0; fi
  done
  dir="$repo"
  while [ "$dir" != "/" ] && [ -n "$dir" ]; do
    for f in "$dir/.cargo/config.toml" "$dir/.cargo/config"; do
      if [ -f "$f" ] && grep -q 'build-dir' "$f" 2>/dev/null; then return 0; fi
    done
    dir="$(dirname "$dir")"
  done
  return 1
}

# The WORKSPACE BUILD CONTRACT: the parts of the manifests that decide what gets
# compiled, as distinct from Cargo.lock's resolved graph.
#
# Cargo.lock is necessary and not sufficient. Two changes move the artifacts while
# leaving the lock byte-identical, both verified:
#
#     [profile.dev] opt-level / lto changed in the root Cargo.toml
#     a dependency's features, optionality or target condition changed
#
# In both cases the key was identical, so a seed built under one profile was
# offered to a consumer compiling under another. Dependency features are resolved
# from the MANIFESTS: the lock records which versions were chosen, not which
# features they were built with.
#
# Deliberately NOT a hash of Cargo.toml. That would re-key on every commit that
# touches a comment, a version string or a dev-dependency, destroying reuse across
# the whole point of the seed. This digests only the declarative inputs that can
# change a reusable artifact.
#
# Two sources, because neither is complete:
#   - `cargo metadata --no-deps` for the dependency graph, features, optional and
#     target-conditional edges, the workspace members and the resolver. Fields
#     that are not part of the declaration are dropped: the format is versioned,
#     and some of its internal representations are explicitly not a stable API.
#   - the root Cargo.toml's own tables (profile.*, profile.*.package.*,
#     build-override, workspace.resolver), read with a real TOML parser, because
#     cargo metadata exposes no profiles at all.
seed_recipe_workspace_contract() {
  local repo="$1" meta out
  # NOTE: these return, they do not exit. This function is called inside a
  # command substitution, and `exit` there terminates only the subshell — the
  # caller carries on with an EMPTY digest and every tree looks compatible with
  # every other. That was the first version, and it failed open while claiming to
  # fail loud, which is worse than not checking at all.
  meta="$( cd "$repo" && cargo metadata --format-version 1 --no-deps --offline 2>/dev/null )" \
    || { echo "seed_recipe: cargo metadata failed for $repo — cannot compute the" >&2
         echo "  workspace build contract, and hashing an EMPTY one would make every" >&2
         echo "  tree look compatible with every other." >&2
         return 1; }
  printf '%s' "$meta" | jq -e -S -c '{
            # By NAME, not by package id. `workspace_members` is a list of id
            # strings, and for a path dependency the id IS the absolute path —
            # "path+file:///…/crates/webfang_core#2.4.1". Hashing it made the key
            # depend on where the checkout is, which is the same location
            # dependence that had to be removed from the config digest, arriving
            # through a completely different field. An id is bookkeeping for
            # cargo; a name is the thing a human reasons about.
            workspace_members: [ .workspace_members[] as $id
                                 | .packages[] | select(.id == $id) | .name ] | sort,
            workspace_default_members: [ (.workspace_default_members // [])[] as $id
                                 | .packages[] | select(.id == $id) | .name ] | sort,
            packages: [ .packages[] | {
              name,
              dependencies: [ .dependencies[] | {
                name, req, kind, optional,
                features: (.features | sort),
                target, rename, registry
              } ] | sort_by(.name, .kind, .target // ""),
              features: (.features | to_entries | map({ (.key): (.value | sort) }) | add // {})
            } ] | sort_by(.name)
          }' >/dev/null 2>&1 \
    || { echo "seed_recipe: could not normalise cargo metadata for $repo" >&2; return 1; }

  out="$(python3 "$SEED_RECIPE_SELF_DIR/seed_workspace_contract.py" "$repo/Cargo.toml")" \
    || { echo "seed_recipe: could not read the profile tables of $repo/Cargo.toml" >&2; return 1; }
  case "$out" in
    *unreadable*|*unavailable*|*absent*)
      echo "seed_recipe: $repo/Cargo.toml is $out — refusing to publish or consume" >&2
      echo "  a seed whose profile contract is unknown is not a seed." >&2
      return 1 ;;
  esac

  {
    printf '%s' "$meta" | jq -S -c '{
            # By NAME, not by package id. `workspace_members` is a list of id
            # strings, and for a path dependency the id IS the absolute path —
            # "path+file:///…/crates/webfang_core#2.4.1". Hashing it made the key
            # depend on where the checkout is, which is the same location
            # dependence that had to be removed from the config digest, arriving
            # through a completely different field. An id is bookkeeping for
            # cargo; a name is the thing a human reasons about.
            workspace_members: [ .workspace_members[] as $id
                                 | .packages[] | select(.id == $id) | .name ] | sort,
            workspace_default_members: [ (.workspace_default_members // [])[] as $id
                                 | .packages[] | select(.id == $id) | .name ] | sort,
            packages: [ .packages[] | {
              name,
              dependencies: [ .dependencies[] | {
                name, req, kind, optional,
                features: (.features | sort),
                target, rename, registry
              } ] | sort_by(.name, .kind, .target // ""),
              features: (.features | to_entries | map({ (.key): (.value | sort) }) | add // {})
            } ] | sort_by(.name)
          }'
    printf '\n--- profiles ---\n%s\n' "$out"
  } | sha256sum | cut -d' ' -f1 | sed 's/^/sha256:/'
}

# config `include` refusal.
#
# The digest reads the files cargo reads, but cargo can be told to read MORE of
# them: an `include` in a config file pulls in another file whose content is not
# in this digest. Changing the included file changes the effective configuration
# and leaves every hash here untouched.
#
#     .cargo/config.toml      include = "../shared/cargo-policy.toml"
#     shared/cargo-policy.toml [profile.dev] …
#
# `include` was stabilised in Cargo 1.93, so on the 1.88 this repository builds
# with today it cannot bite. Refusing anyway, for the same reason build-dir is
# refused: the policy has to survive the next toolchain bump, and a config key
# whose meaning changes with the channel is not something to leave in place to be
# discovered later. The alternative — resolving includes recursively — needs a real
# config parser with cargo's exact precedence, and a wrong answer here is silent.
seed_recipe_has_config_include() {
  local repo="$1" home="${CARGO_HOME:-$HOME/.cargo}" f dir
  for f in "$repo/.cargo/config.toml" "$repo/.cargo/config" "$home/config.toml" "$home/config"; do
    if [ -f "$f" ] && grep -qE '^[[:space:]]*include[[:space:]]*=' "$f" 2>/dev/null; then return 0; fi
  done
  dir="$repo"
  while [ "$dir" != "/" ] && [ -n "$dir" ]; do
    for f in "$dir/.cargo/config.toml" "$dir/.cargo/config"; do
      if [ -f "$f" ] && grep -qE '^[[:space:]]*include[[:space:]]*=' "$f" 2>/dev/null; then return 0; fi
    done
    dir="$(dirname "$dir")"
  done
  return 1
}

# --- the recipe -------------------------------------------------------------
#
# Populated by seed_recipe_parse(). Every field is resolved once, from an
# explicit source, and consumed verbatim by both the key and the build.

# Normalise CLI options into the recipe. Unknown or contract-affecting options
# are REJECTED rather than ignored.
#
# There is deliberately no passthrough for arbitrary cargo arguments. The old
# `--` escape hatch let `--release` and `--all-features` reach cargo without
# reaching the key, which is the clearest possible statement that the key did not
# describe the build. Anything that changes which units get compiled has to be a
# named recipe field; there is no second, unkeyed door.
seed_recipe_parse() {
  local repo="$1"; shift
  SEED_RECIPE_FEATURES=""
  SEED_RECIPE_PROFILE="dev"
  SEED_RECIPE_TARGET=""
  SEED_RECIPE_INCREMENTAL="0"
  SEED_RECIPE_SEEDS_ROOT="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}"
  SEED_RECIPE_TEST_TX=""
  SEED_RECIPE_FAIL_AT=""
  SEED_RECIPE_FORCE=0
  # RUSTFLAGS is captured ONCE here, from the ambient value, and then both sides
  # use exactly this string. Previously the build re-read ${RUSTFLAGS:-} at
  # invocation time while the key had read it at hash time.
  SEED_RECIPE_RUSTFLAGS="${RUSTFLAGS:-}"

  while [ $# -gt 0 ]; do
    case "$1" in
      --features) SEED_RECIPE_FEATURES="${2:-}"; shift 2 ;;
      --profile)  SEED_RECIPE_PROFILE="${2:-}";  shift 2 ;;
      --target)   SEED_RECIPE_TARGET="${2:-}";   shift 2 ;;
      --seeds-root) SEED_RECIPE_SEEDS_ROOT="${2:-}"; shift 2 ;;
      --force)    SEED_RECIPE_FORCE=1; shift ;;
      --test-transaction)
        SEED_RECIPE_TEST_TX="${2:-}"; shift 2
        if [ "${1:-}" = "--fail-at" ]; then SEED_RECIPE_FAIL_AT="${2:-}"; shift 2; fi ;;
      --)
        echo "seed_recipe: refusing arbitrary cargo arguments after '--'." >&2
        echo "  Every argument that changes which units cargo compiles has to be" >&2
        echo "  part of the recipe, or it changes the build without changing the" >&2
        echo "  SeedCompatibilityKey that describes it." >&2
        echo "  Use --features / --profile / --target, or extend the recipe." >&2
        exit 1 ;;
      *) echo "seed_recipe: unknown argument '$1'" >&2; exit 1 ;;
    esac
  done

  seed_recipe_resolve_toolchain "$repo"
  SEED_RECIPE_CONFIG_DIGEST="$(seed_recipe_config_digest "$repo")"
  SEED_RECIPE_WORKSPACE_DIGEST="$(seed_recipe_workspace_contract "$repo")" \
    || { echo "seed_recipe: refusing to proceed without a workspace build contract" >&2; exit 1; }
  SEED_RECIPE_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  # Canonicalise the feature list once, so "ai,mcp" and "mcp,ai" cannot become
  # two seeds, and so the string handed to cargo is the same one that was keyed.
  SEED_RECIPE_FEATURES_CANON="$(
    printf '%s' "$SEED_RECIPE_FEATURES" | tr ',' '\n' | sed '/^$/d' | LC_ALL=C sort | paste -sd, -
  )"
  SEED_RECIPE_KEY=""
}

# The full argument list for seed_compat_key.sh, derived from the recipe.
#
# One list, used by the producer, the consumer and the tests. It was duplicated
# in three places while this was being written, which is precisely the kind of
# drift that produced the original bug: two callers agreeing about what the key
# means until one of them changed.
seed_recipe_key_args() {
  printf '%s\n' \
    --features "$SEED_RECIPE_FEATURES_CANON" \
    --profile "$SEED_RECIPE_PROFILE" \
    --recipe-schema "$SEED_RECIPE_SCHEMA" \
    --wrapper-policy "$SEED_RECIPE_WRAPPER_POLICY" \
    --toolchain-id "$SEED_RECIPE_TOOLCHAIN" \
    --config-digest "$SEED_RECIPE_CONFIG_DIGEST" \
    --workspace-digest "$SEED_RECIPE_WORKSPACE_DIGEST"
  [ -n "$SEED_RECIPE_TARGET" ] && printf -- '--target\n%s\n' "$SEED_RECIPE_TARGET"
  return 0
}

# The key, computed from the recipe and from nothing else.
seed_recipe_compute_key() {
  local repo="$1" args
  mapfile -t args < <(seed_recipe_key_args)
  SEED_RECIPE_KEY="$(
    RUSTUP_TOOLCHAIN="$SEED_RECIPE_TOOLCHAIN" \
    RUSTFLAGS="$SEED_RECIPE_RUSTFLAGS" \
    CARGO_INCREMENTAL="$SEED_RECIPE_INCREMENTAL" \
    bash "$SEED_RECIPE_SELF_DIR/seed_compat_key.sh" "${args[@]}"
  )"
}

# The cargo invocation, from the same recipe.
#
# Every field that the key encodes appears here, and every field that appears
# here is in the key. That correspondence is the whole point: it is what makes
# "the key describes the build" a structural property rather than a promise.
seed_recipe_cargo_args() {
  printf '%s\n' \
    build --workspace --offline \
    ${SEED_RECIPE_PROFILE:+--profile "$SEED_RECIPE_PROFILE"} \
    ${SEED_RECIPE_TARGET:+--target "$SEED_RECIPE_TARGET"} \
    ${SEED_RECIPE_FEATURES_CANON:+--features "$SEED_RECIPE_FEATURES_CANON"}
}

# The environment the build runs under.
#
# A CLOSED policy, not an allowlist that grows over time. An allowlist is a list
# of the variables somebody thought of; cargo has a whole namespace of them, and
# its config variables take precedence over the TOML files, so anything inherited
# silently outranks the configuration the key was computed from. Measured with
# seven of them set, and the key unchanged in every case:
#
#     CARGO_BUILD_RUSTC=/opt/otro/rustc        key: unchanged
#     CARGO_BUILD_RUSTFLAGS=-C debuginfo=0     key: unchanged
#     CARGO_BUILD_TARGET=aarch64-...           key: unchanged
#     CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER=/w  key: unchanged
#     CARGO_PROFILE_DEV_OPT_LEVEL=3            key: unchanged
#     CARGO_PROFILE_DEV_DEBUG=1                key: unchanged
#     CARGO_PROFILE_DEV_LTO=thin               key: unchanged
#
# Every one of those reached cargo while the key said the same thing, which is the
# whole failure this mechanism exists to prevent, arriving through a variable
# nobody named.
#
# So: every CARGO_* and RUST* in the ambient environment is removed, and the
# recipe then sets exactly what it means to set. CARGO_HOME is resolved once and
# pinned, because it decides where cargo reads configuration from and is part of
# the config surface the key digests.
seed_recipe_build_env() {
  local target_dir="$1"
  local -a env_args=()
  local name
  while IFS= read -r name; do
    case "$name" in
      CARGO_HOME) continue ;;   # pinned explicitly below, not simply dropped
    esac
    env_args+=(-u "$name")
  done < <(compgen -v | grep -E '^(CARGO_|RUST)')

  env_args+=(
    CARGO_HOME="$SEED_RECIPE_CARGO_HOME"
    CARGO_TARGET_DIR="$target_dir"
    CARGO_INCREMENTAL="$SEED_RECIPE_INCREMENTAL"
    RUSTFLAGS="$SEED_RECIPE_RUSTFLAGS"
  )
  [ -n "$SEED_RECIPE_TOOLCHAIN" ] && env_args+=(RUSTUP_TOOLCHAIN="$SEED_RECIPE_TOOLCHAIN")
  [ -n "${RUSTUP_HOME:-}" ]      && env_args+=(RUSTUP_HOME="$RUSTUP_HOME")
  printf '%s\n' "${env_args[@]}"
}
