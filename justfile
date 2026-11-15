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

# Build the debug Android APK (target: aarch64-linux-android phones, x86_64-linux-android emulator; see docs/development.md)
[group('android')]
android target="aarch64-linux-android":
    dx build --platform android --target {{target}} --no-default-features --features mobile --package nuvoletta-app
    @echo "APK: target/dx/nuvoletta-app/debug/android/app/app/build/outputs/apk/debug/app-debug.apk"

# Fail unless the debug APK requests exactly the INTERNET permission (see ADR 0006)
[group('android')]
android-permissions:
    #!/usr/bin/env bash
    set -euo pipefail
    aapt2=$(ls "${ANDROID_HOME:?set ANDROID_HOME}"/build-tools/*/aapt2 | sort -V | tail -n1)
    apk=target/dx/nuvoletta-app/debug/android/app/app/build/outputs/apk/debug/app-debug.apk
    perms=$("$aapt2" dump permissions "$apk" | sed -n "s/^uses-permission: name='\(.*\)'.*/\1/p" | grep -v '\.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION$' | sort)
    echo "permissions: $perms"
    [ "$perms" = "android.permission.INTERNET" ]
    badging=$("$aapt2" dump badging "$apk")
    grep -q "^minSdkVersion:'28'" <<< "$badging"

# Build the release Android bundle (minified; signing is configured outside the repository)
[group('android')]
android-release target="aarch64-linux-android":
    dx bundle --platform android --release --target {{target}} --no-default-features --features mobile --package nuvoletta-app

# Install the debug APK on the connected device or emulator and start it
[group('android')]
android-run:
    adb install -r target/dx/nuvoletta-app/debug/android/app/app/build/outputs/apk/debug/app-debug.apk
    adb shell am start -n dev.nuvoletta.app/dev.dioxus.main.MainActivity

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

# Fuzz targets (cargo-fuzz, nightly toolchain)
fuzz_targets := "vault_header envelope note_payload log_segment state_docs remote_names webdav_xml webdav_href markdown_render wikilinks portable_markdown app_links"

# List the fuzz targets
[group('fuzz')]
fuzz-list:
    @for t in {{fuzz_targets}}; do echo $t; done

# Run one fuzz target for `seconds` (0 = until stopped); crashes land in fuzz/artifacts/<target>
[group('fuzz')]
fuzz target seconds="60":
    mkdir -p fuzz/corpus/{{target}}
    cargo +nightly fuzz run {{target}} fuzz/corpus/{{target}} fuzz/seeds/{{target}} -- -max_total_time={{seconds}}

# Run every fuzz target for `seconds` each (what the nightly workflow does)
[group('fuzz')]
fuzz-all seconds="60":
    #!/usr/bin/env bash
    set -euo pipefail
    for t in {{fuzz_targets}}; do
        echo "== $t"
        mkdir -p "fuzz/corpus/$t"
        cargo +nightly fuzz run "$t" "fuzz/corpus/$t" "fuzz/seeds/$t" -- -max_total_time={{seconds}}
    done

# Regenerate the code-produced fuzz seeds in fuzz/seeds
[group('fuzz')]
fuzz-seeds:
    cd fuzz && cargo +nightly run --release --bin gen_seeds
