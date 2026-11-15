# Supply chain and dependency policy

Nuvoletta handles keys and plaintext notes, so every dependency is code that can read them. This
page records what we depend on directly and why, which dependencies deserve extra scrutiny, what
the automated checks enforce, and the rules for adding a dependency. It is reviewed whenever a
direct dependency is added or removed. It is a review of our own choices, not an audit of the
dependencies themselves.

## What is enforced automatically

`just deny` (also in `just ci` and the CI `deny` job) runs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny)
with `deny.toml` against the whole dependency graph of every target and feature (`all-features`):

| Check | Setting |
| --- | --- |
| Advisories | RustSec database; vulnerabilities, yanked crates, unmaintained and unsound crates anywhere in the tree are errors. The known notices that come only through the Dioxus webview stack are listed in `deny.toml` with a reason each and are removed as soon as the crate leaves the tree. |
| Licences | Allowlist of permissive licences (plus MPL-2.0 for the webview stack and `dirs`, NCSA, Unicode-3.0). A new licence needs a review and a comment in `deny.toml`. Our own crates are unpublished and exempt. |
| Bans | No wildcard version requirements. No OpenSSL or `native-tls` except the one path through the Dioxus devtools websocket (`tungstenite`), which never carries notes or keys: TLS for sync is rustls only. Duplicate versions are reported as warnings (most come from the GTK/windowing stack). |
| Sources | crates.io only; no git dependencies, no alternative registries. |

`cargo deny` is the `cargo audit` equivalent: the `[advisories]` section checks the same RustSec
database, and it runs on every pull request and on pushes to `main`. The advisory database is
fetched fresh by the check, so a new advisory fails the next run even if nothing changed. There is
no separate scheduled audit job; the nightly workflow would be the place to add one if advisories
need to be noticed on days without a commit.

`Cargo.lock` is committed and every CI command uses `--locked`, so builds use exactly the reviewed
versions with the checksums recorded in the lockfile. The `fuzz/` crate has its own lockfile
(`fuzz/Cargo.lock`) because it is excluded from the workspace; it is not covered by `just deny`
and only ever runs in the nightly fuzz job and on a developer's machine, never in a release.

### Why not `cargo vet`

`cargo vet` records that a trusted party audited a specific version of a crate. It is valuable
when an organisation has people to perform and maintain those audits or can import audits from
Mozilla, Google or the Bytecode Alliance. For this project it would add a few hundred unaudited
exemptions on day one (the Dioxus tree alone is over a thousand crates), which makes the signal
weak, and nobody currently has the capacity to keep audits current. We therefore rely on:
minimising direct dependencies, keeping the security-critical path (crypto, sync, storage) on a
small and well-known set of crates, and the policy below. Revisit when a second maintainer joins,
or if the imported audit sets cover our crypto and parsing crates, in which case vetting just
those crates (not the GUI stack) is a reasonable first step.

## Direct dependencies per crate

"Critical path" means the crate sees keys or plaintext. Versions are in `Cargo.toml` and
`Cargo.lock`.

### `nuvoletta-core` (no I/O)

| Dependency | Purpose |
| --- | --- |
| `getrandom` | Note ids from the OS CSPRNG. |
| `thiserror` | Error types. |
| `unicode-normalization` | NFC normalisation of titles and virtual paths. |
| `zeroize` | Wipe note content on drop. |
| `proptest` (dev) | Property tests. |

### `nuvoletta-crypto` (critical path)

All cryptography is RustCrypto or `subtle`; there is no hand-written primitive. The format is in
`docs/CRYPTO_FORMAT.md`.

| Dependency | Purpose |
| --- | --- |
| `argon2` | Argon2id passphrase KDF. |
| `chacha20poly1305` | XChaCha20-Poly1305 AEAD for envelopes and key wrapping (`getrandom` for nonces). |
| `hkdf`, `hmac`, `sha2` | Subkey derivation, the header MAC and hashes. |
| `subtle` | Constant-time comparisons. |
| `minicbor` | CBOR for note payloads. Chosen over serde-based CBOR crates for a small, strict decoder with derive support and no `serde` in the critical path. |
| `getrandom` | The only source of randomness (OS CSPRNG). |
| `unicode-normalization` | Passphrase normalisation. |
| `zeroize` | Wipe key material. |
| `thiserror` | Errors. |
| `proptest`, `serde_json` (dev) | Property tests and the test-vector files. |

### `nuvoletta-storage` (critical path)

