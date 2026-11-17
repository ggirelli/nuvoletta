#!/usr/bin/env bash
# Write the CycloneDX 1.5 SBOM of the app (all platforms and both UI feature sets) to <output-dir>.
#
# Usage: scripts/release/sbom.sh <output-dir>
#
# cargo-cyclonedx reads Cargo.lock and the manifests; it does not build anything. It also drops a
# file next to every workspace crate it visits, so this script moves the app's file and deletes the rest.
set -euo pipefail
out_dir=$(realpath -m "${1:?usage: sbom.sh <output-dir>}")
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
version=$(scripts/release/version.sh)

cargo cyclonedx --manifest-path apps/nuvoletta-app/Cargo.toml --format json --spec-version 1.5 \
  --all-features --target all --override-filename nuvoletta-app.cdx
mkdir -p "$out_dir"
mv apps/nuvoletta-app/nuvoletta-app.cdx.json "$out_dir/nuvoletta-${version}.cdx.json"
find apps crates -name '*.cdx.json' -delete
echo "$out_dir/nuvoletta-${version}.cdx.json"
