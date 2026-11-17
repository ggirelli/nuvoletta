#!/usr/bin/env bash
# Write the NSIS installer template that `dx bundle` uses for the Windows .exe installer.
#
# Usage: scripts/release/windows-nsis.sh
#
# dx's built-in NSIS template puts the SemVer crate version in VIProductVersion, which makensis only
# accepts as four numbers (X.X.X.X), so a pre-release such as 0.1.0-rc.1 fails. The template in
# apps/nuvoletta-app/windows/installer.nsi.in is dx's own with that one line changed; this script
# fills in the numeric version from `version.sh --windows-version`. The generated installer.nsi is
# not committed. Dioxus.toml points at it, so run this before `dx bundle` for Windows.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
dir=apps/nuvoletta-app/windows
version=$(scripts/release/version.sh --windows-version)
awk -v v="$version" '{ gsub(/@NSIS_VIPRODUCTVERSION@/, v); print }' "$dir/installer.nsi.in" > "$dir/installer.nsi"
echo "$dir/installer.nsi (VIProductVersion $version)"