| Dependency | Purpose |
| --- | --- |
| `nuvoletta-core`, `nuvoletta-crypto` | Domain types and envelopes. |
| `getrandom` | Names of temporary files. |
| `unicode-normalization` | Case/normalisation folding of exported file names. |
| `thiserror`, `zeroize` | Errors; wipe buffers that held plaintext. |
| `tempfile` (dev) | Test vaults. |

No SQLite or other database (PLAN.md section 10); the file system is the store.

### `nuvoletta-markdown`

| Dependency | Purpose |
| --- | --- |
| `pulldown-cmark` (no default features) | `CommonMark` parser. We do not use its HTML writer: the renderer in `render.rs` emits a fixed set of tags and attributes itself so that sanitising is by construction (escaped text, scheme allowlist). Raw HTML in notes is escaped. |
| `unicode-normalization`, `zeroize` | Link target matching; wipe. |
| `nuvoletta-core` | Types. |
| `proptest` (dev) | Property tests. |

### `nuvoletta-sync` (critical path)

| Dependency | Purpose |
| --- | --- |
| `minicbor` | CBOR for log entries and local state. |
| `sha2` | Entry hash chain and object hashes. |
| `getrandom` | Device ids. |
| `nuvoletta-core`, `-crypto`, `-storage` | Domain, envelopes, vault. |
| `thiserror`, `zeroize` | Errors; wipe. |
| `proptest`, `tempfile` (dev) | Tests. |

### `nuvoletta-webdav` (network; sees only ciphertext and credentials)

| Dependency | Purpose |
| --- | --- |
| `ureq` (`rustls` feature, no default features) | Blocking HTTPS client. No async runtime, no OpenSSL. |
| `rustls` (`ring`, `std`, `tls12`) | TLS with the `ring` provider and the built-in `webpki-roots` (no system certificate store, hence no JNI on Android; ADR 0007). Certificate pinning by SHA-256 fingerprint is implemented in `tls.rs` as a rustls `ServerCertVerifier`. |
| `quick-xml` | PROPFIND response parsing. Nothing in the document is followed and unknown entities are errors, so entity-expansion attacks do not apply. |
| `percent-encoding` | `href` decoding and path encoding. |
| `sha2`, `getrandom`, `thiserror`, `zeroize` | Pin fingerprints; probe names; errors; wipe the password. |
| `rcgen` (optional, `test-server` feature) | Self-signed certificates for the in-process test server. Never in a release build. |

### `nuvoletta-app-core` (UI-independent application logic)

Only workspace crates plus `serde_json` (settings files), `thiserror`, `zeroize`, and in tests
`tempfile`.

### `nuvoletta-app` (desktop and Android UI)

| Dependency | Purpose |
| --- | --- |
| `dioxus` (`desktop` or `mobile`) | UI framework and webview host. Brings `wry`, `tao`, `tokio`, GTK/WebKitGTK bindings on Linux, and most of the transitive tree. |
| `tokio` (`time` only) | Auto-lock timer. |
| `webbrowser` | Open a clicked `http(s)`/`mailto` link in the system browser after the app has classified it. |
| `dirs` (desktop) | Per-user config directory. |
| `rfd` (desktop, `xdg-portal`) | Native folder picker. |
| `serde_json`, `zeroize` | IPC messages; wipe. |
| JS bundle (`apps/nuvoletta-app/js`) | CodeMirror 6 packages, bundled with esbuild into the checked-in `assets/editor.js`. No CDN and no network access at runtime; `package-lock.json` pins every JS dependency. The bundle is only the editor: it never receives keys, and the webview runs under the CSP described in ADR 0004. |

## Dependencies that deserve a closer look

Rust code can use `unsafe` and build scripts can run arbitrary code at build time, so these are
the ones a reviewer should read first when they change. "unsafe" below means the crate contains
`unsafe` blocks, which is normal for its job and not itself a finding.

| Crate | Why it matters |
| --- | --- |
| `ring` (via `rustls`, `rcgen`) | C and assembly with a build script (`build.rs` compiles the primitives) and a large amount of `unsafe`. It is the crypto provider for TLS only; vault encryption does not use it. It is the provider selected in ADR 0007. |
| `rustls`, `rustls-webpki`, `webpki-roots` | TLS and certificate validation. Security-critical but safe Rust (`rustls` has one `unsafe` line and a build script for cfg detection). |
| RustCrypto (`chacha20`, `poly1305`, `sha2`, `blake2`, `cpufeatures`, `argon2`) | `unsafe` for SIMD intrinsics and CPU feature detection, guarded by runtime detection and portable fallbacks. `zeroize` uses `unsafe` volatile writes by design. |
| `getrandom` | `unsafe` FFI to the OS RNG, with a build script. Several versions are in the tree because of old transitive users. |
| `wry`, `tao`, `rfd`, GTK/WebKitGTK bindings, `webbrowser`, `tokio` | Large amounts of `unsafe` FFI to the platform windowing, webview and file dialog APIs and (tokio) runtime internals; `wry` and `rfd` have build scripts. They run in the UI process, which holds the decrypted note being edited: a compromise here is a compromise of the open session, which the threat model already assumes for a compromised host. |
| `pulldown-cmark` | A parser of untrusted text with a build script (generates entity tables) and a few `unsafe` lines. Fuzzed (`markdown_render`, `wikilinks`). |
| `proc-macro` crates (`thiserror-impl`, `minicbor-derive`, `zeroize_derive`, Dioxus macros) | Run arbitrary code at compile time in every build. The set is small and well known on the critical path. |

