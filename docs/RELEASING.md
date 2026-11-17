# Releasing and verifying downloads

Nuvoletta has no auto-updater (threat model T14): the only way a malicious build reaches you is a
download you install yourself. So every release file can be checked, and you should check it.

The first release channel is [GitHub Releases](https://github.com/ggirelli/nuvoletta/releases) only.
Flathub, Google Play and F-Droid come later.

## Verify your download

Download the file you want, `SHA256SUMS` and `SHA256SUMS.sigstore.json` from the same release. Do
these checks before you open the file. The first two need no extra tools beyond what ships with your OS.

**1. Checksum.** It proves the file is the one listed in `SHA256SUMS`.

=== "Linux"

    ```sh
    sha256sum --ignore-missing --check SHA256SUMS
    ```

=== "macOS"

    ```sh
    shasum -a 256 --ignore-missing --check SHA256SUMS
    ```

=== "Windows (PowerShell)"

    ```powershell
    (Get-FileHash .\nuvoletta-X.Y.Z-windows-x86_64.msi -Algorithm SHA256).Hash
    # compare with the matching line in SHA256SUMS (case-insensitive)
    ```

**2. Signature of the checksum list.** `SHA256SUMS` is signed with
[Sigstore](https://www.sigstore.dev/) keyless signing by the release workflow of this repository.
There is no project key to steal or lose: the signature proves the list was produced by
`release.yml` in `ggirelli/nuvoletta` for that tag, and is recorded in a public transparency log.
Install [`cosign`](https://docs.sigstore.dev/cosign/system_config/installation/), then (replace `X.Y.Z`):

```sh
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/ggirelli/nuvoletta/.github/workflows/release.yml@refs/tags/vX.Y.Z" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  SHA256SUMS
```

**3. Build provenance.** Each file also has a GitHub build provenance attestation: a signed statement
of which commit and workflow built it. With the [GitHub CLI](https://cli.github.com/):

```sh
gh attestation verify nuvoletta-X.Y.Z-linux-x86_64.tar.gz --repo ggirelli/nuvoletta
```

If any check fails, do not install the file, and report it as described in
[SECURITY.md](https://github.com/ggirelli/nuvoletta/blob/main/SECURITY.md).

### What the platform signatures mean

These are separate from the checks above and are **optional**: they depend on the maintainer having
paid for and configured the certificates. A file whose name ends in `-unsigned` has no platform
signature, and the release notes say so. For those, your OS warns you (Gatekeeper on macOS,
SmartScreen on Windows, "unknown sources" on Android). Steps 1 to 3 are then your assurance.

| Platform | Files | Platform signature |
| --- | --- | --- |
| Linux | `.tar.gz`, `.AppImage`, `.deb` | None (the checks above only) |
| macOS | `.dmg` (Apple Silicon and Intel) | Developer ID plus Apple notarization, when configured |
| Windows | `.msi`, `.exe` installer | Authenticode, when configured |
| Android | `.apk` (arm64) | Release keystore, when configured. Otherwise a throwaway key: installable, but a later release cannot upgrade it in place |

The CycloneDX software bill of materials (`nuvoletta-X.Y.Z.cdx.json`) lists every dependency in the build.

## Releasing (maintainers)

### One-time setup

All secrets are optional; without one the matching platform file is built unsigned and labelled
`-unsigned`. Add them under Settings, Secrets and variables, Actions.

| Secret | Used for |
| --- | --- |
| `APPLE_CERTIFICATE_BASE64`, `APPLE_CERTIFICATE_PASSWORD` | "Developer ID Application" certificate with its key, exported as `.p12` and base64-encoded |
| `APPLE_SIGNING_IDENTITY` | For example `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID` | Notarization with `notarytool` (an app-specific password) |
| `WINDOWS_CERT_PFX_BASE64`, `WINDOWS_CERT_PASSWORD` | Authenticode code-signing certificate (`.pfx`), base64-encoded |
| `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | Release keystore (`.jks`), base64-encoded |

macOS signing needs all six Apple secrets, Android all four. Never commit keys or certificates.
Use a protected environment for the secrets if you want an approval before they are exposed. Keep an
offline backup of the Android keystore: Android refuses updates signed with a different key, and the
key cannot be replaced once the app is published.

Sigstore signing and the build provenance need no secrets: they use the workflow's OIDC identity.

### Steps

1. `just release-prepare X.Y.Z` sets the version everywhere (see Versioning), then commit it as
   `chore(release): prepare X.Y.Z` and merge it through a pull request.
2. Optionally dry-run: Actions, Release, "Run workflow" on `main`. It builds everything and keeps the
   result as a workflow artifact (`dry-run-release`); nothing is signed with Sigstore or published.
3. Tag the merge commit on `main` and push the tag: `git tag -s vX.Y.Z && git push origin vX.Y.Z`.
4. The workflow runs `just ci`, builds Linux, macOS, Windows and Android, writes `SHA256SUMS`, the
   SBOM and the attestations, signs `SHA256SUMS`, and creates a **draft** release.
5. Open the draft, read the notes, run the verification steps above on a few files, then publish.

The workflow refuses a tag that does not equal `v` plus the Cargo version, or that is not on `main`.
A tag containing `-` (for example `v0.2.0-rc.1`) makes a pre-release.

### Versioning

`[workspace.package] version` in the root `Cargo.toml` is the single source of truth (SemVer). The
crate manifests' path dependencies repeat it and `Cargo.lock` records it; `just release-prepare`
updates all of them. The app and bundle versions come from the crate version through `dx`
(`Dioxus.toml` has no version). The Android `versionName` is the version and the `versionCode` is
derived by `scripts/release/version.sh --android-code`: `(major*10000 + minor*100 + patch)*100 + 99`
for a final release, or `+ N` for `-rc.N`, so it always increases. `dx` hard-codes `versionCode = 1`,
so `scripts/release/android-apk.sh` patches the generated Gradle file and runs Gradle itself.

The Windows NSIS installer needs a purely numeric four-part `VIProductVersion`, which dx fills with
the SemVer version and so rejects `-rc.N`. `scripts/release/version.sh --windows-version` gives
`major.minor.patch.N` (`0.1.0-rc.1` is `0.1.0.1`, `0.1.0` is `0.1.0.99`, the same suffix rule as the
Android `versionCode`). `scripts/release/windows-nsis.sh` writes that into
`apps/nuvoletta-app/windows/installer.nsi` from `installer.nsi.in`, which is dx's own template with
that single line changed; `Dioxus.toml` points `[bundle.windows.nsis] template` at it. Only the
file-properties resource uses this number: the displayed and registered version stays `0.1.0-rc.1`
(the MSI already derives its own numeric version). When you bump `dioxus-cli`, compare
`installer.nsi.in` with the new built-in template. Run `windows-nsis.sh` before any local Windows
`dx bundle`.

### Release notes

`git-cliff` (configured in `cliff.toml`) groups the Conventional Commit subjects since the previous
tag. It was chosen over hand-written notes because the repository already enforces the commit format
(CONTRIBUTING.md), and over a changelog service because it is a single static binary that sends
nothing anywhere. Pull request titles are the commit subjects, so write them for readers.

### SBOM

`cargo-cyclonedx` writes a CycloneDX 1.5 JSON bill of materials from `Cargo.lock` for all platforms
and both UI feature sets. CycloneDX was chosen over SPDX because the Rust tooling and most
vulnerability scanners consume it directly; `cargo-cyclonedx` reads the lock file without building.
It lists Rust crates only: bundled JavaScript (the vendored editor), Gradle/Maven dependencies of
the Android shell and system libraries are not in it.

## Reproducibility

`just release-check` builds the Linux tarball twice, in two different directories with separate
target directories, and compares the SHA-256 of the results. What it controls:

- the toolchain is pinned in `rust-toolchain.toml` (bump it deliberately, in its own pull request);
- `--locked` everywhere, so `Cargo.lock` decides every dependency version;
- `scripts/release/env.sh`: `SOURCE_DATE_EPOCH` (the commit time), `CARGO_INCREMENTAL=0`,
  `--remap-path-prefix` for the checkout, cargo home and rustup home, and `LC_ALL=C`, `TZ=UTC`;
- the tarball is packed with sorted members, fixed owner and times, and `gzip -n`.

**Result on the maintainer's Linux machine: the two tarballs are byte-identical**, including when
built from different checkout paths.

What is **not** covered, so do not read more into it:

- Only the Linux tarball is reproducible and checked. The release workflow builds the `.AppImage`
  and `.deb` with `SOURCE_DATE_EPOCH` unset, because linuxdeploy's `mksquashfs` aborts when it is set.
  Those two (bundled by `dx`, which embeds timestamps and the build host's shared libraries) and
  the macOS, Windows and Android files are not claimed to be reproducible. The Android APK is also signed, and signatures differ per signing key.
- The check runs on one machine with one toolchain, one linker and one set of system libraries.
  Building on another distribution or glibc version can give different bytes. A fully independent
  rebuild needs a pinned container image, which is future work.
- It does not prove the downloaded file equals a local build; compare checksums yourself to find out.

## Files

| Path | Role |
| --- | --- |
| `.github/workflows/release.yml` | The pipeline |
| `scripts/release/` | Build, sign, SBOM, version and check scripts (same ones the workflow runs) |
| `cliff.toml` | Release notes format |
| `rust-toolchain.toml` | Pinned toolchain |
