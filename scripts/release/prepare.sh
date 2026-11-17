#!/usr/bin/env bash
# Set the release version everywhere it lives.
#
# Usage: scripts/release/prepare.sh <version>      (for example 0.1.0 or 0.2.0-rc.1)
#
# Touches: [workspace.package] version and the `version = "..."` of every path dependency in the
# crate manifests, then refreshes Cargo.lock. Everything else derives from there: the app and
# bundle versions (dx reads the crate version) and the Android versionCode (version.sh).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

new=${1:?usage: prepare.sh <version>}
re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'
[[ $new =~ $re ]] || { echo "not a SemVer version: $new" >&2; exit 1; }

old=$(scripts/release/version.sh)
[ "$old" != "$new" ] || { echo "already at $new" >&2; exit 1; }

# sed -i.bak (and then rm) is the form both GNU and BSD sed accept.
sed -i.bak "/^\[workspace.package\]/,/^\[/s/^version = \"$old\"$/version = \"$new\"/" Cargo.toml

old_re=${old//./\\.}
sed -i.bak "/path = \"/s/version = \"$old_re\"/version = \"$new\"/" crates/*/Cargo.toml apps/*/Cargo.toml
rm -f Cargo.toml.bak crates/*/Cargo.toml.bak apps/*/Cargo.toml.bak

cargo update --workspace --offline
echo "version: $old -> $new (Android versionCode $(scripts/release/version.sh --android-code))"
echo "next: review 'jj diff', commit as 'chore(release): prepare $new', open a pull request,"
echo "      and after it merges tag the merge commit v$new (see docs/RELEASING.md)."