Our own crates contain no `unsafe`: the workspace lint is `unsafe_code = "forbid"`. The only
`unsafe` we compile that we did not review in this repository is in dependencies. The `fuzz/`
crate is the one exception to the lint, because `libfuzzer-sys` generates `unsafe` glue; it is
excluded from the workspace and never shipped.

Build scripts and proc macros from dependencies run on the developer's and CI's machines. CI
runs without secrets on pull requests and the release workflow is the only place that signs.

## GitHub Actions

Workflows run third-party code with access to the repository token. Actions are pinned to full
commit SHAs with a comment naming the tag, so a moved tag cannot change what runs.

* Pinned: everything in `.github/workflows/nightly.yml`.
* Not yet pinned (tag or branch references, to be pinned in a follow-up so as not to collide with
  other work on those files): `.github/workflows/ci.yml` and `.github/workflows/docs.yml`
  (`actions/checkout`, `actions/upload-pages-artifact`, `actions/deploy-pages`,
  `astral-sh/setup-uv`, `Swatinem/rust-cache`, `taiki-e/install-action`,
  `dtolnay/rust-toolchain`, `EmbarkStudios/cargo-deny-action`), and the release workflow when it
  lands. Local composite actions (`./.github/actions/*`) are part of the repository and need no
  pin.

Note for `dtolnay/rust-toolchain` and `taiki-e/install-action`: the tag or branch name selects the
toolchain or tool, so a SHA pin has to pass it explicitly with `toolchain:` or `tool:`. Renovate
or Dependabot (`github-actions` ecosystem) can then keep the SHAs current; neither is configured
yet.

## Policy for adding or changing a dependency

1. **Do you need it?** Prefer the standard library or a few lines of code. Every dependency adds
   its whole tree. Do not add a crate for something we can write and test in under a hundred
   lines, unless it is cryptography, a parser of a complex format or platform FFI, where the
   opposite is true.
2. **Never write cryptography ourselves**; use RustCrypto (or `ring` through `rustls`) and the
   protocol already in `docs/CRYPTO_FORMAT.md`. A new primitive or protocol needs an ADR first.
3. **Choose well-maintained crates**: recent releases, multiple maintainers or a reputable
   organisation, documented `unsafe`, no known advisories (`cargo deny` shows them). Prefer pure
   Rust and crates already in the tree.
4. **Minimise surface**: `default-features = false` and only the features used. No `openssl`,
   no git dependencies, no alternative registries (enforced by `deny.toml`).
5. **Critical-path crates** (`crypto`, `storage`, `sync`, `webdav`, `markdown`) accept new
   dependencies only with an explicit note in the pull request: what it is for, why the existing
   ones do not suffice, its `unsafe` and build-script footprint, and its transitive additions
   (`cargo tree -p <crate> -i`, `cargo tree -d` for duplicates).
6. **Parsers of untrusted input** (any crate that reads bytes from a file, peer or server) get a
   fuzz target in `fuzz/` in the same pull request.
7. **Update this page** (the per-crate table and the scrutiny table) and, if `deny.toml` needs a
   new licence or advisory exception, justify it there with a reason that names the dependency path
   and when it can be removed.
8. **Updates**: lockfile updates are separate pull requests (`build(deps): ...`) that state what
   changed (`cargo update` output) and the cargo-deny result. Do not bump major versions of
   critical-path crates together with other work.
9. **Vendored or JS code** (the editor bundle) is rebuilt from the pinned `package-lock.json`;
   the diff of `assets/editor.js` is checked in with the change that caused it.

## Fuzzing

Parsers of untrusted input are fuzzed with `cargo-fuzz`; see `fuzz/` and the "Fuzzing" section in
`docs/development.md`. A bug found by fuzzing gets a regression test in the crate that owns it.
