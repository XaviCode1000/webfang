# Contributing

## Development Setup

```bash
# Clone repository
git clone https://github.com/XaviCode1000/rust-scraper.git
cd rust-scraper

# Install dependencies
cargo fetch

# Build
cargo build

# Run tests
cargo test
```

## Code Style

This project uses standard Rust formatting:

```bash
# Format code
cargo fmt

# Check for clippy warnings
cargo clippy
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_validate_url

# Run with output
cargo test -- --nocapture

# Run integration tests
cargo test --test integration_test
```

## Project Structure

```
src/
├── lib.rs          # Library root, re-exports
├── main.rs         # CLI entry point
├── scraper.rs      # Core scraping logic
├── config.rs       # Logging configuration
└── url_path.rs    # Type-safe URL handling
```

## Adding Features

1. **New output format**: Add to `OutputFormat` enum in `lib.rs`
2. **New URL handling**: Add methods to `url_path.rs`
3. **New dependencies**: Update `Cargo.toml`

## Pull Request Process

1. Fork the repository
2. Create a feature branch
3. Make changes with tests
4. Ensure `cargo test` passes
5. Push and create PR

## Commit Messages

Follow conventional commits:

- `feat:` New feature
- `fix:` Bug fix
- `refactor:` Code refactoring
- `docs:` Documentation
- `test:` Adding tests

Example:
```
feat: add syntax highlighting for code blocks
```

## Questions

Open an issue for:
- Bug reports
- Feature requests
- General questions
