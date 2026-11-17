#!/usr/bin/env bash
# Build the Linux desktop app and pack it into a deterministic tarball.
#
# Usage: scripts/release/linux-tarball.sh <output-dir> [target-dir]
#
# The tarball holds the `dx build --release` output (the binary plus its assets directory), the
# licence and a README. Members are sorted, owned by root, and stamped with SOURCE_DATE_EPOCH;
# gzip is run without a name or timestamp, so equal inputs give equal bytes.
set -euo pipefail

out_dir=$(realpath -m "${1:?usage: linux-tarball.sh <output-dir> [target-dir]}")
export CARGO_TARGET_DIR="${2:-target}"
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# shellcheck source=scripts/release/env.sh
. scripts/release/env.sh

version=$(scripts/release/version.sh)
dx build --platform desktop --release --locked --package nuvoletta-app
app_dir="$CARGO_TARGET_DIR/dx/nuvoletta-app/release/linux/app"
[ -x "$app_dir/nuvoletta-app" ] || { echo "missing $app_dir/nuvoletta-app" >&2; exit 1; }

name="nuvoletta-${version}-linux-x86_64"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
cp -r "$app_dir/." "$stage/$name/"
cp LICENSE "$stage/$name/LICENSE"
cp DISCLAIMER.md "$stage/$name/DISCLAIMER.md"
cat > "$stage/$name/README.txt" <<TXT
Nuvoletta ${version} for Linux (x86_64).

Nuvoletta is fully vibe coded and not audited; read DISCLAIMER.md first.

Run ./nuvoletta-app. It needs GTK 3 and WebKitGTK 4.1 installed on the system.
Verify this download before running it: https://ggirelli.github.io/nuvoletta/RELEASING/
TXT

mkdir -p "$out_dir"
# Normalise modification times and modes the archive cannot otherwise pin down.
find "$stage/$name" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +
tar --sort=name --format=posix --pax-option=delete=atime,delete=ctime \
    --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 --numeric-owner \
    -C "$stage" -cf - "$name" | gzip -9n > "$out_dir/$name.tar.gz"
echo "$out_dir/$name.tar.gz"
