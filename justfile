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

# Run every test (the desktop app needs GTK and WebKitGTK development packages on Linux)
test:
    cargo test --workspace --locked

# Run the tests of everything except the desktop app: no webview libraries needed
test-core:
    cargo test --workspace --exclude nuvoletta-app --locked

# Run slow tests (full-cost Argon2id vectors; nightly CI, Linux)
test-slow:
    cargo test --workspace --locked --release -- --ignored

# Check advisories, licences and sources
deny:
    cargo deny --locked check

# Build everything, including the desktop app
build:
    cargo build --workspace --locked

# Build only the crates that need no webview libraries
build-core:
    cargo build --workspace --exclude nuvoletta-app --locked

# Run the desktop app
app:
    cargo run -p nuvoletta-app --locked

# Run everything CI checks
ci: fmt-check clippy test deny build

# docs/requirements.txt pins MkDocs below 2.0, so Material's banner warning about MkDocs 2.0 is noise
export NO_MKDOCS_2_WARNING := "true"

# Serve the docs with live reload
[group('docs')]
docs:
    uvx --with-requirements docs/requirements.txt mkdocs serve --livereload

# Build the docs into ./site
[group('docs')]
docs-build:
    uvx --with-requirements docs/requirements.txt mkdocs build --strict
