# Reproducible build environment. Source it (do not execute it) before any release build:
#
#   . scripts/release/env.sh
#
# See docs/RELEASING.md ("Reproducibility") for what each setting does and its limits.

# Fixed timestamp for everything that embeds one (archive members, bundle metadata): the commit time.
# Release builds run from a git checkout; release-check passes the value in because it builds in copies.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

# No incremental state can leak into the artifacts.
export CARGO_INCREMENTAL=0

# Strip the checkout, cargo home and rustup home paths from debug info and panic messages, so the
# result does not depend on where it was built. cargo-registry sources are covered by CARGO_HOME.
_repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
_rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
export RUSTFLAGS="--remap-path-prefix=${_repo_root}=/build/nuvoletta --remap-path-prefix=${_cargo_home}=/build/cargo --remap-path-prefix=${_rustup_home}=/build/rustup"
unset _repo_root _cargo_home _rustup_home

# Locale and timezone must not influence tools.
export LC_ALL=C
export TZ=UTC
