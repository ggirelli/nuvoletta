# Development

- Work happens in [jj](https://github.com/jj-vcs/jj) workspaces under `.workspaces/` (git-ignored).
- `just ci` runs everything CI checks: format, clippy, tests, cargo-deny and the build.
- `just docs` serves this site with live reload; `just docs-build` builds it with `--strict`.
- The Dioxus editor spike lives in `spikes/dioxus-editor`.
