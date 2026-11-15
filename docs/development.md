# Development

- Work happens in [jj](https://github.com/jj-vcs/jj) workspaces under `.workspaces/` (git-ignored).
- `just ci` runs everything CI checks: format, clippy, tests, cargo-deny and the build.
- `just docs` serves this site with live reload; `just docs-build` builds it with `--strict`.
- The Dioxus editor spike lives in `spikes/dioxus-editor`.
- Commits and pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/); see
  [CONTRIBUTING.md](https://github.com/ggirelli/nuvoletta/blob/main/CONTRIBUTING.md) for types, scopes and style.
- `just test` runs the sync engine scenarios (two or three in-process vaults over a fault-injecting folder),
  the crash-at-every-step tests and the convergence property tests; `just test-slow` also runs the long
  four-device property test.

## Android

`just android` builds a debug APK with [`dx`](https://dioxuslabs.com/learn/0.7/getting_started/) (the
Dioxus CLI); `just android-release` builds the minified release bundle (signing keys are configured
outside the repository); `just android-run` installs the debug APK with `adb` and starts it.

Toolchain (Linux; the first build downloads Gradle and its dependencies):

1. Java 17 or newer (`java -version`).
2. Android SDK with platform 34 and build-tools 34, plus the NDK. Set `ANDROID_HOME` (for example
   `~/Android/Sdk`) and `ANDROID_NDK_HOME` (for example `$ANDROID_HOME/ndk/<version>`).
3. Rust targets: `rustup target add aarch64-linux-android` (phones) and
   `x86_64-linux-android` (emulator on a PC).
4. `dx` 0.7.x: `cargo install dioxus-cli --version 0.7.10 --locked` (or `cargo binstall dioxus-cli`).

`just android x86_64-linux-android` targets the emulator. The Android-specific files are
`apps/nuvoletta-app/Dioxus.toml` (application id, SDK levels), `apps/nuvoletta-app/android/`
(manifest and `MainActivity.kt`) and `apps/nuvoletta-app/src/platform.rs`. After a build, check the
permissions with `$ANDROID_HOME/build-tools/<version>/aapt2 dump badging <apk> | grep -E
'uses-permission|sdkVersion'`: only `android.permission.INTERNET` (and the app's own internal
receiver permission) may appear, and any change needs a justification in the pull request.

To try an existing vault on a debug build, `adb push` its folder to `/data/local/tmp/vault`, then
`adb shell run-as dev.nuvoletta.app sh -c 'mkdir -p files && cp -r /data/local/tmp/vault files/vault'`
(debug builds only), and delete the copy in `/data/local/tmp` afterwards. There is no emulator or
device in CI: the CI jobs only prove the APK builds.

## Fuzzing

`fuzz/` holds [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) targets for every parser of
untrusted input: the vault header, object envelopes (raw bytes and sealed-then-tampered), note
payloads, log segments, local sync state documents, the remote name grammar, the WebDAV PROPFIND XML
and `href` handling, the Markdown renderer and sanitiser, wikilinks, the plaintext import parser and
the preview link classifier. Targets assert invariants (for example: no tag, attribute or URL
outside the allowlist in rendered HTML; a tampered envelope never opens; an `href` never leaves the
sync root), not only absence of panics.

```sh
cargo install cargo-fuzz --locked   # once; needs the nightly toolchain
just fuzz-list                      # the targets
just fuzz markdown_render 60        # one target for 60 seconds
just fuzz-all 60                    # every target
```

The crate is excluded from the workspace. Seeds are in `fuzz/seeds/<target>/` (test vectors and
hand-written hostile samples; `just fuzz-seeds` regenerates the ones produced by code); the evolving
corpus and crash reproducers go to `fuzz/corpus/` and `fuzz/artifacts/` (git-ignored). The nightly
workflow runs each target for ten minutes and uploads reproducers of any crash. To fix one:
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`, add a regression test in the
crate that owns the bug (and the reproducer to `fuzz/seeds/<target>/`). The crates expose a few
`#[doc(hidden)]` entry points (`*_for_fuzz`, `fuzz_*`, `portable::fuzzing`) so that targets can reach
private parsers; they are not API.
