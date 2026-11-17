#!/usr/bin/env bash
# Print version facts. The single source of truth is [workspace.package] in Cargo.toml.
#
#   version.sh                 the release version (SemVer), for example 0.1.0 or 0.2.0-rc.1
#   version.sh --android-code  the Android versionCode derived from it
#   version.sh --windows-version  the numeric X.Y.Z.N version Windows installers need (NSIS)
#
# versionCode = (major*10000 + minor*100 + patch) * 100 + suffix, where suffix is 99 for a final
# release and the last number of the pre-release identifier (rc.3 gives 3, capped at 98) otherwise,
# so every release and pre-release is strictly greater than the ones before it.
# The Windows version is major.minor.patch.suffix with the same suffix (the VIProductVersion resource
# only accepts four numbers, so 0.1.0-rc.1 becomes 0.1.0.1 and 0.1.0 becomes 0.1.0.99).
# Limits: minor and patch below 100, major below 21.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
# awk, not sed: a range with a braced block is not portable between GNU and BSD (macOS) sed.
version=$(awk '/^\[/ { in_pkg = ($0 == "[workspace.package]"); next }
  in_pkg && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' Cargo.toml)
[ -n "$version" ] || { echo "no version in [workspace.package] of Cargo.toml" >&2; exit 1; }

# Parse the version into major, minor, patch and the 0-99 suffix described above.
parse() {
  local re='^([0-9]+)\.([0-9]+)\.([0-9]+)(-([0-9A-Za-z.-]+))?$' pre n
  [[ $version =~ $re ]] || { echo "not a SemVer version: $version" >&2; exit 1; }
  major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]} pre=${BASH_REMATCH[5]:-}
  suffix=99
  if [ -n "$pre" ]; then
    n=$(grep -oE '[0-9]+' <<< "$pre" | tail -n1 || true)
    suffix=$(( ${n:-0} > 98 ? 98 : ${n:-0} ))
  fi
}

case "${1:-}" in
  "") echo "$version" ;;
  --android-code)
    parse
    if [ "$minor" -ge 100 ] || [ "$patch" -ge 100 ] || [ "$major" -ge 21 ]; then
      echo "version out of range for an Android versionCode: $version" >&2; exit 1
    fi
    echo $(( (major * 10000 + minor * 100 + patch) * 100 + suffix ))
    ;;
  --windows-version)
    parse
    echo "$major.$minor.$patch.$suffix"
    ;;
  *) echo "usage: version.sh [--android-code|--windows-version]" >&2; exit 2 ;;
esac
