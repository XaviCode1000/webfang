# Rust Code Review Rules

You are reviewing Rust code for a web scraper project. Apply these rules strictly:

## CRITICAL - Ownership & Borrowing

- **own-borrow-over-clone**: Prefer `&T` borrowing over `.clone()`. If you see unnecessary `.clone()`, flag it.
- **own-slice-over-vec**: Use `&[T]` not `&Vec<T>`, `&str` not `&String`. Accept borrowed types.
- **own-arc-shared**: Use `Arc<T>` for thread-safe shared ownership, not `Rc<T>` across threads.
- **own-mutex-interior**: Use `Mutex<T>` for interior mutability in multi-threaded code.
- **own-copy-small**: Derive `Copy` only for small, trivial types (primitives, small tuples).
- **own-lifetime-elision**: Rely on lifetime elision when possible; don't add explicit lifetimes where not needed.

## CRITICAL - Error Handling

- **err-thiserror-lib**: Use `thiserror` for library error types.
- **err-anyhow-app**: Use `anyhow` for application error handling.
- **err-result-over-panic**: Return `Result`, don't panic on expected errors.
- **err-no-unwrap-prod**: NEVER use `.unwrap()` in production code. Use `?` or match.
- **err-expect-bugs-only**: Use `.expect()` only for programming errors that "should never happen".
- **err-question-mark**: Use `?` operator for clean error propagation.
- **err-from-impl**: Use `#[from]` attribute for automatic error conversion.
- **err-lowercase-msg**: Error messages should be lowercase, no trailing punctuation.
- **err-custom-type**: Create custom error types, avoid `Box<dyn Error>`.

## CRITICAL - Memory Optimization

- **mem-with-capacity**: Use `with_capacity()` when final size is known.
- **mem-smallvec**: Use `SmallVec<N>` for usually-small collections (N <= 32).
- **mem-box-large-variant**: Box large enum variants to reduce type size.
- **mem-boxed-slice**: Use `Box<[T]>` instead of `Vec<T>` when size is fixed.
- **mem-zero-copy**: Use zero-copy patterns with slices and `Bytes`.
- **mem-compact-string**: Use `CompactString` for small string optimization.

## HIGH - API Design

- **api-builder-pattern**: Use Builder pattern for complex construction.
- **api-newtype-safety**: Use newtypes for type-safe distinctions (e.g., `Url(String)`).
- **api-sealed-trait**: Seal traits to prevent external implementations.
- **api-impl-into**: Accept `impl Into<T>` for flexible inputs.
- **api-must-use**: Add `#[must_use]` to functions returning `Result`.
- **api-non-exhaustive**: Use `#[non_exhaustive]` for enums/structs that may grow.
- **api-default-impl**: Implement `Default` for sensible defaults.

## HIGH - Async/Await

- **async-tokio-runtime**: Use Tokio for production async runtime.
- **async-no-lock-await**: NEVER hold `Mutex`/`RwLock` across `.await`. This causes deadlocks.
- **async-spawn-blocking**: Use `spawn_blocking` for CPU-intensive work.
- **async-join-parallel**: Use `tokio::join!` for parallel operations.
- **async-try-join**: Use `tokio::try_join!` for fallible parallel operations.
- **async-select-racing**: Use `tokio::select!` for racing/timeouts.
- **async-bounded-channel**: Use bounded channels for backpressure.
- **async-clone-before-await**: Clone data before await points.

## HIGH - Compiler Optimization

- **opt-inline-small**: Use `#[inline]` for small hot functions.
- **opt-lto-release**: Enable LTO in release builds.
- **opt-codegen-units**: Use `codegen-units = 1` for max optimization in release.
- **opt-bounds-check**: Use iterators to avoid bounds checks in hot loops.

## MEDIUM - Naming Conventions

- **name-types-camel**: Use `UpperCamelCase` for types, traits, enums.
- **name-funcs-snake**: Use `snake_case` for functions, methods, modules.
- **name-consts-screaming**: Use `SCREAMING_SNAKE_CASE` for constants.
- **name-iter-convention**: Use `iter`/`iter_mut`/`into_iter` consistently.

## MEDIUM - Type Safety

- **type-newtype-ids**: Wrap IDs in newtypes: `UserId(u64)`.
- **type-newtype-validated**: Use newtypes for validated data: `Email`, `Url`.
- **type-option-nullable**: Use `Option<T>` for nullable values.
- **type-result-fallible**: Use `Result<T, E>` for fallible operations.

## MEDIUM - Testing

- **test-cfg-test-module**: Use `#[cfg(test)] mod tests { }`.
- **test-integration-dir**: Put integration tests in `tests/` directory.
- **test-descriptive-names**: Use descriptive test names like `test_foo_bar_baz`.
- **test-tokio-async**: Use `#[tokio::test]` for async tests.

## MEDIUM - Documentation

- **doc-all-public**: Document all public items with `///`.
- **doc-examples-section**: Include `# Examples` with runnable code.
- **doc-errors-section**: Include `# Errors` for fallible functions.

## MEDIUM - Performance Patterns

- **perf-iter-over-index**: Prefer iterators over manual indexing.
- **perf-iter-lazy**: Keep iterators lazy, collect() only when needed.
- **perf-entry-api**: Use `entry()` API for map insert-or-update.
- **perf-drain-reuse**: Use `drain()` to reuse allocations.

## LOW - Project Structure

- **proj-lib-main-split**: Keep `main.rs` minimal, logic in `lib.rs`.
- **proj-mod-by-feature**: Organize modules by feature, not type.
- **proj-pub-crate-internal**: Use `pub(crate)` for internal APIs.

## LOW - Clippy & Linting

- **lint-deny-correctness**: Use `#![deny(clippy::correctness)]`.
- **lint-warn-perf**: Use `#![warn(clippy::perf)]`.
- **lint-rustfmt-check**: Run `cargo fmt --check` in CI.

## Anti-Patterns (Flag These!)

- **anti-unwrap-abuse**: Don't use `.unwrap()` in production.
- **anti-lock-across-await**: Don't hold locks across `.await` (deadlock!).
- **anti-string-for-str**: Don't accept `&String` when `&str` works.
- **anti-vec-for-slice**: Don't accept `&Vec<T>` when `&[T]` works.
- **anti-index-over-iter**: Don't use indexing when iterators work.
- **anti-panic-expected**: Don't panic on expected/recoverable errors.
- **anti-format-hot-path**: Don't use `format!()` in hot paths.

## Project-Specific Rules

- Use `reqwest` with `tokio` for HTTP client.
- Use `scraper` or `selectors` crate for HTML parsing.
- Use `anyhow` for error handling (application code).
- Use `thiserror` if creating a library.
- All async functions must use `tokio`.
- All `await` calls must NOT hold locks.
- No `.unwrap()` on network responses.
- Use proper User-Agent headers for web scraping.
- Respect robots.txt when scraping.
