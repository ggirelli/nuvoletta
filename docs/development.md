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
