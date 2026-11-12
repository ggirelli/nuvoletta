# Nuvoletta

Encrypted note app in Rust + Dioxus. Read [PLAN.md](PLAN.md) first: it is the source of truth for
scope, architecture and phases. Update it when a decision changes.

## Workflow

- The repo is a jj colocated repo (git underneath). Use `jj`, not raw `git`, for history changes.
- Work in a jj workspace under `.workspaces/<name>` (ignored by git), not in the main checkout.
- `main` is protected: open a PR; merges are squash or rebase, commits signed, history linear.
- Run `just ci` (fmt-check, clippy, test, deny, build) before pushing. CI runs the same jobs.
- Work incrementally: small, reviewable steps with tests; stop at phase exit criteria for review.
- Commit messages and PR titles follow Conventional Commits (`feat(crypto): …`, `docs(plan): …`);
  types, scopes and breaking-change rules are in [CONTRIBUTING.md](CONTRIBUTING.md). Any change to an
  on-disk or on-wire format is a breaking change (`!`).
- Stacked PRs: retarget the upper PR to `main` before merging the one below it.
- Contribution conventions (style, tests, security rules) live in [CONTRIBUTING.md](CONTRIBUTING.md).

## Layout

- `crates/nuvoletta-core`: domain types (note ids, virtual paths, note model); no I/O.
- `crates/nuvoletta-crypto`: vault header, keys and encrypted object format (`docs/CRYPTO_FORMAT.md`).
- `crates/nuvoletta-storage`: filesystem vault, atomic writes, note CRUD (`docs/STORAGE_FORMAT.md`).
- `spikes/`: throwaway experiments, excluded from the workspace.

## Security rules (PLAN.md section 10)

- Do not invent cryptographic protocols or silently change the encrypted file format.
- No database unless explicitly justified and approved.
- Never upload plaintext notes, keys or secrets to any provider.
- Never silently discard conflicting edits or remote data.
- Do not weaken security for platform convenience; explain the limit and propose alternatives.
- Prefer well-maintained crates and official provider APIs; keep dependencies proportional.
- `unsafe` code is forbidden by workspace lint.
