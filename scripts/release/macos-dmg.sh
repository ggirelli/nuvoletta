#!/usr/bin/env bash
# Turn the dx-built macOS app bundle into a signed (when possible) disk image. Runs on macOS only.
#
# Usage: scripts/release/macos-dmg.sh <dx-bundle-dir> <output-dir> <arch>   (arch: aarch64 or x86_64)
#
# Signing and notarization need ALL of these environment variables (see docs/RELEASING.md):
#   APPLE_CERTIFICATE_BASE64    base64 of the "Developer ID Application" certificate and key (.p12)
#   APPLE_CERTIFICATE_PASSWORD  password of that .p12
#   APPLE_SIGNING_IDENTITY      for example "Developer ID Application: Name (TEAMID)"
#   APPLE_ID, APPLE_APP_PASSWORD, APPLE_TEAM_ID   notarytool credentials (app-specific password)
# Without them the app gets an ad-hoc signature (Apple Silicon refuses unsigned code) and the disk
# image is named *-unsigned.dmg; Gatekeeper then asks the user to approve it manually.
set -euo pipefail

bundle_dir=${1:?usage: macos-dmg.sh <dx-bundle-dir> <output-dir> <arch>}
out_dir=$(mkdir -p "${2:?}" && cd "$2" && pwd)
arch=${3:?}
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
version=$(scripts/release/version.sh)

src_app=$(find "$bundle_dir" -maxdepth 3 -name '*.app' -type d | head -n1)
[ -n "$src_app" ] || { echo "no .app bundle under $bundle_dir" >&2; exit 1; }

tmp=$(mktemp -d)
keychain="$tmp/release.keychain-db"
cleanup() { security delete-keychain "$keychain" 2>/dev/null || true; rm -rf "$tmp"; }
trap cleanup EXIT

stage="$tmp/stage"
mkdir -p "$stage"
cp -R "$src_app" "$stage/Nuvoletta.app"
ln -s /Applications "$stage/Applications"

signed=false
if [ -n "${APPLE_CERTIFICATE_BASE64:-}" ] && [ -n "${APPLE_CERTIFICATE_PASSWORD:-}" ] \
   && [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ -n "${APPLE_ID:-}" ] \
   && [ -n "${APPLE_APP_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  signed=true
  kc_pass=$(head -c 18 /dev/urandom | base64)
  base64 -d <<< "$APPLE_CERTIFICATE_BASE64" > "$tmp/cert.p12"
  security create-keychain -p "$kc_pass" "$keychain"
  security set-keychain-settings -lut 3600 "$keychain"
  security unlock-keychain -p "$kc_pass" "$keychain"
  security import "$tmp/cert.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kc_pass" "$keychain" >/dev/null
  security list-keychains -d user -s "$keychain" login.keychain
  codesign --force --deep --options runtime --timestamp --keychain "$keychain" \
    --sign "$APPLE_SIGNING_IDENTITY" "$stage/Nuvoletta.app"
  name="nuvoletta-${version}-macos-${arch}.dmg"
else
  echo "no Apple signing secrets: ad-hoc signing, the disk image is labelled unsigned" >&2
  codesign --force --deep --sign - "$stage/Nuvoletta.app"
  name="nuvoletta-${version}-macos-${arch}-unsigned.dmg"
fi

hdiutil create -volname "Nuvoletta" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$out_dir/$name"

if $signed; then
  codesign --force --timestamp --keychain "$keychain" --sign "$APPLE_SIGNING_IDENTITY" "$out_dir/$name"
  xcrun notarytool submit "$out_dir/$name" --apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait
  xcrun stapler staple "$out_dir/$name"
  spctl --assess --type open --context context:primary-signature -v "$out_dir/$name" >&2
fi
echo "$out_dir/$name"
