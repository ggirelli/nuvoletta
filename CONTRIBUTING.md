# Contributing to Nuvoletta

Thanks for helping! Nuvoletta is a pre-alpha, local-first encrypted notes app. Read
[PLAN.md](PLAN.md) and the design documents in [docs/](docs/) before proposing changes: they are
the source of truth for scope, architecture and the encrypted formats.

## Before you start

- **Bugs and ideas:** open an issue. For a bigger change, describe it in an issue first so we can
  agree on the approach before you spend time on it.
- **Security problems:** do not open a public issue; see [SECURITY.md](SECURITY.md).
- **Format or protocol changes** (anything in `docs/CRYPTO_FORMAT.md` or `docs/SYNC_PROTOCOL.md`)
  need a design discussion and an ADR in `docs/ADR/` before code.

## Setting up

You need Rust (stable, pinned in `rust-toolchain.toml`), [`just`](https://github.com/casey/just),
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny), [`uv`](https://docs.astral.sh/uv/)
for the docs and, for the app, [`dx`](https://dioxuslabs.com/learn/0.7/getting_started) (Dioxus CLI
0.7).

The repository is a [jj](https://github.com/jj-vcs/jj) colocated repository (plain git works too).
Maintainers work in one jj workspace per task under `.workspaces/<name>` (git-ignored).

## Making a change

1. Keep changes focused: one feature or fix per pull request, in small commits that each build and
   pass the checks.
2. Run the checks CI runs:

   ```sh
   just ci          # rustfmt, clippy (warnings are errors), tests, cargo-deny, build
   just docs-build  # if you touched docs/ or mkdocs.yml
   just test-slow   # if you touched key derivation (full-cost Argon2)
   ```

3. Add or update tests. Security-sensitive code (parsers, envelopes, sync state) needs tamper,
   truncation and malformed-input tests, and property tests where they fit.
4. Update the docs that describe what you changed: [PLAN.md](PLAN.md) when a decision changes, the
   design documents in [docs/](docs/), and an ADR for significant decisions.
5. Open a pull request against `main`. `main` is protected: changes land through pull requests
   (squash or rebase, so history stays linear), with signed commits, once the CI checks pass.
   Stacked pull requests must be retargeted to `main` before the one below them is merged.

## Commits and pull requests

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `<type>(<optional scope>): <summary>`, in the imperative mood, lower case, no trailing full stop.
  - Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`, `revert`.
  - Scopes name the area: `crypto`, `core`, `storage`, `markdown`, `sync`, `provider`, `webdav`,
    `app`, `plan`, `design`, `spike`, `deps`.
  - Breaking changes (including any change to an on-disk or on-wire format) add `!` after the
    type/scope and a `BREAKING CHANGE:` footer.
  - The body explains what changed and why.
- Pull request titles follow the same format: a squash merge uses the title as the commit message.

## Style

- Code follows `rustfmt` and clippy; `unsafe` code is forbidden by a workspace lint.
- Every public item has a doc comment and every module opens with a `//!` comment. Public functions
  that can fail say when in an `# Errors` section.
- Split code by subject, not by kind. Code that moves keeps its shape: a change that moves code
  does not also modify it.
- Test names are sentences describing the behaviour (`wrong_passphrase_is_rejected`).

## Security rules

- Do not invent cryptographic primitives or protocols; use established crates.
- Never log or print passphrases, keys, plaintext note content, tokens or decrypted metadata.
  Secret types redact their `Debug` output and are zeroized on drop.
- No database unless explicitly justified and approved.
- Never silently discard conflicting edits or remote data.
- Do not weaken security to make a platform integration easier; explain the limit and propose
  alternatives.
- Keep dependencies proportional; new dependencies must pass `cargo deny` and are reviewed like
  code.

By contributing, you agree that your contributions are licensed under the [MIT licence](LICENSE).
