# Development

- Work happens in [jj](https://github.com/jj-vcs/jj) workspaces under `.workspaces/` (git-ignored).
- `just ci` runs everything CI checks: format, clippy, tests, cargo-deny and the build.
- `just docs` serves this site with live reload; `just docs-build` builds it with `--strict`.
- The Dioxus editor spike lives in `spikes/dioxus-editor`.
- Commits and pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/); see
  [CONTRIBUTING.md](https://github.com/ggirelli/nuvoletta/blob/main/CONTRIBUTING.md) for types, scopes and style.
