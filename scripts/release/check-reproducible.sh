#!/usr/bin/env bash
# Build the Linux tarball twice, in two different directories, and compare checksums.
#
# Usage: scripts/release/check-reproducible.sh
#
# The two builds use different checkout paths and target directories, so a path that leaks into the
# binary shows up as a mismatch. Both share this machine's toolchain, cargo registry and system
# libraries: that is what the check proves reproducible, and nothing more (docs/RELEASING.md).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
root=$PWD
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"

work=$(mktemp -d "${TMPDIR:-/tmp}/nuvoletta-repro.XXXXXX")
trap 'rm -rf "$work"' EXIT

for n in a b; do
  mkdir -p "$work/$n/checkout-$n"
  rsync -a --exclude /target --exclude /.git --exclude /.jj --exclude /.workspaces --exclude /site \
    "$root/" "$work/$n/checkout-$n/"
  (cd "$work/$n/checkout-$n" && CARGO_TARGET_DIR="$work/$n/target" \
    scripts/release/linux-tarball.sh "$work/$n/out" "$work/$n/target" >/dev/null)
done

sum_a=$(sha256sum "$work"/a/out/*.tar.gz | cut -d' ' -f1)
sum_b=$(sha256sum "$work"/b/out/*.tar.gz | cut -d' ' -f1)
echo "build a: $sum_a"
echo "build b: $sum_b"
if [ "$sum_a" = "$sum_b" ]; then
  echo "reproducible: the Linux tarballs are byte-identical"
else
  echo "NOT reproducible: the Linux tarballs differ" >&2
  mkdir -p "$root/target/release-check"
  cp -r "$work/a/out" "$root/target/release-check/a"
  cp -r "$work/b/out" "$root/target/release-check/b"
  echo "kept in target/release-check/{a,b}; compare with diffoscope" >&2
  exit 1
fi
