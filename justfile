# List recipes
default:
    @just --list --unsorted

# Format the code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all -- --check

# Lint everything
clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run every test
test:
    cargo test --workspace --locked

# Check advisories, licences and sources
deny:
    cargo deny --locked check

# Build everything
build:
    cargo build --workspace --locked

# Run everything CI checks
ci: fmt-check clippy test deny build
